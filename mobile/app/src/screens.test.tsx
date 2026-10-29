import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render, waitFor } from '@testing-library/react-native';
import { useEffect as mockUseEffect } from 'react';
import { Alert, Linking } from 'react-native';
import Needs from './app/(tabs)/needs';
import Settings from './app/(tabs)/settings';
import Usage from './app/(tabs)/usage';
import Worktrees from './app/(tabs)/index';
import { WorktreeList } from './components/WorktreeList';
import { SplitContext } from './lib/layout';
import Host from './app/host/[host]';
import NewTask from './app/new/[host]';
import Worktree from './app/worktree/[host]/[worktree]';
import StartingTask from './app/starting/[host]/[at]';
import { STARTING_FOR_MS } from './lib/connection';
import type { HostConnection } from './lib/connection';

const mockNavigation = { setParams: jest.fn() };
const mockRouter = {
  push: jest.fn(),
  back: jest.fn(),
  replace: jest.fn(),
  setParams: jest.fn(),
  canGoBack: jest.fn(() => true),
  canDismiss: jest.fn(() => true),
  dismissAll: jest.fn(),
};
let mockPath = '/';
let mockParams: Record<string, string> = {};
let mockConns: HostConnection[] = [];
let mockHost: HostConnection | null = null;
const mockSaveDesktop = jest.fn(async (_id: string) => {});
const mockSaveCollapsed = jest.fn(async (_host: string, _projects: string[]) => {});
const mockConfirmIdentity = jest.fn(async (_reason: string) => true);

jest.mock('expo-router', () => {
  return {
    Redirect: () => null,
    router: {
      push: (path: string) => mockRouter.push(path),
      back: () => mockRouter.back(),
      replace: (path: string) => mockRouter.replace(path),
      setParams: (params: object) => mockRouter.setParams(params),
      canGoBack: () => mockRouter.canGoBack(),
      canDismiss: () => mockRouter.canDismiss(),
      dismissAll: () => mockRouter.dismissAll(),
    },
    useFocusEffect: (callback: () => void) => mockUseEffect(callback, [callback]),
    useIsFocused: () => true,
    usePathname: () => mockPath,
    useGlobalSearchParams: () => mockParams,
    useLocalSearchParams: () => mockParams,
    useNavigation: () => mockNavigation,
  };
});
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('./components/Unpaired', () => ({ Unpaired: () => null }));
// The terminal's chat a starting screen becomes: what it was handed.
const mockChat = jest.fn((_props: object) => null);
jest.mock('./app/session/[host]/[terminal]', () => ({ Chat: (props: object) => mockChat(props) }));
// Nothing is laid out under test: a held row or a picked pill is anywhere.
jest.mock('./components/FloatMenu', () => ({
  ...jest.requireActual<typeof import('./components/FloatMenu')>('./components/FloatMenu'),
  placeOf: (_view: unknown, then: (place: object) => void) => then({ x: 10, y: 200, width: 360, height: 80 }),
}));
jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
  Answer_Choice: { ALLOW_ONCE: 1, ALLOW_ALWAYS: 2, DENY: 3 },
  CheckSummary_State: { STATE_UNSPECIFIED: 0, PASSED: 1, FAILED: 2, PENDING: 3, SKIPPED: 4 },
  FileChange_Status: { ADDED: 1, DELETED: 2 },
  PullRequestMergeMethod: { UNSPECIFIED: 0, MERGE_COMMIT: 1, SQUASH: 2, REBASE: 3 },
  PullRequestSummary_Review: { REVIEW_UNSPECIFIED: 0, APPROVED: 1, CHANGES_REQUESTED: 2, REVIEW_REQUIRED: 3 },
  PullRequestSummary_State: { OPEN: 0, CLOSED: 1, MERGED: 2 },
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
  getLastWork: jest.fn(async () => null),
  saveCollapsed: (host: string, projects: string[]) => mockSaveCollapsed(host, projects),
  saveDesktop: (id: string) => mockSaveDesktop(id),
  saveLastWork: jest.fn(async () => {}),
}));
const mockNotify = jest.fn(async () => true);
const mockQuiet = jest.fn(async () => {});
const mockChoose = jest.fn(async (_kind: string, _on: boolean) => {});
let mockNotices = { config: { on: false, waiting: true, finished: true, checks: true }, refused: false };
jest.mock('./lib/notices', () => ({
  available: true,
  choose: (kind: string, on: boolean) => mockChoose(kind, on),
  openPhoneSettings: jest.fn(),
  turnOff: () => mockQuiet(),
  turnOn: () => mockNotify(),
  useNotices: () => mockNotices,
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
  questions: [],
  asleep: [],
  ...extra,
});
const project = (worktrees: object[]) => ({ id: 'project', name: 'Project', color: '#22c55e', worktrees });
const connection = (projects: object[], usage: object[] = []) =>
  ({
    host: { id: 'host', name: 'Studio Mac', relay: 'ws://studio.local:7979', hostPublic: '', pairedAt: 0 },
    hostMinor: 7,
    status: 'connected',
    snapshot: { projects, usage },
    starting: [],
    asked: [],
    startWork: jest.fn(async () => null),
    changes: jest.fn(async () => ({ base: 'main', error: '', files: [] })),
    answer: jest.fn(async () => null),
    merge: jest.fn(async () => null),
    retry: jest.fn(),
    addressForDisplay: () => 'ws://studio.local:7979',
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
    await fireEvent.press(view.getByLabelText('Start a task'));
    expect(mockRouter.push).toHaveBeenCalledWith('/new/host');
    await fireEvent.press(view.getByLabelText('Waiting, 1'));
    await fireEvent.press(view.getByText('Project'));
    expect(mockSaveCollapsed).toHaveBeenCalled();
    await fireEvent.press(view.getByLabelText('Switch desktop'));
    await fireEvent.press(view.getByText('Other Mac'));
    await view.unmount();
  });

  test('Worktrees opens up running work, holds a worktree for its pull request', async () => {
    const pr = {
      number: 21n,
      url: '',
      draft: false,
      review: 0,
      unresolvedComments: 0,
      behind: true,
      base: 'main',
      state: 0,
      squashMergeAllowed: true,
      checks: [{ name: 'rust', state: 2, url: '' }],
    };
    const conn = Object.assign(
      connection([
        project([
          tree('busy', 2, { agentState: 'compile workspace', pullRequest: pr }),
          tree('fresh', 0),
          tree('main', 0, { base: '' }),
        ]),
      ]),
      {
        hostMinor: 19,
        fixChecks: jest.fn(async (_worktree: string) => ({ starting: false })),
        updatePullRequest: jest.fn(async (_worktree: string) => 'Nothing to update'),
        mergePullRequest: jest.fn(async (_worktree: string, _method: number) => null),
      },
    );
    (conn.snapshot as unknown as { agents: string[] }).agents = ['claude', 'codex'];
    mockConns = [conn];
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(() => {});
    const view = await render(<Worktrees />);

    // At work, a worktree opens up: its step in a terminal's well, its lines,
    // its pull request.
    expect(view.getByText('Compile workspace')).toBeTruthy();
    expect(view.getByText('#21')).toBeTruthy();

    // The badge says how the line is kept.
    await fireEvent.press(view.getByLabelText('Secure: end-to-end encrypted'));
    expect(alert).toHaveBeenLastCalledWith('End-to-end encrypted', expect.stringContaining('Studio Mac'));

    // Held, a row lifts out with its menu.
    const holdAt = async (id: string) => {
      await fireEvent(view.getByText(id), 'longPress');
    };
    await holdAt('busy');
    expect(view.getByLabelText('busy actions')).toBeTruthy();
    expect(view.getByText('Pull request #21')).toBeTruthy();
    expect(view.queryByText('Address comments with agent')).toBeNull();
    await fireEvent.press(view.getByText('Fix checks with agent'));
    await waitFor(() => expect(conn.fixChecks).toHaveBeenCalledWith('busy'));
    expect(mockRouter.push).toHaveBeenLastCalledWith('/session/host/7');

    await holdAt('busy');
    await fireEvent.press(view.getByText('Update branch'));
    await waitFor(() => expect(alert).toHaveBeenLastCalledWith("Couldn't update the branch", 'Nothing to update.'));

    await holdAt('busy');
    await fireEvent.press(view.getByText('Merge pull request…'));
    const [, , buttons] = alert.mock.calls.at(-1)! as [string, string, { text: string; onPress?: () => void }[]];
    buttons.find((b) => b.text === 'Squash')!.onPress!();
    await waitFor(() => expect(conn.mergePullRequest).toHaveBeenCalledWith('busy', 2));
    expect(mockConfirmIdentity).toHaveBeenLastCalledWith('Merge #21');

    // With none open, a worktree offers to open one; a repository's own
    // checkout, the base one would merge into, offers nothing.
    await holdAt('fresh');
    await fireEvent.press(view.getByText('Open pull request…'));
    expect(mockRouter.push).toHaveBeenLastCalledWith('/pull-request/host/fresh');
    await holdAt('main');
    expect(view.queryByText('Open pull request…')).toBeNull();

    // The tab bar: Worktrees, Usage, Needs you, Settings.
    const tabs = view.getAllByRole('tab').map((tab) => tab.props.accessibilityLabel);
    expect(tabs).toEqual(['Worktrees', 'Usage', 'Needs you', 'Settings']);
    alert.mockRestore();
    await view.unmount();
  });

  test('Worktrees pins and deletes a worktree from its menu, but never the repository’s own checkout', async () => {
    const conn = Object.assign(
      connection([project([tree('fresh', 0), tree('kept', 0, { pinned: true }), tree('main', 0, { base: '' })])]),
      {
        hostMinor: 20,
        pinWorktree: jest.fn(async (_worktree: string, _pinned: boolean) => null),
        deleteWorktree: jest.fn(async (_worktree: string, force: boolean) =>
          force ? { keptBranch: 'branch-fresh' } : { why: 'it has uncommitted changes', losesWork: true },
        ),
      },
    );
    mockConns = [conn];
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(() => {});
    const buttons = () => (alert.mock.calls.at(-1)![2] ?? []) as { text: string; style?: string; onPress?: () => void }[];
    const press = (text: string) => buttons().find((button) => button.text === text)!.onPress?.();
    const view = await render(<Worktrees />);
    const hold = async (id: string) => fireEvent(view.getByText(id), 'longPress');

    // A pinned worktree wears its pin, and its menu unpins it.
    expect(view.getByLabelText('Pinned')).toBeTruthy();
    await hold('kept');
    await fireEvent.press(view.getByText('Unpin'));
    await waitFor(() => expect(conn.pinWorktree).toHaveBeenLastCalledWith('kept', false));
    await hold('fresh');
    await fireEvent.press(view.getByText('Pin'));
    await waitFor(() => expect(conn.pinWorktree).toHaveBeenLastCalledWith('fresh', true));

    // The repository's own checkout has neither, nor anything else to hold.
    await hold('main');
    expect(view.queryByText('Pin')).toBeNull();
    expect(view.queryByText('Delete…')).toBeNull();

    // Delete asks first. Cancelled, or not you, nothing goes.
    await hold('fresh');
    await fireEvent.press(view.getByText('Delete…'));
    expect(alert).toHaveBeenLastCalledWith('Delete fresh?', expect.stringContaining('Unmerged commits keep their branch'), expect.any(Array));
    expect(buttons().find((button) => button.text === 'Cancel')?.style).toBe('cancel');
    mockConfirmIdentity.mockResolvedValueOnce(false);
    press('Delete');
    await waitFor(() => expect(mockConfirmIdentity).toHaveBeenLastCalledWith('Delete fresh'));
    expect(conn.deleteWorktree).not.toHaveBeenCalled();

    // Confirmed: the row goes at once, before the desktop answers. Refused
    // outright, it is back with the reason.
    let answer!: (result: { why: string; losesWork: boolean }) => void;
    conn.deleteWorktree.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
    await hold('fresh');
    await fireEvent.press(view.getByText('Delete…'));
    press('Delete');
    await waitFor(() => expect(view.queryByText('fresh')).toBeNull());
    answer({ why: 'the desktop is busy', losesWork: false });
    await waitFor(() => expect(alert).toHaveBeenLastCalledWith("Couldn't delete fresh", 'The desktop is busy.'));
    expect(view.getByText('fresh')).toBeTruthy();

    // Git refuses it for the work it would lose, which is said, with Delete
    // anyway; the row stays out while that is asked, and Cancel brings it back.
    await hold('fresh');
    await fireEvent.press(view.getByText('Delete…'));
    press('Delete');
    await waitFor(() => expect(conn.deleteWorktree).toHaveBeenLastCalledWith('fresh', false));
    await waitFor(() =>
      expect(alert).toHaveBeenLastCalledWith('Delete fresh anyway?', 'It has uncommitted changes.', expect.any(Array), expect.any(Object)),
    );
    expect(view.queryByText('fresh')).toBeNull();
    press('Cancel');
    await waitFor(() => expect(view.getByText('fresh')).toBeTruthy());

    // Delete anyway keeps the branch's unmerged commits, and the row stays
    // gone before the desktop's next snapshot says so.
    await hold('fresh');
    await fireEvent.press(view.getByText('Delete…'));
    press('Delete');
    await waitFor(() =>
      expect(alert).toHaveBeenLastCalledWith('Delete fresh anyway?', 'It has uncommitted changes.', expect.any(Array), expect.any(Object)),
    );
    press('Delete anyway');
    await waitFor(() => expect(conn.deleteWorktree).toHaveBeenLastCalledWith('fresh', true));
    await waitFor(() =>
      expect(alert).toHaveBeenLastCalledWith('Worktree deleted', 'Kept the branch branch-fresh: it has unmerged commits.'),
    );
    expect(view.queryByText('fresh')).toBeNull();
    alert.mockRestore();
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
          tree('waiting', 4, { question: { id: 1n, subject: 'Run command', terminalId: 9n } }),
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

  test('Settings controls pairing, notices, security, and devices', async () => {
    mockConns = [connection([project([tree('waiting', 4)])])];
    const view = await render(<Settings />);
    await waitFor(() => expect(view.getByText('Require Face ID')).toBeTruthy());
    expect(view.getByText('Version')).toBeTruthy();
    await fireEvent.press(view.getByText('Studio Mac'));
    await fireEvent.press(view.getByText('Pair a desktop'));
    await fireEvent.press(view.getByText('Change PIN'));
    expect(mockRouter.push.mock.calls).toEqual(expect.arrayContaining([['/devices/host'], ['/pair'], ['/pin']]));
    // Off: only the one switch shows. On, it asks the phone.
    expect(view.queryByText('A task finishes')).toBeNull();
    await fireEvent.press(view.getByLabelText('Notify me'));
    expect(mockNotify).toHaveBeenCalled();
    await view.unmount();

    // On but refused by the phone: the way to its Settings, and the kinds.
    mockNotices = { config: { on: true, waiting: true, finished: false, checks: true }, refused: true };
    const on = await render(<Settings />);
    await waitFor(() => expect(on.getByText("Open this phone's Settings")).toBeTruthy());
    await fireEvent.press(on.getByLabelText('A task finishes'));
    expect(mockChoose).toHaveBeenCalledWith('finished', true);
    await fireEvent.press(on.getByLabelText('Notify me'));
    expect(mockQuiet).toHaveBeenCalled();
    mockNotices = { config: { on: false, waiting: true, finished: true, checks: true }, refused: false };
    await on.unmount();
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
  test('starts new work with a trimmed prompt, and goes on to its chat', async () => {
    mockParams = { host: 'host' };
    const host = connection([project([])]);
    // The desktop takes it: the connection holds a row for it from then on.
    host.startWork = jest.fn(async (projectId: string, prompt: string, agent: string) => {
      const work = { project: projectId, prompt, agent, at: 1234, before: [] };
      host.starting.push(work);
      host.asked.push(work);
      return null;
    }) as HostConnection['startWork'];
    mockHost = host;
    const view = await render(<NewTask />);
    await fireEvent.changeText(view.getByPlaceholderText('What should the agent do?'), '  fix it  ');
    await fireEvent.press(view.getByText('Start task'));
    // An empty agent is the project's own.
    await waitFor(() => expect(mockHost?.startWork).toHaveBeenCalledWith('project', 'fix it', ''));
    // The sheet slides away, then the chat standing in for the work takes
    // its place in the stack.
    await waitFor(() => expect(mockRouter.replace).toHaveBeenCalledWith('/starting/host/1234'));
    expect(mockRouter.back).not.toHaveBeenCalled();
    await view.unmount();
  });

  test('lists work just asked for as starting, with its stage, and opens its chat', async () => {
    const host = connection([project([tree('one', 1)])]);
    const work = { project: 'project', prompt: 'Fix the login\nKeep the path', agent: 'codex', at: 1234, before: ['one'] };
    host.starting.push(work);
    host.asked.push(work);
    mockConns = [host];
    mockHost = host;
    const view = await render(<Worktrees />);
    await waitFor(() => expect(view.getByText('Fix the login')).toBeTruthy());
    expect(view.getByText(/Making the worktree on Studio Mac · \d+ s/)).toBeTruthy();
    await fireEvent.press(view.getByText('Fix the login'));
    expect(mockRouter.push).toHaveBeenCalledWith('/starting/host/1234');
    await view.unmount();
  });

  test('shows an answerable worktree prompt, and its buttons go on the tap', async () => {
    mockParams = { host: 'host', worktree: 'waiting' };
    const question = { id: 2n, terminalId: 7n, tool: 'Bash', subject: 'npm test', answerable: true, always: true, asks: [], plan: false };
    const waiting = tree('waiting', 4, { question });
    mockHost = connection([project([waiting])]);
    const view = await render(<Worktree />);
    expect(view.getByText('Codex wants to run a command')).toBeTruthy();
    expect(view.getByText('Always allow')).toBeTruthy();
    await fireEvent.press(view.getByText('Allow once'));
    await waitFor(() => expect(mockHost?.answer).toHaveBeenCalledWith(question, 1));
    // Answered: the buttons are gone until the desktop's snapshot drops the
    // question, and the prompt's words stay.
    expect(view.queryByText('Allow once')).toBeNull();
    expect(view.queryByText('Always allow')).toBeNull();
    expect(view.getByText('Codex wants to run a command')).toBeTruthy();
    await fireEvent.press(view.getByText('Open the terminal'));
    expect(mockRouter.push).toHaveBeenCalledWith('/terminal/host/7');
    await view.unmount();
  });

  test('a refused worktree answer brings the buttons back with the reason', async () => {
    mockParams = { host: 'host', worktree: 'waiting' };
    const question = { id: 3n, terminalId: 7n, tool: 'Bash', subject: 'npm test', answerable: true, always: true, asks: [], plan: false };
    mockHost = connection([project([tree('waiting', 4, { question })])]);
    mockHost.answer = jest.fn(async () => 'that prompt has changed — look again before answering');
    const view = await render(<Worktree />);
    await fireEvent.press(view.getByText('Always allow'));
    await waitFor(() => expect(mockConfirmIdentity).toHaveBeenCalled());
    await waitFor(() => expect(view.getByText('that prompt has changed — look again before answering')).toBeTruthy());
    expect(view.getByText('Allow once')).toBeTruthy();
    await view.unmount();
  });

  const pullRequest = {
    number: 17n,
    url: 'https://github.com/o/r/pull/17',
    state: 0,
    draft: true,
    review: 2,
    checks: [
      { name: 'rust', state: 1, url: 'https://github.com/o/r/actions/runs/1/job/1' },
      { name: 'clippy', state: 2, url: 'https://github.com/o/r/actions/runs/1/job/2' },
      { name: 'docs', state: 4, url: '' },
    ],
  };

  test('shows a worktree’s pull request and hands its failed checks to the agent', async () => {
    mockParams = { host: 'host', worktree: 'pr' };
    const conn = Object.assign(connection([project([tree('pr', 0, { pullRequest })])]), {
      hostMinor: 11,
      fixChecks: jest.fn(async (_worktree: string) => ({ starting: false })),
    });
    mockHost = conn;
    const open = jest.spyOn(Linking, 'openURL').mockResolvedValue(undefined);
    const view = await render(<Worktree />);
    expect(view.getByText('#17')).toBeTruthy();
    expect(view.getByText('Draft · changes requested · 1 of 2 checks failing')).toBeTruthy();
    // What failed is listed; what passed or said nothing waits behind a tap.
    expect(view.getByText('clippy')).toBeTruthy();
    expect(view.queryByText('rust')).toBeNull();
    await fireEvent.press(view.getByText('Show 2 more checks'));
    expect(view.getByText('rust')).toBeTruthy();
    expect(view.getByText('skipped')).toBeTruthy();
    await fireEvent.press(view.getByText('clippy'));
    expect(open).toHaveBeenCalledWith('https://github.com/o/r/actions/runs/1/job/2');
    // Beside the first thing to do, GitHub is just GitHub.
    await fireEvent.press(view.getByText('GitHub'));
    expect(open).toHaveBeenCalledWith('https://github.com/o/r/pull/17');
    // One tap sends them, and the agent's conversation opens.
    await fireEvent.press(view.getByText('Fix checks with agent'));
    await waitFor(() => expect(conn.fixChecks).toHaveBeenCalledWith('pr'));
    await waitFor(() => expect(mockRouter.push).toHaveBeenCalledWith('/session/host/7'));
    await view.unmount();
    open.mockRestore();
  });

  test('says why checks could not be fixed, and offers no fix to a desktop too old for one', async () => {
    mockParams = { host: 'host', worktree: 'pr' };
    const conn = Object.assign(connection([project([tree('pr', 0, { pullRequest, terminals: [] })])]), {
      hostMinor: 11,
      fixChecks: jest.fn(async (_worktree: string) => ({ why: 'open ket on your Mac to fix checks from here' })),
    });
    mockHost = conn;
    const view = await render(<Worktree />);
    await fireEvent.press(view.getByText('Fix checks with agent'));
    await waitFor(() => expect(view.getByText('open ket on your Mac to fix checks from here')).toBeTruthy());
    expect(mockRouter.push).not.toHaveBeenCalled();
    await view.unmount();

    // An agent still starting is not a failure, and nothing is opened yet.
    mockHost = Object.assign(connection([project([tree('pr', 0, { pullRequest })])]), {
      hostMinor: 11,
      fixChecks: jest.fn(async (_worktree: string) => ({ starting: true })),
    });
    const starting = await render(<Worktree />);
    await fireEvent.press(starting.getByText('Fix checks with agent'));
    await waitFor(() => expect(starting.getByText(/is still starting the agent/)).toBeTruthy());
    expect(mockRouter.push).not.toHaveBeenCalled();
    await starting.unmount();

    mockHost = connection([project([tree('pr', 0, { pullRequest })])]);
    const old = await render(<Worktree />);
    expect(old.getByText('Open on GitHub')).toBeTruthy();
    expect(old.queryByText('Fix checks with agent')).toBeNull();
    await old.unmount();
  });

  test('offers merge, update, ready, reopen and address comments on a 1.19 desktop', async () => {
    mockParams = { host: 'host', worktree: 'pr' };
    const ready = {
      ...pullRequest,
      draft: false,
      behind: true,
      base: 'main',
      mergeCommitAllowed: true,
      squashMergeAllowed: true,
      rebaseMergeAllowed: false,
      unresolvedComments: 2,
    };
    const conn = Object.assign(connection([project([tree('pr', 0, { pullRequest: ready })])]), {
      hostMinor: 19,
      mergePullRequest: jest.fn(async (_worktree: string, _method: number) => null),
      updatePullRequest: jest.fn(async (_worktree: string) => null),
      addressComments: jest.fn(async (_worktree: string) => ({ starting: false })),
    });
    mockHost = conn;
    const alert = jest.spyOn(Alert, 'alert').mockImplementation((_title, _msg, buttons) => {
      const squash = buttons?.find((button) => 'text' in button && button.text === 'Squash');
      if (squash && 'onPress' in squash) squash.onPress?.();
    });
    const view = await render(<Worktree />);
    expect(view.getByText('behind main · changes requested · 1 of 2 checks failing')).toBeTruthy();
    // The foot has the first thing to do; the rest waits in the bar's menu.
    expect(view.getByText('Fix checks with agent')).toBeTruthy();
    expect(view.queryByText('Update branch')).toBeNull();
    const more = async (item: string) => {
      await fireEvent.press(view.getByLabelText('More'));
      await fireEvent.press(await view.findByText(item));
    };
    await more('Merge pull request…');
    await waitFor(() => expect(mockConfirmIdentity).toHaveBeenCalled());
    await waitFor(() => expect(conn.mergePullRequest).toHaveBeenCalledWith('pr', 2));
    await more('Update branch');
    await waitFor(() => expect(conn.updatePullRequest).toHaveBeenCalledWith('pr'));
    await more('Address comments with agent');
    await waitFor(() => expect(conn.addressComments).toHaveBeenCalledWith('pr'));
    await view.unmount();
    alert.mockRestore();

    mockHost = Object.assign(
      connection([project([tree('pr', 0, { pullRequest: { ...pullRequest, state: 1 } })])]),
      { hostMinor: 19, reopenPullRequest: jest.fn(async () => null) },
    );
    const closed = await render(<Worktree />);
    expect(closed.getByText('Closed · changes requested · 1 of 2 checks failing')).toBeTruthy();
    // Closed, the branch can still land from here; opening another waits
    // behind Reopen in the menu.
    await waitFor(() => expect(closed.getByText('Merge into main')).toBeTruthy());
    expect(closed.queryByText('Pull request…')).toBeNull();
    expect(closed.queryByText('Open on GitHub')).toBeNull();
    await fireEvent.press(closed.getByLabelText('More'));
    await fireEvent.press(await closed.findByText('Reopen pull request'));
    await waitFor(() => expect(mockHost?.reopenPullRequest).toHaveBeenCalledWith('pr'));
    await closed.unmount();

    mockHost = Object.assign(connection([project([tree('pr', 0, { pullRequest })])]), {
      hostMinor: 19,
      readyPullRequest: jest.fn(async () => null),
    });
    const draft = await render(<Worktree />);
    await fireEvent.press(draft.getByLabelText('More'));
    await fireEvent.press(await draft.findByText('Ready for review'));
    await waitFor(() => expect(mockHost?.readyPullRequest).toHaveBeenCalledWith('pr'));
    await draft.unmount();
  });

  test('a worktree’s foot merges it or opens a pull request, and its menu holds the rest', async () => {
    mockParams = { host: 'host', worktree: 'work' };
    const conn = Object.assign(connection([project([tree('work', 2)])]), {
      hostMinor: 16,
      changes: jest.fn(async () => ({
        base: 'main',
        error: '',
        files: [{ path: 'crates/ket-remote/src/lib.rs', added: 1, removed: 1, binary: false, status: 0 }],
      })),
    });
    mockHost = conn;
    const alert = jest.spyOn(Alert, 'alert').mockImplementation((_title, _msg, buttons) => {
      const merge = buttons?.find((button) => 'text' in button && button.text === 'Merge');
      if (merge && 'onPress' in merge) merge.onPress?.();
    });
    const view = await render(<Worktree />);
    await waitFor(() => expect(view.getByText('lib.rs')).toBeTruthy());
    expect(view.getByText('crates/ket-remote/src')).toBeTruthy();
    // The lines are the listed files', not the worktree's uncommitted ones.
    expect(view.getByText(/1 commit ahead of main/)).toHaveTextContent('+1 −1 · 1 commit ahead of main');
    // Gone from the page: the figure cards, and the buttons the chat header
    // already has.
    expect(view.queryByText('Sessions')).toBeNull();
    expect(view.queryByText('Terminal')).toBeNull();
    expect(view.queryByText('New session or shell')).toBeNull();
    await fireEvent.press(view.getByText('Merge into main'));
    await waitFor(() => expect(mockConfirmIdentity).toHaveBeenCalled());
    await waitFor(() => expect(conn.merge).toHaveBeenCalledWith('work'));
    await fireEvent.press(view.getByText('Pull request…'));
    expect(mockRouter.push).toHaveBeenCalledWith('/pull-request/host/work');
    await fireEvent.press(view.getByLabelText('More'));
    await fireEvent.press(await view.findByText('Open the terminal'));
    expect(mockRouter.push).toHaveBeenCalledWith('/terminal/host/7');
    await fireEvent.press(view.getByLabelText('More'));
    // Opening one from the foot, the menu leaves out.
    expect(view.queryByText('Open pull request…')).toBeNull();
    await fireEvent.press(await view.findByText('New session or shell'));
    expect(mockRouter.push).toHaveBeenCalledWith('/new/host?worktree=work');
    alert.mockRestore();
    await view.unmount();
  });

  test('a worktree row wears its pull request in its checks’ colour', async () => {
    mockConns = [connection([project([tree('pr', 1, { pullRequest })])])];
    const view = await render(<WorktreeList />);
    await waitFor(() => expect(view.getByText('#17')).toBeTruthy());
    expect(view.getByText('#17')).toHaveStyle({ color: '#ef6a74' });
    await view.unmount();
  });

  test('opens a shell in an existing worktree without a prompt', async () => {
    mockParams = { host: 'host', worktree: 'existing' };
    const startSession = jest.fn(
      async (_worktree: string, _agent: string, _prompt: string, _model: string, _effort: string) => null,
    );
    mockHost = Object.assign(connection([project([tree('existing', 0)])]), {
      hostMinor: 15,
      startSession,
      startOptions: jest.fn(async () => ({ projectId: 'project', defaultBase: 'main', localBranches: ['main'], agents: [{ agent: 'codex', models: ['gpt-6.1-sol'], efforts: ['high'] }] })),
      snapshot: { projects: [project([tree('existing', 0)])], usage: [], agents: ['codex'] },
    });
    const view = await render(<NewTask />);
    await waitFor(() => expect(view.getByText('Agent session')).toBeTruthy());
    await fireEvent(view.getByLabelText('Open: Agent session'), 'layout', { nativeEvent: { layout: { x: 10, width: 200 } } });
    await fireEvent.press(view.getByLabelText('Open: Agent session'));
    await fireEvent.press(view.getByText('Shell'));
    await waitFor(() => expect(view.getByText('Open shell')).toBeTruthy());
    await fireEvent.press(view.getByText('Open shell'));
    await waitFor(() => expect(startSession).toHaveBeenCalledWith('existing', '', '', '', ''));
    await waitFor(() => expect(mockRouter.back).toHaveBeenCalled());
    await view.unmount();
  });

  test('sends the selected base, model, and effort for new work', async () => {
    mockParams = { host: 'host' };
    const startWork = jest.fn(
      async (_project: string, _prompt: string, _agent: string, _options: { base: string; model: string; effort: string }) =>
        null,
    );
    mockHost = Object.assign(connection([project([])]), {
      hostMinor: 15,
      startWork,
      startOptions: jest.fn(async () => ({
        projectId: 'project', defaultBase: 'main', localBranches: ['main', 'release'],
        agents: [{ agent: 'codex', models: ['gpt-6.1-sol'], efforts: ['high'] }],
      })),
      snapshot: { projects: [project([])], usage: [], agents: ['codex'] },
    });
    const view = await render(<NewTask />);
    await waitFor(() => expect(view.getByLabelText('Base branch: main')).toBeTruthy());
    const choose = async (field: string, value: string) => {
      const control = view.getByLabelText(new RegExp(`^${field}:`));
      await fireEvent(control, 'layout', { nativeEvent: { layout: { x: 10, width: 200 } } });
      await fireEvent.press(control);
      await fireEvent.press(view.getByText(value));
      await waitFor(() => expect(view.getByLabelText(`${field}: ${value}`)).toBeTruthy());
    };
    await choose('Agent', 'Codex');
    await choose('Base branch', 'release');
    await choose('Model', 'gpt-6.1-sol');
    await choose('Effort', 'high');
    await fireEvent.changeText(view.getByPlaceholderText('What should the agent do?'), 'build it');
    await fireEvent.press(view.getByText('Start task'));
    await waitFor(() => expect(startWork).toHaveBeenCalledWith('project', 'build it', 'codex', {
      base: 'release', model: 'gpt-6.1-sol', effort: 'high',
    }));
    await view.unmount();
  });
});

describe('starting chat', () => {
  const asked = (fields: object = {}) => ({ project: 'project', prompt: 'Fix the login', agent: 'codex', at: 1_000_000, before: ['one'], ...fields });

  // The branch the desktop names the worktree after the prompt, with its
  // four-character tag — see branchStem.
  const made = (activity: number, extra: object = {}) => tree('new', activity, { branch: 'fix-the-login-k3f9', ...extra });

  test('shows the task as sending and what the desktop is doing, then becomes the real chat in place', async () => {
    const at = Date.now();
    mockParams = { host: 'host', at: String(at) };
    const host = connection([project([tree('one', 1)])]);
    // The row on Worktrees may be gone already; the request is kept.
    host.asked.push(asked({ at }));
    mockHost = host;
    const view = await render(<StartingTask />);
    // Named after the task in the bar, and the task itself as a message.
    expect(view.getAllByText('Fix the login')).toHaveLength(2);
    expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
    expect(view.getByLabelText(/Making the worktree on Studio Mac · \d+ s/)).toBeTruthy();
    expect(mockRouter.replace).not.toHaveBeenCalled();
    await fireEvent.changeText(view.getByLabelText('A message for the agent, once it is running'), 'and the tests');

    // A worktree made on the desktop meanwhile is not this task's.
    host.snapshot = { projects: [project([tree('one', 1), tree('other', 0, { branch: 'other-thing-a1b2', terminals: [] })])], usage: [] } as never;
    await view.rerender(<StartingTask />);
    expect(view.getByLabelText(/Making the worktree on Studio Mac · \d+ s/)).toBeTruthy();

    // The worktree is made, with a shell open and no agent's tab yet: the
    // screen carries on from the snapshot rather than opening the shell.
    host.snapshot = { projects: [project([tree('one', 1), made(0, { terminals: [terminal(8n, '')] })])], usage: [] } as never;
    await view.rerender(<StartingTask />);
    expect(view.getByLabelText(/Starting Codex · \d+ s/)).toBeTruthy();
    expect(view.getByLabelText('The worktree')).toBeTruthy();
    expect(mockRouter.replace).not.toHaveBeenCalled();

    // Its agent's tab opens: this screen is that terminal's chat now, where
    // it is — no second move — the task and the draft going with it.
    host.snapshot = { projects: [project([tree('one', 1), made(1, { terminals: [terminal(8n, ''), terminal(9n)] })])], usage: [] } as never;
    await view.rerender(<StartingTask />);
    expect(mockChat).toHaveBeenLastCalledWith({
      started: { host: 'host', terminal: '9', seed: { text: 'Fix the login', draft: 'and the tests' } },
    });
    expect(view.queryByLabelText('A message for the agent, once it is running')).toBeNull();
    // Named in this screen's address, for when the page is loaded again —
    // not in whatever screen is on top of it.
    expect(mockNavigation.setParams).toHaveBeenCalledWith({ terminal: '9' });
    expect(mockRouter.setParams).not.toHaveBeenCalled();
    // And stays it, whatever the snapshot says next.
    host.snapshot = { projects: [project([tree('one', 1), made(1, { terminals: [terminal(9n, 'codex', true)] })])], usage: [] } as never;
    await view.rerender(<StartingTask />);
    expect(view.queryByLabelText('A message for the agent, once it is running')).toBeNull();
    expect(mockChat).toHaveBeenLastCalledWith({
      started: { host: 'host', terminal: '9', seed: { text: 'Fix the login', draft: 'and the tests' } },
    });
    expect(mockRouter.replace).not.toHaveBeenCalled();
    expect(mockRouter.push).not.toHaveBeenCalled();
    await view.unmount();
  });

  test('after two minutes without a worktree, holds the task and offers to wait, ask again or let go', async () => {
    mockParams = { host: 'host', at: '1000000' };
    const host = connection([project([tree('one', 1)])]);
    const choices = { base: 'release', model: 'gpt-6.1-sol', effort: 'high' };
    const startWork = jest.fn(async (projectId: string, prompt: string, agent: string, _choices: object) => {
      host.asked.push({ project: projectId, prompt, agent, at: 2_000_000, before: ['one'] });
      return null;
    });
    host.startWork = startWork as HostConnection['startWork'];
    const spy = jest.spyOn(Date, 'now').mockReturnValue(1_000_000 + STARTING_FOR_MS + 5_000);
    try {
      // Too long ago: the connection has let go of the row already.
      host.asked.push(asked({ choices }));
      mockHost = host;
      const view = await render(<StartingTask />);
      expect(view.getByLabelText('Fix the login. Waiting on your Mac.')).toBeTruthy();
      expect(view.getByText('not started')).toBeTruthy();
      expect(view.getByText(/took the task 2 min ago/)).toBeTruthy();
      await fireEvent.changeText(view.getByLabelText('A message for the agent, once it is running'), 'and the tests');
      // Asked again with the same picks; the screen follows the new request
      // and the draft stays.
      await fireEvent.press(view.getByText('Ask again'));
      await waitFor(() => expect(startWork).toHaveBeenCalledWith('project', 'Fix the login', 'codex', choices));
      spy.mockReturnValue(2_000_000 + 5_000);
      await waitFor(() => expect(view.getByText('starting')).toBeTruthy());
      expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
      expect(view.getByLabelText('A message for the agent, once it is running').props.value).toBe('and the tests');
      expect(mockRouter.replace).not.toHaveBeenCalled();
      await view.unmount();
    } finally {
      spy.mockRestore();
    }
  });

  test('after two minutes with a worktree but no agent in it, says so and offers the worktree', async () => {
    mockParams = { host: 'host', at: '1000000' };
    const host = connection([project([tree('one', 1), made(0, { terminals: [] })])]);
    const spy = jest.spyOn(Date, 'now').mockReturnValue(1_000_000 + STARTING_FOR_MS + 5_000);
    try {
      host.asked.push(asked());
      mockHost = host;
      const view = await render(<StartingTask />);
      expect(view.getByText(/made the worktree 2 min ago, but no agent/)).toBeTruthy();
      expect(view.queryByText('Ask again')).toBeNull();
      await fireEvent.press(view.getByText('Open the worktree'));
      expect(mockRouter.push).toHaveBeenCalledWith('/worktree/host/new');
      await fireEvent.press(view.getByText('Forget it'));
      expect(mockRouter.back).toHaveBeenCalled();
      await view.unmount();
    } finally {
      spy.mockRestore();
    }
  });

  test('is the chat named in its address, with the request gone', async () => {
    // Loaded again on the web after the hand-off: the request is not kept.
    mockParams = { host: 'host', at: '42', terminal: '9' };
    mockHost = connection([project([])]);
    const view = await render(<StartingTask />);
    expect(mockChat).toHaveBeenLastCalledWith({ started: { host: 'host', terminal: '9' } });
    expect(view.queryByText(/isn’t starting from this phone any more/)).toBeNull();
    expect(mockNavigation.setParams).not.toHaveBeenCalled();
    await view.unmount();
  });

  test('moves once from New task to the chat, and the chat is where the agent arrives', async () => {
    mockParams = { host: 'host' };
    const host = connection([project([tree('one', 1)])]);
    const at = Date.now();
    host.startWork = jest.fn(async (projectId: string, prompt: string, agent: string) => {
      const work = { project: projectId, prompt, agent, at, before: ['one'] };
      host.starting.push(work);
      host.asked.push(work);
      return null;
    }) as HostConnection['startWork'];
    mockHost = host;
    const sheet = await render(<NewTask />);
    await fireEvent.changeText(sheet.getByPlaceholderText('What should the agent do?'), 'Fix the login');
    await fireEvent.press(sheet.getByText('Start task'));
    await waitFor(() => expect(mockRouter.replace).toHaveBeenCalledWith(`/starting/host/${at}`));
    await sheet.unmount();

    // On the starting screen the desktop makes the worktree, then opens the
    // agent in it: no move for either.
    mockParams = { host: 'host', at: String(at) };
    const view = await render(<StartingTask />);
    host.snapshot = { projects: [project([tree('one', 1), made(0, { terminals: [] })])], usage: [] } as never;
    await view.rerender(<StartingTask />);
    host.snapshot = { projects: [project([tree('one', 1), made(1, { terminals: [terminal(9n, 'claude')] })])], usage: [] } as never;
    await view.rerender(<StartingTask />);
    expect(mockChat).toHaveBeenLastCalledWith({ started: { host: 'host', terminal: '9', seed: { text: 'Fix the login', draft: '' } } });
    expect(mockRouter.replace).toHaveBeenCalledTimes(1);
    expect(mockRouter.push).not.toHaveBeenCalled();
    expect(mockRouter.back).not.toHaveBeenCalled();
    expect(mockRouter.dismissAll).not.toHaveBeenCalled();
    await view.unmount();
  });

  test.each([
    ['claude', 'claude'],
    ['codex', 'codex'],
    ['grok', 'grok'],
    ['opencode', 'opencode'],
    // The project's own agent, whichever it turns out to be.
    ['', 'claude'],
  ])('becomes the chat in place for work asked of %j once its %s tab opens', async (asked_, opened) => {
    const at = Date.now();
    mockParams = { host: 'host', at: String(at) };
    const host = connection([project([tree('one', 1), made(1, { agent: opened, terminals: [terminal(9n, opened)] })])]);
    host.asked.push(asked({ at, agent: asked_ }));
    mockHost = host;
    const view = await render(<StartingTask />);
    expect(mockChat).toHaveBeenLastCalledWith({ started: { host: 'host', terminal: '9', seed: { text: 'Fix the login', draft: '' } } });
    expect(mockRouter.replace).not.toHaveBeenCalled();
    expect(mockRouter.push).not.toHaveBeenCalled();
    await view.unmount();
  });

  test('becomes the chat in place when the agent opens after the task was held as not started', async () => {
    mockParams = { host: 'host', at: '1000000' };
    const host = connection([project([tree('one', 1)])]);
    const spy = jest.spyOn(Date, 'now').mockReturnValue(1_000_000 + STARTING_FOR_MS + 5_000);
    try {
      host.asked.push(asked());
      mockHost = host;
      const view = await render(<StartingTask />);
      expect(view.getByText('not started')).toBeTruthy();
      expect(mockChat).not.toHaveBeenCalled();
      host.snapshot = { projects: [project([tree('one', 1), made(1, { terminals: [terminal(9n)] })])], usage: [] } as never;
      await view.rerender(<StartingTask />);
      expect(mockChat).toHaveBeenLastCalledWith({ started: { host: 'host', terminal: '9', seed: { text: 'Fix the login', draft: '' } } });
      expect(view.queryByText('not started')).toBeNull();
      expect(mockRouter.replace).not.toHaveBeenCalled();
      await view.unmount();
    } finally {
      spy.mockRestore();
    }
  });

  test('says so when the task is no longer this phone’s to show', async () => {
    mockParams = { host: 'host', at: '42' };
    mockHost = connection([project([])]);
    const view = await render(<StartingTask />);
    expect(view.getByText(/isn’t starting from this phone any more/)).toBeTruthy();
    await fireEvent.press(view.getByText('Worktrees'));
    expect(mockRouter.replace).toHaveBeenCalledWith('/');
    await view.unmount();
  });
});
