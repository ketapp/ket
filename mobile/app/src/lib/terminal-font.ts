// The terminal page's face: Fira Code, read from the files the app already
// bundles for its own text. The page in its WebView cannot see the fonts the
// app registers, and a copy built into the code would ship the font twice.

import { FiraCode_400Regular, FiraCode_700Bold } from '@expo-google-fonts/fira-code';
import { toBase64Url } from '@ket/remote';
import { Asset } from 'expo-asset';
import { readFile } from './read-file';

/** Fira Code's regular and bold, as base64url for the page to decode. */
export interface TerminalFont {
  regular: string;
  bold: string;
}

let loading: Promise<TerminalFont | null> | null = null;

/** The two faces, read once and kept for every terminal after the first.
 * `null` when they cannot be read, and the page draws in Menlo instead. */
export function terminalFont(): Promise<TerminalFont | null> {
  loading ??= (async () => {
    const [regular, bold] = await Asset.loadAsync([FiraCode_400Regular, FiraCode_700Bold]);
    if (!regular.localUri || !bold.localUri) return null;
    const [regularBytes, boldBytes] = await Promise.all([readFile(regular.localUri), readFile(bold.localUri)]);
    return { regular: toBase64Url(regularBytes), bold: toBase64Url(boldBytes) };
  })().catch(() => null);
  return loading;
}
