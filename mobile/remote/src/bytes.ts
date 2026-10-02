// Byte helpers written out by hand rather than taken from DataView's 64-bit
// methods and TextDecoder, which React Native's Hermes engine has not always
// had. Plain arithmetic runs everywhere the app does.

/** Writes an unsigned 64-bit integer. */
export function writeU64(out: Uint8Array, at: number, value: bigint, littleEndian: boolean): void {
  for (let i = 0; i < 8; i++) {
    const byte = Number((value >> BigInt(8 * i)) & 0xffn);
    out[littleEndian ? at + i : at + 7 - i] = byte;
  }
}

/** Reads a big-endian unsigned 64-bit integer. */
export function readU64BE(bytes: Uint8Array, at: number): bigint {
  let value = 0n;
  for (let i = 0; i < 8; i++) value = (value << 8n) | BigInt(bytes[at + i]);
  return value;
}

/** Decodes UTF-8, refusing anything malformed. */
export function utf8(bytes: Uint8Array): string {
  let out = '';
  for (let i = 0; i < bytes.length; ) {
    const b = bytes[i];
    const need = b < 0x80 ? 0 : b >= 0xf0 && b < 0xf5 ? 3 : b >= 0xe0 ? 2 : b >= 0xc2 && b < 0xe0 ? 1 : -1;
    if (need < 0 || i + need >= bytes.length + (need === 0 ? 1 : 0)) throw new Error('not UTF-8');
    let code = need === 0 ? b : b & (0x3f >> need);
    for (let k = 1; k <= need; k++) {
      const c = bytes[i + k];
      if ((c & 0xc0) !== 0x80) throw new Error('not UTF-8');
      code = (code << 6) | (c & 0x3f);
    }
    out += String.fromCodePoint(code);
    i += need + 1;
  }
  return out;
}
