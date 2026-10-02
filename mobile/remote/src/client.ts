// A phone's connection to its ket host: through the relay, over a WebSocket,
// inside a Noise session. What the app builds on.
//
// The shapes match crates/ket-remote/src/lib.rs: a cleartext header — magic,
// mode, host id, offer id — bound into the handshake as its prologue; the
// host's cleartext refusal; and the rule that the phone speaks first after
// the handshake, because the host trusts nothing until that first message
// decrypts.

import { create, fromBinary, type MessageInitShape, toBinary } from '@bufbuild/protobuf';

import { type Envelope, EnvelopeSchema, HelloSchema } from './gen/ket/remote/v1/remote_pb.js';

/** What a phone says in its hello: its name, its version, and where to
 * resume from. */
export type Greeting = MessageInitShape<typeof HelloSchema>;
import { Initiator, type Keypair, concat } from './noise.ts';
import { type Offer, parseOffer } from './offer.ts';
import {
  CONTROL_CLOSED,
  CONTROL_CONNECTED,
  CONTROL_REFUSED,
  KIND_CONTROL,
  KIND_DATA,
  REFUSED_HOST_OFFLINE,
  connectFrame,
  decodeFrame,
  encodeFrame,
} from './relay.ts';
import { Session } from './session.ts';

export const MAJOR = 1;
export const MINOR = 10;

const MAGIC = new TextEncoder().encode('KET1');
const MODE_PAIR = 1;
const MODE_RESUME = 2;
const MODE_REFUSED = 0xff;
const HANDSHAKE_TIMEOUT_MS = 15_000;
/** How long the relay has to answer a dial. A socket that neither opens nor
 * fails — a wrong address on some networks, a relay that accepts and says
 * nothing — would otherwise hang the caller forever. */
const DIAL_TIMEOUT_MS = 10_000;

/** Frames, and bytes, a link holds for a reader that has not asked for
 * them yet. Past either the desktop — or whatever stands in for it on the
 * relay — is sending faster than the app reads, and the link ends rather
 * than holding it all. The session resumes from its cursor. */
const MAX_QUEUED_FRAMES = 4096;
const MAX_QUEUED_BYTES = 32 * 1024 * 1024;

/** One phone-to-host connection through the relay, as whole frames. */
class Link {
  private readonly socket: WebSocket;
  private connection = 0n;
  private readonly queue: Uint8Array[] = [];
  private queuedBytes = 0;
  private waiting: { resolve: (frame: Uint8Array) => void; reject: (error: Error) => void } | null =
    null;
  private failure: Error | null = null;

  private constructor(socket: WebSocket) {
    this.socket = socket;
  }

  /** Dials the relay and asks for the host. */
  static open(relay: string, hostId: Uint8Array): Promise<Link> {
    return new Promise((resolve, reject) => {
      const socket = new WebSocket(relay);
      socket.binaryType = 'arraybuffer';
      const link = new Link(socket);
      let connected = false;
      const dialling = setTimeout(() => {
        if (connected) return;
        const why = new Error(`the relay at ${relay} did not answer`);
        link.fail(why);
        socket.close();
        reject(why);
      }, DIAL_TIMEOUT_MS);
      socket.onopen = () => socket.send(connectFrame(hostId));
      socket.onmessage = (event: MessageEvent) => {
        let frame: ReturnType<typeof decodeFrame>;
        try {
          frame = decodeFrame(new Uint8Array(event.data as ArrayBuffer));
        } catch {
          // Nothing a relay sends is worth reading past a frame that does
          // not decode: the link ends, and says why.
          const why = new Error('the relay sent something that is not a frame');
          link.fail(why);
          if (!connected) {
            clearTimeout(dialling);
            reject(why);
          }
          return;
        }
        if (frame.kind === KIND_DATA) {
          link.deliver(frame.body);
          return;
        }
        if (frame.kind !== KIND_CONTROL) return;
        switch (frame.body[0]) {
          case CONTROL_CONNECTED:
            clearTimeout(dialling);
            link.connection = frame.connection;
            connected = true;
            resolve(link);
            break;
          case CONTROL_REFUSED: {
            const why =
              frame.body[1] === REFUSED_HOST_OFFLINE
                ? 'the host is not connected to the relay'
                : `the relay refused (${frame.body[1]})`;
            link.fail(new Error(why));
            if (!connected) reject(new Error(why));
            break;
          }
          case CONTROL_CLOSED:
            link.fail(new Error('the relay closed the connection'));
            break;
        }
      };
      socket.onerror = () => {
        link.fail(new Error('the relay connection failed'));
        if (!connected) reject(new Error(`could not reach the relay at ${relay}`));
      };
      socket.onclose = () => link.fail(new Error('the relay connection closed'));
    });
  }

  private deliver(frame: Uint8Array): void {
    if (this.failure) return;
    if (this.waiting) {
      const { resolve } = this.waiting;
      this.waiting = null;
      resolve(frame);
      return;
    }
    if (this.queue.length >= MAX_QUEUED_FRAMES || this.queuedBytes + frame.length > MAX_QUEUED_BYTES) {
      this.fail(new Error('the desktop sent more than this phone could keep up with'));
      return;
    }
    this.queue.push(frame);
    this.queuedBytes += frame.length;
  }

  /** Ends the link: the first failure is the one reported, a reader
   * waiting is told, what was queued is dropped, and the socket closes. */
  private fail(error: Error): void {
    this.failure ??= error;
    this.queue.length = 0;
    this.queuedBytes = 0;
    if (this.waiting) {
      const { reject } = this.waiting;
      this.waiting = null;
      reject(error);
    }
    if (this.socket.readyState === WebSocket.CONNECTING || this.socket.readyState === WebSocket.OPEN) {
      this.socket.close();
    }
  }

  send(body: Uint8Array): void {
    if (this.failure) throw this.failure;
    this.socket.send(encodeFrame({ kind: KIND_DATA, connection: this.connection, body }));
  }

  /** The next frame; rejects after `timeoutMs`, if given. */
  recv(timeoutMs?: number): Promise<Uint8Array> {
    const queued = this.queue.shift();
    if (queued) {
      this.queuedBytes -= queued.length;
      return Promise.resolve(queued);
    }
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      this.waiting = {
        resolve: (frame) => {
          clearTimeout(timer);
          resolve(frame);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      };
      if (timeoutMs !== undefined) {
        timer = setTimeout(() => {
          this.waiting = null;
          reject(new Error('timed out'));
        }, timeoutMs);
      }
    });
  }

  close(): void {
    this.socket.close();
  }
}

/** What a phone keeps about the host it paired with. */
export interface PairedHost {
  hostId: Uint8Array;
  hostPublic: Uint8Array;
  relay: string;
}

/** An envelope from this build around `payload`. */
export function envelope(requestId: bigint, payload: Envelope['payload']): Envelope {
  return create(EnvelopeSchema, { major: MAJOR, minor: MINOR, requestId, payload });
}

/** A live session with the host. */
export class Remote {
  private readonly link: Link;
  private readonly session: Session;

  constructor(link: Link, session: Session) {
    this.link = link;
    this.session = session;
  }

  send(message: Envelope): void {
    for (const frame of this.session.seal(toBinary(EnvelopeSchema, message))) {
      this.link.send(frame);
    }
  }

  /** The next envelope from the host. */
  async recv(timeoutMs?: number): Promise<Envelope> {
    for (;;) {
      const message = this.session.open(await this.link.recv(timeoutMs));
      if (message) return fromBinary(EnvelopeSchema, message);
    }
  }

  close(): void {
    this.link.close();
  }
}

/** Six digits, `"123 456"`, from a handshake hash — the host's
 * `ket_remote::confirmation_code`, byte for byte: what a person compares
 * between this phone and the desktop before approving a pairing. */
export function confirmationCode(hash: Uint8Array): string {
  const number = (((hash[0] << 24) | (hash[1] << 16) | (hash[2] << 8) | hash[3]) >>> 0) % 1_000_000;
  const three = (n: number) => String(n).padStart(3, '0');
  return `${three(Math.floor(number / 1000))} ${three(number % 1000)}`;
}

async function handshake(
  link: Link,
  header: Uint8Array,
  initiator: Initiator,
): Promise<{ session: Session; hash: Uint8Array }> {
  link.send(concat(header, initiator.writeMessage1()));
  const reply = await link.recv(HANDSHAKE_TIMEOUT_MS);
  if (
    reply.length === 5 &&
    reply.subarray(0, 4).every((b, i) => b === MAGIC[i]) &&
    reply[4] === MODE_REFUSED
  ) {
    throw new Error('the host refused the connection');
  }
  const { transport, handshakeHash } = initiator.readMessage2(reply);
  return { session: new Session(transport), hash: handshakeHash };
}

/** A handshake's result, with the link closed if it fails: one left open
 * kept its socket and its callbacks, and kept queueing whatever arrived. */
async function closingOnFailure<T>(link: Link, work: Promise<T>): Promise<T> {
  try {
    return await work;
  } catch (error) {
    link.close();
    throw error;
  }
}

function hello(fields: Greeting): Envelope {
  return envelope(1n, { case: 'hello', value: create(HelloSchema, fields) });
}

/** Pairs with a host from its QR code's text. Since 1.6 the host grants
 * this phone only once a person approves it on the desktop: it answers the
 * hello with `pairingPending` first, and `confirmation` is the code the
 * desktop shows then — worked out here, not taken from the host. */
export async function pair(
  code: string,
  keys: Keypair,
  greeting: Greeting,
): Promise<{ remote: Remote; host: PairedHost; confirmation: string }> {
  const offer: Offer = parseOffer(code);
  if (Date.now() / 1000 >= offer.expiresAt) throw new Error('that pairing code has expired');
  const link = await Link.open(offer.relay, offer.hostId);
  const header = concat(MAGIC, Uint8Array.of(MODE_PAIR), offer.hostId, offer.id);
  const { session, hash } = await closingOnFailure(
    link,
    handshake(
      link,
      header,
      new Initiator({ staticKey: keys, remoteStatic: offer.hostPublic, prologue: header, psk: offer.secret }),
    ),
  );
  const remote = new Remote(link, session);
  remote.send(hello(greeting));
  return {
    remote,
    host: { hostId: offer.hostId, hostPublic: offer.hostPublic, relay: offer.relay },
    confirmation: confirmationCode(hash),
  };
}

/** Reconnects to a host this phone has paired with. Give the greeting a
 * resume cursor to pick up where the last session left off. */
export async function resume(host: PairedHost, keys: Keypair, greeting: Greeting): Promise<Remote> {
  const link = await Link.open(host.relay, host.hostId);
  const header = concat(MAGIC, Uint8Array.of(MODE_RESUME), host.hostId, new Uint8Array(16));
  const { session } = await closingOnFailure(
    link,
    handshake(link, header, new Initiator({ staticKey: keys, remoteStatic: host.hostPublic, prologue: header })),
  );
  const remote = new Remote(link, session);
  remote.send(hello(greeting));
  return remote;
}
