// Checks the TypeScript Noise initiator against vectors written by `snow`
// (crates/ket-remote/examples/vectors.rs): with every key fixed, the phone's
// first message must be byte-for-byte snow's, snow's reply must finish the
// handshake, and both sides' first transport messages must agree.
//
//   node test/vectors.ts

import { readFileSync } from 'node:fs';

import { Initiator, keypairFrom } from '../src/noise.ts';

interface Vector {
  pattern: string;
  phone_private: string;
  phone_public: string;
  host_public: string;
  phone_ephemeral: string;
  psk: string;
  prologue: string;
  message1: string;
  message2: string;
  phone_plaintext: string;
  transport1: string;
  host_plaintext: string;
  transport2: string;
}

const bytes = (hex: string) => Uint8Array.from(Buffer.from(hex, 'hex'));
const hex = (b: Uint8Array) => Buffer.from(b).toString('hex');
const EMPTY = new Uint8Array(0);

const vectors: Vector[] = JSON.parse(readFileSync(new URL('./vectors.json', import.meta.url), 'utf8'));
let failures = 0;
const check = (name: string, ok: boolean, detail = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ` — ${detail}` : ''}`);
  if (!ok) failures++;
};

for (const v of vectors) {
  const staticKey = keypairFrom(bytes(v.phone_private));
  check(`${v.pattern}: the phone's public key derives the same`, hex(staticKey.publicKey) === v.phone_public);

  const initiator = new Initiator({
    staticKey,
    remoteStatic: bytes(v.host_public),
    prologue: bytes(v.prologue),
    psk: v.psk ? bytes(v.psk) : undefined,
    ephemeralForTestingOnly: bytes(v.phone_ephemeral),
  });
  const message1 = initiator.writeMessage1(EMPTY);
  check(`${v.pattern}: message 1 matches snow byte for byte`, hex(message1) === v.message1);

  let transport;
  try {
    ({ transport } = initiator.readMessage2(bytes(v.message2)));
    check(`${v.pattern}: snow's reply finishes the handshake`, true);
  } catch (error) {
    check(`${v.pattern}: snow's reply finishes the handshake`, false, String(error));
    continue;
  }

  const sent = transport.send.encrypt(EMPTY, bytes(v.phone_plaintext));
  check(`${v.pattern}: the phone's first transport message matches`, hex(sent) === v.transport1);
  try {
    const received = transport.receive.decrypt(EMPTY, bytes(v.transport2));
    check(`${v.pattern}: the host's first transport message decrypts`, hex(received) === v.host_plaintext);
  } catch (error) {
    check(`${v.pattern}: the host's first transport message decrypts`, false, String(error));
  }

  // The negatives: a flipped bit in the reply, and a wrong pairing secret.
  const tampered = bytes(v.message2);
  tampered[tampered.length - 1] ^= 1;
  const again = new Initiator({
    staticKey,
    remoteStatic: bytes(v.host_public),
    prologue: bytes(v.prologue),
    psk: v.psk ? bytes(v.psk) : undefined,
    ephemeralForTestingOnly: bytes(v.phone_ephemeral),
  });
  again.writeMessage1(EMPTY);
  let refused = false;
  try {
    again.readMessage2(tampered);
  } catch {
    refused = true;
  }
  check(`${v.pattern}: a tampered reply is refused`, refused);

  if (v.psk) {
    const wrong = bytes(v.psk);
    wrong[0] ^= 1;
    const guesser = new Initiator({
      staticKey,
      remoteStatic: bytes(v.host_public),
      prologue: bytes(v.prologue),
      psk: wrong,
      ephemeralForTestingOnly: bytes(v.phone_ephemeral),
    });
    guesser.writeMessage1(EMPTY);
    let failed = false;
    try {
      guesser.readMessage2(bytes(v.message2));
    } catch {
      failed = true;
    }
    check(`${v.pattern}: a wrong pairing secret fails`, failed);
  }
}

console.log();
if (failures) {
  console.log(`${failures} check(s) failed`);
  process.exit(1);
}
console.log('all checks passed');
