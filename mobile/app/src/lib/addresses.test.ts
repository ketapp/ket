import { describe, expect, jest, test } from '@jest/globals';

import { addReachable, reachFrom, reachIn } from './addresses';
import type { SavedHost } from './store';

jest.mock('@ket/remote', () => ({
  // As the protocol package has it, near enough: ws or wss, a host, a path.
  validRelayUrl: (url: string) => /^wss?:\/\/[^/\s?#@]+(\/[^\s?#@]*)?$/.test(url),
}));

const host: SavedHost = {
  id: '-_v7-_v7-_v7-_v7-_v7-w',
  name: 'Studio Mac',
  relay: 'ws://Studio.local:7979',
  addresses: ['ws://Studio.local:7979'],
  hostPublic: '',
  pairedAt: 0,
};

describe('reach codes', () => {
  test('read what a desktop writes', () => {
    // The code `ket_core::host::reach_code` is tested to write.
    const code =
      'ket://reach?host=-_v7-_v7-_v7-_v7-_v7-w&at=ws%3A%2F%2FStudio.local%3A7979%20wss%3A%2F%2Frelay.example.test%2Fa%2Cb';
    expect(reachIn(code)).toEqual({
      hostId: '-_v7-_v7-_v7-_v7-_v7-w',
      addresses: ['ws://Studio.local:7979', 'wss://relay.example.test/a,b'],
    });
  });

  test('are only ket’s, for one desktop, with an address this phone can dial', () => {
    expect(reachIn('ket://pair/abc')).toBeNull();
    expect(reachIn('https://reach?host=-_v7-_v7-_v7-_v7-_v7-w&at=ws%3A%2F%2Fa')).toBeNull();
    expect(reachIn('not a url')).toBeNull();
    expect(reachFrom('short', 'ws://a')).toBeNull();
    expect(reachFrom(host.id, undefined)).toBeNull();
    expect(reachFrom(host.id, 'http://a ftp://b')).toBeNull();
    expect(reachFrom(host.id, 'ws://a http://b  ws://a')).toEqual({
      hostId: host.id,
      addresses: ['ws://a'],
    });
  });

  test('add to what the desktop listed, without repeating it', () => {
    const reached = addReachable(host, ['ws://Studio.local:7979', 'ws://100.101.102.103:7979', 'nope']);
    expect(reached.addresses).toEqual(['ws://Studio.local:7979', 'ws://100.101.102.103:7979']);
    expect(reached.manualAddresses).toBeUndefined();
    // A pairing from before 1.17 kept only its one address.
    const old = { ...host, addresses: undefined };
    expect(addReachable(old, ['wss://relay.ketapp.dev']).addresses).toEqual([
      'ws://Studio.local:7979',
      'wss://relay.ketapp.dev',
    ]);
  });
});
