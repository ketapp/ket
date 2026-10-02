// A Noise session after the handshake, and the splitting of messages larger
// than one Noise frame — the same framing as crates/ket-remote/src/session.rs:
// each frame's plaintext is a flag byte (0: last, 1: more follows) and a
// chunk of the message.

import { type CipherState, MAXMSGLEN, TAGLEN, concat, type Transport } from './noise.ts';

/** The largest application message either side sends. */
export const MAX_MESSAGE = 16 * 1024 * 1024;
const CHUNK = MAXMSGLEN - TAGLEN - 1;
const LAST = 0;
const MORE = 1;
const EMPTY = new Uint8Array(0);

export class Session {
  private readonly send: CipherState;
  private readonly receive: CipherState;
  private partial: Uint8Array[] = [];
  private partialLength = 0;

  constructor(transport: Transport) {
    this.send = transport.send;
    this.receive = transport.receive;
  }

  /** Encrypts a message into the frames that carry it, in order. */
  seal(message: Uint8Array): Uint8Array[] {
    if (message.length > MAX_MESSAGE) throw new Error(`a message of ${message.length} bytes is too large`);
    const frames: Uint8Array[] = [];
    let at = 0;
    do {
      const chunk = message.subarray(at, at + CHUNK);
      at += chunk.length;
      const flag = at < message.length ? MORE : LAST;
      frames.push(this.send.encrypt(EMPTY, concat(Uint8Array.of(flag), chunk)));
    } while (at < message.length);
    return frames;
  }

  /** Decrypts one frame: the whole message once its last frame is in, or
   * `null` while more are to come. Throws on anything tampered, replayed or
   * out of order — which ends the session. */
  open(frame: Uint8Array): Uint8Array | null {
    const plain = this.receive.decrypt(EMPTY, frame);
    if (plain.length === 0) throw new Error('an empty frame');
    const chunk = plain.subarray(1);
    if (this.partialLength + chunk.length > MAX_MESSAGE) throw new Error('a message too large');
    this.partial.push(chunk);
    this.partialLength += chunk.length;
    if (plain[0] === MORE) return null;
    if (plain[0] !== LAST) throw new Error('a frame with an unknown flag');
    const message = concat(...this.partial);
    this.partial = [];
    this.partialLength = 0;
    return message;
  }
}
