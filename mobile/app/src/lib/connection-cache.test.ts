import { afterEach, describe, expect, jest, test } from '@jest/globals';
import { waitFor } from '@testing-library/react-native';
import { connection, dropConnection } from './connection';
import type { SavedHost } from './store';

// A dial that never answers: these tests are about which pairing a
// connection dials with, not what happens after.
const mockResume = jest.fn((_host: { relay: string }, _keys: unknown, _options: unknown) => new Promise(() => {}));

// Only `resume` is called here; the protocol's schemas and enums the module
// names stand in as empty objects.
jest.mock(
  '@ket/remote',
  () =>
    new Proxy(
      { resume: (host: { relay: string }, keys: unknown, options: unknown) => mockResume(host, keys, options) },
      { get: (target, name) => (name in target ? target[name as 'resume'] : {}) },
    ),
);
jest.mock('./store', () => ({
  phoneKey: jest.fn(async () => ({})),
  saveHost: jest.fn(async () => {}),
  toPaired: (host: { relay: string }) => ({ relay: host.relay }),
}));
jest.mock('./version', () => ({ versionLabel: '1.0 (1)' }));

const saved = (relay: string): SavedHost => ({ id: 'host', name: 'Studio Mac', relay, hostPublic: 'AAAA', pairedAt: 0 });

describe('connection', () => {
  afterEach(() => dropConnection('host'));

  test('a desktop keeps one connection, whatever record it is asked with', () => {
    const first = connection(saved('ws://old.local:7979'));
    expect(connection(saved('ws://new.local:7979'))).toBe(first);
    expect(first.host.relay).toBe('ws://old.local:7979');
  });

  test('after it is dropped, the next connection dials the new pairing', async () => {
    const old = connection(saved('ws://old.local:7979'));
    dropConnection('host');
    const fresh = connection(saved('ws://new.local:7979'));
    expect(fresh).not.toBe(old);
    await waitFor(() => expect(mockResume).toHaveBeenLastCalledWith(expect.objectContaining({ relay: 'ws://new.local:7979' }), expect.anything(), expect.anything()));
  });
});
