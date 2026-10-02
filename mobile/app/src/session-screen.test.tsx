import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render, waitFor } from '@testing-library/react-native';
import { Linking } from 'react-native';
import Session from './app/session/[host]/[terminal]';

const mockRouter = { push: jest.fn() };
const mockConfirmIdentity = jest.fn(async (_reason: string) => true);
let mockConn: any;

jest.mock('expo-router', () => ({
  router: { push: (path: string) => mockRouter.push(path) },
  useLocalSearchParams: () => ({ host: 'host', terminal: '7' }),
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
  Answer_Choice: { ALLOW_ONCE: 1, ALLOW_ALWAYS: 2, DENY: 3 },
  Turn_Kind: { USER: 1, ASSISTANT: 2, TOOL: 3 },
}));
jest.mock('./lib/hosts', () => ({ useHost: () => mockConn }));
jest.mock('./lib/lock', () => ({ confirmIdentity: (reason: string) => mockConfirmIdentity(reason) }));

const tree = (fields: object = {}) => ({
  id: 'tree', name: 'Tree', agent: 'codex', activity: 0, agentState: '', subagents: [],
  terminals: [{ id: 7n, agent: 'codex', title: 'Codex', closed: false }],
  ...fields,
});

const connection = (worktree = tree(), fields: object = {}) => ({
  host: { id: 'host', name: 'Studio Mac' },
  hostMinor: 7,
  status: 'connected',
  snapshot: { projects: [{ id: 'project', name: 'Project', color: '#fff', worktrees: [worktree] }] },
  conversation: jest.fn(async () => ({ supported: true, reason: '', fresh: true, session: 'one', next: 4n, queued: [], turns: [] })),
  snippets: jest.fn(async () => [{ name: 'Ship', body: 'ship it' }, { name: 'Blank', body: ' ' }]),
  takeControl: jest.fn(async () => true),
  type: jest.fn(async () => true),
  releaseControl: jest.fn(),
  answer: jest.fn(async () => null),
  ...fields,
});

beforeEach(() => {
  jest.clearAllMocks();
  mockConfirmIdentity.mockResolvedValue(true);
  mockConn = connection();
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

    mockConn = connection(tree(), {
      conversation: jest.fn(async () => ({ supported: false, reason: 'not available', fresh: false, session: '', next: 0n, queued: [], turns: [] })),
    });
    const unsupported = await render(<Session />);
    await waitFor(() => expect(unsupported.getByText('Not available.')).toBeTruthy());
    await unsupported.unmount();

    mockConn = connection();
    const view = await render(<Session />);
    const input = view.getByPlaceholderText('Message Codex…');
    await fireEvent.changeText(input, ' hello ');
    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'hello'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '\r'));
    expect(view.getByText('hello')).toBeTruthy();
    await view.unmount();
    expect(mockConn.releaseControl).toHaveBeenCalledWith(7n);
  });
});
