// ket's push service. A Mac posts here when something on it needs a paired
// phone that isn't connected, and this forwards it to Apple's push service
// with ket's APNs key. What the Mac sends is already encrypted to the phone,
// which decrypts it in its notification extension, so this sees a device
// token, a ciphertext and the Mac's address, and keeps none of them.
//
// Apple only sees the generic alert below and the ciphertext. Anyone holding
// a phone's token could post to it; the token only ever reaches the Macs
// that phone paired with, and the rate limits cap what a leaked one can do.
// That is the price of push needing no account.

/** Where Apple takes notifications, by the environment the app was built for. */
const APNS_HOSTS = {
  sandbox: "https://api.sandbox.push.apple.com",
  production: "https://api.push.apple.com",
} as const;

type Environment = keyof typeof APNS_HOSTS;

/** The largest body read: a token, a ciphertext and two short ids fit well inside it. */
const MOST_BODY = 8 * 1024;

/** The largest ciphertext, as base64. Apple takes a 4 KB payload and the
 * generic alert around the ciphertext needs some of it. */
const MOST_CIPHERTEXT = 3 * 1024;

/** `apns-collapse-id` may be at most 64 bytes; a thread id is held to the same. */
const MOST_ID = 64;

/** How long Apple keeps trying a phone that is off. A prompt half a day old
 * isn't worth a buzz. */
const KEEP_FOR_SECONDS = 12 * 60 * 60;

/**
 * The window a provider token is signed for. Apple refuses a token issued
 * more than an hour ago, and one replaced more often than every 20 minutes.
 * Every isolate signs with the window's start as its issue time, so however
 * many isolates are running, the issue time changes once per window and a
 * token is at most 40 minutes old.
 */
const TOKEN_WINDOW_SECONDS = 40 * 60;

/** The generic alert, shown as is if the phone's extension can't decrypt. */
const ALERT = { title: "ket", body: "An agent has an update" };

/** Reasons Apple gives for a token that will never work again: the Mac drops it. */
const DEAD_TOKEN = new Set(["Unregistered", "BadDeviceToken", "DeviceTokenNotForTopic"]);

/** What a Mac posts. */
interface Push {
  /** The phone's APNs device token, as hex. */
  token: string;
  environment: Environment;
  /** The notification, encrypted to the phone's push key, as base64. */
  ciphertext: string;
  /** Replaces an earlier notification with the same id: one question, one notification. */
  collapse_id?: string;
  /** Groups notifications on the phone: one thread per project. */
  thread_id?: string;
}

/** The provider token, signed once per window — see TOKEN_WINDOW_SECONDS.
 * Module-level because it is shared by every request, not any one's state. */
let signed: { window: number; keyId: string; jwt: string } | null = null;

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/health" && request.method === "GET") return json(200, { ok: true });
    if (url.pathname !== "/v1/push") return json(404, { error: "not_found" });
    if (request.method !== "POST") return json(405, { error: "method_not_allowed" });

    const ip = request.headers.get("CF-Connecting-IP") ?? "unknown";
    if (!(await env.BY_IP.limit({ key: ip })).success) return json(429, { error: "rate_limited" });

    const body = await readCapped(request, MOST_BODY);
    if (body === null) return json(413, { error: "too_large" });
    const push = parse(body);
    if (typeof push === "string") return json(400, { error: push });

    if (!(await env.BY_TOKEN.limit({ key: push.token })).success) return json(429, { error: "rate_limited" });

    let jwt: string;
    try {
      jwt = await providerToken(env);
    } catch (e) {
      console.error(JSON.stringify({ message: "provider token", error: e instanceof Error ? e.message : String(e) }));
      return json(500, { error: "not_configured" });
    }
    return forward(push, jwt, env);
  },
} satisfies ExportedHandler<Env>;

/** Sends `push` to Apple and says what Apple said, in terms the Mac acts on:
 * 410 means drop the token, 429 means slow down, 502 means try once more. */
async function forward(push: Push, jwt: string, env: Env): Promise<Response> {
  const aps: Record<string, unknown> = { alert: ALERT, sound: "default", "mutable-content": 1 };
  if (push.thread_id) aps["thread-id"] = push.thread_id;
  const headers: Record<string, string> = {
    authorization: `bearer ${jwt}`,
    "apns-topic": env.BUNDLE_ID,
    "apns-push-type": "alert",
    "apns-priority": "10",
    "apns-expiration": String(Math.floor(Date.now() / 1000) + KEEP_FOR_SECONDS),
    "content-type": "application/json",
  };
  if (push.collapse_id) headers["apns-collapse-id"] = push.collapse_id;

  let answer: Response;
  try {
    answer = await fetch(`${APNS_HOSTS[push.environment]}/3/device/${push.token}`, {
      method: "POST",
      headers,
      body: JSON.stringify({ aps, e: push.ciphertext }),
    });
  } catch (e) {
    console.error(JSON.stringify({ message: "apns unreachable", error: e instanceof Error ? e.message : String(e) }));
    return json(502, { error: "apns_unreachable" });
  }

  if (answer.ok) {
    log(push.environment, answer.status);
    return json(200, { ok: true });
  }
  // Apple's errors are a small JSON object: { "reason": "..." }.
  const reason = await answer
    .json()
    .then((said: { reason?: unknown }) => (typeof said.reason === "string" ? said.reason : "unknown"))
    .catch(() => "unknown");
  log(push.environment, answer.status, reason);
  if (answer.status === 410 || DEAD_TOKEN.has(reason)) return json(410, { error: reason });
  if (answer.status === 429) return json(429, { error: reason });
  if (answer.status === 403) {
    // A refused provider token is this service's configuration, not the Mac's doing.
    console.error(JSON.stringify({ message: "apns refused the provider token", reason }));
  }
  return json(502, { error: reason });
}

/** The provider token for this window, signed with the APNs key (ES256). */
async function providerToken(env: Env): Promise<string> {
  const window = Math.floor(Date.now() / 1000 / TOKEN_WINDOW_SECONDS) * TOKEN_WINDOW_SECONDS;
  if (signed && signed.window === window && signed.keyId === env.KEY_ID) return signed.jwt;
  if (!env.TEAM_ID || !env.KEY_ID || !env.APNS_KEY) throw new Error("TEAM_ID, KEY_ID and APNS_KEY must all be set");
  const key = await crypto.subtle.importKey(
    "pkcs8",
    pemBody(env.APNS_KEY),
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"],
  );
  const input = `${base64url(JSON.stringify({ alg: "ES256", kid: env.KEY_ID }))}.${base64url(
    JSON.stringify({ iss: env.TEAM_ID, iat: window }),
  )}`;
  // Web Crypto's ECDSA signature is r and s side by side, which is what ES256 wants.
  const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, new TextEncoder().encode(input));
  const jwt = `${input}.${base64url(new Uint8Array(signature))}`;
  signed = { window, keyId: env.KEY_ID, jwt };
  return jwt;
}

/** Checks what a Mac posted; a string says what was wrong with it. */
function parse(body: string): Push | string {
  let raw: unknown;
  try {
    raw = JSON.parse(body);
  } catch {
    return "not_json";
  }
  if (typeof raw !== "object" || raw === null) return "not_an_object";
  const said = raw as Record<string, unknown>;
  const { token, environment, ciphertext, collapse_id, thread_id } = said;
  // A token is 32 bytes today; Apple says it may grow, so up to 100.
  if (typeof token !== "string" || !/^[0-9a-fA-F]{64,200}$/.test(token)) return "bad_token";
  if (environment !== "sandbox" && environment !== "production") return "bad_environment";
  if (
    typeof ciphertext !== "string" ||
    ciphertext.length === 0 ||
    ciphertext.length > MOST_CIPHERTEXT ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(ciphertext)
  ) {
    return "bad_ciphertext";
  }
  if (!optionalId(collapse_id)) return "bad_collapse_id";
  if (!optionalId(thread_id)) return "bad_thread_id";
  return {
    token: token.toLowerCase(),
    environment,
    ciphertext,
    collapse_id: collapse_id as string | undefined,
    thread_id: thread_id as string | undefined,
  };
}

/** Absent, or short visible ASCII: it goes into a header as it is. */
function optionalId(value: unknown): boolean {
  return value === undefined || (typeof value === "string" && /^[\x21-\x7e]{1,64}$/.test(value) && value.length <= MOST_ID);
}

/** The body as text, or null past `most` bytes, without reading more than that. */
async function readCapped(request: Request, most: number): Promise<string | null> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > most) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > most) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.byteLength;
  }
  return new TextDecoder().decode(bytes);
}

/** The DER inside a .p8 file's PEM armour. */
function pemBody(pem: string): ArrayBuffer {
  const base64 = pem.replace(/-----[A-Z ]+-----/g, "").replace(/\s+/g, "");
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

function base64url(input: string | Uint8Array): string {
  const bytes = typeof input === "string" ? new TextEncoder().encode(input) : input;
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** One line per push: never the token or the ciphertext. */
function log(environment: Environment, status: number, reason?: string): void {
  console.log(JSON.stringify({ message: "push", environment, status, reason }));
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}
