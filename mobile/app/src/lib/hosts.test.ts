import { describe, expect, jest, test } from '@jest/globals';
import type { HostConnection } from './connection';
import { statusLine } from './hosts';

jest.mock('./connection', () => ({ connection: jest.fn() }));
jest.mock('./store', () => ({ getHost: jest.fn(), listHosts: jest.fn() }));

const connection = (status: HostConnection['status'], error: string | null = null) =>
  ({ status, error }) as HostConnection;

describe('statusLine', () => {
  test('describes every connection state', () => {
    expect(statusLine(connection('connected'))).toEqual({ text: 'Connected through the relay', connected: true });
    expect(statusLine(connection('offline', 'timed out'))).toEqual({ text: 'Offline · timed out', connected: false });
    expect(statusLine(connection('offline'))).toEqual({ text: 'Offline · retrying', connected: false });
    expect(statusLine(connection('connecting'))).toEqual({ text: 'Connecting…', connected: false });
    expect(statusLine(null)).toEqual({ text: 'Connecting…', connected: false });
  });
});
