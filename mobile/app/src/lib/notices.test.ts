import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, renderHook } from '@testing-library/react-native';
import * as Notifications from 'expo-notifications';
import { AppState } from 'react-native';

import { connection } from './connection';
import { listHosts } from './store';

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
  addPushTokenListener: jest.fn(() => ({ remove: () => {} })),
  getPresentedNotificationsAsync: jest.fn(async () => []),
  useLastNotificationResponse: () => mockResponse,
}));
// Over Worktrees, whatever was open: the stack is cleared down to it first.
jest.mock('expo-router', () => ({
  router: { dismissTo: () => {}, navigate: () => {}, push: (path: string) => mockPush(path) },
}));
jest.mock('./connection', () => ({ connection: jest.fn() }));
jest.mock('./store', () => ({
  DEFAULT_NOTIFY: { on: false, waiting: true, finished: true, checks: true, limits: true },
  getNotify: jest.fn(async () => mockStored ?? { on: false, waiting: true, finished: true, checks: true, limits: true }),
  saveNotify: jest.fn(async (config: NotifyConfig) => {
    mockStored = config;
  }),
  listHosts: jest.fn(async () => []),
}));
jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
  CheckSummary_State: { STATE_UNSPECIFIED: 0, PASSED: 1, FAILED: 2, PENDING: 3, SKIPPED: 4 },
  SessionLimit_Scope: { SCOPE_UNSPECIFIED: 0, PLAN: 1, MODEL: 2 },
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

describe('a plan running out', () => {
  const NOW = 1_800_000_000_000;
  const HOUR = 60 * 60 * 1000;
  const limit = (since: number, resetsAt: number, scope = 1) => ({
    scope,
    window: 1,
    sinceMs: BigInt(since),
    resetsAtMs: BigInt(resetsAt),
    said: 'You\u2019ve hit your usage limit.',
  });
  const stopped = (id: string, resetsAt: number, activity = 0) =>
    tree(id, activity, { terminals: [{ id: 7n, agent: 'codex', closed: false, limit: limit(NOW - 1000, resetsAt) }] });

  test('is said in place of finished, with when it is back, and opens the chat it stopped', () => {
    compare(host, snapshot([tree('a', 2)]), false, NOW);
    const { fresh } = compare(host, snapshot([stopped('a', NOW + 2 * HOUR)]), false, NOW);
    // Both are news in one snapshot; the same id, so the later one shown stands.
    expect(fresh.map((notice) => [notice.kind, notice.id])).toEqual([
      ['finished', 'host/a/finished'],
      ['limits', 'host/a/finished'],
    ]);
    expect(fresh[1]).toMatchObject({
      title: 'Project · a',
      body: 'Codex has reached its usage limit and resets in about 2 hours.',
      path: '/session/host/7',
    });
    // The same stop seen again is not said again, nor a retry refused again.
    expect(compare(host, snapshot([stopped('a', NOW + 2 * HOUR)]), false, NOW + 5000).fresh).toEqual([]);
  });

  test('already so at the first snapshot is not news', () => {
    expect(compare(host, snapshot([stopped('a', NOW + HOUR)]), false, NOW).fresh).toEqual([]);
    expect(compare(host, snapshot([stopped('a', NOW + HOUR)]), false, NOW + 1000).fresh).toEqual([]);
  });

  test('its limit resetting is said once for an agent, while nobody has carried on', () => {
    const both = () => snapshot([stopped('a', NOW + HOUR), stopped('b', NOW + HOUR)]);
    compare(host, both(), false, NOW);
    const { fresh } = compare(host, both(), true, NOW + HOUR);
    expect(fresh.map((notice) => notice.id)).toEqual(['host/reset/codex', 'host/reset/codex']);
    expect(fresh[0]).toMatchObject({
      kind: 'limits',
      subtitle: 'Studio Mac',
      body: 'Codex\u2019s limit has reset. It can carry on.',
      path: '/session/host/7',
    });
    expect(compare(host, both(), false, NOW + HOUR + 1000).fresh).toEqual([]);

    // Carried on before it reset: the mark is off, and nothing is said.
    forgetSeen();
    compare(host, snapshot([stopped('a', NOW + HOUR)]), false, NOW);
    expect(compare(host, snapshot([tree('a', 2)]), false, NOW + HOUR).fresh).toEqual([]);
  });

  test('a tab put to sleep is still stopped, and opens its worktree', () => {
    const asleep = (resetsAt: number) =>
      tree('a b', 0, { agent: '', asleep: [{ key: 'k', agent: 'claude', limit: limit(NOW - 1000, resetsAt, 2) }] });
    compare(host, snapshot([tree('a b', 2)]), false, NOW);
    const { fresh } = compare(host, snapshot([asleep(0)]), false, NOW);
    expect(fresh.at(-1)).toMatchObject({
      kind: 'limits',
      body: 'Claude has reached its limit for this model.',
      path: '/worktree/host/a%20b',
    });
    // No reset was named, so none is ever said.
    expect(compare(host, snapshot([asleep(0)]), false, NOW + 100 * HOUR).fresh).toEqual([]);
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

describe('following a desktop', () => {
  test('hears of an agent that worked and went quiet between two draws', async () => {
    expect(await turnOn()).toBe(true);
    // Not 'active' under jest's React Native, as in the background.
    expect(AppState.currentState).not.toBe('active');
    const told = new Set<(message: object) => void>();
    const conn = {
      id: host.id,
      state: { host, snapshot: snapshot([tree('a', 0)]), status: 'offline', hostMinor: 0, upSince: null },
      onChange: () => () => {},
      onMessage: (listener: (message: object) => void) => (told.add(listener), () => told.delete(listener)),
    };
    jest.mocked(listHosts).mockResolvedValue([{ id: host.id }] as never);
    jest.mocked(connection).mockReturnValue(conn as never);
    const hook = await renderHook(() => useNoticeFollowing(true));
    await act(async () => {});

    // A run of snapshots, drawn as one: the screens are told only of the
    // last, quiet again, and never see it working.
    for (const activity of [2, 2, 0]) {
      conn.state = { ...conn.state, snapshot: snapshot([tree('a', activity)]) };
      for (const listener of told) listener({ payload: { case: 'snapshot' } });
    }
    await act(async () => {});
    expect(Notifications.scheduleNotificationAsync).toHaveBeenCalledTimes(1);
    expect(jest.mocked(Notifications.scheduleNotificationAsync).mock.calls[0][0]).toMatchObject({
      identifier: 'host/a/finished',
      content: { body: 'Codex finished.' },
    });

    await hook.unmount();
    jest.mocked(listHosts).mockResolvedValue([]);
    await turnOff();
  });
});

describe('the switch', () => {
  test('turning on asks the phone, and a refusal leaves it on but marked', async () => {
    expect(await turnOn()).toBe(true);
    expect(mockNotifications.requestPermissionsAsync).toHaveBeenCalled();
    expect(mockStored).toEqual({ on: true, waiting: true, finished: true, checks: true, limits: true });

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
