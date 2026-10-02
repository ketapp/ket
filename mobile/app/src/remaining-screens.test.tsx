import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render, waitFor } from '@testing-library/react-native';
import Devices from './app/devices/[host]';
import DiffScreen from './app/diff/[host]/[worktree]';
import Pair from './app/pair';
import type { HostConnection } from './lib/connection';

let mockParams: Record<string, string> = {};
let mockHost: HostConnection | null = null;
let mockPermission = { granted: false };
const mockRequestPermission = jest.fn(async () => {});
const mockReplace = jest.fn();
const mockPair = jest.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
const mockSaveHost = jest.fn(async (_host: object) => {});
const mockForgetHost = jest.fn(async (_id: string) => {});
const mockDropConnection = jest.fn();
const mockConfirm = jest.fn(async (..._args: unknown[]) => true);

jest.mock('expo-router', () => ({
  router: { replace: (path: string) => mockReplace(path), back: jest.fn(), canGoBack: () => true },
  useLocalSearchParams: () => mockParams,
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('expo-camera', () => ({
  CameraView: 'CameraView',
  useCameraPermissions: () => [mockPermission, mockRequestPermission],
}));
jest.mock('expo-device', () => ({ deviceName: 'Test Phone' }));
jest.mock('@ket/remote', () => ({
  pair: (...args: unknown[]) => mockPair(...args),
  toBase64Url: (bytes: Uint8Array) => [...bytes].join('-'),
}));
jest.mock('./lib/hosts', () => ({ useHost: () => mockHost }));
jest.mock('./lib/store', () => ({
  phoneKey: jest.fn(async () => ({ privateKey: Uint8Array.of(1), publicKey: Uint8Array.of(2) })),
  saveHost: (host: object) => mockSaveHost(host),
  forgetHost: (id: string) => mockForgetHost(id),
}));
jest.mock('./lib/connection', () => ({ dropConnection: (id: string) => mockDropConnection(id) }));
jest.mock('./lib/confirm', () => ({ confirm: (...args: unknown[]) => mockConfirm(...args) }));
jest.mock('./lib/version', () => ({ versionLabel: 'test' }));

const host = (fields: object = {}) =>
  ({
    host: { id: 'host', name: 'Studio Mac', relay: 'ws://studio', hostPublic: '', pairedAt: 0 },
    devices: jest.fn(async () => []),
    revoke: jest.fn(async () => {}),
    diff: jest.fn(async () => ({ text: '', error: '', truncated: false })),
    ...fields,
  }) as unknown as HostConnection;

beforeEach(() => {
  jest.clearAllMocks();
  mockParams = {};
  mockHost = null;
  mockPermission = { granted: false };
  mockConfirm.mockResolvedValue(true);
});

describe('Devices', () => {
  test('loads, revokes, and forgets devices', async () => {
    mockParams = { host: 'host' };
    const devices = [
      { id: 'self', name: '', thisDevice: true, connected: true, pairedAt: 1n },
      { id: 'other', name: 'iPad', thisDevice: false, connected: false, pairedAt: 2n },
    ];
    const conn = host();
    const load = conn.devices as jest.MockedFunction<HostConnection['devices']>;
    load.mockResolvedValue(devices as Awaited<ReturnType<HostConnection['devices']>>);
    mockHost = conn;
    const view = await render(<Devices />);
    await waitFor(() => expect(view.getByText('iPad')).toBeTruthy());
    expect(view.getByText('Unnamed phone (this one)')).toBeTruthy();
    await fireEvent.press(view.getByText('iPad'));
    await waitFor(() => expect(conn.revoke).toHaveBeenCalledWith('other'));
    await fireEvent.press(view.getByText('Forget this desktop'));
    await waitFor(() => expect(mockReplace).toHaveBeenCalledWith('/settings'));
    expect(conn.revoke).toHaveBeenCalledWith('self');
    expect(mockDropConnection).toHaveBeenCalledWith('host');
    expect(mockForgetHost).toHaveBeenCalledWith('host');
    await view.unmount();
  });

  test('shows loading and host errors', async () => {
    mockParams = { host: 'host' };
    let reject!: (error: Error) => void;
    mockHost = host({ devices: jest.fn(() => new Promise((_resolve, no) => (reject = no))) });
    const view = await render(<Devices />);
    expect(view.getByText('Asking the desktop…')).toBeTruthy();
    reject(new Error('offline'));
    await waitFor(() => expect(view.getByText('offline')).toBeTruthy());
    await view.unmount();
  });
});

describe('DiffScreen', () => {
  test('parses hunks, line kinds, tabs, and truncation', async () => {
    mockParams = { host: 'host', worktree: 'tree', path: 'src/file.ts' };
    mockHost = host({
      diff: jest.fn(async () => ({
        text: '--- a/src/file.ts\n+++ b/src/file.ts\n@@ -2,2 +2,3 @@ fn\n same\n-old\n+new\n+\ttab\n\\ No newline',
        error: '',
        truncated: true,
      })),
    });
    const view = await render(<DiffScreen />);
    await waitFor(() => expect(view.getByText('Line 2 · fn')).toBeTruthy());
    expect(view.getByText('old')).toBeTruthy();
    expect(view.getByText('new')).toBeTruthy();
    expect(view.getByText('  tab')).toBeTruthy();
    expect(view.getByText('The rest is too long to show here; see it on your Mac.')).toBeTruthy();
    await view.unmount();
  });

  test('shows empty and error results', async () => {
    mockParams = { host: 'host', worktree: 'tree', path: 'image.png' };
    mockHost = host();
    const empty = await render(<DiffScreen />);
    await waitFor(() => expect(empty.getByText('No line changes — a binary file, or only its mode changed.')).toBeTruthy());
    await empty.unmount();

    mockHost = host({ diff: jest.fn(async () => ({ text: '', error: 'cannot diff', truncated: false })) });
    const error = await render(<DiffScreen />);
    await waitFor(() => expect(error.getByText('Cannot diff')).toBeTruthy());
    await error.unmount();
  });
});

describe('Pair', () => {
  test('requests camera permission and completes pairing', async () => {
    mockParams = {};
    const recv = jest.fn(async (): Promise<unknown> => undefined);
    recv
      .mockResolvedValueOnce({ payload: { case: 'pairingPending', value: { code: '123456', timeoutSeconds: 1 } } })
      .mockResolvedValueOnce({ payload: { case: 'welcome', value: { hostName: 'Home Mac' } } });
    const remote = {
      recv,
      close: jest.fn(),
    };
    mockPair.mockResolvedValue({
      remote,
      host: { hostId: Uint8Array.of(1, 2), hostPublic: Uint8Array.of(3, 4), relay: 'ws://home' },
      confirmation: '123456',
    });
    const view = await render(<Pair />);
    await fireEvent.press(view.getByText('Allow the camera'));
    expect(mockRequestPermission).toHaveBeenCalled();
    await fireEvent.changeText(view.getByPlaceholderText('ket://pair/…'), ' ket://pair/code ');
    await fireEvent.press(view.getByText('Pair with a pasted code'));
    await waitFor(() => expect(mockReplace).toHaveBeenCalledWith('/host/1-2'));
    expect(mockSaveHost).toHaveBeenCalledWith(expect.objectContaining({ name: 'Home Mac', hostPublic: '3-4' }));
    expect(remote.close).toHaveBeenCalled();
    await view.unmount();
  });

  test('shows handshake failures and ignores invalid pasted text', async () => {
    mockPair.mockRejectedValue(new Error('expired code'));
    const view = await render(<Pair />);
    await fireEvent.changeText(view.getByPlaceholderText('ket://pair/…'), 'not a code');
    await fireEvent.press(view.getByText('Pair with a pasted code'));
    expect(mockPair).not.toHaveBeenCalled();
    await fireEvent.changeText(view.getByPlaceholderText('ket://pair/…'), 'https://ket.test/pair?code=ket%3A%2F%2Fpair%2Fabc');
    await fireEvent.press(view.getByText('Pair with a pasted code'));
    await waitFor(() => expect(view.getByText('expired code')).toBeTruthy());
    await view.unmount();
  });
});
