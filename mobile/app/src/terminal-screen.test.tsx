import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render, waitFor } from '@testing-library/react-native';
import * as mockReact from 'react';
import { Pressable as MockPressable, Text as MockText } from 'react-native';
import TerminalScreen from './app/terminal/[host]/[terminal]';

const mockRouter = { replace: jest.fn() };
const mockSaveRecent = jest.fn(async (_recent: object) => {});
const mockReset = jest.fn();
const mockWrite = jest.fn();
const mockSetInteractive = jest.fn();
const mockSetFit = jest.fn();
let mockChange: (() => void) | undefined;
let mockMessage: ((message: any) => void) | undefined;
let mockConn: any;

jest.mock('expo-router', () => ({
  router: { replace: (path: string) => mockRouter.replace(path) },
  useLocalSearchParams: () => ({ host: 'host', terminal: '7' }),
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('@ket/remote', () => ({ Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 } }));
jest.mock('./lib/hosts', () => ({ useHost: () => mockConn }));
jest.mock('./lib/store', () => ({
  getFit: jest.fn(async () => true),
  saveRecent: (recent: object) => mockSaveRecent(recent),
}));
jest.mock('./components/TerminalView', () => {
  return { TerminalView: mockReact.forwardRef(function MockTerminalView(props: any, ref: any) {
    mockReact.useImperativeHandle(ref, () => ({
      reset: (value: object) => mockReset(value),
      write: (value: Uint8Array) => mockWrite(value),
      setInteractive: (value: boolean) => mockSetInteractive(value),
      setFit: (value: boolean) => mockSetFit(value),
    }));
    mockReact.useEffect(() => props.onReady(), [props.onReady]);
    return (
      <>
        <MockPressable accessibilityLabel="Terminal input" onPress={() => props.onInput('x')}><MockText>input</MockText></MockPressable>
        <MockPressable accessibilityLabel="Terminal size" onPress={() => props.onSize(90, 30)}><MockText>size</MockText></MockPressable>
      </>
    );
  }) };
});

const makeConnection = () => ({
  status: 'connected',
  snapshot: {
    projects: [{
      id: 'project', name: 'Project', color: '#fff', worktrees: [{
        id: 'tree', name: 'Tree', activity: 1, agent: 'codex', subagents: [],
        terminals: [
          { id: 7n, agent: 'codex', title: 'Main', closed: false },
          { id: 8n, agent: '', title: 'Shell', closed: false },
        ],
      }],
    }],
  },
  subscribe: jest.fn(),
  unsubscribe: jest.fn(),
  onChange: jest.fn((fn: () => void) => { mockChange = fn; return jest.fn(); }),
  onMessage: jest.fn((fn: (message: any) => void) => { mockMessage = fn; return jest.fn(); }),
  takeControl: jest.fn(async () => true),
  releaseControl: jest.fn(),
  type: jest.fn(async () => true),
  resize: jest.fn(async () => {}),
  interrupt: jest.fn(),
});

beforeEach(() => {
  jest.useFakeTimers();
  jest.clearAllMocks();
  mockChange = undefined;
  mockMessage = undefined;
  mockConn = makeConnection();
});

afterEach(() => {
  jest.useRealTimers();
});

describe('TerminalScreen', () => {
  test('subscribes, draws host output, resizes, and navigates tabs', async () => {
    const view = await render(<TerminalScreen />);
    await waitFor(() => expect(mockConn.subscribe).toHaveBeenCalledWith(7n));
    expect(mockSaveRecent).toHaveBeenCalledWith({ host: 'host', terminal: '7' });
    expect(mockSetFit).toHaveBeenCalledWith(true);

    await fireEvent.press(view.getByLabelText('Terminal size'));
    expect(mockConn.resize).toHaveBeenCalledWith(7n, 90, 30);
    await fireEvent.press(view.getByText('Shell'));
    expect(mockRouter.replace).toHaveBeenCalledWith('/terminal/host/8');

    await act(async () => {
      mockMessage?.({ payload: { case: 'checkpoint', value: { terminalId: 7n, cols: 80, rows: 24, scrollback: 2, ansi: new Uint8Array([1]) } } });
      mockMessage?.({ payload: { case: 'chunk', value: { terminalId: 7n, bytes: new Uint8Array([2]) } } });
      mockMessage?.({ payload: { case: 'closed', value: { terminalId: 7n } } });
      await Promise.resolve();
    });
    expect(mockReset).toHaveBeenCalled();
    expect(mockWrite).toHaveBeenCalled();
    expect(view.getByText('The program has exited.')).toBeTruthy();

    mockConn.status = 'offline';
    await act(async () => mockChange?.());
    mockConn.status = 'connected';
    await act(async () => mockChange?.());
    expect(mockConn.subscribe).toHaveBeenCalledTimes(2);
    await view.unmount();
    expect(mockConn.unsubscribe).toHaveBeenCalledWith(7n);
  });

  test('takes control, sends messages and special keys, then releases', async () => {
    const view = await render(<TerminalScreen />);
    await waitFor(() => expect(view.getByPlaceholderText('Message codex…')).toBeTruthy());
    await fireEvent.press(view.getByLabelText('Terminal input'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'x'));

    await fireEvent.press(view.getByLabelText('Show keys'));
    await fireEvent.press(view.getByLabelText('ctrl'));
    await fireEvent.press(view.getByLabelText('Left'));
    await fireEvent.press(view.getByLabelText('Interrupt'));
    expect(mockConn.interrupt).toHaveBeenCalledWith(7n);

    const input = view.getByPlaceholderText('Message codex…');
    await fireEvent.changeText(input, 'hello');
    await fireEvent.press(view.getByLabelText('Send'));
    await act(async () => { await jest.advanceTimersByTimeAsync(90); });
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '\r'));

    await fireEvent.press(view.getByLabelText('Let go of control'));
    expect(mockConn.releaseControl).toHaveBeenCalledWith(7n);
    await fireEvent.press(view.getByLabelText('Full screen'));
    expect(view.getByLabelText('Leave full screen')).toBeTruthy();
    await fireEvent.press(view.getByLabelText('Leave full screen'));
    await view.unmount();
  });

  test('reports denied control, takeover, and transport errors', async () => {
    mockConn.takeControl.mockResolvedValueOnce(false);
    const view = await render(<TerminalScreen />);
    await waitFor(() => expect(view.getByPlaceholderText('Message codex…')).toBeTruthy());
    const input = view.getByPlaceholderText('Message codex…');
    await fireEvent.changeText(input, 'blocked');
    await fireEvent.press(view.getByLabelText('Send'));
    await waitFor(() => expect(view.getByText('Another device is in control of this terminal.')).toBeTruthy());
    expect(input.props.value).toBe('blocked');

    mockConn.takeControl.mockResolvedValue(true);
    mockConn.type.mockResolvedValueOnce(false);
    await fireEvent.press(view.getByLabelText('Terminal input'));
    await waitFor(() => expect(view.getByText('Your desktop took control back.')).toBeTruthy());

    mockConn.takeControl.mockResolvedValue(true);
    mockConn.type.mockRejectedValueOnce(new Error('offline'));
    await fireEvent.press(view.getByLabelText('Terminal input'));
    await waitFor(() => expect(view.getByText('Not sent: offline')).toBeTruthy());
    await view.unmount();
  });
});
