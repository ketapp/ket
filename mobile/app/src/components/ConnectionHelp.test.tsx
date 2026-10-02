import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render } from '@testing-library/react-native';
import type { HostConnection } from '../lib/connection';
import { ConnectionHelp } from './ConnectionHelp';

const mockPush = jest.fn();
jest.mock('expo-router', () => ({ router: { push: (path: string) => mockPush(path) } }));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));

const connection = (fields: Partial<HostConnection> = {}) =>
  ({
    status: 'connecting',
    downSince: 0,
    attempts: 2,
    error: null,
    host: { name: 'Studio Mac', relay: 'ws://studio.local:7979' },
    retry: jest.fn(),
    ...fields,
  }) as HostConnection;

describe('ConnectionHelp', () => {
  beforeEach(() => {
    jest.spyOn(Date, 'now').mockReturnValue(10_000);
  });
  afterEach(() => {
    jest.restoreAllMocks();
  });

  test('hides while connected and stays quiet during an initial attempt', async () => {
    const connected = await render(<ConnectionHelp conn={connection({ status: 'connected' })} />);
    expect(connected.toJSON()).toBeNull();
    await connected.unmount();
    const quiet = await render(<ConnectionHelp conn={connection({ downSince: 9_000 })} />);
    expect(quiet.getByText('Connecting to Studio Mac…')).toBeTruthy();
    await quiet.unmount();
  });

  test('explains a stuck connection and offers recovery', async () => {
    const conn = connection({ status: 'offline', error: 'connection refused', attempts: 0 });
    const view = await render(<ConnectionHelp conn={conn} />);
    const { getByText } = view;
    expect(getByText('Can’t reach Studio Mac')).toBeTruthy();
    expect(getByText('Connection refused.')).toBeTruthy();
    expect(getByText('studio.local:7979')).toBeTruthy();
    await fireEvent.press(getByText('Retry now'));
    await fireEvent.press(getByText('Pair again'));
    expect(conn.retry).toHaveBeenCalled();
    expect(mockPush).toHaveBeenCalledWith('/pair');
    await view.unmount();
  });

  test('falls back to an invalid relay address', async () => {
    const view = await render(
      <ConnectionHelp conn={connection({ host: { id: 'mac', name: 'Mac', relay: 'not a url', hostPublic: '', pairedAt: 0 } })} />,
    );
    const { getByText } = view;
    expect(getByText('not a url')).toBeTruthy();
    await view.unmount();
  });
});
