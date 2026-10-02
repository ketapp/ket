// The TypeScript phone against a real ket host, through the local relay:
// pair from a code, list the host's terminals, attach to one and render it in
// xterm.js — the emulator the app will use — take control, type, see the
// result; then reconnect with the cursor and check it resumes.
//
//   node test/live.ts "$(ket host pair)"

import { create } from '@bufbuild/protobuf';
import headless from '@xterm/headless';

import {
  type Envelope,
  InputSchema,
  LeaseSchema,
  Lease_Action,
  SubscribeSchema,
  envelope,
  generateKeypair,
  pair,
  resume,
} from '../src/index.ts';

const { Terminal } = headless;

const code = process.argv[2];
if (!code) {
  console.error('usage: node test/live.ts <pairing code>');
  process.exit(2);
}

let failures = 0;
const check = (name: string, ok: boolean, detail = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ` — ${detail}` : ''}`);
  if (!ok) failures++;
};

const keys = generateKeypair();
const { remote, host } = await pair(code, keys, { deviceName: 'typescript phone', appVersion: '0' });

let epoch = 0n;
let seq = 0n;
const seen = (message: Envelope) => {
  if (message.seq > seq) seq = message.seq;
  return message;
};

// Welcome, then the snapshot.
let terminalId = 0n;
for (;;) {
  const message = seen(await remote.recv(5000));
  if (message.payload.case === 'welcome') {
    epoch = message.payload.value.hostEpoch;
    check('paired through the relay, and welcomed', true, `epoch ${epoch}`);
  }
  if (message.payload.case === 'snapshot') {
    const terminals = message.payload.value.projects.flatMap((p) => p.worktrees.flatMap((w) => w.terminals));
    check('the snapshot lists the host\'s terminals', terminals.length > 0, terminals.map((t) => `[${t.id}] ${t.title}`).join(', '));
    terminalId = terminals[0]?.id ?? 0n;
    break;
  }
}

// Attach, and render what arrives in xterm.js.
let term: InstanceType<typeof Terminal> | null = null;
const write = (bytes: Uint8Array) => new Promise<void>((done) => term!.write(bytes, done));
const apply = async (message: Envelope) => {
  switch (message.payload.case) {
    case 'checkpoint': {
      const c = message.payload.value;
      term?.dispose();
      term = new Terminal({ cols: c.cols, rows: c.rows, scrollback: c.scrollback, allowProposedApi: true });
      await write(c.ansi);
      break;
    }
    case 'chunk':
      if (term) await write(message.payload.value.bytes);
      break;
    case 'reset':
      term?.dispose();
      term = null;
      break;
  }
};
const pump = async (ms: number, until?: (m: Envelope) => boolean): Promise<Envelope | null> => {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    let message: Envelope;
    try {
      message = seen(await remote.recv(Math.max(1, deadline - Date.now())));
    } catch {
      return null;
    }
    await apply(message);
    if (until?.(message)) return message;
  }
  return null;
};
const screen = () => {
  const buffer = term!.buffer.active;
  const lines: string[] = [];
  for (let y = 0; y < buffer.length; y++) lines.push(buffer.getLine(y)!.translateToString(true));
  return lines;
};

remote.send(envelope(2n, { case: 'subscribe', value: create(SubscribeSchema, { terminalId }) }));
await pump(3000, (m) => m.payload.case === 'checkpoint');
// Read through a function: `term` is assigned inside callbacks, and a
// direct read here is narrowed to its initial `null`.
const rendered = (): InstanceType<typeof Terminal> | null => term;
const shown = rendered();
check('a checkpoint arrives and renders', shown !== null, shown ? `${shown.cols}x${shown.rows}` : '');

remote.send(
  envelope(3n, {
    case: 'lease',
    value: create(LeaseSchema, { terminalId, action: Lease_Action.ACQUIRE }),
  }),
);
const reply = await pump(3000, (m) => m.requestId === 3n);
check('control of the terminal is granted', reply?.payload.case === 'ack');

remote.send(
  envelope(4n, {
    case: 'input',
    value: create(InputSchema, {
      terminalId,
      bytes: new TextEncoder().encode('echo hi from typescript $((6*7))\r'),
    }),
  }),
);
await pump(1500);
const lines = screen();
check(
  'what the phone typed ran on the host, and the phone shows it',
  lines.some((l) => l.trim() === 'hi from typescript 42'),
);
remote.close();

// Back again, with the cursor.
const again = await resume(host, keys, { deviceName: 'typescript phone', resumeEpoch: epoch, resumeSeq: seq });
const welcome = await again.recv(5000);
check(
  `reconnecting with the cursor (${epoch}, ${seq}) resumes`,
  welcome.payload.case === 'welcome' && welcome.payload.value.resumed,
);
again.close();

console.log('\n--- what the phone showed ---');
const last = lines.map((l) => l.trimEnd());
while (last.length && !last[last.length - 1]) last.pop();
console.log(last.slice(-6).join('\n'));

console.log();
if (failures) {
  console.log(`${failures} check(s) failed`);
  process.exit(1);
}
console.log('all checks passed');
process.exit(0);
