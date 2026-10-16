import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render, waitFor } from '@testing-library/react-native';
import { useEffect as mockUseEffect } from 'react';
import Needs from './app/needs';
import Settings from './app/settings';
import Usage from './app/usage';
import Worktrees from './app/index';
import { WorktreeList } from './components/WorktreeList';
import { SplitContext } from './lib/layout';
import Host from './app/host/[host]';
import NewTask from './app/new/[host]';
import Worktree from './app/worktree/[host]/[worktree]';
import type { HostConnection } from './lib/connection';

const mockRouter = {
  push: jest.fn(),
  back: jest.fn(),
  replace: jest.fn(),
  canGoBack: jest.fn(() => true),
  canDismiss: jest.fn(() => true),
  dismissAll: jest.fn(),
};
let mockPath = '/';
let mockParams: Record<string, string> = {};
let mockConns: HostConnection[] = [];
let mockHost: HostConnection | null = null;
const mockSaveDesktop = jest.fn(async (_id: string) => {});
const mockSaveFit = jest.fn(async (_on: boolean) => {});
const mockSaveCollapsed = jest.fn(async (_host: string, _projects: string[]) => {});
const mockConfirmIdentity = jest.fn(async (_reason: string) => true);

jest.mock('expo-router', () => {
  return {
    Redirect: () => null,
    router: {
      push: (path: string) => mockRouter.push(path),
      back: () => mockRouter.back(),
      replace: (path: string) => mockRouter.replace(path),
      canGoBack: () => mockRouter.canGoBack(),
      canDismiss: () => mockRouter.canDismiss(),
      dismissAll: () => mockRouter.dismissAll(),
    },
    useFocusEffect: (callback: () => void) => mockUseEffect(callback, [callback]),
    useIsFocused: () => true,
    usePathname: () => mockPath,
    useLocalSearchParams: () => mockParams,
  };
});
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('./components/Unpaired', () => ({ Unpaired: () => null }));
jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
  Answer_Choice: { ALLOW_ONCE: 1, ALLOW_ALWAYS: 2, DENY: 3 },
  FileChange_Status: { ADDED: 1, DELETED: 2 },
  SubagentSummary_State: { WORKING: 1, BLOCKED: 2, IDLE: 3 },
  UsageStatus: { UNSPECIFIED: 0, FRESH: 1, STALE: 2, UNAVAILABLE: 3, UNSUPPORTED: 4 },
}));
jest.mock('./lib/hosts', () => ({
  statusLine: (conn: HostConnection) => ({
    text: conn.status === 'connected' ? 'Connected through the relay' : 'Offline',
    connected: conn.status === 'connected',
  }),
  useHost: () => mockHost,
  useHosts: () => mockConns,
}));
jest.mock('./lib/store', () => ({
  getDesktop: jest.fn(async () => 'host'),
  getCollapsed: jest.fn(async () => []),
  getFit: jest.fn(async () => false),
  getLastWork: jest.fn(async () => null),
  saveCollapsed: (host: string, projects: string[]) => mockSaveCollapsed(host, projects),
  saveDesktop: (id: string) => mockSaveDesktop(id),
  saveFit: (on: boolean) => mockSaveFit(on),
  saveLastWork: jest.fn(async () => {}),
}));
jest.mock('./lib/lock', () => ({
  LOCK_AFTER: [
    { key: '0', label: 'Immediately', ms: 0 },
    { key: '60000', label: '1 min', ms: 60_000 },
  ],
  biometry: jest.fn(async () => 'Face ID'),
  confirmIdentity: (reason: string) => mockConfirmIdentity(reason),
  setLockAfter: jest.fn(async () => {}),
  turnOff: jest.fn(async () => {}),
  useLock: () => ({ ready: true, on: true, locked: false, confirming: null, covered: false, afterMs: 60_000 }),
}));

const terminal = (id: bigint, agent = 'codex', closed = false) => ({ id, agent, closed, title: agent });
const tree = (id: string, activity: number, extra: object = {}) => ({
  id,
  name: id,
  branch: `branch-${id}`,
  agent: 'codex',
  agentState: '',
  activity,
  terminals: [terminal(7n)],
  filesChanged: 2,
  linesAdded: 5,
  linesRemoved: 1,
  commitsAhead: 1,
  base: 'main',
  subagents: [],
  ...extra,
});
const project = (worktrees: object[]) => ({ id: 'project', name: 'Project', color: '#22c55e', worktrees });
const connection = (projects: object[], usage: object[] = []) =>
  ({
    host: { id: 'host', name: 'Studio Mac', relay: 'ws://studio.local:7979', hostPublic: '', pairedAt: 0 },
    hostMinor: 7,
    status: 'connected',
    snapshot: { projects, usage },
    startWork: jest.fn(async () => null),
    changes: jest.fn(async () => ({ base: 'main', error: '', files: [] })),
    answer: jest.fn(async () => null),
    merge: jest.fn(async () => null),
    retry: jest.fn(),
  }) as unknown as HostConnection;

beforeEach(() => {
  jest.clearAllMocks();
  mockParams = {};
  mockPath = '/';
  mockConns = [];
  mockHost = null;
});

describe('top-level screens', () => {
  test('Worktrees handles unpaired, filters, switching, folds, and routing', async () => {
    const unpaired = await render(<Worktrees />);
    expect(unpaired.getByLabelText('Worktrees')).toBeTruthy();
    await unpaired.unmount();

    const subagents = Array.from({ length: 5 }, (_, index) => ({
      id: String(index),
      label: index === 0 ? '' : `helper ${index}`,
      state: index === 1 ? 2 : index === 2 ? 3 : 1,
    }));
    const first = connection([
      project([
        tree('waiting', 4, { question: { subject: 'Approve', terminalId: 9n } }),
        tree('working', 2, { subagents }),
        tree('quiet', 0, { agent: '', terminals: [terminal(8n, '')] }),
      ]),
    ]);
    const second = connection([]);
    second.host.id = 'second';
    second.host.name = 'Other Mac';
    mockConns = [first, second];
    const view = await render(<Worktrees />);
    expect(view.getByText('1 waiting on you')).toBeTruthy();
    expect(view.getByText('+1 more')).toBeTruthy();
    await fireEvent.press(view.getByLabelText('New task'));
    expect(mockRouter.push).toHaveBeenCalledWith('/new/host');
    await fireEvent.press(view.getAllByText('Waiting').at(-1)!);
    await fireEvent.press(view.getByText('Project'));
    expect(mockSaveCollapsed).toHaveBeenCalled();
    await fireEvent.press(view.getByLabelText('Switch desktop'));
    await fireEvent.press(view.getByText('Other Mac'));
    await view.unmount();
  });

  test('Worktrees, beside the sidebar, waits for a pick; the sidebar opens beside itself', async () => {
    mockConns = [connection([project([tree('waiting', 4, { question: { subject: 'Approve', terminalId: 9n } }), tree('quiet', 0)])])];
    const empty = await render(
      <SplitContext.Provider value>
        <Worktrees />
      </SplitContext.Provider>,
    );
    expect(empty.getByText('Pick a worktree to follow its agent.')).toBeTruthy();
    expect(empty.queryByText('Studio Mac')).toBeNull();
    await empty.unmount();

    mockPath = '/session/host/7';
    const sidebar = await render(
      <SplitContext.Provider value>
        <WorktreeList sidebar refresh={mockPath} />
      </SplitContext.Provider>,
    );
    await fireEvent.press(sidebar.getByText('quiet'));
    expect(mockRouter.dismissAll).toHaveBeenCalled();
    expect(mockRouter.replace).toHaveBeenCalledWith('/session/host/7');
    expect(mockRouter.push).not.toHaveBeenCalled();
    await fireEvent.press(sidebar.getByText('Answer'));
    expect(mockRouter.replace).toHaveBeenLastCalledWith('/session/host/9');
    // Its tab bar is the one drawn, with the open screen's tab lit.
    expect(sidebar.getByLabelText('Worktrees').props.accessibilityState).toEqual({ selected: true });
    await fireEvent.press(sidebar.getByLabelText('Usage'));
    expect(mockRouter.replace).toHaveBeenLastCalledWith('/usage');
    await sidebar.unmount();
  });

  test('Needs shows empty, waiting, and failed states', async () => {
    const empty = await render(<Needs />);
    expect(empty.getByText('Nothing needs you.')).toBeTruthy();
    await empty.unmount();

    mockConns = [
      connection([
        project([
          tree('waiting', 4, { question: { subject: 'Run command', terminalId: 9n } }),
          tree('failed', 5),
        ]),
      ]),
    ];
    const view = await render(<Needs />);
    expect(view.getByText('Waiting on you')).toBeTruthy();
    expect(view.getByText('Failed')).toBeTruthy();
    await fireEvent.press(view.getAllByText('waiting')[0]);
    expect(mockRouter.push).toHaveBeenCalledWith('/session/host/9');
    await fireEvent.press(view.getAllByText('failed')[0]);
    expect(mockRouter.push).toHaveBeenCalledWith('/worktree/host/failed');
    await view.unmount();
  });

  test('Settings controls pairing, terminal fit, security, and devices', async () => {
    mockConns = [connection([project([tree('waiting', 4)])])];
    const view = await render(<Settings />);
    await waitFor(() => expect(view.getByText('Require Face ID')).toBeTruthy());
    expect(view.getByText('Version')).toBeTruthy();
    await fireEvent.press(view.getByText('Studio Mac'));
    await fireEvent.press(view.getByText('Pair a desktop'));
    await fireEvent.press(view.getByLabelText('Fit terminal to phone'));
    await fireEvent.press(view.getByText('Change PIN'));
    expect(mockRouter.push.mock.calls).toEqual(expect.arrayContaining([['/devices/host'], ['/pair'], ['/pin']]));
    expect(mockSaveFit).toHaveBeenCalledWith(true);
    await view.unmount();
  });

  test('Usage renders every provider status and window', async () => {
    mockConns = [
      connection(
        [project([tree('working', 2)])],
        [
          { provider: 'Codex', status: 1, plan: 'Pro', fetchedAtMs: BigInt(Date.now()), reason: '', windows: [{ name: 'Weekly', usedPercent: 75, resetsAtMs: BigInt(Date.now() + 60_000) }] },
          { provider: 'Claude', status: 2, plan: '', fetchedAtMs: 0n, reason: '', windows: [] },
          { provider: 'Gemini', status: 3, plan: '', fetchedAtMs: 0n, reason: 'signed out', windows: [] },
          { provider: 'OpenCode', status: 4, plan: '', fetchedAtMs: 0n, reason: '', windows: [] },
        ],
      ),
    ];
    const view = await render(<Usage />);
    expect(view.getByText('75%')).toBeTruthy();
    expect(view.getByText('No windows reported')).toBeTruthy();
    expect(view.getByText('Unavailable — signed out')).toBeTruthy();
    expect(view.getByText('No usage reporting')).toBeTruthy();
    expect(view.getByText('1 session running here')).toBeTruthy();
    await view.unmount();
  });

  test('Host saves selection then redirects', async () => {
    mockParams = { host: 'chosen' };
    const view = await render(<Host />);
    await waitFor(() => expect(view.toJSON()).toBeNull());
    expect(mockSaveDesktop).toHaveBeenCalledWith('chosen');
    await view.unmount();
  });
});

describe('work screens', () => {
  test('starts new work with a trimmed prompt', async () => {
    mockParams = { host: 'host' };
    mockHost = connection([project([])]);
    const view = await render(<NewTask />);
    await fireEvent.changeText(view.getByPlaceholderText('What should the agent do?'), '  fix it  ');
    await fireEvent.press(view.getByText('Start task'));
    // An empty agent is the project's own.
    await waitFor(() => expect(mockHost?.startWork).toHaveBeenCalledWith('project', 'fix it', ''));
    // The sheet slides away before it leaves the stack.
    await waitFor(() => expect(mockRouter.back).toHaveBeenCalled());
    await view.unmount();
  });

  test('shows an answerable worktree prompt and sends answers', async () => {
    mockParams = { host: 'host', worktree: 'waiting' };
    const waiting = tree('waiting', 4, {
      question: { id: 2n, terminalId: 7n, tool: 'Bash', subject: 'npm test', answerable: true, always: true, asks: [], plan: false },
    });
    mockHost = connection([project([waiting])]);
    const view = await render(<Worktree />);
    expect(view.getByText('Codex wants to run a command')).toBeTruthy();
    await fireEvent.press(view.getByText('Allow once'));
    await waitFor(() => expect(mockHost?.answer).toHaveBeenCalled());
    await fireEvent.press(view.getByText('Always allow'));
    await waitFor(() => expect(mockConfirmIdentity).toHaveBeenCalled());
    await fireEvent.press(view.getByText('Open the terminal'));
    expect(mockRouter.push).toHaveBeenCalledWith('/terminal/host/7');
    await view.unmount();
  });
});
