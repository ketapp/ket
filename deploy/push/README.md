# ket's push service

What lets a Mac reach a paired phone that isn't connected: when an agent is
waiting on you, or has finished, while the phone is in your pocket. It is a
Cloudflare Worker that holds ket's APNs key and forwards each notification to
Apple. It stores nothing and keeps no record of what it forwarded: one log
line per push, with Apple's status and never the token or the payload.

The Mac encrypts each notification to the phone's own push key before it
leaves the Mac, and the phone's notification extension decrypts it. So this
service sees a device token, a ciphertext and the Mac's address, and Apple
sees the same ciphertext under a generic alert ("An agent has an update").
Neither sees a project, a branch or a command.

Push needs no account. Anyone holding a phone's token could post to it, but
the token only ever reaches the Macs that phone paired with, and the rate
limits cap what a leaked one can do: 30 a minute per token and 120 a minute
per address, counted per Cloudflare location.

## The API

`POST /v1/push` with a JSON body:

| Field | |
|---|---|
| `token` | The phone's APNs device token, as hex. |
| `environment` | `sandbox` for a development build, `production` for TestFlight and the App Store. |
| `ciphertext` | The notification encrypted to the phone's push key, as base64, at most 3 KB. |
| `collapse_id` | Optional, at most 64 visible ASCII characters. A newer notification with the same id replaces the older one. |
| `thread_id` | Optional, the same rule. Groups notifications on the phone. |

What comes back, in terms the Mac acts on:

| Status | Meaning |
|---|---|
| 200 | Apple took it. |
| 400 | The body was wrong; `error` says which field. |
| 410 | The token will never work again: Apple said `Unregistered`, `BadDeviceToken` or `DeviceTokenNotForTopic`. Drop it. |
| 413 | The body was over 8 KB. |
| 429 | Too many for this token or address, or Apple said so. Wait. |
| 500 | The service isn't configured: the key, key id or team id is missing. |
| 502 | Apple refused or couldn't be reached; `error` carries Apple's reason. One retry is fine. |

`GET /health` answers `{"ok":true}`.

## First deploy

You need a Cloudflare account and the APNs key from the Apple developer
account: Certificates, Identifiers & Profiles → Keys → a key with Apple Push
Notifications service enabled. Download its `.p8` file once; Apple doesn't
offer it again. Note the key's id and your team id.

1. Put the team id and key id in `wrangler.jsonc` under `vars`. Neither is
   secret.
2. Sign in and set the key as a secret, from this folder:

   ```sh
   npx wrangler@4.157.0 login
   npx wrangler@4.157.0 secret put APNS_KEY < AuthKey_XXXXXXXXXX.p8
   ```

3. Deploy:

   ```sh
   npx wrangler@4.157.0 deploy
   ```

   It answers on `https://push.ketapp.dev`, the route in `wrangler.jsonc`,
   which needs the `ketapp.dev` zone on the same Cloudflare account. On
   another account, see "Another domain" below.

APNs can't be reached from `wrangler dev` on a laptop, only from a deployed
Worker, so try it deployed: with a development build's token,

```sh
curl -s https://push.ketapp.dev/v1/push \
  -H 'content-type: application/json' \
  -d '{"token":"<hex token>","environment":"sandbox","ciphertext":"AA=="}'
```

shows the generic alert on the phone (the extension can't open `AA==`, so the
alert stays generic). A 500 means the key, key id or team id isn't set; a 502
with `InvalidProviderToken` means they don't belong together.

## Trying it end to end

1. The Mac sends to `https://push.ketapp.dev` unless told otherwise. A
   Worker anywhere else needs `push_url` under `[phones]` in ket's config,
   then a host restart:

   ```toml
   [phones]
   push_url = "https://push.example.com"
   ```

2. Build the phone app with its notification extension (see
   `mobile/app/README.md`, "Notifications"), turn on Settings ›
   Notifications on the phone and connect it to the Mac once. The Mac's
   Settings › Devices then says "notifications on" beside it.
3. Press Send test on that row. The phone shows "Test notification" at
   once, connected or not; if not, the toast on the Mac says why, in
   Apple's words where Apple refused it (`TopicDisallowed`: no App ID with
   push for the bundle id under the key's team).
4. Close ket on the phone and wait for iOS to drop the connection, about
   half a minute. Have an agent stop on a prompt: about 20 seconds later
   the phone shows it, in its own words. Answer it at the Mac within those
   20 seconds and nothing is sent.

## Another domain

A Worker takes a custom domain only from a zone on its own Cloudflare
account; deploying elsewhere fails with "Can't infer zone" or "does not
exist on your account". For your own domain, change the route's `pattern`
and `zone_name` in `wrangler.jsonc`. With no domain at all, drop `routes`
and set `"workers_dev": true`: it then answers on
`https://ket-push.<your-subdomain>.workers.dev`. Either way, point the Mac at
it with `push_url`.

## Type-checking

The Worker needs no packages of its own. `src/platform.d.ts` writes out the
bindings that `wrangler types` would generate, so keep it in step with
`wrangler.jsonc`. To check it with the phone app's TypeScript:

```sh
node mobile/app/node_modules/typescript/bin/tsc -p deploy/push/tsconfig.json
```
