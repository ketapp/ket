// Push: a paired desktop's notices while ket isn't running (desktop 1.34).
//
// The local notices (lib/notices.ts) come from the snapshots, so they stop
// about half a minute after ket leaves the screen, when iOS suspends it.
// Push carries on from there. The phone tells each desktop its APNs token
// and a key of its own. The desktop seals each notice to that key and sends
// it through ket's push service and Apple, neither of which can read it. The
// phone's notification extension opens it and shows the real words; until
// then iOS shows the service's generic line.
//
// The same switches govern both: Notify me and each kind in Settings. A
// desktop sends only while this phone isn't connected to it, and only what
// is still so after a short wait, so a prompt answered at the Mac doesn't
// buzz the phone as well. iOS only: Android has no push service here.

import { PushEnvironment, PushKind, toBase64Url } from '@ket/remote';
import { isRunningInExpoGo } from 'expo';
import * as Application from 'expo-application';
import * as Device from 'expo-device';
import * as Notifications from 'expo-notifications';
import { Platform } from 'react-native';

import type { HostConnection } from './connection';
import { pushKey } from './store';

/** Whether this phone can be sent pushes at all: ket's own app on an
 * iPhone. Not Expo Go, whose token is for Expo Go's app, which ket's key
 * can't push to, and which can't carry ket's notification extension. */
export const pushable = Platform.OS === 'ios' && !isRunningInExpoGo();

/** The first desktop protocol minor that takes a registration. */
const PUSH_MINOR = 34;

/** How long after this phone failed to get a token it asks again: a build
 * without push asks Apple only now and then, not on every snapshot. */
const RETRY_TOKEN_MS = 60_000;

/** What a desktop is told about this phone. */
interface ThisPhone {
  token: Uint8Array;
  environment: PushEnvironment;
  key: Uint8Array;
}

let phone: Promise<ThisPhone | null> | null = null;
let failedAt = 0;

/** Which of Apple's two services this build's token belongs to: the
 * provisioning profile says, for a development build or one installed from
 * Xcode. The App Store and TestFlight embed none and use production; the
 * simulator embeds none and uses the sandbox. */
async function environment(): Promise<PushEnvironment> {
  const said = await Application.getIosPushNotificationServiceEnvironmentAsync().catch(() => null);
  if (said === 'development') return PushEnvironment.SANDBOX;
  if (said === 'production') return PushEnvironment.PRODUCTION;
  return Device.isDevice ? PushEnvironment.PRODUCTION : PushEnvironment.SANDBOX;
}

/** `hex` as bytes, or null when it isn't hex. */
function hexBytes(hex: string): Uint8Array | null {
  if (!/^(?:[0-9a-f]{2})+$/i.test(hex)) return null;
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

async function readPhone(): Promise<ThisPhone | null> {
  const [token, env, keys] = await Promise.all([Notifications.getDevicePushTokenAsync(), environment(), pushKey()]);
  const bytes = token.type === 'ios' && typeof token.data === 'string' ? hexBytes(token.data) : null;
  return bytes ? { token: bytes, environment: env, key: keys.publicKey } : null;
}

/** This phone's token, environment and push key, asked for once. Null when
 * Apple gives no token: a build without the push entitlement, or no
 * network yet. */
function thisPhone(): Promise<ThisPhone | null> {
  if (!phone && Date.now() - failedAt < RETRY_TOKEN_MS) return Promise.resolve(null);
  phone ??= readPhone().then(
    (found) => {
      if (!found) {
        phone = null;
        failedAt = Date.now();
      }
      return found;
    },
    () => {
      phone = null;
      failedAt = Date.now();
      return null;
    },
  );
  return phone;
}

/** What the switches ask for. */
export interface Wanted {
  on: boolean;
  waiting: boolean;
  finished: boolean;
  checks: boolean;
  limits: boolean;
}

function kindsOf(wanted: Wanted): PushKind[] {
  if (!wanted.on) return [];
  const kinds: PushKind[] = [];
  if (wanted.waiting) kinds.push(PushKind.WAITING);
  if (wanted.finished) kinds.push(PushKind.FINISHED);
  if (wanted.checks) kinds.push(PushKind.CHECKS);
  // A desktop from before 1.38 passes over a kind it does not know.
  if (wanted.limits) kinds.push(PushKind.LIMITS);
  return kinds;
}

/** What each desktop was last told, and in which session: told again when
 * either changes, so a desktop that lost it hears it on the next connect. */
const told = new Map<string, { session: number; said: string }>();
const telling = new Set<string>();

/** Tells `conn`'s desktop where to push this phone's notices, or that it
 * shouldn't, once per session and whenever that changes. A no-op for a
 * desktop before 1.34, or one not connected. */
export async function syncPush(conn: HostConnection, wanted: Wanted): Promise<void> {
  if (!pushable || telling.has(conn.id)) return;
  const { status, hostMinor, upSince } = conn.state;
  if (status !== 'connected' || hostMinor < PUSH_MINOR || upSince === null) return;
  telling.add(conn.id);
  try {
    const kinds = kindsOf(wanted);
    const found = kinds.length > 0 ? await thisPhone() : null;
    // On, with no token for now: what the desktop has stays, rather than
    // taking back a registration that still works.
    if (kinds.length > 0 && !found) return;
    const said = found
      ? [toBase64Url(found.token), found.environment, toBase64Url(found.key), kinds.join(',')].join('|')
      : 'off';
    const was = told.get(conn.id);
    if (was?.session === upSince && was.said === said) return;
    if (found) await conn.registerPush(found.token, found.environment, found.key, kinds);
    else await conn.unregisterPush();
    told.set(conn.id, { session: upSince, said });
  } catch {
    // The session ended, or the desktop refused: tried again on the next look.
  } finally {
    telling.delete(conn.id);
  }
}

/** Asks Apple for the token again, and every desktop to be told again: iOS
 * hands out a new token now and then. */
export function forgetToken(): void {
  phone = null;
  failedAt = 0;
  told.clear();
}

/** Tells `conn`'s desktop to stop pushing to this phone, before the phone
 * forgets it. Best effort: a desktop out of reach is told nothing. */
export async function stopPushing(conn: HostConnection | undefined): Promise<void> {
  if (!pushable || !conn) return;
  const { status, hostMinor } = conn.state;
  if (status !== 'connected' || hostMinor < PUSH_MINOR) return;
  await conn.unregisterPush().catch(() => {});
  told.delete(conn.id);
}

/** A pushed notice, as the notification extension hands it to the app once
 * it has opened it: the desktop's payload without its words. */
export interface Pushed {
  host: string;
  kind: 'waiting' | 'finished' | 'failed' | 'checks' | 'limit' | 'reset';
  worktree: string;
  opens: 'chat' | 'worktree' | 'pull_request';
  /** The terminal whose chat a tap opens, for `chat`. */
  terminal?: string;
}

const KINDS = new Set(['waiting', 'finished', 'failed', 'checks', 'limit', 'reset']);
const OPENS = new Set(['chat', 'worktree', 'pull_request']);

/** A tapped or listed notification's data as a pushed notice, or null for
 * a local one, one the extension couldn't open, or anything malformed. */
export function pushedOf(data: unknown): Pushed | null {
  if (!data || typeof data !== 'object') return null;
  const d = data as Record<string, unknown>;
  if (d.v !== 1) return null;
  const { host, kind, worktree, opens, terminal } = d;
  if (typeof host !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(host)) return null;
  if (typeof worktree !== 'string' || worktree === '') return null;
  if (typeof kind !== 'string' || !KINDS.has(kind)) return null;
  if (typeof opens !== 'string' || !OPENS.has(opens)) return null;
  return {
    host,
    kind: kind as Pushed['kind'],
    worktree,
    opens: opens as Pushed['opens'],
    terminal: typeof terminal === 'string' && /^\d+$/.test(terminal) ? terminal : undefined,
  };
}

/** Where a tap on a pushed notice goes: where a local one about the same
 * thing goes. */
export function pushedPath(pushed: Pushed): string {
  const worktree = encodeURIComponent(pushed.worktree);
  if (pushed.opens === 'chat' && pushed.terminal) return `/session/${pushed.host}/${pushed.terminal}`;
  if (pushed.opens === 'pull_request') return `/pull-request/${pushed.host}/${worktree}`;
  return `/worktree/${pushed.host}/${worktree}`;
}
