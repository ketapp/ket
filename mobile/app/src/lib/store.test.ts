import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import {
  forgetHost,
  getCollapsed,
  getDesktop,
  getFit,
  getHost,
  getLock,
  getRecent,
  listHosts,
  phoneKey,
  saveCollapsed,
  saveDesktop,
  saveFit,
  saveHost,
  saveLock,
  saveRecent,
  toPaired,
  type LockConfig,
  type SavedHost,
} from './store';

const mockStorage = new Map<string, string>();
const mockPrivate = Uint8Array.of(1, 2, 3);

jest.mock('expo-secure-store', () => ({
  getItemAsync: jest.fn(async (key: string) => mockStorage.get(key) ?? null),
  setItemAsync: jest.fn(async (key: string, value: string) => void mockStorage.set(key, value)),
  deleteItemAsync: jest.fn(async (key: string) => void mockStorage.delete(key)),
}));

jest.mock('@ket/remote', () => ({
  fromBase64Url: (text: string) => Uint8Array.from(text.split('-').map(Number)),
  generateKeypair: () => ({ privateKey: mockPrivate, publicKey: Uint8Array.of(4, 5, 6) }),
  keypairFrom: (privateKey: Uint8Array) => ({ privateKey, publicKey: Uint8Array.of(9) }),
  toBase64Url: (bytes: Uint8Array) => [...bytes].join('-'),
  utf8: (bytes: Uint8Array) => String.fromCharCode(...bytes),
}));

const host = (id: string): SavedHost => ({
  id,
  name: `Mac ${id}`,
  relay: `ws://${id}`,
  hostPublic: '7-8',
  pairedAt: 123,
});

beforeEach(() => mockStorage.clear());

describe('secure mobile storage', () => {
  test('creates the phone key once, then restores it', async () => {
    expect(await phoneKey()).toEqual({ privateKey: mockPrivate, publicKey: Uint8Array.of(4, 5, 6) });
    expect(mockStorage.get('ket.phone.key')).toBe('1-2-3');
    expect(await phoneKey()).toEqual({ privateKey: mockPrivate, publicKey: Uint8Array.of(9) });
  });

  test('saves, lists, updates, and forgets hosts without duplicate ids', async () => {
    const one = host('one');
    const two = host('two');
    await saveHost(one);
    await saveHost({ ...one, name: 'Renamed' });
    await saveHost(two);
    mockStorage.delete('ket.host.two');

    expect(await getHost('one')).toMatchObject({ name: 'Renamed' });
    expect(await getHost('missing')).toBeNull();
    expect(await listHosts()).toEqual([{ ...one, name: 'Renamed' }]);

    await saveCollapsed('one', ['project']);
    await forgetHost('one');
    expect(await listHosts()).toEqual([]);
    expect(await getCollapsed('one')).toEqual([]);
  });

  test('round-trips preferences and lock state', async () => {
    expect(await getRecent()).toBeNull();
    await saveRecent({ host: 'h', terminal: '9' });
    expect(await getRecent()).toEqual({ host: 'h', terminal: '9' });

    expect(await getDesktop()).toBeNull();
    await saveDesktop('h');
    expect(await getDesktop()).toBe('h');

    expect(await getCollapsed('h')).toEqual([]);
    await saveCollapsed('h', ['a', 'b']);
    expect(await getCollapsed('h')).toEqual(['a', 'b']);

    expect(await getFit()).toBe(false);
    await saveFit(true);
    expect(await getFit()).toBe(true);
    await saveFit(false);
    expect(await getFit()).toBe(false);

    const lock: LockConfig = { salt: 's', hash: 'h', afterMs: 60, failures: 2, waitUntil: 3 };
    expect(await getLock()).toBeNull();
    await saveLock(lock);
    expect(await getLock()).toEqual(lock);
    await saveLock(null);
    expect(await getLock()).toBeNull();
  });

  test('converts a saved host to protocol bytes', () => {
    expect(toPaired(host('1-2'))).toEqual({
      hostId: Uint8Array.of(1, 2),
      hostPublic: Uint8Array.of(7, 8),
      relay: 'ws://1-2',
    });
  });
});
