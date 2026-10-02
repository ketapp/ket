import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render, waitFor } from '@testing-library/react-native';
import { Alert, Text } from 'react-native';
import ChoosePin from '../app/pin';
import { LockGate, PinPad } from './Lock';

let mockLock = { ready: true, on: true, locked: false, confirming: null as string | null, covered: false, afterMs: 60_000 };
const mockBiometry = jest.fn(async () => 'Face ID' as string | null);
const mockUnlockBiometrics = jest.fn(async () => true);
type PinResult = { ok: true } | { ok: false; waitMs: number; left: number };
const mockUnlockPin = jest.fn(async (_pin: string): Promise<PinResult> => ({ ok: true }));
const mockForgetEverything = jest.fn(async () => {});
const mockCancelConfirm = jest.fn();
const mockSetPin = jest.fn(async (_pin: string) => {});
const mockBack = jest.fn();
const mockReplace = jest.fn();

jest.mock('expo-router', () => ({
  router: { back: () => mockBack(), replace: (path: string) => mockReplace(path) },
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 10, bottom: 20 }) }));
jest.mock('../lib/lock', () => ({
  PIN_LENGTH: 6,
  biometry: () => mockBiometry(),
  cancelConfirm: () => mockCancelConfirm(),
  forgetEverything: () => mockForgetEverything(),
  setPin: (pin: string) => mockSetPin(pin),
  unlockWithBiometrics: () => mockUnlockBiometrics(),
  unlockWithPin: (pin: string) => mockUnlockPin(pin),
  useLock: () => mockLock,
  waitMs: () => 30_000,
}));

const enter = async (view: Awaited<ReturnType<typeof render>>, digits: string) => {
  for (const digit of digits) await fireEvent.press(view.getByLabelText(digit));
  await act(async () => {
    await jest.advanceTimersByTimeAsync(90);
  });
};

beforeEach(() => {
  jest.useFakeTimers();
  jest.clearAllMocks();
  mockLock = { ready: true, on: true, locked: false, confirming: null, covered: false, afterMs: 60_000 };
  mockBiometry.mockResolvedValue('Face ID');
  mockUnlockPin.mockResolvedValue({ ok: true });
});

afterEach(() => {
  jest.useRealTimers();
  jest.restoreAllMocks();
});

describe('PinPad', () => {
  test('collects six digits, deletes, and completes', async () => {
    const complete = jest.fn();
    const view = await render(<PinPad title="PIN" detail="Six digits" extra={<Text>Face</Text>} onComplete={complete} />);
    await fireEvent.press(view.getByLabelText('1'));
    await fireEvent.press(view.getByLabelText('Delete'));
    expect(view.getByLabelText('0 of 6 digits')).toBeTruthy();
    await enter(view, '123456');
    expect(complete).toHaveBeenCalledWith('123456');
    expect(view.getByText('Face')).toBeTruthy();
    await view.unmount();
  });

  test('ignores disabled keys and draws errors', async () => {
    const complete = jest.fn();
    const view = await render(<PinPad title="PIN" error="Wrong PIN" disabled onComplete={complete} />);
    await fireEvent.press(view.getByLabelText('1'));
    expect(view.getByLabelText('0 of 6 digits')).toBeTruthy();
    expect(view.getByText('Wrong PIN')).toBeTruthy();
    expect(complete).not.toHaveBeenCalled();
    await view.unmount();
  });
});

describe('LockGate', () => {
  test('covers loading/background and disappears when unlocked', async () => {
    mockLock.ready = false;
    const loading = await render(<LockGate />);
    expect(loading.toJSON()).toBeTruthy();
    await loading.unmount();

    mockLock = { ...mockLock, ready: true, covered: true };
    const covered = await render(<LockGate />);
    expect(covered.toJSON()).toBeTruthy();
    await covered.unmount();

    mockLock.covered = false;
    const open = await render(<LockGate />);
    expect(open.toJSON()).toBeNull();
    await open.unmount();
  });

  test('unlocks with biometrics or PIN and reports lockout', async () => {
    mockLock.locked = true;
    mockUnlockPin.mockResolvedValueOnce({ ok: false, waitMs: 30_000, left: 0 });
    const view = await render(<LockGate />);
    await waitFor(() => expect(view.getByLabelText('Use Face ID')).toBeTruthy());
    await fireEvent.press(view.getByLabelText('Use Face ID'));
    expect(mockUnlockBiometrics).toHaveBeenCalled();
    await enter(view, '123456');
    await waitFor(() => expect(view.getByText('Try again in 0:30')).toBeTruthy());
    await view.unmount();
  });

  test('cancels confirmation and forgets desktops', async () => {
    mockLock.confirming = 'Merge branch';
    const alert = jest.spyOn(Alert, 'alert').mockImplementation((_title, _message, buttons) => buttons?.[1]?.onPress?.());
    const view = await render(<LockGate />);
    expect(view.getByText("Confirm it's you")).toBeTruthy();
    await fireEvent.press(view.getByText('Cancel'));
    expect(mockCancelConfirm).toHaveBeenCalled();
    await fireEvent.press(view.getByText('Forgot PIN?'));
    await waitFor(() => expect(mockForgetEverything).toHaveBeenCalled());
    expect(alert).toHaveBeenCalled();
    expect(mockReplace).toHaveBeenCalledWith('/');
    await view.unmount();
  });
});

describe('ChoosePin', () => {
  test('rejects mismatched PINs', async () => {
    mockLock.on = false;
    const view = await render(<ChoosePin />);
    expect(view.getByText('Choose a PIN')).toBeTruthy();
    await enter(view, '123456');
    expect(view.getByText('Enter it again')).toBeTruthy();
    await enter(view, '123457');
    expect(view.getByText("The PINs didn't match · start again")).toBeTruthy();
    expect(view.getByText('Choose a PIN')).toBeTruthy();
    await view.unmount();
  });

  test('saves matching PINs and supports cancellation', async () => {
    const view = await render(<ChoosePin />);
    expect(view.getByText('Choose a new PIN')).toBeTruthy();
    await enter(view, '654321');
    await enter(view, '654321');
    await waitFor(() => expect(mockSetPin).toHaveBeenCalledWith('654321'));
    expect(mockBack).toHaveBeenCalled();
    await fireEvent.press(view.getByText('Cancel'));
    expect(mockBack).toHaveBeenCalledTimes(2);
    await view.unmount();
  });
});
