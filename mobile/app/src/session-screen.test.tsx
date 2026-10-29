import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render, waitFor, within } from '@testing-library/react-native';
import { useEffect as mockUseEffect } from 'react';
import { Linking } from 'react-native';
import Session, { Chat } from './app/session/[host]/[terminal]';

const mockRouter = { push: jest.fn() };
const mockConfirmIdentity = jest.fn(async (_reason: string) => true);
let mockConn: any;
let mockParams = { host: 'host', terminal: '7' };

// The stack's slide-in has always ended: the conversation paints at once.
const mockNavigation = {
  addListener: (_type: string, listener: () => void) => {
    listener();
    return () => {};
  },
};
jest.mock('expo-router', () => ({
  router: { push: (path: string) => mockRouter.push(path) },
  useFocusEffect: (callback: () => void) => mockUseEffect(callback, [callback]),
  useLocalSearchParams: () => mockParams,
  useNavigation: () => mockNavigation,
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
  Answer_Choice: { ALLOW_ONCE: 1, ALLOW_ALWAYS: 2, DENY: 3 },
  BackgroundSession_State: { STATE_UNSPECIFIED: 0, DONE: 1, WORKING: 2, NEEDS_YOU: 3 },
  Turn_Kind: { USER: 1, ASSISTANT: 2, TOOL: 3 },
}));
jest.mock('./lib/hosts', () => ({ useHost: () => mockConn }));
jest.mock('./components/FloatMenu', () => ({
  ...jest.requireActual<typeof import('./components/FloatMenu')>('./components/FloatMenu'),
  placeOf: (_view: unknown, then: (place: object) => void) => then({ x: 10, y: 200, width: 120, height: 40 }),
}));
jest.mock('./lib/lock', () => ({ confirmIdentity: (reason: string) => mockConfirmIdentity(reason) }));

// Photos as the pickers would hand them over, and a mic that hears what a
// test says.
const mockPhoto = { key: 1, name: 'IMG_1.jpg', uri: 'file:///IMG_1.jpg', image: true, bytes: new Uint8Array([1, 2, 3]) };
const mockFromPhotos = jest.fn(async (_room: number) => [mockPhoto]);
jest.mock('./lib/attach', () => ({
  ...(jest.requireActual('./lib/attach') as object),
  canPaste: async () => false,
  fromPhotos: (room: number) => mockFromPhotos(room),
}));
let mockEars: { onHeard(text: string): void; onLevel(level: number): void; onEnd(error?: string): void } | undefined;
const mockListen = jest.fn(async (_local: boolean, ears: NonNullable<typeof mockEars>) => {
  mockEars = ears;
  return { onDevice: true, stop: () => ears.onEnd(), abandon: () => {} };
});
jest.mock('./lib/voice', () => ({
  canDictate: true,
  listen: (local: boolean, ears: any) => mockListen(local, ears),
}));

const modes = {
  current: '',
  choices: [
    { id: 'default', label: 'Default', detail: 'Works as it goes' },
    { id: 'plan', label: 'Plan', detail: 'Plans first; changes nothing' },
  ],
};

const tree = (fields: object = {}) => ({
  id: 'tree', name: 'Tree', agent: 'codex', activity: 0, agentState: '', subagents: [], questions: [],
  terminals: [{ id: 7n, agent: 'codex', title: 'Codex', closed: false, modes }],
  ...fields,
});

const connection = (worktree = tree(), fields: object = {}) => ({
  host: { id: 'host', name: 'Studio Mac' },
  hostMinor: 12,
  status: 'connected',
  snapshot: { projects: [{ id: 'project', name: 'Project', color: '#fff', worktrees: [worktree] }] },
  conversation: jest.fn(async () => ({ supported: true, reason: '', fresh: true, session: 'one', next: 4n, queued: [], turns: [] })),
  snippets: jest.fn(async () => [{ name: 'Ship', body: 'ship it' }, { name: 'Blank', body: ' ' }]),
  takeControl: jest.fn(async () => true),
  type: jest.fn(async () => true),
  setMode: jest.fn(async () => true),
  sendFiles: jest.fn(async () => true),
  releaseControl: jest.fn(),
  answer: jest.fn(async () => null),
  onChange: jest.fn(() => () => {}),
  ...fields,
});

beforeEach(() => {
  jest.clearAllMocks();
  mockConfirmIdentity.mockResolvedValue(true);
  mockConn = connection();
  mockParams = { host: 'host', terminal: '7' };
});

describe('Session', () => {
  test('picks a mode from the pill in the bar without leaving chat or sending a message', async () => {
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Mode. Change it'));
    expect(mockConn.setMode).not.toHaveBeenCalled();
    await fireEvent.press(view.getByLabelText('Plan'));
    await waitFor(() => expect(mockConn.setMode).toHaveBeenCalledWith(7n, 'plan'));
    expect(mockConn.takeControl).toHaveBeenCalledWith(7n);
    expect(mockConn.type).not.toHaveBeenCalled();
    expect(mockRouter.push).not.toHaveBeenCalled();
    await waitFor(() => expect(view.getByText('Switched to Plan.')).toBeTruthy());
    expect(view.queryByLabelText('Plan')).toBeNull();
    await view.unmount();
  });

  test('names the mode the agent is in on the pill, and ticks it in the menu', async () => {
    mockConn = connection(
      tree({ agent: 'claude', terminals: [{ id: 7n, agent: 'claude', closed: false, modes: { ...modes, current: 'plan' } }] }),
    );
    const view = await render(<Session />);
    expect(view.getByLabelText('Mode: Plan. Change it')).toBeTruthy();
    expect(view.getByText('Plan')).toBeTruthy();
    await view.unmount();
  });

  test('keeps chat open when switching mode cannot take control', async () => {
    mockConn.takeControl.mockResolvedValue(false);
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Mode. Change it'));
    await fireEvent.press(view.getByLabelText('Plan'));
    await waitFor(() => expect(view.getByText('Another device is typing in this terminal.')).toBeTruthy());
    expect(mockConn.setMode).not.toHaveBeenCalled();
    expect(mockRouter.push).not.toHaveBeenCalled();
    await view.unmount();
  });

  // The model and effort (1.26): the pill names them and opens the session
  // sheet; a pick goes to the desktop as SetModel under control.
  const settings = (fields: object = {}) => ({
    model: 'opus',
    effort: 'high',
    models: [
      { id: 'fable', label: 'Fable' },
      { id: 'opus', label: 'Opus' },
      { id: 'sonnet', label: 'Sonnet' },
      { id: 'haiku', label: 'Haiku' },
    ],
    efforts: [
      { id: 'low', label: 'Low' },
      { id: 'medium', label: 'Medium' },
      { id: 'high', label: 'High' },
      { id: 'xhigh', label: 'Extra high' },
    ],
    savesDefault: true,
    ...fields,
  });
  const claudeWith = (fields: object = {}, worktree: object = {}) =>
    connection(
      tree({
        agent: 'claude',
        terminals: [{ id: 7n, agent: 'claude', title: 'Claude', closed: false, modes, settings: settings(fields) }],
        ...worktree,
      }),
      { setModel: jest.fn(async () => true) },
    );

  test('names the model and effort on the pill, and switches the model from its sheet', async () => {
    mockConn = claudeWith();
    const view = await render(<Session />);
    expect(view.getByText('Opus · High')).toBeTruthy();
    await fireEvent.press(view.getByLabelText('Opus, High. Change them'));
    expect(view.getByText('Model')).toBeTruthy();
    expect(mockConn.setModel).not.toHaveBeenCalled();
    await fireEvent.press(view.getByText('Sonnet'));
    await waitFor(() => expect(mockConn.setModel).toHaveBeenCalledWith(7n, 'sonnet', ''));
    expect(mockConn.takeControl).toHaveBeenCalledWith(7n);
    expect(mockConn.type).not.toHaveBeenCalled();
    expect(mockConn.setMode).not.toHaveBeenCalled();
    // Said in the sheet, and in the chat's note under it for after it closes.
    await waitFor(() => expect(within(view.getByLabelText('Claude settings')).getByText('Switched to Sonnet.')).toBeTruthy());
    expect(view.getAllByText('Switched to Sonnet.')).toHaveLength(2);
    // The pick stands in on the pill until the snapshot reports it.
    expect(view.getByText('Sonnet · High')).toBeTruthy();
    await view.unmount();
  });

  test("a pick stands in only until the snapshot moves, never over the Mac's next change", async () => {
    mockConn = claudeWith();
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Opus, High. Change them'));
    await fireEvent.press(view.getByText('Sonnet'));
    await waitFor(() => expect(view.getByText('Sonnet · High')).toBeTruthy());
    // The snapshot reports the pick, then the Mac switches back.
    mockConn.snapshot = claudeWith({ model: 'sonnet' }).snapshot;
    await view.rerender(<Session />);
    expect(view.getByText('Sonnet · High')).toBeTruthy();
    mockConn.snapshot = claudeWith({ model: 'opus' }).snapshot;
    await view.rerender(<Session />);
    expect(view.getByText('Opus · High')).toBeTruthy();
    await view.unmount();
  });

  test('switches the effort alone, and a mode from the same sheet', async () => {
    mockConn = claudeWith();
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Opus, High. Change them'));
    await fireEvent.press(view.getByLabelText('Extra high'));
    await waitFor(() => expect(mockConn.setModel).toHaveBeenCalledWith(7n, '', 'xhigh'));
    await waitFor(() => expect(within(view.getByLabelText('Claude settings')).getByText('Switched to Extra high.')).toBeTruthy());
    await fireEvent.press(view.getByLabelText('Plan'));
    await waitFor(() => expect(mockConn.setMode).toHaveBeenCalledWith(7n, 'plan'));
    await view.unmount();
  });

  test("says why the desktop refused a model, in the agent's own words", async () => {
    mockConn = claudeWith();
    mockConn.setModel.mockRejectedValue(new Error('Effort not supported for Haiku'));
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Opus, High. Change them'));
    await fireEvent.press(view.getByText('Haiku'));
    await waitFor(() =>
      expect(within(view.getByLabelText('Claude settings')).getByText('Not switched: Effort not supported for Haiku')).toBeTruthy(),
    );
    // Nothing stands in for a refused pick.
    expect(view.getByText('Opus · High')).toBeTruthy();
    await view.unmount();
  });

  test('opens the sheet while the agent works, but holds every pick', async () => {
    mockConn = claudeWith({}, { activity: 2 });
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Opus, High. Change them'));
    expect(view.getByText("Claude is working. These can change once it's done.")).toBeTruthy();
    await fireEvent.press(view.getByText('Sonnet'));
    expect(mockConn.setModel).not.toHaveBeenCalled();
    await view.unmount();
  });

  test('shows the model alone while the effort is unknown, and Mode before either is', async () => {
    mockConn = claudeWith({ effort: '' });
    let view = await render(<Session />);
    expect(view.getByText('Opus')).toBeTruthy();
    await view.unmount();
    mockConn = claudeWith({ model: '', effort: '' });
    view = await render(<Session />);
    expect(view.getByText('Mode')).toBeTruthy();
    await view.unmount();
  });

  test('does not switch mode while the agent is working', async () => {
    mockConn = connection(tree({ activity: 2 }));
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Mode. Change it'));
    expect(view.queryByLabelText('Plan')).toBeNull();
    expect(mockConn.setMode).not.toHaveBeenCalled();
    await view.unmount();
  });

  test('shows no mode pill for an agent without modes', async () => {
    mockConn = connection(tree({ agent: 'grok', terminals: [{ id: 7n, agent: 'grok', closed: false }] }));
    const view = await render(<Session />);
    expect(view.queryByLabelText(/^Mode/)).toBeNull();
    await view.unmount();
  });

  test('renders transcript, markdown, working state, queue, and replies', async () => {
    mockConn = connection(tree({ activity: 2, agentState: 'Running checks' }), {
      conversation: jest.fn(async () => ({
        supported: true,
        reason: '',
        fresh: true,
        session: 'one',
        next: 4n,
        queued: ['queued request'],
        turns: [
          { kind: 1, text: 'User says https://example.com.' },
          { kind: 3, text: 'Read file' },
          { kind: 2, text: '# Result\n**strong** and *emphasis* with `code`\n```ts\nconst ok = true;\n```' },
        ],
      })),
    });
    const open = jest.spyOn(Linking, 'openURL').mockResolvedValue(undefined);
    const view = await render(<Session />);
    await waitFor(() => expect(view.getByText('Read file')).toBeTruthy());
    expect(view.getByText('queued request')).toBeTruthy();
    expect(view.getByLabelText(/queued request.*Queued/)).toBeTruthy();
    expect(view.getByText('Running checks')).toBeTruthy();
    expect(view.getByText('const ok = true;')).toBeTruthy();
    expect(view.getByText('Ship')).toBeTruthy();
    expect(view.queryByText('Blank')).toBeNull();
    await fireEvent.press(view.getByText('https://example.com'));
    expect(open).toHaveBeenCalledWith('https://example.com');
    await fireEvent.press(view.getByLabelText('Open the terminal'));
    await fireEvent.press(view.getByLabelText('The worktree'));
    expect(mockRouter.push.mock.calls).toEqual(expect.arrayContaining([
      ['/terminal/host/7'], ['/worktree/host/tree'],
    ]));
    await view.unmount();
  });

  test('shuts a run of tool calls under its count, and opens it on a tap', async () => {
    mockParams = { host: 'host', terminal: '8' };
    mockConn = connection(tree(), {
      conversation: jest.fn(async () => ({
        supported: true,
        reason: '',
        fresh: true,
        session: 'one',
        next: 5n,
        queued: [],
        turns: [
          { kind: 1, text: 'Fix the build' },
          { kind: 3, text: '$ cargo test', tool: 'Bash' },
          { kind: 3, text: 'Edited main.rs', tool: 'Edit' },
          { kind: 3, text: '$ cargo build', tool: 'Bash' },
          { kind: 2, text: 'Fixed it.' },
          { kind: 3, text: 'Read lib.rs', tool: 'Read' },
        ],
      })),
    });
    const view = await render(<Session />);
    const group = await waitFor(() => view.getByLabelText('3 tool calls: Bash, Edit'));
    expect(view.queryByText('$ cargo test')).toBeNull();
    // A lone tool call is its own line, as it was.
    expect(view.getByText('Read lib.rs')).toBeTruthy();
    await fireEvent.press(group);
    expect(view.getByText('$ cargo test')).toBeTruthy();
    expect(view.getByText('Edited main.rs')).toBeTruthy();
    expect(view.getByText('$ cargo build')).toBeTruthy();
    await fireEvent.press(group);
    expect(view.queryByText('$ cargo test')).toBeNull();
    await view.unmount();
  });

  const permission = (id: bigint) => ({
    id, terminalId: 7n, answerable: true, tool: 'Bash', subject: 'npm test', always: true,
    risk: { level: 'medium', confidence: 0.8 },
  });

  test('answering a permission tucks its card away on the tap, before the desktop replies', async () => {
    const question = permission(1n);
    // The desktop's reply is held: the card must not wait for it.
    let reply: (why: string | null) => void = () => {};
    mockConn = connection(tree({ activity: 4, question }), {
      answer: jest.fn(() => new Promise<string | null>((resolve) => { reply = resolve; })),
    });
    const view = await render(<Session />);
    expect(view.getByText('Wants to use Bash')).toBeTruthy();
    expect(view.getByText('$ npm test')).toBeTruthy();
    expect(view.getByText('Medium risk · 80%')).toBeTruthy();
    await fireEvent.press(view.getByText('Allow once'));
    expect(mockConn.answer).toHaveBeenCalledWith(question, 1);
    // Going, the button tapped wearing its mark: no button takes a second tap.
    await fireEvent.press(view.getByText('Deny'));
    expect(mockConn.answer).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(view.queryByText('Wants to use Bash')).toBeNull());
    expect(view.queryByText('Allow once')).toBeNull();
    await act(async () => reply(null));
    // Taken: the card stays gone until the desktop's snapshot drops the question.
    expect(view.queryByText('Wants to use Bash')).toBeNull();
    expect(mockConn.answer).toHaveBeenCalledTimes(1);
    await view.unmount();
  });

  test('a refused answer brings the card back with the reason', async () => {
    const question = permission(2n);
    mockConn = connection(tree({ activity: 4, question }), {
      answer: jest.fn(async () => 'that prompt has already been answered'),
    });
    const view = await render(<Session />);
    await fireEvent.press(view.getByText('Deny'));
    await waitFor(() => expect(mockConn.answer).toHaveBeenCalledWith(question, 3));
    await waitFor(() => expect(view.getByText('that prompt has already been answered')).toBeTruthy());
    expect(view.getByText('Wants to use Bash')).toBeTruthy();
    expect(view.getByText('Deny')).toBeTruthy();
    await view.unmount();
  });

  test('Always asks who you are first, and keeps the card when you are not', async () => {
    const question = permission(3n);
    mockConn = connection(tree({ activity: 4, question }));
    const view = await render(<Session />);
    mockConfirmIdentity.mockResolvedValueOnce(false);
    await fireEvent.press(view.getByText('Always'));
    expect(mockConfirmIdentity).toHaveBeenCalledWith('Always allow Bash');
    expect(mockConn.answer).not.toHaveBeenCalled();
    expect(view.getByText('Wants to use Bash')).toBeTruthy();
    await fireEvent.press(view.getByText('Always'));
    await waitFor(() => expect(mockConn.answer).toHaveBeenCalledWith(question, 2));
    await waitFor(() => expect(view.queryByText('Wants to use Bash')).toBeNull());
    await view.unmount();
  });

  test('a permission answered on the Mac tucks its card away, and the chips wait for it to go', async () => {
    mockConn = connection(tree({ activity: 4, question: permission(4n) }));
    const view = await render(<Session />);
    expect(view.getByText('Wants to use Bash')).toBeTruthy();
    expect(view.queryByText('Continue')).toBeNull();
    // The snapshot drops the question: the card slides away rather than
    // vanishing, and the chips come back only once it has gone.
    mockConn.snapshot = connection(tree({ activity: 2 })).snapshot;
    await view.rerender(<Session />);
    expect(view.getByText('Wants to use Bash')).toBeTruthy();
    expect(view.queryByText('Continue')).toBeNull();
    await waitFor(() => expect(view.queryByText('Wants to use Bash')).toBeNull());
    expect(view.getByText('Continue')).toBeTruthy();
    await view.unmount();
  });

  test('a permission asked while the last one tucks away shows once it has gone', async () => {
    mockConn = connection(tree({ activity: 4, question: permission(5n) }));
    const view = await render(<Session />);
    await fireEvent.press(view.getByText('Allow once'));
    mockConn.snapshot = connection(tree({ activity: 4, question: { ...permission(6n), subject: 'npm run build' } })).snapshot;
    await view.rerender(<Session />);
    expect(view.getByText('$ npm test')).toBeTruthy();
    expect(view.queryByText('$ npm run build')).toBeNull();
    await waitFor(() => expect(view.getByText('$ npm run build')).toBeTruthy());
    expect(view.queryByText('$ npm test')).toBeNull();
    await fireEvent.press(view.getByText('Deny'));
    expect(mockConn.answer).toHaveBeenLastCalledWith(expect.objectContaining({ id: 6n }), 3);
    await view.unmount();
  });

  test("answers Codex's question form with its options and no words of your own", async () => {
    const question = {
      id: 4n, terminalId: 7n, answerable: true, tool: 'request_user_input', subject: 'Which crate?', always: false,
      plan: false,
      asks: [
        { question: 'Which crate?', header: 'Scope', multiSelect: false, choices: [
          { label: 'ket-core', description: 'The engine' },
          { label: 'ket-ui', description: 'The window' },
        ] },
      ],
    };
    mockConn = connection(tree({ activity: 4, question }), { respond: jest.fn(async () => null) });
    const view = await render(<Session />);
    expect(view.getByText('Which crate?')).toBeTruthy();
    expect(view.queryByPlaceholderText('Or write your own')).toBeNull();
    expect(view.queryByText('Allow once')).toBeNull();
    await fireEvent.press(view.getByText('ket-ui'));
    await fireEvent.press(view.getByText('Send answer'));
    await waitFor(() =>
      expect(mockConn.respond).toHaveBeenCalledWith(question, { picks: [{ choices: [1], other: '' }] }),
    );
    await view.unmount();
  });

  test("answers Codex's Implement this plan? from the chat", async () => {
    const question = {
      id: 5n, terminalId: 7n, answerable: true, tool: 'Dialog', subject: '', always: false, plan: false,
      asks: [
        { question: 'Implement this plan?', header: '', multiSelect: false, choices: [
          { label: 'Yes, implement this plan', description: '' },
          { label: 'Yes, clear context and implement', description: '' },
          { label: 'No, stay in Plan mode', description: '' },
        ] },
      ],
    };
    mockConn = connection(tree({ activity: 4, questions: [question] }), { respond: jest.fn(async () => null) });
    const view = await render(<Session />);
    expect(view.getByText('Implement this plan?')).toBeTruthy();
    await fireEvent.press(view.getByText('Yes, implement this plan'));
    await fireEvent.press(view.getByText('Send answer'));
    await waitFor(() =>
      expect(mockConn.respond).toHaveBeenCalledWith(question, { picks: [{ choices: [0], other: '' }] }),
    );
    await view.unmount();
  });

  test('shows old/unsupported hosts and sends a composed message', async () => {
    mockConn = connection(tree(), { hostMinor: 6 });
    const old = await render(<Session />);
    expect(old.getByText(/older than this app/)).toBeTruthy();
    await fireEvent.press(old.getByText('Open the terminal'));
    expect(mockRouter.push).toHaveBeenCalledWith('/terminal/host/7');
    await old.unmount();

    // The screen keeps each conversation it has read for the app's lifetime,
    // and shows those turns over the reason; another desktop has none kept.
    mockParams = { host: 'other', terminal: '7' };
    mockConn = connection(tree(), {
      conversation: jest.fn(async () => ({ supported: false, reason: 'not available', fresh: false, session: '', next: 0n, queued: [], turns: [] })),
    });
    const unsupported = await render(<Session />);
    await waitFor(() => expect(unsupported.getByText('Not available.')).toBeTruthy());
    await unsupported.unmount();

    mockParams = { host: 'host', terminal: '7' };
    mockConn = connection();
    const view = await render(<Session />);
    const input = view.getByPlaceholderText('Message Codex');
    await fireEvent.changeText(input, ' hello ');
    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'hello'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '\r'));
    expect(view.getByText('hello')).toBeTruthy();
    await view.unmount();
    expect(mockConn.releaseControl).toHaveBeenCalledWith(7n);
  });

  test('a task started from this phone is the whole chat until it arrives, whatever the desktop says meanwhile', async () => {
    // Drawn in place by the starting screen, under that screen's params.
    mockParams = { host: 'started', at: '1000' } as never;
    const waiting = { supported: false, reason: 'no agent has reported from this terminal yet', fresh: false, session: '', next: 0n, queued: [], turns: [] };
    let reply: object = waiting;
    mockConn = connection(tree({ activity: 1 }), { conversation: jest.fn(async () => reply) });
    const started = { host: 'started', terminal: '7', seed: { text: 'Fix the login', draft: 'and the tests' } };
    const view = await render(<Chat started={started} />);
    expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
    expect(view.getByPlaceholderText('Message Codex').props.value).toBe('and the tests');
    expect(view.queryByText('Loading the conversation…')).toBeNull();
    await waitFor(() => expect(mockConn.conversation).toHaveBeenCalledWith(7n, 0n, ''));
    await act(async () => {});
    expect(view.queryByText('No agent has reported from this terminal yet.')).toBeNull();
    expect(view.queryByText('Open the terminal')).toBeNull();
    expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();

    // The agent has it: the conversation takes over from the message.
    reply = { supported: true, reason: '', fresh: true, session: 'one', next: 2n, queued: [], turns: [{ kind: 1, text: 'Fix the login' }, { kind: 2, text: 'On it.' }] };
    await waitFor(() => expect(view.getByText('On it.')).toBeTruthy(), { timeout: 3000 });
    expect(view.queryByLabelText('Fix the login. Sending.')).toBeNull();
    await view.unmount();

    // A reason that isn't the agent opening is shown, task or no task.
    mockConn = connection(tree({ activity: 1 }), {
      conversation: jest.fn(async () => ({ ...waiting, reason: 'that terminal has closed' })),
    });
    const closed = await render(<Chat started={{ ...started, host: 'closed' }} />);
    await waitFor(() => expect(closed.getByText('That terminal has closed.')).toBeTruthy());
    expect(closed.getByLabelText('Fix the login. Sending.')).toBeTruthy();
    await closed.unmount();

    // A chat opened on its own still says why it has nothing to show.
    mockParams = { host: 'fresh', terminal: '7' };
    mockConn = connection(tree(), { conversation: jest.fn(async () => waiting) });
    const plain = await render(<Session />);
    await waitFor(() => expect(plain.getByText('No agent has reported from this terminal yet.')).toBeTruthy());
    await plain.unmount();
  });

  // A task started from this phone, drawn in place by the starting screen:
  // the agent it was started on makes no difference to what the chat shows.
  describe('started from this phone', () => {
    const opening = ['no agent has reported from this terminal yet', "the session's transcript isn't there yet"];
    const reply = (reason: string) => ({ supported: false, reason, fresh: false, session: '', next: 0n, queued: [], turns: [] });
    const agentTree = (agent: string, terminal: object = {}) =>
      tree({ agent, activity: 1, terminals: [{ id: 7n, agent, title: agent, closed: false, ...terminal }] });
    const started = (host: string) => ({ host, terminal: '7', seed: { text: 'Fix the login', draft: '' } });
    let hosts = 0;
    // A host of its own each time: chats are kept by host and terminal.
    const fresh = () => `started-${(hosts += 1)}`;

    test.each(
      ['claude', 'codex', 'grok', 'opencode'].flatMap((agent) => opening.map((reason) => [agent, reason] as const)),
    )('shows only the task on its way for %s while the desktop says "%s"', async (agent, reason) => {
      mockParams = { host: 'x', at: '1000' } as never;
      mockConn = connection(agentTree(agent), { conversation: jest.fn(async () => reply(reason)) });
      const view = await render(<Chat started={started(fresh())} />);
      await waitFor(() => expect(mockConn.conversation).toHaveBeenCalled());
      await act(async () => {});
      expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
      expect(view.queryByText(/has reported|isn.t there yet|Loading the conversation/)).toBeNull();
      expect(view.queryByText('Open the terminal')).toBeNull();
      await view.unmount();
    });

    test('says nothing while the first read is on its way, where a chat opened on its own says it is loading', async () => {
      mockParams = { host: 'x', at: '1000' } as never;
      mockConn = connection(agentTree('codex'), { conversation: jest.fn(() => new Promise(() => {})) });
      const view = await render(<Chat started={started(fresh())} />);
      expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
      expect(view.queryByText('Loading the conversation…')).toBeNull();
      await view.unmount();

      mockParams = { host: fresh(), terminal: '7' };
      const plain = await render(<Session />);
      expect(plain.getByText('Loading the conversation…')).toBeTruthy();
      await plain.unmount();
    });

    test('still says when the Mac can never read this agent’s conversation', async () => {
      mockParams = { host: 'x', at: '1000' } as never;
      mockConn = connection(agentTree('aider'), {
        conversation: jest.fn(async () => reply("aider's sessions can't be read as a conversation yet")),
      });
      const view = await render(<Chat started={started(fresh())} />);
      await waitFor(() => expect(view.getByText("Aider's sessions can't be read as a conversation yet.")).toBeTruthy());
      expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
      await view.unmount();
    });

    test('still says when the desktop keeps failing to send the conversation', async () => {
      mockParams = { host: 'x', at: '1000' } as never;
      mockConn = connection(agentTree('claude'), {
        conversation: jest.fn(async () => {
          throw new Error('socket closed');
        }),
      });
      const view = await render(<Chat started={started(fresh())} />);
      await waitFor(() => expect(view.getByText(/isn’t sending the conversation: socket closed/)).toBeTruthy(), {
        timeout: 4000,
      });
      expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
      await view.unmount();
    });

    test('stays the chat when the Claude terminal shows agent view', async () => {
      mockParams = { host: 'x', at: '1000' } as never;
      mockConn = connection(agentTree('claude', { agentView: { listed: true, sessions: [] } }), {
        conversation: jest.fn(async () => reply(opening[0])),
      });
      const view = await render(<Chat started={started(fresh())} />);
      await waitFor(() => expect(mockConn.conversation).toHaveBeenCalled());
      expect(view.queryByText('Agent view')).toBeNull();
      expect(view.getByLabelText('Fix the login. Sending.')).toBeTruthy();
      expect(view.getByPlaceholderText('Message Claude')).toBeTruthy();
      await view.unmount();

      // The same terminal opened on its own is the list.
      mockParams = { host: fresh(), terminal: '7' };
      const list = await render(<Session />);
      expect(list.getByText('Agent view')).toBeTruthy();
      await list.unmount();
    });
  });

  test('attaches a photo from the + menu and sends it with the words', async () => {
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Attach a photo or file'));
    await fireEvent.press(view.getByText('Photos'));
    await waitFor(() => expect(view.getByLabelText('Remove IMG_1.jpg')).toBeTruthy());
    expect(mockFromPhotos).toHaveBeenCalledWith(10);
    // The chips send at once, so not while a photo waits for the message.
    expect(view.queryByText('Continue')).toBeNull();

    await fireEvent.changeText(view.getByPlaceholderText('Message Codex'), 'the tab bar');
    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(mockConn.sendFiles).toHaveBeenCalledWith(7n, 'the tab bar', [mockPhoto]));
    // The desktop types the words and Enter itself.
    expect(mockConn.type).not.toHaveBeenCalled();
    expect(view.getByLabelText('IMG_1.jpg, sending')).toBeTruthy();
    expect(view.queryByLabelText('Remove IMG_1.jpg')).toBeNull();
    await view.unmount();
  });

  test('puts photos back in the field when they could not be sent', async () => {
    mockConn = connection(tree(), { sendFiles: jest.fn(async () => { throw new Error('the connection to the desktop dropped'); }) });
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Attach a photo or file'));
    await fireEvent.press(view.getByText('Photos'));
    await waitFor(() => expect(view.getByLabelText('Remove IMG_1.jpg')).toBeTruthy());
    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(view.getByText(/Not sent: the connection to the desktop dropped. Everything is back in the field/)).toBeTruthy());
    expect(view.getByLabelText('Remove IMG_1.jpg')).toBeTruthy();
    await view.unmount();
  });

  test('says a desktop too old for files is, under +', async () => {
    mockConn = connection(tree(), { hostMinor: 11 });
    const view = await render(<Session />);
    await fireEvent.press(view.getByLabelText('Attach a photo or file'));
    expect(view.getByText(/is older than this app\. Update it, rebuild ket and start a\s+new host, to send photos and files/)).toBeTruthy();
    expect(view.queryByText('Photos')).toBeNull();
    await view.unmount();
  });

  test('sends with one press after the desktop took control back', async () => {
    let first = true;
    mockConn = connection(tree(), {
      type: jest.fn(async () => {
        if (!first) return true;
        first = false;
        return false;
      }),
    });
    const view = await render(<Session />);
    await fireEvent.changeText(view.getByPlaceholderText('Message Codex'), 'again');
    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '\r'));
    expect(mockConn.takeControl).toHaveBeenCalledTimes(2);
    expect(mockConn.type.mock.calls.filter(([, text]: any) => text === 'again')).toHaveLength(2);
    expect(view.queryByText(/Send again/)).toBeNull();
    await view.unmount();
  });

  test('reads back a message with images and files as chips', async () => {
    mockParams = { host: 'readback', terminal: '7' };
    mockConn = connection(tree(), {
      conversation: jest.fn(async () => ({
        supported: true, reason: '', fresh: true, session: 'one', next: 4n, queued: [],
        turns: [{ kind: 1, images: 2, text: 'Both are from Safari\n\nAttached files:\n- /up/1/console.txt' }],
      })),
    });
    const view = await render(<Session />);
    await waitFor(() => expect(view.getByText('2 images')).toBeTruthy());
    expect(view.getByText('Both are from Safari')).toBeTruthy();
    expect(view.getByText('console.txt')).toBeTruthy();
    expect(view.queryByText(/Attached files/)).toBeNull();
    await view.unmount();
  });

  test('writes what the mic hears into the draft, and Send sends it', async () => {
    const view = await render(<Session />);
    const input = view.getByPlaceholderText('Message Codex');
    await fireEvent.press(view.getByLabelText('Dictate'));
    await waitFor(() => expect(view.getByText('Listening')).toBeTruthy());
    expect(mockListen).toHaveBeenCalledWith(false, expect.anything());
    expect(view.getByText(/transcribed on this iPhone/)).toBeTruthy();
    await act(async () => mockEars?.onHeard('run the tests'));
    expect(input.props.value).toBe('run the tests');

    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'run the tests'));
    expect(view.queryByText('Listening')).toBeNull();
    await view.unmount();
  });
});
