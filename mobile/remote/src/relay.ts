// The relay's outer frame, as crates/ket-relay-protocol defines it:
//
//   version u8 | kind u8 | connection u64 BE | length u32 BE | body
//
// and its control messages. Everything a phone says to the host is a data
// frame whose body is Noise ciphertext.

import { readU64BE, writeU64 } from './bytes.ts';

export const VERSION = 1;
export const HEADER = 14;
export const MAX_FRAME = 1024 * 1024;
export const KIND_CONTROL = 1;
export const KIND_DATA = 2;

export interface Frame {
  kind: number;
  connection: bigint;
  body: Uint8Array;
}

export function encodeFrame(frame: Frame): Uint8Array {
  const out = new Uint8Array(HEADER + frame.body.length);
  if (out.length > MAX_FRAME) throw new Error('a relay frame over 1 MiB');
  const view = new DataView(out.buffer);
  out[0] = VERSION;
  out[1] = frame.kind;
  writeU64(out, 2, frame.connection, false);
  view.setUint32(10, frame.body.length);
  out.set(frame.body, HEADER);
  return out;
}

/** Reads one relay frame, refusing what crates/ket-relay-protocol refuses:
 * a short or oversized frame, another version, an unknown kind, or a length
 * that disagrees with the body. */
export function decodeFrame(bytes: Uint8Array): Frame {
  if (bytes.length < HEADER) throw new Error('a truncated relay frame');
  if (bytes.length > MAX_FRAME) throw new Error('a relay frame over 1 MiB');
  if (bytes[0] !== VERSION) throw new Error(`relay frame version ${bytes[0]}`);
  if (bytes[1] !== KIND_CONTROL && bytes[1] !== KIND_DATA) throw new Error(`relay frame kind ${bytes[1]}`);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const length = view.getUint32(10);
  if (bytes.length !== HEADER + length) throw new Error('a relay frame of the wrong length');
  return { kind: bytes[1], connection: readU64BE(bytes, 2), body: bytes.subarray(HEADER) };
}

export const CONTROL_CONNECT = 2;
export const CONTROL_CONNECTED = 4;
export const CONTROL_CLOSED = 5;
export const CONTROL_REFUSED = 6;

export const REFUSED_HOST_OFFLINE = 1;
export const REFUSED_BACKPRESSURE = 3;

export function connectFrame(hostId: Uint8Array): Uint8Array {
  const body = new Uint8Array(17);
  body[0] = CONTROL_CONNECT;
  body.set(hostId, 1);
  return encodeFrame({ kind: KIND_CONTROL, connection: 0n, body });
}
