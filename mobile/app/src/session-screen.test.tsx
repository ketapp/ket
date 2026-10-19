import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render, waitFor } from '@testing-library/react-native';
import { useEffect as mockUseEffect } from 'react';
import { Linking } from 'react-native';
import Session from './app/session/[host]/[terminal]';

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
  Turn_Kind: { USER: 1, ASSISTANT: 2, TOOL: 3 },
}));
jest.mock('./lib/hosts', () => ({ useHost: () => mockConn }));
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

const tree = (fields: object = {}) => ({
  id: 'tree', name: 'Tree', agent: 'codex', activity: 0, agentState: '', subagents: [], questions: [],
  terminals: [{ id: 7n, agent: 'codex', title: 'Codex', closed: false }],
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
    const open = jest.spyOn(Linking, 'openURL').mockResolvedValue(true);
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

  test('answers permission prompts, including identity checks', async () => {
    const question = {
      terminalId: 7n, answerable: true, tool: 'Bash', subject: 'npm test', always: true,
      risk: { level: 'medium', confidence: 0.8 },
    };
    mockConn = connection(tree({ activity: 4, question }));
    const view = await render(<Session />);
    expect(view.getByText('Wants to use Bash')).toBeTruthy();
    expect(view.getByText('$ npm test')).toBeTruthy();
    expect(view.getByText('Medium risk · 80%')).toBeTruthy();
    await fireEvent.press(view.getByText('Allow once'));
    await waitFor(() => expect(mockConn.answer).toHaveBeenCalledWith(question, 1));

    mockConfirmIdentity.mockResolvedValueOnce(false);
    await fireEvent.press(view.getByText('Always'));
    expect(mockConfirmIdentity).toHaveBeenCalledWith('Always allow Bash');
    expect(mockConn.answer).toHaveBeenCalledTimes(1);
    await fireEvent.press(view.getByText('Deny'));
    await waitFor(() => expect(mockConn.answer).toHaveBeenCalledWith(question, 3));
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
