import { describe, expect, test } from '@jest/globals';

import { addressKind, reachesAway } from './address-kind';
import type { SavedHost } from './store';

const host = (fields: Partial<SavedHost>): SavedHost => ({
  id: 'mac',
  name: 'Studio Mac',
  relay: 'ws://Studio.local:7979',
  hostPublic: '',
  pairedAt: 0,
  ...fields,
});

describe('address kinds', () => {
  test('tell the Mac’s network, its tailnet and the relays apart', () => {
    expect(addressKind('ws://Studio.local:7979')).toBe('local');
    expect(addressKind('ws://192.168.1.20:7979')).toBe('local');
    expect(addressKind('ws://studio:7979')).toBe('local');
    expect(addressKind('ws://localhost:7979')).toBe('local');
    expect(addressKind('ws://100.101.102.103:7979')).toBe('tailnet');
    expect(addressKind('ws://studio.tail1234.ts.net:7979')).toBe('tailnet');
    expect(addressKind('wss://relay.ketapp.dev')).toBe('ket-relay');
    expect(addressKind('wss://relay.example.test')).toBe('relay');
  });

  test('say whether the phone can reach the Mac from off its network', () => {
    expect(reachesAway(host({}))).toBe(false);
    expect(reachesAway(host({ addresses: ['ws://Studio.local:7979', 'ws://192.168.1.20:7979'] }))).toBe(false);
    expect(reachesAway(host({ addresses: ['ws://Studio.local:7979', 'ws://100.101.102.103:7979'] }))).toBe(true);
    expect(reachesAway(host({ addresses: ['wss://relay.ketapp.dev'] }))).toBe(true);
    expect(reachesAway(host({ manualAddresses: ['ws://vpn.example.test:7979'] }))).toBe(true);
  });
});
