import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { render, waitFor } from '@testing-library/react-native';
import Pair from './app/pair';

// Every step the screen takes once the desktop answers, in the order taken.
const mockSteps: string[] = [];
let mockAnswer: { payload: { case: string; value: object } };

jest.mock('expo-router', () => ({
  router: { replace: (path: string) => mockSteps.push(`replace ${path}`), back: jest.fn(), canGoBack: () => true },
  useLocalSearchParams: () => ({ code: 'ket://pair/code' }),
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('expo-camera', () => ({ CameraView: () => null, useCameraPermissions: () => [{ granted: false }, jest.fn()] }));
jest.mock('expo-device', () => ({ deviceName: 'Test iPhone' }));
jest.mock('./lib/version', () => ({ versionLabel: '1.0 (1)' }));
jest.mock('./components/Ping', () => ({ Ping: () => null }));
jest.mock('@ket/remote', () => ({
  pair: jest.fn(async () => ({
    remote: { recv: async () => mockAnswer, close: () => mockSteps.push('close') },
    host: { hostId: new Uint8Array([1]), relay: 'ws://studio.local:7979', hostPublic: new Uint8Array([2]) },
    confirmation: '123456',
  })),
  toBase64Url: (bytes: Uint8Array) => (bytes[0] === 1 ? 'host' : 'key'),
}));
jest.mock('./lib/store', () => ({
  phoneKey: jest.fn(async () => ({})),
  saveHost: async (host: { id: string; relay: string }) => {
    mockSteps.push(`save ${host.id} ${host.relay}`);
  },
}));
jest.mock('./lib/connection', () => ({ dropConnection: (id: string) => mockSteps.push(`drop ${id}`) }));

describe('Pair', () => {
  beforeEach(() => {
    mockSteps.length = 0;
  });

  test('pairing drops the open connection to that desktop before showing it', async () => {
    mockAnswer = { payload: { case: 'welcome', value: { hostName: 'Studio Mac' } } };
    await render(<Pair />);
    await waitFor(() => expect(mockSteps).toContain('replace /host/host'));
    // Saved first, so the connection the next screen builds reads the new
    // pairing; dropped before it opens, so it can't reuse the old one.
    expect(mockSteps).toEqual(['close', 'save host ws://studio.local:7979', 'drop host', 'replace /host/host']);
  });

  test('a refused pairing leaves the open connection alone', async () => {
    mockAnswer = { payload: { case: 'error', value: { message: 'That code has expired.' } } };
    const view = await render(<Pair />);
    await waitFor(() => expect(view.getByText('That code has expired.')).toBeTruthy());
    expect(mockSteps).toEqual(['close']);
  });
});
