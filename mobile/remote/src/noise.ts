// The phone's half of ket's Noise handshakes, on audited primitives.
//
// Exactly what the host's `snow` speaks and nothing more: the initiator of
// `Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s` (pairing, with the QR code's secret
// as the PSK) and `Noise_IK_25519_ChaChaPoly_BLAKE2s` (reconnecting). The
// cryptography is @noble's — X25519, ChaCha20-Poly1305, BLAKE2s, HMAC —
// and this file is only the handshake's bookkeeping, written line for line
// against the Noise specification (revision 34), section by section:
// CipherState (5.1), SymmetricState (5.2), the IK pattern (7.5) and PSK
// handling (9). test/vectors.ts checks it byte for byte against `snow`.

import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { x25519 } from '@noble/curves/ed25519.js';
import { blake2s } from '@noble/hashes/blake2.js';
import { hmac } from '@noble/hashes/hmac.js';

import { writeU64 } from './bytes.ts';

const EMPTY = new Uint8Array(0);
const DHLEN = 32;
/** A ChaChaPoly authentication tag. */
export const TAGLEN = 16;
/** The largest Noise message. */
export const MAXMSGLEN = 65535;

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

/** A static or ephemeral Curve25519 key pair. */
export interface Keypair {
  privateKey: Uint8Array;
  publicKey: Uint8Array;
}

export function generateKeypair(): Keypair {
  const privateKey = x25519.utils.randomSecretKey();
  return { privateKey, publicKey: x25519.getPublicKey(privateKey) };
}

export function keypairFrom(privateKey: Uint8Array): Keypair {
  return { privateKey, publicKey: x25519.getPublicKey(privateKey) };
}

function dh(privateKey: Uint8Array, publicKey: Uint8Array): Uint8Array {
  return x25519.getSharedSecret(privateKey, publicKey);
}

/** ChaChaPoly's nonce: 32 bits of zeros, then the counter, little-endian. */
function nonce(n: bigint): Uint8Array {
  const bytes = new Uint8Array(12);
  writeU64(bytes, 4, n, true);
  return bytes;
}

/** HKDF over HMAC-BLAKE2s, as Noise defines it (section 4.3). */
function hkdf(chainingKey: Uint8Array, ikm: Uint8Array, outputs: 2 | 3): Uint8Array[] {
  const temp = hmac(blake2s, chainingKey, ikm);
  const out1 = hmac(blake2s, temp, Uint8Array.of(1));
  const out2 = hmac(blake2s, temp, concat(out1, Uint8Array.of(2)));
  if (outputs === 2) return [out1, out2];
  return [out1, out2, hmac(blake2s, temp, concat(out2, Uint8Array.of(3)))];
}

const MAX_NONCE = 2n ** 64n - 1n;

/** Section 5.1: a key and a counter. */
export class CipherState {
  private key: Uint8Array | null;
  private n = 0n;

  constructor(key: Uint8Array | null = null) {
    this.key = key;
  }

  hasKey(): boolean {
    return this.key !== null;
  }

  encrypt(ad: Uint8Array, plaintext: Uint8Array): Uint8Array {
    if (this.key === null) return plaintext;
    if (this.n >= MAX_NONCE) throw new Error('noise: nonce exhausted');
    const ciphertext = chacha20poly1305(this.key, nonce(this.n), ad).encrypt(plaintext);
    this.n += 1n;
    return ciphertext;
  }

  /** Throws on a failed tag — a wrong key, a tampered, replayed or reordered
   * message — and then does not advance the counter, per the spec. */
  decrypt(ad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
    if (this.key === null) return ciphertext;
    if (this.n >= MAX_NONCE) throw new Error('noise: nonce exhausted');
    const plaintext = chacha20poly1305(this.key, nonce(this.n), ad).decrypt(ciphertext);
    this.n += 1n;
    return plaintext;
  }
}

/** Section 5.2: the chaining key, the handshake hash, and a cipher. */
class SymmetricState {
  ck: Uint8Array;
  h: Uint8Array;
  cipher = new CipherState();

  constructor(protocolName: string) {
    const name = new TextEncoder().encode(protocolName);
    if (name.length <= 32) {
      this.h = new Uint8Array(32);
      this.h.set(name);
    } else {
      this.h = blake2s(name);
    }
    this.ck = this.h;
  }

  mixKey(ikm: Uint8Array): void {
    const [ck, key] = hkdf(this.ck, ikm, 2);
    this.ck = ck;
    this.cipher = new CipherState(key);
  }

  mixHash(data: Uint8Array): void {
    this.h = blake2s(concat(this.h, data));
  }

  mixKeyAndHash(ikm: Uint8Array): void {
    const [ck, hashed, key] = hkdf(this.ck, ikm, 3);
    this.ck = ck;
    this.mixHash(hashed);
    this.cipher = new CipherState(key);
  }

  encryptAndHash(plaintext: Uint8Array): Uint8Array {
    const ciphertext = this.cipher.encrypt(this.h, plaintext);
    this.mixHash(ciphertext);
    return ciphertext;
  }

  decryptAndHash(ciphertext: Uint8Array): Uint8Array {
    const plaintext = this.cipher.decrypt(this.h, ciphertext);
    this.mixHash(ciphertext);
    return plaintext;
  }

  split(): [CipherState, CipherState] {
    const [k1, k2] = hkdf(this.ck, EMPTY, 2);
    return [new CipherState(k1), new CipherState(k2)];
  }
}

export const PAIR = 'Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s';
export const RESUME = 'Noise_IK_25519_ChaChaPoly_BLAKE2s';

export interface InitiatorOptions {
  /** The phone's static key pair. */
  staticKey: Keypair;
  /** The host's static public key, pinned from the pairing code. */
  remoteStatic: Uint8Array;
  /** Bound into the handshake: ket's cleartext header. */
  prologue: Uint8Array;
  /** The pairing secret, for `IKpsk2`; absent for `IK`. */
  psk?: Uint8Array;
  /** Only for test vectors: a fixed ephemeral private key. */
  ephemeralForTestingOnly?: Uint8Array;
}

/** The transport ciphers a finished handshake yields: the phone sends
 * with one and receives with the other. */
export interface Transport {
  send: CipherState;
  receive: CipherState;
}

/**
 * The initiator of `IK` or `IKpsk2`:
 *
 *     <- s
 *     ...
 *     -> e, es, s, ss
 *     <- e, ee, se          (IKpsk2: e, ee, se, psk)
 */
export class Initiator {
  private readonly symmetric: SymmetricState;
  private readonly options: InitiatorOptions;
  private ephemeral: Keypair | null = null;

  constructor(options: InitiatorOptions) {
    if (options.remoteStatic.length !== DHLEN) throw new Error('noise: a host key of the wrong size');
    if (options.psk !== undefined && options.psk.length !== 32) throw new Error('noise: a psk of the wrong size');
    this.options = options;
    this.symmetric = new SymmetricState(options.psk ? PAIR : RESUME);
    this.symmetric.mixHash(options.prologue);
    // The pre-message `<- s`: the host's static key, known in advance.
    this.symmetric.mixHash(options.remoteStatic);
  }

  /** `-> e, es, s, ss`, carrying `payload`. */
  writeMessage1(payload: Uint8Array = EMPTY): Uint8Array {
    const { staticKey, remoteStatic, psk, ephemeralForTestingOnly } = this.options;
    const s = this.symmetric;
    const e = ephemeralForTestingOnly ? keypairFrom(ephemeralForTestingOnly) : generateKeypair();
    this.ephemeral = e;
    s.mixHash(e.publicKey);
    // Section 9.2: in a PSK handshake, `e` is also mixed into the key.
    if (psk) s.mixKey(e.publicKey);
    s.mixKey(dh(e.privateKey, remoteStatic));
    const encryptedStatic = s.encryptAndHash(staticKey.publicKey);
    s.mixKey(dh(staticKey.privateKey, remoteStatic));
    return concat(e.publicKey, encryptedStatic, s.encryptAndHash(payload));
  }

  /** `<- e, ee, se` (`, psk`): the host's reply, which finishes the
   * handshake. Throws if the host is not the one whose key was pinned, or
   * the pairing secret was wrong. */
  readMessage2(message: Uint8Array): { payload: Uint8Array; transport: Transport; handshakeHash: Uint8Array } {
    const { staticKey, psk } = this.options;
    const e = this.ephemeral;
    if (e === null) throw new Error('noise: the first message has not been written');
    if (message.length < DHLEN + TAGLEN) throw new Error('noise: a reply too short');
    const s = this.symmetric;
    const re = message.subarray(0, DHLEN);
    s.mixHash(re);
    if (psk) s.mixKey(re);
    s.mixKey(dh(e.privateKey, re));
    s.mixKey(dh(staticKey.privateKey, re));
    if (psk) s.mixKeyAndHash(psk);
    const payload = s.decryptAndHash(message.subarray(DHLEN));
    // The handshake hash, final now: the same bytes `snow` gives the host,
    // for the code a person compares when approving a pairing.
    const handshakeHash = s.h.slice();
    const [send, receive] = s.split();
    return { payload, transport: { send, receive }, handshakeHash };
  }
}
