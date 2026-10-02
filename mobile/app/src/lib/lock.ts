// The app lock: with it on, ket asks for Face ID (Touch ID, a fingerprint,
// face unlock — whatever the phone has) or the app's own PIN before showing
// anything, on launch and after a while in the background. Merge and
// "Always allow" ask again at the moment they are pressed.
//
// The PIN is always set when the lock is turned on: it is the way in when
// biometrics fail, are not enrolled, or the phone has none. Only a salted
// hash of it is kept, in the Keychain. Wrong PINs in a row earn a growing
// wait; a forgotten PIN is recovered by forgetting every paired desktop.
//
// This guards the screen, not the keys: the pairing keys stay readable to
// the app whenever the phone itself is unlocked, so it can reconnect in the
// background.

import { CryptoDigestAlgorithm, digestStringAsync, getRandomBytes } from 'expo-crypto';
import * as LocalAuthentication from 'expo-local-authentication';
import { useEffect, useState } from 'react';
import { AppState, Platform } from 'react-native';

import { toBase64Url } from '@ket/remote';

import { dropConnection } from './connection';
import { type LockConfig, forgetHost, getLock, listHosts, saveLock } from './store';

/** Digits in a PIN. */
export const PIN_LENGTH = 6;

/** Wrong PINs allowed before the first wait. */
const FREE_TRIES = 5;

/** The first wait, doubled for each wrong PIN after it, up to the cap. */
const FIRST_WAIT_MS = 30_000;
const LONGEST_WAIT_MS = 15 * 60_000;

/** How long the app may be away before it locks again: the choices. */
export const LOCK_AFTER = [
  { key: '0', label: 'Immediately', ms: 0 },
  { key: '60000', label: '1 min', ms: 60_000 },
  { key: '300000', label: '5 min', ms: 300_000 },
] as const;

/** What the rest of the app sees. */
export interface LockState {
  /** Read from storage yet. Nothing is shown until it has been. */
  ready: boolean;
  /** The lock is turned on. */
  on: boolean;
  /** The lock screen is up. */
  locked: boolean;
  /** Something asked to confirm it is you, and is waiting on the PIN. */
  confirming: string | null;
  /** The app is not in front: its contents are covered. */
  covered: boolean;
  afterMs: number;
}

let config: LockConfig | null = null;
let state: LockState = { ready: false, on: false, locked: false, confirming: null, covered: false, afterMs: 0 };
let confirmed: ((yes: boolean) => void) | null = null;
let awayAt: number | null = null;
const listeners = new Set<() => void>();

function set(next: Partial<LockState>) {
  state = { ...state, ...next };
  for (const listener of listeners) listener();
}

/** Reads the lock and starts watching the app come and go. Once, at launch. */
let started = false;
export function startLock(): void {
  if (started) return;
  started = true;
  void getLock().then((stored) => {
    config = stored;
    set({ ready: true, on: !!stored, locked: !!stored, afterMs: stored?.afterMs ?? 0 });
  });
  AppState.addEventListener('change', (next) => {
    // Covered as soon as the app is not in front, so the app switcher's
    // snapshot shows nothing. A Face ID sheet makes the app inactive too,
    // which is only a cover over the lock screen.
    set({ covered: next !== 'active' });
    if (next === 'background') awayAt = Date.now();
    if (next === 'active' && awayAt !== null) {
      const away = Date.now() - awayAt;
      awayAt = null;
      if (config && away >= config.afterMs) set({ locked: true });
    }
  });
}

export function useLock(): LockState {
  const [, redraw] = useState(0);
  useEffect(() => {
    const listener = () => redraw((n) => n + 1);
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);
  return state;
}

/** What this phone unlocks with besides the PIN, by its own name — or
 * `null` when it has nothing enrolled. */
export async function biometry(): Promise<string | null> {
  if (Platform.OS === 'web') return null;
  const [hardware, enrolled] = await Promise.all([
    LocalAuthentication.hasHardwareAsync(),
    LocalAuthentication.isEnrolledAsync(),
  ]);
  if (!hardware || !enrolled) return null;
  const kinds = await LocalAuthentication.supportedAuthenticationTypesAsync();
  const face = kinds.includes(LocalAuthentication.AuthenticationType.FACIAL_RECOGNITION);
  const finger = kinds.includes(LocalAuthentication.AuthenticationType.FINGERPRINT);
  if (Platform.OS === 'ios') return face ? 'Face ID' : finger ? 'Touch ID' : null;
  return finger ? 'Fingerprint' : face ? 'Face unlock' : 'Biometrics';
}

/** Asks for biometrics. `true` once they pass; `false` when they fail, are
 * cancelled, or the person chose the PIN instead. */
async function biometrics(prompt: string): Promise<boolean> {
  if (!(await biometry())) return false;
  const result = await LocalAuthentication.authenticateAsync({
    promptMessage: prompt,
    cancelLabel: 'Use PIN',
    fallbackLabel: 'Use PIN',
    // The way around biometrics is ket's PIN, not the phone's passcode.
    disableDeviceFallback: true,
  }).catch(() => ({ success: false as const }));
  return result.success;
}

/** Unlocks with biometrics, if the phone has them. */
export async function unlockWithBiometrics(): Promise<boolean> {
  const ok = await biometrics(state.confirming ?? 'Unlock ket');
  if (ok) pass();
  return ok;
}

/** The outcome of a PIN: in, wrong, or not yet — wait this long. */
export type PinResult = { ok: true } | { ok: false; waitMs: number; left: number };

/** How long before another PIN may be tried; 0 when one may be now. */
export function waitMs(): number {
  return config ? Math.max(0, config.waitUntil - Date.now()) : 0;
}

export async function unlockWithPin(pin: string): Promise<PinResult> {
  if (!config) {
    pass();
    return { ok: true };
  }
  const wait = waitMs();
  if (wait > 0) return { ok: false, waitMs: wait, left: 0 };
  if ((await hash(config.salt, pin)) === config.hash) {
    config = { ...config, failures: 0, waitUntil: 0 };
    await saveLock(config);
    pass();
    return { ok: true };
  }
  const failures = config.failures + 1;
  const over = failures - FREE_TRIES;
  const next = over >= 0 ? Math.min(FIRST_WAIT_MS * 2 ** over, LONGEST_WAIT_MS) : 0;
  config = { ...config, failures, waitUntil: next ? Date.now() + next : 0 };
  await saveLock(config);
  return { ok: false, waitMs: next, left: Math.max(0, FREE_TRIES - failures) };
}

/** In: the lock screen goes, or the confirmation waiting on it is given. */
function pass() {
  if (confirmed) {
    confirmed(true);
    confirmed = null;
    set({ confirming: null });
  } else {
    set({ locked: false });
  }
}

/** Gives up on a confirmation. */
export function cancelConfirm(): void {
  confirmed?.(false);
  confirmed = null;
  set({ confirming: null });
}

/**
 * That it is you, before something that cannot be taken back: `reason` says
 * what — "Merge fix-login into main". `true` straight away with the lock off.
 * Biometrics first; the PIN when they fail or there are none.
 */
export async function confirmIdentity(reason: string): Promise<boolean> {
  if (!config) return true;
  if (await biometrics(reason)) return true;
  confirmed?.(false);
  return new Promise((resolve) => {
    confirmed = resolve;
    set({ confirming: reason });
  });
}

/** Turns the lock on with `pin`, or changes the PIN when it is on. */
export async function setPin(pin: string): Promise<void> {
  const salt = toBase64Url(getRandomBytes(16));
  config = {
    salt,
    hash: await hash(salt, pin),
    afterMs: config?.afterMs ?? LOCK_AFTER[1].ms,
    failures: 0,
    waitUntil: 0,
  };
  await saveLock(config);
  set({ on: true, afterMs: config.afterMs });
}

export async function setLockAfter(ms: number): Promise<void> {
  if (!config) return;
  config = { ...config, afterMs: ms };
  await saveLock(config);
  set({ afterMs: ms });
}

/** Turns the lock off. The caller confirms it is you first. */
export async function turnOff(): Promise<void> {
  config = null;
  await saveLock(null);
  set({ on: false, locked: false });
}

/** A forgotten PIN: every paired desktop is forgotten with it, since the PIN
 * is what stood between them and whoever holds the phone. */
export async function forgetEverything(): Promise<void> {
  for (const host of await listHosts()) {
    dropConnection(host.id);
    await forgetHost(host.id);
  }
  await turnOff();
  cancelConfirm();
}

async function hash(salt: string, pin: string): Promise<string> {
  return digestStringAsync(CryptoDigestAlgorithm.SHA256, `ket-pin:${salt}:${pin}`);
}
