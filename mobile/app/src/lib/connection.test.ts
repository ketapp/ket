import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { ErrorCode, type Envelope } from '@ket/remote';
import { HostConnection } from './connection';
import type { SavedHost } from './store';

jest.mock('expo-device', () => ({ deviceName: 'test phone' }));
jest.mock('./store', () => ({
  phoneKey: jest.fn(),
  saveHost: jest.fn(),
  toPaired: jest.fn(),
}));
jest.mock('./version', () => ({ versionLabel: 'test' }));
jest.mock('@ket/remote', () => ({
  AnswerSchema: 'answer',
  ErrorCode: { UNSUPPORTED_VERSION: 1, NOT_CONTROLLER: 2 },
  FixChecksSchema: 'fixChecks',
  GetChangesSchema: 'getChanges',
  GetConversationSchema: 'getConversation',
  GetDiffSchema: 'getDiff',
  GetSnippetsSchema: 'getSnippets',
  InputSchema: 'input',
  Lease_Action: { ACQUIRE: 1, RENEW: 2, RELEASE: 3 },
  LeaseSchema: 'lease',
  ListDevicesSchema: 'listDevices',
  MergeWorktreeSchema: 'mergeWorktree',
  ResizeSchema: 'resize',
  RevokeDeviceSchema: 'revokeDevice',
  Signal_Kind: { INTERRUPT: 1 },
  SignalSchema: 'signal',
  StartWorkSchema: 'startWork',
  GetStartOptionsSchema: 'getStartOptions',
  StartSessionSchema: 'startSession',
  SubscribeSchema: 'subscribe',
  UnsubscribeSchema: 'unsubscribe',
  create: (_schema: string, value: object) => value,
  envelope: (requestId: bigint, payload: Envelope['payload']) => ({ requestId, payload, idempotencyKey: '' }),
  resume: jest.fn(),
  toBase64Url: (bytes: Uint8Array) => `key-${bytes[0]}`,
  utf8: (bytes: Uint8Array) => String.fromCharCode(...bytes),
}));

type RemoteStub = { send: jest.Mock<(message: Envelope) => void>; close: jest.Mock<() => void> };
type Internals = {
  remote: RemoteStub | null;
  nextRequest: bigint;
  pending: Map<bigint, { resolve: (reply: Envelope) => void; reject: (error: Error) => void }>;
  listeners: Set<(message: Envelope) => void>;
  changed: Set<() => void>;
  leases: Map<bigint, ReturnType<typeof setInterval>>;
  closed: boolean;
  backoff: number;
  wake: (() => void) | null;
  notify: () => void;
};

const reply = (payload: { case: string; value?: unknown }): Envelope =>
  ({ requestId: 10n, seq: 0n, payload }) as unknown as Envelope;

let nextReply: Envelope;
let keys = 0;
let conn: HostConnection;
let inside: Internals;
let remote: RemoteStub;
let sent: Envelope[];

beforeEach(() => {
  jest.useFakeTimers();
  Object.defineProperty(globalThis, 'crypto', {
    configurable: true,
    // A new key each time one is made, so a test can tell them apart.
    value: { getRandomValues: (bytes: Uint8Array) => (bytes.fill(++keys % 256), bytes) },
  });
  sent = [];
  nextReply = reply({ case: 'ack', value: {} });
  conn = Object.create(HostConnection.prototype) as HostConnection;
  inside = conn as unknown as Internals;
  remote = {
    close: jest.fn(),
    send: jest.fn((message: Envelope) => {
      sent.push(message);
      const response = { ...nextReply, requestId: message.requestId } as Envelope;
      void Promise.resolve().then(() => inside.pending.get(message.requestId)?.resolve(response));
    }),
  };
  Object.assign(inside, {
    remote,
    nextRequest: 10n,
    pending: new Map(),
    listeners: new Set(),
    changed: new Set(),
    leases: new Map(),
    closed: false,
    backoff: 8000,
    wake: null,
    watched: new Set(),
    backgroundedAt: null,
    redialing: false,
    appState: { remove: () => {} },
    snapshot: null,
    starting: [],
  });
  Object.assign(conn, { host: { id: 'host' } as SavedHost, status: 'connected' });
});

afterEach(() => {
  conn.close();
  jest.useRealTimers();
});

describe('HostConnection requests', () => {
  test('subscribes and unsubscribes', async () => {
    conn.subscribe(4n);
    conn.unsubscribe(4n);
    await Promise.resolve();
    expect(sent.map(({ payload }) => payload)).toEqual([
      { case: 'subscribe', value: { terminalId: 4n } },
      { case: 'unsubscribe', value: { terminalId: 4n } },
    ]);
  });

  test('takes, renews, and releases terminal control', async () => {
    await expect(conn.takeControl(4n)).resolves.toBe(true);
    expect(sent[0].idempotencyKey).toMatch(/^key-/);
    expect(inside.leases.has(4n)).toBe(true);

    nextReply = reply({ case: 'error', value: { code: 0, message: 'busy' } });
    await expect(conn.takeControl(5n)).resolves.toBe(false);

    conn.releaseControl(4n);
    await Promise.resolve();
    expect(inside.leases.has(4n)).toBe(false);
    expect(sent.at(-1)?.payload).toEqual({ case: 'lease', value: { terminalId: 4n, action: 3 } });
  });

  test('types bytes and drops its lease when control was lost', async () => {
    inside.leases.set(4n, setInterval(() => {}, 1000));
    nextReply = reply({ case: 'error', value: { code: ErrorCode.NOT_CONTROLLER, message: 'lost' } });
    await expect(conn.type(4n, 'hello')).resolves.toBe(false);
    expect(inside.leases.has(4n)).toBe(false);
    expect([...((sent[0].payload.value as { bytes: Uint8Array }).bytes)]).toEqual([104, 101, 108, 108, 111]);

    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.type(4n, 'ok')).resolves.toBe(true);
  });

  test('lists devices and rejects malformed replies', async () => {
    const devices = [{ id: 'phone' }];
    nextReply = reply({ case: 'devices', value: { devices } });
    await expect(conn.devices()).resolves.toBe(devices);
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.devices()).rejects.toThrow('did not list');
  });

  test('starts and merges work and fixes checks, returning host errors', async () => {
    await expect(conn.startWork('project', 'fix it', 'codex')).resolves.toBeNull();
    expect(sent[0].payload).toEqual({
      case: 'startWork',
      value: { projectId: 'project', prompt: 'fix it', agent: 'codex' },
    });
    // The desktop took it: a row stands in for the work until it arrives.
    expect(conn.starting).toEqual([
      { project: 'project', prompt: 'fix it', agent: 'codex', at: expect.any(Number), before: [] },
    ]);
    nextReply = reply({ case: 'error', value: { code: 0, message: 'ket is closed' } });
    await expect(conn.startWork('project', 'fix it')).resolves.toBe('ket is closed');
    expect(conn.starting).toHaveLength(1);
    // Given up on once it has taken too long to arrive.
    jest.advanceTimersByTime(120_100);
    expect(conn.starting).toEqual([]);
    await expect(conn.merge('tree')).resolves.toBe('ket is closed');
    await expect(conn.fixChecks('tree')).resolves.toEqual({ why: 'ket is closed' });
    expect(sent.at(-1)?.payload).toEqual({ case: 'fixChecks', value: { worktreeId: 'tree' } });
    nextReply = reply({ case: 'checksHandedOver', value: { starting: true } });
    await expect(conn.fixChecks('tree')).resolves.toEqual({ starting: true });
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.merge('tree')).resolves.toBeNull();
  });

  test('sends explicit start choices and opens fresh sessions', async () => {
    await expect(conn.startWork('project', 'fix it', 'codex', { base: 'release', model: 'gpt-6.1-sol', effort: 'high' })).resolves.toBeNull();
    expect(sent[0].payload).toEqual({ case: 'startWork', value: {
      projectId: 'project', prompt: 'fix it', agent: 'codex', base: 'release', model: 'gpt-6.1-sol', effort: 'high',
    } });
    nextReply = reply({ case: 'startOptions', value: { projectId: 'project', defaultBase: 'main', localBranches: ['main'] } });
    await expect(conn.startOptions('project')).resolves.toMatchObject({ defaultBase: 'main' });
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.startSession('tree', 'codex', 'review it', 'gpt-6.1-sol', 'high')).resolves.toBeNull();
    expect(sent.at(-1)?.payload).toEqual({ case: 'startSession', value: {
      worktreeId: 'tree', agent: 'codex', prompt: 'review it', model: 'gpt-6.1-sol', effort: 'high',
    } });
  });

  test('fixing checks asks again under the same key after the connection drops', async () => {
    // The first try's session ends before the desktop answers.
    remote.send.mockImplementationOnce((message: Envelope) => {
      sent.push(message);
      void Promise.resolve().then(() => inside.pending.get(message.requestId)?.reject(new Error('the connection to the desktop dropped')));
    });
    Object.assign(conn, { status: 'offline' });
    nextReply = reply({ case: 'checksHandedOver', value: { starting: false } });
    const fixed = conn.fixChecks('tree');
    await Promise.resolve();
    await Promise.resolve();
    expect(sent).toHaveLength(1);
    // Something else asked meanwhile gets a key of its own.
    void conn.merge('other').catch(() => {});
    // Welcomed again: the same ask, under the same key.
    Object.assign(conn, { status: 'connected' });
    inside.notify();
    await expect(fixed).resolves.toEqual({ starting: false });
    const tries = sent.filter((message) => message.payload.case === 'fixChecks');
    expect(tries).toHaveLength(2);
    expect(tries[1].idempotencyKey).toBe(tries[0].idempotencyKey);
    expect(tries[1].requestId).not.toBe(tries[0].requestId);
    const merge = sent.find((message) => message.payload.case === 'mergeWorktree');
    expect(merge?.idempotencyKey).not.toBe(tries[0].idempotencyKey);
  });

  test('reads changes and diffs, preserving host errors', async () => {
    const changes = { files: [] };
    nextReply = reply({ case: 'changes', value: changes });
    await expect(conn.changes('tree')).resolves.toBe(changes);
    nextReply = reply({ case: 'error', value: { code: 0, message: 'gone' } });
    await expect(conn.changes('tree')).rejects.toThrow('gone');
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.changes('tree')).rejects.toThrow('did not list');

    const diff = { text: 'patch' };
    nextReply = reply({ case: 'diff', value: diff });
    await expect(conn.diff('tree', 'file')).resolves.toBe(diff);
    nextReply = reply({ case: 'error', value: { code: 0, message: 'binary' } });
    await expect(conn.diff('tree', 'file')).rejects.toThrow('binary');
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.diff('tree', 'file')).rejects.toThrow('did not send');
  });

  test('reads snippets and conversations', async () => {
    const snippets = [{ name: 'review', body: 'Review this' }];
    nextReply = reply({ case: 'snippets', value: { snippets } });
    await expect(conn.snippets()).resolves.toBe(snippets);
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.snippets()).resolves.toEqual([]);

    const conversation = { turns: [], next: 2n };
    nextReply = reply({ case: 'conversation', value: conversation });
    await expect(conn.conversation(4n, 1n)).resolves.toBe(conversation);
    nextReply = reply({ case: 'error', value: { code: 0, message: 'gone' } });
    await expect(conn.conversation(4n, 1n)).rejects.toThrow('gone');
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.conversation(4n, 1n)).rejects.toThrow('did not send');
  });

  test('revokes devices and answers permissions', async () => {
    await expect(conn.revoke('phone')).resolves.toBeUndefined();
    nextReply = reply({ case: 'error', value: { code: 0, message: 'unknown' } });
    await expect(conn.revoke('phone')).rejects.toThrow('unknown');
    await expect(conn.answer({ id: 8n, terminalId: 4n }, 1)).resolves.toBe('unknown');
    nextReply = reply({ case: 'ack', value: {} });
    await expect(conn.answer({ id: 8n, terminalId: 4n }, 1)).resolves.toBeNull();
  });

  test('resizes and interrupts terminals', async () => {
    await expect(conn.resize(4n, 80, 24)).resolves.toBe(true);
    nextReply = reply({ case: 'error', value: { code: 0, message: 'busy' } });
    await expect(conn.resize(4n, 80, 24)).resolves.toBe(false);
    nextReply = reply({ case: 'ack', value: {} });
    conn.interrupt(4n);
    await Promise.resolve();
    expect(sent.at(-1)?.payload).toEqual({ case: 'signal', value: { terminalId: 4n, kind: 1 } });
  });

  test('rejects disconnected, thrown, and timed-out sends', async () => {
    inside.remote = null;
    await expect(conn.takeControl(1n)).rejects.toThrow('not connected');

    inside.remote = remote;
    remote.send.mockImplementationOnce(() => {
      throw new Error('socket closed');
    });
    await expect(conn.startWork('p', 'x')).rejects.toThrow('socket closed');

    remote.send.mockImplementationOnce(() => {});
    const timedOut = conn.startWork('p', 'x');
    const rejects = expect(timedOut).rejects.toThrow('no answer');
    await jest.advanceTimersByTimeAsync(10_000);
    await rejects;
  });

  test('manages listeners, retry, and close state', () => {
    const changed = jest.fn();
    const message = jest.fn();
    const offChanged = conn.onChange(changed);
    const offMessage = conn.onMessage(message);
    inside.notify();
    expect(changed).toHaveBeenCalledTimes(1);
    offChanged();
    offMessage();
    expect(inside.changed.size).toBe(0);
    expect(inside.listeners.size).toBe(0);

    const wake = jest.fn();
    inside.wake = wake;
    conn.status = 'offline';
    conn.retry();
    expect(remote.close).toHaveBeenCalled();
    expect(wake).toHaveBeenCalled();
    expect(inside.backoff).toBe(1000);

    conn.close();
    expect(inside.closed).toBe(true);
  });
});
