import { describe, expect, jest, test } from '@jest/globals';
import type { HostConnection } from './connection';
import { statusLine } from './hosts';

jest.mock('./connection', () => ({ connection: jest.fn() }));
jest.mock('./store', () => ({ getHost: jest.fn(), listHosts: jest.fn() }));

const connection = (status: HostConnection['status'], error: string | null = null, activeAddress: string | null = null) =>
  ({ status, error, activeAddress, host: { manualAddresses: ['ws://typed.example:7979'] } }) as unknown as HostConnection;

describe('statusLine', () => {
  test('describes every connection state', () => {
    expect(statusLine(connection('connected'))).toEqual({ text: 'Connected', connected: true });
    expect(statusLine(connection('offline', 'timed out'))).toEqual({ text: 'Offline · timed out', connected: false });
    expect(statusLine(connection('offline'))).toEqual({ text: 'Offline · retrying', connected: false });
    expect(statusLine(connection('connecting'))).toEqual({ text: 'Connecting…', connected: false });
    expect(statusLine(null)).toEqual({ text: 'Connecting…', connected: false });
  });

  test('says which way an open connection runs', () => {
    const over = (address: string) => statusLine(connection('connected', null, address));
    expect(over('ws://Studio.local:7979')).toEqual({ text: 'On the local network', connected: true, kind: 'local' });
    expect(over('ws://100.101.102.103:7979')).toEqual({ text: 'Over Tailscale', connected: true, kind: 'tailnet' });
    expect(over('wss://relay.ketapp.dev')).toEqual({ text: 'Through ket’s relay', connected: true, kind: 'ket-relay' });
    expect(over('wss://relay.example.test')).toEqual({ text: 'Through the relay', connected: true, kind: 'relay' });
    expect(over('ws://typed.example:7979')).toEqual({ text: 'At an address you added', connected: true, kind: 'relay' });
  });
});
