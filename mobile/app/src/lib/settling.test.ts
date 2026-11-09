import { describe, expect, test } from '@jest/globals';
import type { HostSnapshot } from '@ket/remote';

import { SETTLE_MS, settlesAt, settling } from './settling';

const worktree = { id: 'w', terminals: [] };
const snapshot = (worktrees: object[] = []) =>
  ({ projects: [{ id: 'p', name: 'P', worktrees }], usage: [], agents: [], addresses: [] }) as unknown as HostSnapshot;
const conn = (over: object = {}) => ({
  status: 'connected' as const,
  snapshot: snapshot(),
  starting: [],
  freshSince: 1_000,
  snapshotAt: 1_200,
  ...over,
});

describe('settling', () => {
  test('connected with no list yet is filling, however long it takes', () => {
    expect(settling(conn({ snapshot: null, snapshotAt: null }), 1_100)).toBe(true);
    expect(settling(conn({ snapshot: null, snapshotAt: null }), 60_000)).toBe(true);
  });

  test('not while the phone is still reaching the desktop', () => {
    expect(settling(conn({ status: 'connecting', snapshot: null }), 1_100)).toBe(false);
    expect(settling(conn({ status: 'offline', snapshot: null }), 1_100)).toBe(false);
  });

  test('an empty list waits out the settle before it is believed', () => {
    expect(settling(conn(), 1_200)).toBe(true);
    expect(settling(conn(), 1_000 + SETTLE_MS - 1)).toBe(true);
    expect(settling(conn(), 1_000 + SETTLE_MS)).toBe(false);
  });

  test('a list with a worktree in it, or work starting, shows at once', () => {
    expect(settling(conn({ snapshot: snapshot([worktree]) }), 1_200)).toBe(false);
    expect(settling(conn({ starting: [{ at: 1 }] }), 1_200)).toBe(false);
  });

  test('a list from before the desktop restarted is not shown as its own', () => {
    const restarted = conn({ snapshot: snapshot([worktree]), freshSince: 5_000, snapshotAt: 4_000 });
    expect(settling(restarted, 5_100)).toBe(true);
    expect(settling({ ...restarted, snapshotAt: 5_050 }, 5_100)).toBe(false);
  });

  test('a connection that has never been welcomed holds nothing back', () => {
    expect(settling(conn({ freshSince: null }), 1_200)).toBe(false);
    expect(settling(conn({ freshSince: undefined, snapshotAt: undefined }), 1_200)).toBe(false);
  });

  test('settlesAt is the end of the wait', () => {
    expect(settlesAt(conn())).toBe(1_000 + SETTLE_MS);
    expect(settlesAt(conn({ freshSince: null }))).toBeNull();
  });
});
