import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import { render, waitFor } from '@testing-library/react-native';
import Reach from './app/reach';

const mockSteps: string[] = [];
let mockParams: Record<string, string> = {};
let mockSaved: object | null = null;

jest.mock('expo-router', () => ({
  router: { replace: (path: string) => mockSteps.push(`replace ${path}`), back: jest.fn(), canGoBack: () => true },
  useLocalSearchParams: () => mockParams,
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('@ket/remote', () => ({ validRelayUrl: (url: string) => /^wss?:\/\/[^/\s]+$/.test(url) }));
jest.mock('./lib/store', () => ({
  getHost: async (id: string) => (mockSaved && id === 'AAAAAAAAAAAAAAAAAAAAAA' ? mockSaved : null),
  saveHost: async (host: { addresses: string[] }) => {
    mockSteps.push(`save ${host.addresses.join(' ')}`);
  },
}));
jest.mock('./lib/connection', () => ({ dropConnection: (id: string) => mockSteps.push(`drop ${id}`) }));

// Decoded, as the link router hands it over.
const at = 'ws://Studio.local:7979 ws://100.101.102.103:7979';

describe('Reach', () => {
  beforeEach(() => {
    mockSteps.length = 0;
    mockSaved = {
      id: 'AAAAAAAAAAAAAAAAAAAAAA',
      name: 'Studio Mac',
      relay: 'ws://Studio.local:7979',
      addresses: ['ws://Studio.local:7979'],
      hostPublic: '',
      pairedAt: 0,
    };
  });

  test('adds the addresses to the desktop this phone knows and shows them', async () => {
    mockParams = { host: 'AAAAAAAAAAAAAAAAAAAAAA', at };
    const view = await render(<Reach />);
    await waitFor(() => expect(mockSteps.at(-1)).toBe('replace /devices/AAAAAAAAAAAAAAAAAAAAAA'));
    expect(mockSteps).toEqual([
      'save ws://Studio.local:7979 ws://100.101.102.103:7979',
      'drop AAAAAAAAAAAAAAAAAAAAAA',
      'replace /devices/AAAAAAAAAAAAAAAAAAAAAA',
    ]);
    await view.unmount();
  });

  test('says when the desktop is not one this phone is paired with', async () => {
    mockSaved = null;
    mockParams = { host: 'AAAAAAAAAAAAAAAAAAAAAA', at };
    const view = await render(<Reach />);
    await waitFor(() => expect(view.getByText(/isn’t paired with/)).toBeTruthy());
    expect(mockSteps).toEqual([]);
    await view.unmount();
  });

  test('says when it is not a code at all', async () => {
    mockParams = { host: 'AAAAAAAAAAAAAAAAAAAAAA' };
    const view = await render(<Reach />);
    await waitFor(() => expect(view.getByText(/isn’t a code ket can use/)).toBeTruthy());
    expect(mockSteps).toEqual([]);
    await view.unmount();
  });
});
