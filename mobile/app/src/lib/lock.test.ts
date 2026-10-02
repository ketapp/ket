import { afterAll, beforeAll, describe, expect, jest, test } from '@jest/globals';
import { AppState, Platform } from 'react-native';
import {
  biometry,
  cancelConfirm,
  confirmIdentity,
  forgetEverything,
  setLockAfter,
  setPin,
  startLock,
  turnOff,
  unlockWithBiometrics,
  unlockWithPin,
  waitMs,
} from './lock';
import type { LockConfig, SavedHost } from './store';

let mockAppStateListener: (state: string) => void = () => {};
let mockStoredLock: LockConfig | null = null;
const mockSaveLock = jest.fn(async (value: LockConfig | null) => {
  mockStoredLock = value;
});
const mockForgetHost = jest.fn(async (_id: string) => {});
const mockDropConnection = jest.fn();
const mockLocalAuth = {
  hasHardwareAsync: jest.fn(async () => true),
  isEnrolledAsync: jest.fn(async () => true),
  supportedAuthenticationTypesAsync: jest.fn(async () => [2]),
  authenticateAsync: jest.fn(async () => ({ success: false })),
};

jest.mock('expo-local-authentication', () => ({
  AuthenticationType: { FINGERPRINT: 1, FACIAL_RECOGNITION: 2 },
  hasHardwareAsync: () => mockLocalAuth.hasHardwareAsync(),
  isEnrolledAsync: () => mockLocalAuth.isEnrolledAsync(),
  supportedAuthenticationTypesAsync: () => mockLocalAuth.supportedAuthenticationTypesAsync(),
  authenticateAsync: (_options: object) => mockLocalAuth.authenticateAsync(),
}));
jest.mock('expo-crypto', () => ({
  CryptoDigestAlgorithm: { SHA256: 'SHA256' },
  digestStringAsync: jest.fn(async (_algorithm: string, text: string) => `hash:${text}`),
  getRandomBytes: jest.fn(() => Uint8Array.of(1, 2, 3)),
}));
jest.mock('@ket/remote', () => ({ toBase64Url: () => 'salt' }));
jest.mock('./connection', () => ({ dropConnection: (id: string) => mockDropConnection(id) }));
jest.mock('./store', () => ({
  forgetHost: (id: string) => mockForgetHost(id),
  getLock: jest.fn(async () => mockStoredLock),
  listHosts: jest.fn(async () => [{ id: 'one' }, { id: 'two' }] as SavedHost[]),
  saveLock: (value: LockConfig | null) => mockSaveLock(value),
}));

const flush = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};
const setPlatform = (os: string) => Object.defineProperty(Platform, 'OS', { configurable: true, value: os });

describe('app lock', () => {
  beforeAll(() => {
    jest.spyOn(Date, 'now').mockReturnValue(1000);
    jest.spyOn(AppState, 'addEventListener').mockImplementation((_type, listener) => {
      mockAppStateListener = listener as (state: string) => void;
      return { remove: jest.fn() };
    });
  });

  afterAll(() => {
    jest.restoreAllMocks();
  });

  test('names available biometrics by platform', async () => {
    setPlatform('web');
    await expect(biometry()).resolves.toBeNull();

    setPlatform('ios');
    mockLocalAuth.hasHardwareAsync.mockResolvedValueOnce(false);
    await expect(biometry()).resolves.toBeNull();
    mockLocalAuth.isEnrolledAsync.mockResolvedValueOnce(false);
    await expect(biometry()).resolves.toBeNull();

    mockLocalAuth.supportedAuthenticationTypesAsync.mockResolvedValueOnce([2]);
    await expect(biometry()).resolves.toBe('Face ID');
    mockLocalAuth.supportedAuthenticationTypesAsync.mockResolvedValueOnce([1]);
    await expect(biometry()).resolves.toBe('Touch ID');

    setPlatform('android');
    mockLocalAuth.supportedAuthenticationTypesAsync.mockResolvedValueOnce([1]);
    await expect(biometry()).resolves.toBe('Fingerprint');
    mockLocalAuth.supportedAuthenticationTypesAsync.mockResolvedValueOnce([2]);
    await expect(biometry()).resolves.toBe('Face unlock');
    mockLocalAuth.supportedAuthenticationTypesAsync.mockResolvedValueOnce([]);
    await expect(biometry()).resolves.toBe('Biometrics');
  });

  test('sets a PIN, confirms identity, and enforces lockout', async () => {
    setPlatform('ios');
    startLock();
    await flush();
    await expect(confirmIdentity('No lock yet')).resolves.toBe(true);

    await setPin('123456');
    expect(mockStoredLock).toMatchObject({ salt: 'salt', afterMs: 60_000, failures: 0, waitUntil: 0 });
    expect(waitMs()).toBe(0);

    mockLocalAuth.authenticateAsync.mockResolvedValue({ success: false });
    const confirmed = confirmIdentity('Merge branch');
    await flush();
    await expect(unlockWithPin('123456')).resolves.toEqual({ ok: true });
    await expect(confirmed).resolves.toBe(true);

    for (let attempt = 1; attempt < 5; attempt++) {
      await expect(unlockWithPin('000000')).resolves.toEqual({ ok: false, waitMs: 0, left: 5 - attempt });
    }
    await expect(unlockWithPin('000000')).resolves.toEqual({ ok: false, waitMs: 30_000, left: 0 });
    expect(waitMs()).toBe(30_000);
    await expect(unlockWithPin('123456')).resolves.toEqual({ ok: false, waitMs: 30_000, left: 0 });

    jest.spyOn(Date, 'now').mockReturnValue(31_001);
    await expect(unlockWithPin('123456')).resolves.toEqual({ ok: true });
  });

  test('supports biometric success, cancellation, timing, and reset', async () => {
    mockLocalAuth.authenticateAsync.mockResolvedValueOnce({ success: true });
    await expect(unlockWithBiometrics()).resolves.toBe(true);

    mockLocalAuth.authenticateAsync.mockResolvedValueOnce({ success: false });
    const cancelled = confirmIdentity('Always allow');
    await flush();
    cancelConfirm();
    await expect(cancelled).resolves.toBe(false);

    await setLockAfter(300_000);
    expect(mockStoredLock?.afterMs).toBe(300_000);
    mockAppStateListener('background');
    jest.spyOn(Date, 'now').mockReturnValue(400_002);
    mockAppStateListener('active');
    mockAppStateListener('inactive');

    await turnOff();
    expect(mockStoredLock).toBeNull();
    await expect(unlockWithPin('anything')).resolves.toEqual({ ok: true });
  });

  test('forgotten PIN removes every paired desktop and lock', async () => {
    await setPin('654321');
    await forgetEverything();
    expect(mockDropConnection.mock.calls).toEqual([['one'], ['two']]);
    expect(mockForgetHost.mock.calls).toEqual([['one'], ['two']]);
    expect(mockStoredLock).toBeNull();
  });
});
