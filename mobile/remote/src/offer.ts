// Pairing codes: the `ket://pair/…` text a host's QR code shows. The same
// layout as `ket_remote::Offer::to_code`:
//
//   version u8 = 1 | host id 16 | host key 32 | offer id 16 | secret 32
//   | expires u64 BE | relay length u16 BE | relay utf8
//
// base64url, unpadded.

import { readU64BE, utf8 } from './bytes.ts';

export const OFFER_PREFIX = 'ket://pair/';

function validRelayUrl(url: string): boolean {
  if (
    url.length === 0 ||
    url.length > 2048 ||
    /[\s\u0000-\u001f\u007f]/.test(url) ||
    /[?#@\\]/.test(url)
  ) return false;
  const match = /^(ws|wss):\/\/([^/]+)(?:\/.*)?$/.exec(url);
  if (!match) return false;
  const authority = match[2];
  if (authority.includes('%')) return false;
  if (authority.startsWith('[')) {
    const close = authority.indexOf(']');
    if (close <= 1) return false;
    const suffix = authority.slice(close + 1);
    return suffix === '' || /^:\d{1,5}$/.test(suffix);
  }
  const parts = authority.split(':');
  return parts.length <= 2 && parts[0].length > 0 && (parts.length === 1 || /^\d{1,5}$/.test(parts[1]));
}

export interface Offer {
  hostId: Uint8Array;
  hostPublic: Uint8Array;
  id: Uint8Array;
  /** The pairing secret: the handshake's PSK. Never sent anywhere. */
  secret: Uint8Array;
  /** Unix seconds. */
  expiresAt: number;
  relay: string;
}

const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

/** base64url without padding. Written out rather than taken from the
 * platform: React Native's engines disagree about what they provide. */
export function fromBase64Url(text: string): Uint8Array {
  const out: number[] = [];
  let bits = 0;
  let value = 0;
  for (const char of text) {
    const digit = ALPHABET.indexOf(char);
    if (digit < 0) throw new Error('not base64url');
    value = (value << 6) | digit;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((value >> bits) & 0xff);
    }
  }
  return Uint8Array.from(out);
}

export function toBase64Url(bytes: Uint8Array): string {
  let out = '';
  let bits = 0;
  let value = 0;
  for (const byte of bytes) {
    value = (value << 8) | byte;
    bits += 8;
    while (bits >= 6) {
      bits -= 6;
      out += ALPHABET[(value >> bits) & 0x3f];
    }
  }
  if (bits > 0) out += ALPHABET[(value << (6 - bits)) & 0x3f];
  return out;
}

export function parseOffer(code: string): Offer {
  const body = code.trim();
  if (!body.startsWith(OFFER_PREFIX)) throw new Error('not a ket pairing code');
  const bytes = fromBase64Url(body.slice(OFFER_PREFIX.length));
  let at = 0;
  const take = (n: number) => {
    if (at + n > bytes.length) throw new Error('a truncated pairing code');
    const out = bytes.subarray(at, at + n);
    at += n;
    return out;
  };
  if (take(1)[0] !== 1) throw new Error('a pairing code from a newer ket');
  const hostId = take(16);
  const hostPublic = take(32);
  const id = take(16);
  const secret = take(32);
  const expiresAt = Number(readU64BE(take(8), 0));
  const length = take(2);
  const relayLength = (length[0] << 8) | length[1];
  const relay = utf8(take(relayLength));
  if (!validRelayUrl(relay)) throw new Error('an unsafe relay URL');
  if (at !== bytes.length) throw new Error('a pairing code with trailing bytes');
  return { hostId, hostPublic, id, secret, expiresAt, relay };
}
