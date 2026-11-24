import { describe, expect, jest, test } from '@jest/globals';
import { act, renderHook, waitFor } from '@testing-library/react-native';
import { connection as connect, type ConnectionState, heldConnections, type HostConnection, liveConnection } from './connection';
import { statusLine, useHeldHosts, useHost, useHostState, useHostStates } from './hosts';
import { getHost, listHosts } from './store';

jest.mock('./connection', () => ({ connection: jest.fn(), heldConnections: jest.fn(), liveConnection: jest.fn() }));
jest.mock('./store', () => ({ getHost: jest.fn(), listHosts: jest.fn() }));
// Focused once, as it mounts.
jest.mock('expo-router', () => ({
  useFocusEffect: (effect: () => void) => jest.requireActual<typeof import('react')>('react').useEffect(effect, [effect]),
}));

const state = (status: ConnectionState['status'], error: string | null = null, activeAddress: string | null = null) =>
  ({ status, error, activeAddress, host: { manualAddresses: ['ws://typed.example:7979'] } }) as unknown as ConnectionState;

/** A connection as the hooks see it: an id, a state that a change replaces,
 * and listeners told of the change. */
const live = (id = 'studio') => {
  const changed = new Set<() => void>();
  const conn = {
    id,
    state: state('connecting'),
    onChange: jest.fn((callback: () => void) => {
      changed.add(callback);
      return () => changed.delete(callback);
    }),
  };
  const change = (next: ConnectionState) => {
    conn.state = next;
    changed.forEach((callback) => callback());
  };
  return { conn: conn as unknown as HostConnection, change, listening: () => changed.size };
};

describe('statusLine', () => {
  test('describes every connection state', () => {
    expect(statusLine(state('connected'))).toEqual({ text: 'Connected', connected: true });
    expect(statusLine(state('offline', 'timed out'))).toEqual({ text: 'Offline · timed out', connected: false });
    expect(statusLine(state('offline'))).toEqual({ text: 'Offline · retrying', connected: false });
    expect(statusLine(state('connecting'))).toEqual({ text: 'Connecting…', connected: false });
    expect(statusLine(null)).toEqual({ text: 'Connecting…', connected: false });
  });

  test('says which way an open connection runs', () => {
    const over = (address: string) => statusLine(state('connected', null, address));
    expect(over('ws://Studio.local:7979')).toEqual({ text: 'On the local network', connected: true, kind: 'local' });
    expect(over('ws://100.101.102.103:7979')).toEqual({ text: 'Over Tailscale', connected: true, kind: 'tailnet' });
    expect(over('wss://relay.ketapp.dev')).toEqual({ text: 'Through ket’s relay', connected: true, kind: 'ket-relay' });
    expect(over('wss://relay.example.test')).toEqual({ text: 'Through the relay', connected: true, kind: 'relay' });
    expect(over('ws://typed.example:7979')).toEqual({ text: 'At an address you added', connected: true, kind: 'relay' });
  });
});

describe('useHost', () => {
  test('has a connection the app already holds on its first draw, without the Keychain', async () => {
    const { conn: held } = live();
    jest.mocked(liveConnection).mockReturnValue(held);
    const drawn: (HostConnection | null)[] = [];
    const { unmount } = await renderHook(() => {
      const conn = useHost('studio');
      drawn.push(conn);
      return conn;
    });
    expect(drawn[0]).toBe(held);
    expect(getHost).not.toHaveBeenCalled();
    await unmount();
  });

  test('reads a desktop not met yet from the Keychain, and connects to it', async () => {
    const { conn: made } = live();
    jest.mocked(liveConnection).mockReturnValue(undefined);
    jest.mocked(getHost).mockResolvedValue({ id: 'studio' } as never);
    jest.mocked(connect).mockReturnValue(made);
    const { result, unmount } = await renderHook(() => useHost('studio'));
    expect(getHost).toHaveBeenCalledWith('studio');
    await waitFor(() => expect(result.current).toBe(made));
    await unmount();
  });

  test('finds a connection another screen opened between drawing and the effect', async () => {
    const { conn: made } = live();
    // None as it draws; another screen's by the time its effect runs.
    jest.mocked(liveConnection).mockReturnValueOnce(undefined).mockReturnValue(made);
    jest.mocked(getHost).mockResolvedValue({ id: 'studio' } as never);
    jest.mocked(connect).mockReturnValue(made);
    const { result, unmount } = await renderHook(() => useHost('studio'));
    await waitFor(() => expect(result.current).toBe(made));
    await unmount();
  });
});

describe('useHeldHosts', () => {
  test('has the connections the app already holds on its first draw, then the list as read', async () => {
    const held = live('studio').conn;
    const read = live('studio').conn;
    jest.mocked(heldConnections).mockReturnValue([held]);
    jest.mocked(listHosts).mockResolvedValue([{ id: 'studio' }] as never);
    jest.mocked(connect).mockReturnValue(read);
    const drawn: (HostConnection[] | null)[] = [];
    const { result, unmount } = await renderHook(() => {
      const conns = useHeldHosts();
      drawn.push(conns);
      return conns;
    });
    expect(drawn[0]).toEqual([held]);
    await waitFor(() => expect(result.current).toEqual([read]));
    await unmount();
  });

  test('has nothing, rather than no desktops, while none is held and the list is unread', async () => {
    jest.mocked(heldConnections).mockReturnValue([]);
    jest.mocked(listHosts).mockResolvedValue([]);
    const drawn: (HostConnection[] | null)[] = [];
    const { result, unmount } = await renderHook(() => {
      const conns = useHeldHosts();
      drawn.push(conns);
      return conns;
    });
    expect(drawn[0]).toBeNull();
    await waitFor(() => expect(result.current).toEqual([]));
    await unmount();
  });
});

describe('useHostState', () => {
  test('draws again with the new state when the connection changes, and keeps the old one until then', async () => {
    const { conn, change, listening } = live();
    const first = conn.state;
    const { result, rerender, unmount } = await renderHook(() => useHostState(conn));
    expect(result.current).toBe(first);
    expect(listening()).toBe(1);
    // Drawn again with nothing changed: the very same state.
    await rerender({});
    expect(result.current).toBe(first);
    const next = state('connected');
    await act(async () => change(next));
    expect(result.current).toBe(next);
    await unmount();
    expect(listening()).toBe(0);
  });

  test('has no state without a connection', async () => {
    const { result, unmount } = await renderHook(() => useHostState(null));
    expect(result.current).toBeNull();
    await unmount();
  });
});

describe('useHostStates', () => {
  test('hands out the same list until one of them changes', async () => {
    const a = live('a');
    const b = live('b');
    const conns = [a.conn, b.conn];
    const { result, rerender, unmount } = await renderHook(() => useHostStates(conns));
    const listed = result.current;
    expect(listed).toEqual([a.conn.state, b.conn.state]);
    await rerender({});
    expect(result.current).toBe(listed);
    const next = state('offline');
    await act(async () => b.change(next));
    expect(result.current).not.toBe(listed);
    expect(result.current).toEqual([a.conn.state, next]);
    await unmount();
  });
});
