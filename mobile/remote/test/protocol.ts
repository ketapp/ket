// The phone's half of the protocol under attack: the hand-written Noise
// initiator, the session framing, the pairing-code parser and the relay
// frame decoder. `vectors.ts` shows the initiator agrees with `snow` when
// everything is right; this shows it refuses when anything is wrong, and
// that no input makes it hang or throw something other than an Error.
//
// The Rust side is crates/ket-remote/tests/protocol.rs.
//
//   node test/protocol.ts

import { readFileSync } from 'node:fs';

import { Initiator, type Keypair, type Transport, generateKeypair, keypairFrom } from '../src/noise.ts';
import { OFFER_PREFIX, parseOffer, toBase64Url } from '../src/offer.ts';
import { HEADER, KIND_CONTROL, KIND_DATA, MAX_FRAME, decodeFrame, encodeFrame } from '../src/relay.ts';
import { MAX_MESSAGE, Session } from '../src/session.ts';

interface Vector {
  pattern: string;
  phone_private: string;
  host_public: string;
  phone_ephemeral: string;
  psk: string;
  prologue: string;
  message2: string;
  transport2: string;
}

const bytes = (hex: string) => Uint8Array.from(Buffer.from(hex, 'hex'));
const EMPTY = new Uint8Array(0);
const vectors: Vector[] = JSON.parse(readFileSync(new URL('./vectors.json', import.meta.url), 'utf8'));

let failures = 0;
let passed = 0;
const check = (name: string, ok: boolean, detail = '') => {
  if (ok) passed++;
  else {
    failures++;
    console.log(`FAIL ${name}${detail ? ` — ${detail}` : ''}`);
  }
};
/** Whether `run` threw an Error — and nothing else, and did not return. */
const refuses = (run: () => unknown): boolean => {
  try {
    run();
    return false;
  } catch (error) {
    return error instanceof Error;
  }
};
/** Whether `run` either returned or threw an Error: never anything else. */
const survives = (run: () => unknown): boolean => {
  try {
    run();
    return true;
  } catch (error) {
    return error instanceof Error;
  }
};

/** A deterministic generator, so a fuzz case that fails fails again. */
function rng(seed: bigint) {
  let state = seed;
  const next = () => {
    state ^= state >> 12n;
    state ^= (state << 25n) & 0xffff_ffff_ffff_ffffn;
    state ^= state >> 27n;
    return Number(((state * 0x2545f4914f6cdd1dn) & 0xffff_ffff_ffff_ffffn) >> 32n);
  };
  const bytesOf = (max: number) => Uint8Array.from({ length: next() % (max + 1) }, () => next() & 0xff);
  return { next, bytes: bytesOf };
}

/** The phone's initiator as the vector ran it, optionally with one input wrong. */
function initiator(v: Vector, change: { host?: Uint8Array; psk?: Uint8Array; prologue?: Uint8Array } = {}) {
  const init = new Initiator({
    staticKey: keypairFrom(bytes(v.phone_private)),
    remoteStatic: change.host ?? bytes(v.host_public),
    prologue: change.prologue ?? bytes(v.prologue),
    psk: change.psk ?? (v.psk ? bytes(v.psk) : undefined),
    ephemeralForTestingOnly: bytes(v.phone_ephemeral),
  });
  init.writeMessage1(EMPTY);
  return init;
}

/** A finished handshake's transport, as the vector finishes it. */
const transport = (v: Vector): Transport => initiator(v).readMessage2(bytes(v.message2)).transport;

/** Two sessions that talk: the phone's, and the host's built from a second
 * run of the same fixed handshake with its ciphers crossed. */
function sessions(v: Vector): { phone: Session; host: Session } {
  const mirror = transport(v);
  return {
    phone: new Session(transport(v)),
    host: new Session({ send: mirror.receive, receive: mirror.send }),
  };
}

// -- the handshake ------------------------------------------------------------

for (const v of vectors) {
  const reply = bytes(v.message2);

  let every = true;
  for (let at = 0; at < reply.length; at++) {
    for (const bit of [0x01, 0x80]) {
      const tampered = reply.slice();
      tampered[at] ^= bit;
      if (!refuses(() => initiator(v).readMessage2(tampered))) every = false;
    }
  }
  check(`${v.pattern}: a flipped bit anywhere in the host's reply is refused`, every);

  check(`${v.pattern}: a truncated reply is refused`, refuses(() => initiator(v).readMessage2(reply.subarray(0, reply.length - 1))));
  check(`${v.pattern}: an empty reply is refused`, refuses(() => initiator(v).readMessage2(EMPTY)));

  const impostor: Keypair = generateKeypair();
  check(
    `${v.pattern}: a phone that pinned another host cannot finish with this one`,
    refuses(() => initiator(v, { host: impostor.publicKey }).readMessage2(reply)),
  );

  const header = bytes(v.prologue);
  header[header.length - 1] ^= 1;
  check(`${v.pattern}: a changed cleartext header fails the handshake`, refuses(() => initiator(v, { prologue: header }).readMessage2(reply)));

  if (v.psk) {
    let everyGuess = true;
    for (let at = 0; at < 32; at++) {
      const guess = bytes(v.psk);
      guess[at] ^= 1;
      if (!refuses(() => initiator(v, { psk: guess }).readMessage2(reply))) everyGuess = false;
    }
    check(`${v.pattern}: every one-bit-wrong pairing secret fails`, everyGuess);
  }

  check(
    `${v.pattern}: a reply before the first message is refused`,
    refuses(() =>
      new Initiator({
        staticKey: keypairFrom(bytes(v.phone_private)),
        remoteStatic: bytes(v.host_public),
        prologue: bytes(v.prologue),
      }).readMessage2(reply),
    ),
  );
  check(
    `${v.pattern}: a host key of the wrong size is refused`,
    refuses(() => new Initiator({ staticKey: generateKeypair(), remoteStatic: new Uint8Array(31), prologue: EMPTY })),
  );

  // -- transport ------------------------------------------------------------------

  const first = bytes(v.transport2);
  const t = transport(v);
  t.receive.decrypt(EMPTY, first);
  check(`${v.pattern}: the host's first message replayed is refused`, refuses(() => t.receive.decrypt(EMPTY, first)));

  let everyTransport = true;
  for (let at = 0; at < first.length; at++) {
    const tampered = first.slice();
    tampered[at] ^= 1;
    if (!refuses(() => transport(v).receive.decrypt(EMPTY, tampered))) everyTransport = false;
  }
  check(`${v.pattern}: a flipped bit anywhere in a transport message is refused`, everyTransport);

  // -- the session ------------------------------------------------------------

  {
    const { phone, host } = sessions(v);
    const [frame] = phone.seal(new TextEncoder().encode('allow once'));
    check(`${v.pattern}: a sealed message opens on the other side`, new TextDecoder().decode(host.open(frame)!) === 'allow once');
    check(`${v.pattern}: the same frame again is refused`, refuses(() => host.open(frame)));
  }
  {
    const { phone, host } = sessions(v);
    phone.seal(Uint8Array.of(1)); // sealed, never delivered
    const [two] = phone.seal(Uint8Array.of(2));
    check(`${v.pattern}: a frame after a lost one is refused`, refuses(() => host.open(two)));
  }
  {
    const { phone, host } = sessions(v);
    const large = Uint8Array.from({ length: 5 * 1024 * 1024 + 123 }, (_, i) => (i * 31) & 0xff);
    const frames = phone.seal(large);
    let whole: Uint8Array | null = null;
    for (const frame of frames) whole = host.open(frame);
    check(
      `${v.pattern}: a 5 MB message crosses in ${frames.length} frames and reassembles`,
      frames.length > 1 && whole !== null && Buffer.compare(Buffer.from(whole), Buffer.from(large)) === 0,
    );
    const [empty] = phone.seal(EMPTY);
    check(`${v.pattern}: an empty message crosses`, host.open(empty)?.length === 0);
  }
  {
    const { phone } = sessions(v);
    check(`${v.pattern}: a message over the limit is refused before it is sealed`, refuses(() => phone.seal(new Uint8Array(MAX_MESSAGE + 1))));
  }
  {
    // Frames the other side's cipher sealed correctly but whose contents
    // break the framing: no flag, an unknown flag, and more than the limit.
    // A second run's receive cipher seals exactly what the phone's opens.
    const phone = new Session(transport(v));
    const raw = transport(v).receive;
    check(`${v.pattern}: a frame with no flag byte is refused`, refuses(() => phone.open(raw.encrypt(EMPTY, EMPTY))));

    const phone2 = new Session(transport(v));
    const raw2 = transport(v).receive;
    check(`${v.pattern}: a frame with an unknown flag is refused`, refuses(() => phone2.open(raw2.encrypt(EMPTY, Uint8Array.of(7, 1, 2)))));

    const phone3 = new Session(transport(v));
    const raw3 = transport(v).receive;
    const chunk = new Uint8Array(65535 - 16);
    chunk[0] = 1; // "more follows", forever
    let refusedGrowth = false;
    for (let i = 0; i < Math.ceil(MAX_MESSAGE / (chunk.length - 1)) + 2; i++) {
      try {
        phone3.open(raw3.encrypt(EMPTY, chunk));
      } catch {
        refusedGrowth = true;
        break;
      }
    }
    check(`${v.pattern}: a message that keeps growing past the limit is refused`, refusedGrowth);
  }
}

// -- pairing codes ------------------------------------------------------------------

const code = (() => {
  const relay = new TextEncoder().encode('ws://Example-MacBook.local:7979');
  const out = new Uint8Array(1 + 16 + 32 + 16 + 32 + 8 + 2 + relay.length);
  let at = 0;
  out[at++] = 1;
  for (const [len, fill] of [
    [16, 0x11],
    [32, 0x22],
    [16, 0x33],
    [32, 0x44],
  ] as const) {
    out.fill(fill, at, at + len);
    at += len;
  }
  new DataView(out.buffer).setBigUint64(at, 1_790_000_000n);
  at += 8;
  out[at++] = relay.length >> 8;
  out[at++] = relay.length & 0xff;
  out.set(relay, at);
  return out;
})();
const text = (raw: Uint8Array) => OFFER_PREFIX + toBase64Url(raw);

{
  const offer = parseOffer(text(code));
  check(
    'a pairing code parses into its fields',
    offer.hostId[0] === 0x11 &&
      offer.hostPublic[31] === 0x22 &&
      offer.secret[0] === 0x44 &&
      offer.expiresAt === 1_790_000_000 &&
      offer.relay === 'ws://Example-MacBook.local:7979',
  );
  check('surrounding whitespace is forgiven', survives(() => parseOffer(`  ${text(code)}\n`)));

  let everyCut = true;
  for (let len = 0; len < code.length; len++) if (!refuses(() => parseOffer(text(code.subarray(0, len))))) everyCut = false;
  check('every truncation of a code is refused', everyCut);

  const longer = new Uint8Array(code.length + 1);
  longer.set(code);
  check('trailing bytes are refused', refuses(() => parseOffer(text(longer))));
  const newer = code.slice();
  newer[0] = 2;
  check('a code from a newer ket is refused', refuses(() => parseOffer(text(newer))));
  for (const bad of ['', OFFER_PREFIX, 'https://example.com', `${OFFER_PREFIX}!!!`]) {
    check(`${JSON.stringify(bad)} is refused`, refuses(() => parseOffer(bad)));
  }

  const r = rng(0x6b6574n);
  let quiet = true;
  for (let i = 0; i < 20_000; i++) {
    const raw = r.bytes(200);
    if (!survives(() => parseOffer(text(raw)))) quiet = false;
    if (!survives(() => parseOffer(new TextDecoder().decode(raw)))) quiet = false;
  }
  check('random codes only ever parse or throw an Error', quiet);
}

// -- relay frames ------------------------------------------------------------------

{
  const body = new TextEncoder().encode('ciphertext');
  const round = decodeFrame(encodeFrame({ kind: KIND_DATA, connection: 2n ** 63n, body }));
  check('a relay frame round-trips', round.kind === KIND_DATA && round.connection === 2n ** 63n && Buffer.compare(Buffer.from(round.body), Buffer.from(body)) === 0);

  const frame = encodeFrame({ kind: KIND_CONTROL, connection: 5n, body });
  let everyCut = true;
  for (let len = 0; len < frame.length; len++) if (!refuses(() => decodeFrame(frame.subarray(0, len)))) everyCut = false;
  check('every truncation of a relay frame is refused', everyCut);

  const longer = new Uint8Array(frame.length + 1);
  longer.set(frame);
  check('a relay frame with bytes past its length is refused', refuses(() => decodeFrame(longer)));
  const version = frame.slice();
  version[0] = 2;
  check('a relay frame of another version is refused', refuses(() => decodeFrame(version)));
  const kind = frame.slice();
  kind[1] = 9;
  check('a relay frame of an unknown kind is refused', refuses(() => decodeFrame(kind)));
  check('a relay frame over 1 MiB is not encoded', refuses(() => encodeFrame({ kind: KIND_DATA, connection: 0n, body: new Uint8Array(MAX_FRAME - HEADER + 1) })));
  const huge = new Uint8Array(MAX_FRAME + 1);
  huge[0] = 1;
  huge[1] = KIND_DATA;
  new DataView(huge.buffer).setUint32(10, huge.length - HEADER);
  check('a relay frame over 1 MiB is not decoded', refuses(() => decodeFrame(huge)));

  const r = rng(0xf00dn);
  let quiet = true;
  for (let i = 0; i < 50_000; i++) {
    const raw = r.bytes(64);
    if (i % 2 === 0 && raw.length >= 2) {
      raw[0] = 1;
      raw[1] = 1 + (i % 2);
    }
    if (!survives(() => decodeFrame(raw))) quiet = false;
  }
  check('random relay bytes only ever decode or throw an Error', quiet);
}

console.log(`${passed} passed, ${failures} failed`);
if (failures > 0) process.exit(1);
