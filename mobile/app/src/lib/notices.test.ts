import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { renderHook } from '@testing-library/react-native';

import { compare, forgetSeen, startNotices, turnOff, turnOn, useNoticeFollowing, useNotices } from './notices';
import type { NotifyConfig } from './store';

const mockNotifications = {
  getPermissionsAsync: jest.fn(async () => ({ granted: false, canAskAgain: true })),
  requestPermissionsAsync: jest.fn(async () => ({ granted: true })),
  dismissAllNotificationsAsync: jest.fn(async () => {}),
  setBadgeCountAsync: jest.fn(async (_n: number) => {}),
};
let mockStored: NotifyConfig | null = null;
// The tap the app last had, as expo-notifications hands it over.
let mockResponse: object | null = null;
const mockListen = jest.fn((_open: (response: object) => void) => ({ remove: () => {} }));
const mockPush = jest.fn((_path: string) => {});

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
  addNotificationResponseReceivedListener: (open: (response: object) => void) => mockListen(open),
  useLastNotificationResponse: () => mockResponse,
}));
jest.mock('expo-router', () => ({ router: { push: (path: string) => mockPush(path) } }));
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

  test("a tap opens the agent's chat: the one that finished, and the newest prompt's", () => {
    // The agent's tab, not the shell beside it: the chat that finished.
    const terminals = [
      { id: 3n, agent: '', closed: false },
      { id: 5n, agent: 'codex', closed: false },
    ];
    compare(host, snapshot([tree('a', 2, { terminals }), tree('b', 0, { terminals })]), false);
    const { fresh } = compare(
      host,
      snapshot([
        tree('a', 0, { terminals }),
        // A dialog read off the screen is only in the list; the single field
        // keeps an older prompt in another tab.
        tree('b', 4, {
          terminals,
          question: { terminalId: 5n, subject: 'Run tests?' },
          questions: [
            { terminalId: 9n, tool: 'Dialog', subject: 'Implement this plan?' },
            { terminalId: 5n, subject: 'Run tests?' },
          ],
        }),
      ]),
      false,
    );
    expect(fresh.map((n) => [n.kind, n.path])).toEqual([
      ['finished', '/session/host/5'],
      ['waiting', '/session/host/9'],
    ]);
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

describe('a tapped notice', () => {
  const tap = (identifier: string, date: number, path: string) => ({
    notification: { date, request: { identifier, content: { data: { path } } } },
  });

  test('opens once the screens are there, and once for the listener and the last response both', async () => {
    mockResponse = tap('host/a', 1_000, '/session/host/7');
    const hook = await renderHook(({ ready }: { ready: boolean }) => useNoticeFollowing(ready), {
      initialProps: { ready: false },
    });
    // Pushed before the stack is drawn, it was dropped.
    expect(mockPush).not.toHaveBeenCalled();
    await hook.rerender({ ready: true });
    expect(mockPush).toHaveBeenCalledWith('/session/host/7');

    // The listener hands over the same tap: not a second chat on top.
    startNotices();
    const open = mockListen.mock.calls[0][0];
    open(mockResponse!);
    expect(mockPush).toHaveBeenCalledTimes(1);
    // The same notice tapped again, later, is a tap of its own.
    open(tap('host/a', 2_000, '/session/host/7'));
    expect(mockPush).toHaveBeenCalledTimes(2);
    // Something that is no screen's path opens nothing.
    open(tap('host/b', 3_000, 'https://example.com'));
    expect(mockPush).toHaveBeenCalledTimes(2);
    await hook.unmount();
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
