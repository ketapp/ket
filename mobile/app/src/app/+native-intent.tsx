// Links the system hands the app. The QR code ket.app shows is the pairing
// code itself — ket://pair/<code> — so pointing the Camera app at it opens
// ket with that URL. That is not a screen: it becomes the pair screen with
// the code, which pairs at once, as ket://pair?code=… does.

export function redirectSystemPath({ path }: { path: string; initial: boolean }): string {
  const code = /^(?:ket:\/\/|\/)?pair\/([A-Za-z0-9_-]+)$/.exec(path)?.[1];
  return code ? `/pair?code=${encodeURIComponent(`ket://pair/${code}`)}` : path;
}
