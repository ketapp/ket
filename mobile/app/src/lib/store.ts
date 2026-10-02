// What the phone keeps, in the iOS Keychain / Android Keystore through
// SecureStore: its own key pair, and the hosts it has paired with. Each host
// is its own entry, because SecureStore is only promised to hold a couple of
// kilobytes per value.

import './polyfills';

import * as SecureStore from 'expo-secure-store';
import { type Keypair, type PairedHost, fromBase64Url, generateKeypair, keypairFrom, toBase64Url } from '@ket/remote';

const KEY = 'ket.phone.key';
const HOSTS = 'ket.hosts';
const hostEntry = (id: string) => `ket.host.${id}`;
const DESKTOP = 'ket.desktop';
const FIT = 'ket.fit';
const LOCK = 'ket.lock';
const collapsedEntry = (host: string) => `ket.collapsed.${host}`;
const lastWorkEntry = (host: string) => `ket.lastwork.${host}`;

/** A paired host as the app lists it. */
export interface SavedHost {
  /** The host id, base64url: stable, and what routes use. */
  id: string;
  name: string;
  relay: string;
  hostPublic: string;
  pairedAt: number;
  /** Where the last session left off, for resuming. */
  cursor?: { epoch: string; seq: string };
}

/** This phone's key pair, made the first time it is asked for. */
export async function phoneKey(): Promise<Keypair> {
  const stored = await SecureStore.getItemAsync(KEY);
  if (stored) return keypairFrom(fromBase64Url(stored));
  const keys = generateKeypair();
  await SecureStore.setItemAsync(KEY, toBase64Url(keys.privateKey));
  return keys;
}

export async function listHosts(): Promise<SavedHost[]> {
  const ids: string[] = JSON.parse((await SecureStore.getItemAsync(HOSTS)) ?? '[]');
  const hosts = await Promise.all(ids.map((id) => getHost(id)));
  return hosts.filter((h): h is SavedHost => h !== null);
}

export async function getHost(id: string): Promise<SavedHost | null> {
  const stored = await SecureStore.getItemAsync(hostEntry(id));
  return stored ? (JSON.parse(stored) as SavedHost) : null;
}

export async function saveHost(host: SavedHost): Promise<void> {
  await SecureStore.setItemAsync(hostEntry(host.id), JSON.stringify(host));
  const ids: string[] = JSON.parse((await SecureStore.getItemAsync(HOSTS)) ?? '[]');
  if (!ids.includes(host.id)) {
    ids.push(host.id);
    await SecureStore.setItemAsync(HOSTS, JSON.stringify(ids));
  }
}

export async function forgetHost(id: string): Promise<void> {
  await SecureStore.deleteItemAsync(hostEntry(id));
  await SecureStore.deleteItemAsync(collapsedEntry(id));
  await SecureStore.deleteItemAsync(lastWorkEntry(id));
  const ids: string[] = JSON.parse((await SecureStore.getItemAsync(HOSTS)) ?? '[]');
  await SecureStore.setItemAsync(HOSTS, JSON.stringify(ids.filter((i) => i !== id)));
}

/** The desktop the Worktrees screen shows, when more than one is paired. */
export async function saveDesktop(id: string): Promise<void> {
  await SecureStore.setItemAsync(DESKTOP, id);
}

export async function getDesktop(): Promise<string | null> {
  return (await SecureStore.getItemAsync(DESKTOP)) ?? null;
}

/** The projects folded on the Worktrees screen, per desktop. The phone's own:
 * the desktop's sidebar folds what fits a window, not what fits a phone. */
export async function saveCollapsed(host: string, projects: string[]): Promise<void> {
  await SecureStore.setItemAsync(collapsedEntry(host), JSON.stringify(projects));
}

export async function getCollapsed(host: string): Promise<string[]> {
  return JSON.parse((await SecureStore.getItemAsync(collapsedEntry(host))) ?? '[]') as string[];
}

/** The project and agent New task last started work with on a desktop, so the
 * next task starts there too. An empty agent is the project's own. */
export interface LastWork {
  project: string;
  agent: string;
}

export async function saveLastWork(host: string, last: LastWork): Promise<void> {
  await SecureStore.setItemAsync(lastWorkEntry(host), JSON.stringify(last));
}

export async function getLastWork(host: string): Promise<LastWork | null> {
  const stored = await SecureStore.getItemAsync(lastWorkEntry(host));
  return stored ? (JSON.parse(stored) as LastWork) : null;
}

/** Whether a terminal is fitted to the phone while the phone has control. */
export async function saveFit(on: boolean): Promise<void> {
  await SecureStore.setItemAsync(FIT, on ? '1' : '0');
}

export async function getFit(): Promise<boolean> {
  return (await SecureStore.getItemAsync(FIT)) === '1';
}

/** The app lock — see lib/lock.ts. The PIN is kept only as a salted hash. */
export interface LockConfig {
  /** Salt and SHA-256 of the PIN, base64url and hex. */
  salt: string;
  hash: string;
  /** How long the app may sit in the background before it locks again. */
  afterMs: number;
  /** Wrong PINs in a row, and when the next try is allowed. */
  failures: number;
  waitUntil: number;
}

export async function getLock(): Promise<LockConfig | null> {
  const stored = await SecureStore.getItemAsync(LOCK);
  return stored ? (JSON.parse(stored) as LockConfig) : null;
}

export async function saveLock(config: LockConfig | null): Promise<void> {
  if (config) await SecureStore.setItemAsync(LOCK, JSON.stringify(config));
  else await SecureStore.deleteItemAsync(LOCK);
}

export function toPaired(host: SavedHost): PairedHost {
  return {
    hostId: fromBase64Url(host.id),
    hostPublic: fromBase64Url(host.hostPublic),
    relay: host.relay,
  };
}
