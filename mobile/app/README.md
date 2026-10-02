# ket for phones

The phone app: pair with a Mac running ket, see its terminals,
watch one live, take control and type into it, interrupt it.

Built on `../remote` (the protocol package) and Expo. Builds are local — Xcode
and Gradle on the Mac, never EAS.

Phones reach the Mac over the local network only: the relay runs on the Mac,
and pairing is the whole of the authentication — there is no sign-in and no
server of ours.

## Try it on your phone

The phone and the Mac on the same Wi-Fi. Expo Go no longer loads this
project on an iPhone (SDK 57), so the phone runs the web build in Safari.

1. Start ket, telling it where the web app is served (until the Mac serves
   it itself):

   ```sh
   KET_PHONE_WEB_URL=http://<this Mac's LAN address>:8081 cargo run -p ket-ui
   ```

2. Start the app: `npm install && npx expo start`.
3. In ket, Settings › Devices: turn on **Devices**. Scan the code it shows with
   the iPhone Camera — Safari opens the app already pairing — or paste the
   code into **Pair a desktop**.

Plain `ws://` is fine on the LAN: every byte in it is Noise ciphertext. It
becomes `wss://` only when the app is served over HTTPS for Web Push — an
HTTPS page may not open a plain socket.

## Build it

```sh
npx expo run:ios                 # simulator
npx expo run:ios --device        # your iPhone over USB (a free Apple ID signs it)
npx expo run:android
```

For TestFlight or the App Store: `npx expo prebuild`, then archive in Xcode.
`ios/` and `android/` are generated from `app.json` and not committed.

## Scripts

- `npm run typecheck`
- `npx expo lint`
- `npm run embed-xterm` — after upgrading `@xterm/xterm`, regenerates
  `src/lib/xterm-assets.ts`, the copy the terminal WebView loads.
