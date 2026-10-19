// A picked file's bytes, from where the picker left it. The browser build's
// is read-file.web.ts.

import { File } from 'expo-file-system';

export function readFile(uri: string): Promise<Uint8Array> {
  return new File(uri).bytes();
}
