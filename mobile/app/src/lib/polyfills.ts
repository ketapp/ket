// What the protocol package needs from the platform and React Native's
// engine does not promise: cryptographic randomness for keys and nonces, and
// a TextDecoder for the strings inside protocol messages. Imported first,
// before anything that could reach for either.

import { getRandomValues } from 'expo-crypto';
import { utf8 } from '@ket/remote';

const g = globalThis as unknown as {
  crypto?: { getRandomValues?: typeof getRandomValues };
  TextDecoder?: unknown;
};

g.crypto ??= {};
g.crypto.getRandomValues ??= getRandomValues;

if (g.TextDecoder === undefined) {
  g.TextDecoder = class {
    decode(input?: ArrayBuffer | ArrayBufferView): string {
      if (!input) return '';
      const bytes =
        input instanceof Uint8Array
          ? input
          : ArrayBuffer.isView(input)
            ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
            : new Uint8Array(input);
      return utf8(bytes);
    }
  };
}
