# ket for phones

The phone app: pair with a Mac running ket, see its terminals,
watch one live, take control and type into it, interrupt it.

Built on `../remote` (the protocol package) and Expo. Builds are local — Xcode
and Gradle on the Mac, never EAS.

Phones reach the Mac on the local network or over Tailscale, through the
relay the Mac runs itself. ket's own relay, for anywhere without Tailscale,
is not offered yet. Pairing is the whole of the authentication: there is no sign-in, and the relay forwards
ciphertext it cannot read.

## Try it on your phone

Install the native iOS or Android app using the commands below. With the
phone and Mac on the same Wi-Fi, open Settings › Devices in ket on the Mac
and turn on **Devices**. On the phone, open **Pair a desktop** and scan the
QR code or paste the pairing code. Compare the six digits and approve the
pairing on the Mac.

## Build it

```sh
npx expo run:ios                 # simulator
npx expo run:ios --device        # your iPhone over USB (a free Apple ID signs it)
npx expo run:android
```

For TestFlight or the App Store: `npx expo prebuild`, then archive in Xcode.
`ios/` and `android/` are generated from `app.json` and not committed.

## Notifications

While it's connected to a Mac, the phone posts its own notifications from
what the Mac tells it (`src/lib/notices.ts`). Once ket is closed, a Mac from
protocol 1.34 pushes the same ones instead:

- the phone registers its APNs token and a push key of its own with each
  Mac (`src/lib/push.ts`);
- the Mac seals each notification to that key and sends it through ket's
  push service (`deploy/push`), which forwards it to Apple;
- the notification service extension in `targets/notification-service`
  opens it on the phone and shows its words. Neither the service nor Apple
  can read it.

The extension is an Xcode target that `@bacons/apple-targets` adds at
prebuild. To build with it:

- Put your Apple team id in `.env.local` as `KET_APPLE_TEAM_ID`, which signs
  the extension's target.
- Run `npx expo prebuild --platform ios --clean` after changing `app.json`
  or anything in `targets/`.
- Sign with a paid team whose App ID has Push Notifications on: Apple
  delivers no push to an app a free Apple ID signed.

The extension reads the push key from the app's own Keychain group,
`$(AppIdentifierPrefix)dev.ketapp.ket`, which both carry as an entitlement.
iOS 17 is the minimum, for CryptoKit's HPKE.

## Scripts

- `npm run typecheck`
- `npx expo lint`
- `npm run embed-xterm` — after upgrading `@xterm/xterm`, regenerates
  `src/lib/xterm-assets.ts`, the copy the terminal WebView loads.
