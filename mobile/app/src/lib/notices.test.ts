import { beforeEach, describe, expect, jest, test } from '@jest/globals';

import { compare, forgetSeen, turnOff, turnOn, useNotices } from './notices';
import type { NotifyConfig } from './store';

const mockNotifications = {
  getPermissionsAsync: jest.fn(async () => ({ granted: false, canAskAgain: true })),
  requestPermissionsAsync: jest.fn(async () => ({ granted: true })),
  dismissAllNotificationsAsync: jest.fn(async () => {}),
  setBadgeCountAsync: jest.fn(async (_n: number) => {}),
};
let mockStored: NotifyConfig | null = null;

jest.mock('expo-notifications', () => ({
  AndroidImportance: { HIGH: 4 },
  getPermissionsAsync: () => mockNotifications.getPermissionsAsync(),
  requestPermissionsAsync: () => mockNotifications.requestPermissionsAsync(),
  dismissAllNotificationsAsync: () => mockNotifications.dismissAllNotificationsAsync(),
  setBadgeCountAsync: (n: number) => mockNotifications.setBadgeCountAsync(n),
  dismissNotificationAsync: jest.fn(async () => {}),
  scheduleNotificationAsync: jest.fn(async () => ''),
  setNotificationHandler: jest.fn(),
  setNotificationChannelAsync: jest.fn(async () => null),
  addNotificationResponseReceivedListener: jest.fn(),
  useLastNotificationResponse: () => null,
}));
jest.mock('expo-router', () => ({ router: { push: jest.fn() } }));
jest.mock('./connection', () => ({ connection: jest.fn() }));
jest.mock('./store', () => ({
  DEFAULT_NOTIFY: { on: false, waiting: true, finished: true, checks: true },
  getNotify: jest.fn(async () => mockStored ?? { on: false, waiting: true, finished: true, checks: true }),
  saveNotify: jest.fn(async (config: NotifyConfig) => {
    mockStored = config;
  }),
  listHosts: jest.fn(async () => []),
}));
jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
  CheckSummary_State: { STATE_UNSPECIFIED: 0, PASSED: 1, FAILED: 2, PENDING: 3, SKIPPED: 4 },
}));

const host = { id: 'host', name: 'Studio Mac' };
const tree = (id: string, activity: number, extra: object = {}) => ({
  id,
  name: id,
  branch: id,
  agent: 'codex',
  agentState: '',
  activity,
  terminals: [],
  asleep: [],
  questions: [],
  ...extra,
});
const snapshot = (worktrees: object[]) => ({ projects: [{ id: 'p', name: 'Project', color: '', worktrees }], usage: [], agents: [] }) as never;

beforeEach(() => {
  jest.clearAllMocks();
  forgetSeen();
  mockStored = null;
});

describe('compare', () => {
  test('the first snapshot is not news; the move into waiting is, and answering takes it back', () => {
    expect(compare(host, snapshot([tree('a', 4)]), false)).toEqual({ fresh: [], answered: [] });
    // Still waiting: said once.
    expect(compare(host, snapshot([tree('a', 4)]), false).fresh).toEqual([]);
    const { fresh } = compare(
      host,
      snapshot([tree('a', 4), tree('b', 4, { question: { terminalId: 9n, subject: 'Run rm -rf build?' } })]),
      true,
    );
    expect(fresh).toEqual([
      {
        kind: 'waiting',
        id: 'host/b',
        title: 'Project · b',
        subtitle: 'Studio Mac',
        body: 'Codex is waiting on you: Run rm -rf build?',
        path: '/session/host/9',
      },
    ]);
    expect(compare(host, snapshot([tree('a', 2)]), false)).toEqual({ fresh: [], answered: ['host/a', 'host/b'] });
  });

  test('a working agent going quiet finished, with its last words; failing failed', () => {
    compare(host, snapshot([tree('a', 2), tree('b', 1)]), false);
    const { fresh } = compare(host, snapshot([tree('a', 0, { agentState: 'pushed the branch' }), tree('b', 5)]), false);
    expect(fresh.map((n) => [n.kind, n.id, n.body, n.path])).toEqual([
      ['finished', 'host/a/finished', 'Codex finished: pushed the branch', '/worktree/host/a'],
      ['finished', 'host/b/finished', 'Codex failed.', '/worktree/host/b'],
    ]);
    // A shell's worktree going quiet is nobody's task.
    compare(host, snapshot([tree('c', 1, { agent: '' })]), false);
    expect(compare(host, snapshot([tree('c', 0, { agent: '' })]), false).fresh).toEqual([]);
  });

  test('a check failing on a pull request is said once, naming the checks', () => {
    const pr = (states: number[]) => ({
      pullRequest: { number: 12n, checks: states.map((state, i) => ({ name: `check ${i}`, state, url: '' })) },
    });
    compare(host, snapshot([tree('a', 2, pr([3, 3]))]), false);
    const { fresh } = compare(host, snapshot([tree('a', 2, pr([1, 2]))]), false);
    expect(fresh).toEqual([
      {
        kind: 'checks',
        id: 'host/a/checks',
        title: 'Project · a',
        subtitle: undefined,
        body: 'Checks failed on #12: check 1',
        path: '/pull-request/host/a',
      },
    ]);
    expect(compare(host, snapshot([tree('a', 2, pr([2, 2]))]), false).fresh).toEqual([]);
  });
});

describe('the switch', () => {
  test('turning on asks the phone, and a refusal leaves it on but marked', async () => {
    expect(await turnOn()).toBe(true);
    expect(mockNotifications.requestPermissionsAsync).toHaveBeenCalled();
    expect(mockStored).toEqual({ on: true, waiting: true, finished: true, checks: true });

    mockNotifications.getPermissionsAsync.mockResolvedValueOnce({ granted: false, canAskAgain: false });
    expect(await turnOn()).toBe(false);
    expect(mockNotifications.requestPermissionsAsync).toHaveBeenCalledTimes(1);

    await turnOff();
    expect(mockStored?.on).toBe(false);
    expect(mockNotifications.dismissAllNotificationsAsync).toHaveBeenCalled();
    expect(mockNotifications.setBadgeCountAsync).toHaveBeenCalledWith(0);
    expect(typeof useNotices).toBe('function');
  });
});
