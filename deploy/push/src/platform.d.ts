// The Worker's bindings and the two platform types it uses. `wrangler types`
// would generate these from wrangler.jsonc; they are written out here so the
// repository needs no Cloudflare packages to type-check the Worker. Keep them
// in step with wrangler.jsonc.

interface Env {
  /** The app's bundle id, which Apple checks the token against. */
  BUNDLE_ID: string;
  /** The Apple developer team that owns the key. */
  TEAM_ID: string;
  /** The APNs key's id, shown beside it in the developer account. */
  KEY_ID: string;
  /** The APNs auth key: the .p8 file's text, set with `wrangler secret put APNS_KEY`. */
  APNS_KEY: string;
  BY_TOKEN: RateLimit;
  BY_IP: RateLimit;
}

/** A Workers rate-limiting binding. */
interface RateLimit {
  limit(options: { key: string }): Promise<{ success: boolean }>;
}

/** The shape of a module Worker's default export. */
interface ExportedHandler<E> {
  fetch?(request: Request, env: E): Response | Promise<Response>;
}
