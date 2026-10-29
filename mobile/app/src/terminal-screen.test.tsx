import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render, waitFor } from '@testing-library/react-native';
import * as mockReact from 'react';
import { Pressable as MockPressable, Text as MockText } from 'react-native';
import TerminalScreen from './app/terminal/[host]/[terminal]';

const mockRouter = { setParams: jest.fn() };
const mockReset = jest.fn();
const mockWrite = jest.fn();
const mockSetInteractive = jest.fn();
const mockSetFit = jest.fn();
let mockChange: (() => void) | undefined;
let mockMessage: ((message: any) => void) | undefined;
let mockConn: any;
let mockParams = { host: 'host', terminal: '7' };

jest.mock('expo-router', () => ({
  router: { setParams: (params: object) => mockRouter.setParams(params) },
  useLocalSearchParams: () => mockParams,
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('@ket/remote', () => ({ Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 } }));
jest.mock('./lib/hosts', () => ({ useHost: () => mockConn }));
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
  mockParams = { host: 'host', terminal: '7' };
});

afterEach(() => {
  jest.useRealTimers();
});

describe('TerminalScreen', () => {
  test('subscribes, draws host output, resizes, and navigates tabs', async () => {
    const view = await render(<TerminalScreen />);
    // With what to show if the desktop refuses it.
    await waitFor(() => expect(mockConn.subscribe).toHaveBeenCalledWith(7n, expect.any(Function)));
    expect(mockSetFit).toHaveBeenCalledWith(true);

    await fireEvent.press(view.getByLabelText('Terminal size'));
    expect(mockConn.resize).toHaveBeenCalledWith(7n, 90, 30);
    await fireEvent.press(view.getByText('Shell'));
    expect(mockRouter.setParams).toHaveBeenCalledWith({ terminal: '8' });

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
    await fireEvent.press(view.getByLabelText('Shift Tab'));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '\x1b[Z'));
    await fireEvent.press(view.getByLabelText('ctrl'));
    await fireEvent.press(view.getByLabelText('Left'));
    await fireEvent.press(view.getByLabelText('Interrupt'));
    expect(mockConn.interrupt).toHaveBeenCalledWith(7n);

    const input = view.getByPlaceholderText('Message codex…');
    await fireEvent.changeText(input, 'hello');
    await fireEvent.press(view.getByLabelText('Send'));
    await act(async () => { await jest.advanceTimersByTimeAsync(90); });
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '\r'));

    // No badge to let go with: leaving the screen does.
    expect(view.queryByText('you have control')).toBeNull();
    expect(mockConn.releaseControl).not.toHaveBeenCalled();
    await view.unmount();
    expect(mockConn.releaseControl).toHaveBeenCalledWith(7n);
  });

  test('live keys go in one at a time, with no Enter of their own', async () => {
    const view = await render(<TerminalScreen />);
    await waitFor(() => expect(view.getByPlaceholderText('Message codex…')).toBeTruthy());
    await fireEvent.press(view.getByText('Live keys'));
    const field = view.getByPlaceholderText('Each key goes straight in');
    expect(view.queryByLabelText('Send')).toBeNull();

    // A lone digit, as Claude's offers want it.
    await fireEvent.changeText(field, '1');
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, '1'));
    await act(async () => { await jest.advanceTimersByTimeAsync(200); });
    expect(mockConn.type).not.toHaveBeenCalledWith(7n, '\r');

    // Each change from where it parts from the last: Backspaces, then keys.
    await fireEvent.changeText(field, '12');
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '2'));
    await fireEvent.changeText(field, '1x');
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\x7fx'));

    // Backspace in a field already empty is only heard as a key.
    await fireEvent(field, 'keyPress', { nativeEvent: { key: 'Backspace' } });
    expect(mockConn.type).toHaveBeenCalledTimes(3);
    await fireEvent.press(view.getByLabelText('Enter'));
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\r'));
    await fireEvent(field, 'keyPress', { nativeEvent: { key: 'Backspace' } });
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\x7f'));

    // The ctrl key still latches onto the next key.
    await fireEvent.press(view.getByLabelText('Show keys'));
    await fireEvent.press(view.getByLabelText('ctrl'));
    await fireEvent.changeText(field, 'c');
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\x03'));

    // And back: the message line, Enter and all.
    await fireEvent.press(view.getByText('Message'));
    expect(view.getByPlaceholderText('Message codex…')).toBeTruthy();
    await view.unmount();
  });

  test('keys typed before control is taken keep their order', async () => {
    let grant: (ok: boolean) => void = () => {};
    mockConn.takeControl.mockImplementationOnce(() => new Promise<boolean>((resolve) => { grant = resolve; }));
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    const field = view.getByPlaceholderText('Each key goes straight in');
    await fireEvent.changeText(field, 'a');
    await fireEvent.changeText(field, 'ab');
    expect(mockConn.type).not.toHaveBeenCalled();
    await act(async () => grant(true));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledTimes(2));
    expect(mockConn.type.mock.calls.map((call: any) => call[1])).toEqual(['a', 'b']);
    expect(mockConn.takeControl).toHaveBeenCalledTimes(1);
    await view.unmount();
  });

  test('ctrl goes to the key pressed while it was held, however slow control is', async () => {
    let grant: (ok: boolean) => void = () => {};
    mockConn.takeControl.mockImplementationOnce(() => new Promise<boolean>((resolve) => { grant = resolve; }));
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    await fireEvent.press(view.getByLabelText('Show keys'));
    await fireEvent.press(view.getByLabelText('ctrl'));
    const field = view.getByPlaceholderText('Each key goes straight in');
    await fireEvent.changeText(field, 'a');
    await fireEvent.changeText(field, 'ab');
    // The key row waits its turn behind them.
    await fireEvent.press(view.getByLabelText('Up'));
    await act(async () => grant(true));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledTimes(3));
    expect(mockConn.type.mock.calls.map((call: any) => call[1])).toEqual(['\x01', 'b', '\x1b[A']);
    await view.unmount();
  });

  test('an Enter held back with its text still comes after a pause', async () => {
    let grant: (ok: boolean) => void = () => {};
    mockConn.takeControl.mockImplementationOnce(() => new Promise<boolean>((resolve) => { grant = resolve; }));
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'hi');
    await fireEvent.press(view.getByLabelText('Enter'));
    await act(async () => grant(true));
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'hi'));
    expect(mockConn.type).not.toHaveBeenCalledWith(7n, '\r');
    await act(async () => { await jest.advanceTimersByTimeAsync(90); });
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\r'));
    await view.unmount();
  });

  test('keys queued behind one the desktop refused are dropped', async () => {
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'x');
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'x'));
    let refuse: (ok: boolean) => void = () => {};
    mockConn.type.mockImplementationOnce(() => new Promise<boolean>((resolve) => { refuse = resolve; }));
    await act(async () => { await jest.advanceTimersByTimeAsync(200); });
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'xy');
    // Enter waits its pause behind the key; the desktop refuses the key in it.
    await fireEvent.press(view.getByLabelText('Enter'));
    await act(async () => refuse(false));
    await waitFor(() => expect(view.getByText('Your desktop took control back.')).toBeTruthy());
    await act(async () => { await jest.advanceTimersByTimeAsync(200); });
    expect(mockConn.type).not.toHaveBeenCalledWith(7n, '\r');
    expect(mockConn.takeControl).toHaveBeenCalledTimes(1);
    await view.unmount();
  });

  test('the Backspace that empties the field counts once, key or change first', async () => {
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    const field = view.getByPlaceholderText('Each key goes straight in');
    const backspace = () => fireEvent(field, 'keyPress', { nativeEvent: { key: 'Backspace' } });
    const sent = () => mockConn.type.mock.calls.map((call: any) => call[1]);

    // The key first, as iOS tells it, then a second Backspace at once.
    await fireEvent.changeText(field, 'a');
    await backspace();
    await fireEvent.changeText(field, '');
    await backspace();
    await waitFor(() => expect(sent()).toEqual(['a', '\x7f', '\x7f']));

    // The change first, as some Android keyboards tell it, then a repeat.
    await fireEvent.changeText(field, 'b');
    await fireEvent.changeText(field, '');
    await backspace();
    await backspace();
    await waitFor(() => expect(sent()).toEqual(['a', '\x7f', '\x7f', 'b', '\x7f', '\x7f']));

    // Two deleted quickly, change first each time: two Backspaces, not three.
    await fireEvent.changeText(field, 'cd');
    await fireEvent.changeText(field, 'c');
    await backspace();
    await fireEvent.changeText(field, '');
    await backspace();
    await waitFor(() => expect(sent().slice(6)).toEqual(['cd', '\x7f', '\x7f']));
    await view.unmount();
  });

  test('a refusal for a terminal left behind leaves the new one alone', async () => {
    let refuse: (ok: boolean) => void = () => {};
    mockConn.type.mockImplementationOnce(() => new Promise<boolean>((resolve) => { refuse = resolve; }));
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'a');
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(7n, 'a'));

    let grant: (ok: boolean) => void = () => {};
    mockConn.takeControl.mockImplementationOnce(() => new Promise<boolean>((resolve) => { grant = resolve; }));
    mockParams = { host: 'host', terminal: '8' };
    await view.rerender(<TerminalScreen />);
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'z');
    await act(async () => refuse(false));
    await act(async () => grant(true));
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(8n, 'z'));
    expect(view.queryByText('Your desktop took control back.')).toBeNull();
    await view.unmount();
  });

  test('an emoji is one Backspace and never sent in halves', async () => {
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    const field = view.getByPlaceholderText('Each key goes straight in');
    await fireEvent.changeText(field, 'a😀');
    await fireEvent.changeText(field, 'a😁');
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\x7f😁'));
    await fireEvent.changeText(field, 'a');
    await waitFor(() => expect(mockConn.type).toHaveBeenLastCalledWith(7n, '\x7f'));
    await view.unmount();
  });

  test('keys still waiting on control stay with the terminal they were typed in', async () => {
    let grant: (ok: boolean) => void = () => {};
    mockConn.takeControl.mockImplementationOnce(() => new Promise<boolean>((resolve) => { grant = resolve; }));
    const view = await render(<TerminalScreen />);
    await fireEvent.press(view.getByText('Live keys'));
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'a');
    mockParams = { host: 'host', terminal: '8' };
    await view.rerender(<TerminalScreen />);
    await act(async () => grant(true));
    // Granted too late for terminal 7: given back, nothing typed into it.
    await waitFor(() => expect(mockConn.releaseControl).toHaveBeenCalledWith(7n));
    await fireEvent.changeText(view.getByPlaceholderText('Each key goes straight in'), 'z');
    await waitFor(() => expect(mockConn.type).toHaveBeenCalledWith(8n, 'z'));
    expect(mockConn.type).not.toHaveBeenCalledWith(7n, 'a');
    await view.unmount();
  });

  test('puts the tabs in the bar, with the state on the agent’s mark', async () => {
    mockConn.snapshot.projects[0].worktrees[0].terminals[0].title = '⠋ Review changes';
    const view = await render(<TerminalScreen />);
    // The tabs are the title; the agent's spinner frame is left off its name.
    expect(view.getByLabelText('Terminals in Tree')).toBeTruthy();
    expect(view.getByLabelText('Review changes, busy').props.accessibilityState).toEqual({ selected: true });
    expect(view.getByLabelText('Shell').props.accessibilityState).toEqual({ selected: false });
    expect(view.queryByText('busy')).toBeNull();
    await view.unmount();
  });

  test('titles a lone terminal in the bar, its state on the mark', async () => {
    mockConn.snapshot.projects[0].worktrees[0].terminals = [{ id: 7n, agent: 'codex', title: '', closed: false }];
    const view = await render(<TerminalScreen />);
    expect(view.queryByLabelText('Terminals in Tree')).toBeNull();
    expect(view.getByLabelText('Codex, busy')).toBeTruthy();
    expect(view.getByText('Codex')).toBeTruthy();
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
