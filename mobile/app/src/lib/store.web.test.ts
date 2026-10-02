import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import * as store from './store.web';

const mockPrivate = Uint8Array.of(1, 2, 3);

jest.mock('@ket/remote', () => ({
  fromBase64Url: (text: string) => Uint8Array.from(text.split('-').map(Number)),
  generateKeypair: () => ({ privateKey: mockPrivate, publicKey: Uint8Array.of(4) }),
  keypairFrom: (privateKey: Uint8Array) => ({ privateKey, publicKey: Uint8Array.of(9) }),
  toBase64Url: (bytes: Uint8Array) => [...bytes].join('-'),
  utf8: (bytes: Uint8Array) => String.fromCharCode(...bytes),
}));

const values = new Map<string, string>();

beforeEach(() => {
  values.clear();
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => void values.set(key, value),
      removeItem: (key: string) => void values.delete(key),
    },
  });
});

describe('web storage', () => {
  test('round-trips keys, hosts, preferences, and lock state', async () => {
    expect(await store.phoneKey()).toEqual({ privateKey: mockPrivate, publicKey: Uint8Array.of(4) });
    expect(await store.phoneKey()).toEqual({ privateKey: mockPrivate, publicKey: Uint8Array.of(9) });

    const host: store.SavedHost = { id: '1-2', name: 'Mac', relay: 'ws://mac', hostPublic: '3-4', pairedAt: 1 };
    await store.saveHost(host);
    await store.saveHost({ ...host, name: 'Renamed' });
    expect(await store.getHost(host.id)).toMatchObject({ name: 'Renamed' });
    expect(await store.getHost('missing')).toBeNull();
    expect(await store.listHosts()).toHaveLength(1);

    await store.saveRecent({ host: host.id, terminal: '2' });
    expect(await store.getRecent()).toEqual({ host: host.id, terminal: '2' });
    await store.saveDesktop(host.id);
    expect(await store.getDesktop()).toBe(host.id);
    await store.saveCollapsed(host.id, ['project']);
    expect(await store.getCollapsed(host.id)).toEqual(['project']);
    await store.saveFit(true);
    expect(await store.getFit()).toBe(true);

    const lock: store.LockConfig = { salt: 's', hash: 'h', afterMs: 1, failures: 0, waitUntil: 0 };
    await store.saveLock(lock);
    expect(await store.getLock()).toEqual(lock);
    await store.saveLock(null);
    expect(await store.getLock()).toBeNull();

    expect(store.toPaired(host)).toEqual({ hostId: Uint8Array.of(1, 2), hostPublic: Uint8Array.of(3, 4), relay: host.relay });
    await store.forgetHost(host.id);
    expect(await store.listHosts()).toEqual([]);
    expect(await store.getCollapsed(host.id)).toEqual([]);
  });

  test('returns empty defaults', async () => {
    expect(await store.listHosts()).toEqual([]);
    expect(await store.getRecent()).toBeNull();
    expect(await store.getDesktop()).toBeNull();
    expect(await store.getCollapsed('missing')).toEqual([]);
    expect(await store.getFit()).toBe(false);
    expect(await store.getLock()).toBeNull();
  });
});
