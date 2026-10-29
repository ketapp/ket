// What the phone keeps, in the iOS Keychain / Android Keystore through
// SecureStore: its own key pair, and the hosts it has paired with. Each host
// is its own entry, because SecureStore is only promised to hold a couple of
// kilobytes per value.

import './polyfills';

import * as SecureStore from 'expo-secure-store';
import { type Keypair, type PairedHost, fromBase64Url, generateKeypair, keypairFrom, toBase64Url } from '@ket/remote';
import { dialAddresses, normalizeHost } from './addresses';

const KEY = 'ket.phone.key';
const HOSTS = 'ket.hosts';
const hostEntry = (id: string) => `ket.host.${id}`;
const DESKTOP = 'ket.desktop';
const NOTIFY = 'ket.notify';
const LOCK = 'ket.lock';
const collapsedEntry = (host: string) => `ket.collapsed.${host}`;
const lastWorkEntry = (host: string) => `ket.lastwork.${host}`;
/** Set once every entry has been moved to `KEYCHAIN`'s class. */
const UPGRADED = 'ket.keychain';
/** The theme's entries — see lib/theme.ts. */
const THEME_KEYS = ['ket.palette', 'ket.palette.reloadedAt'];

/** Readable from the first unlock after a restart, not only while unlocked.
 * The app keeps running for a moment after the phone locks — a redial, the
 * desktop list polled every few seconds — and under iOS's default class every
 * read then failed with "User interaction is not allowed". */
const KEYCHAIN: SecureStore.SecureStoreOptions = {
  keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK,
};

/** Moves entries written under the default class to `KEYCHAIN`'s. Writing an
 * existing entry keeps its class, so each is deleted and written again. Every
 * read and write waits for it, so none sees an entry in between; one that
 * fails, with the phone locked at launch, is tried again on the next. */
async function upgrade(): Promise<void> {
  if (await SecureStore.getItemAsync(UPGRADED)) return;
  const ids: string[] = JSON.parse((await SecureStore.getItemAsync(HOSTS)) ?? '[]');
  const keys = [
    KEY,
    HOSTS,
    DESKTOP,
    NOTIFY,
    LOCK,
    ...ids.flatMap((id) => [hostEntry(id), collapsedEntry(id), lastWorkEntry(id)]),
    ...THEME_KEYS,
  ];
  for (const key of keys) {
    const value = await SecureStore.getItemAsync(key);
    if (value === null) continue;
    await SecureStore.deleteItemAsync(key);
    await SecureStore.setItemAsync(key, value, KEYCHAIN);
  }
  await SecureStore.setItemAsync(UPGRADED, '1', KEYCHAIN);
}

let upgrading: Promise<void> | null = null;
function upgraded(): Promise<void> {
  upgrading ??= upgrade().catch((error: unknown) => {
    upgrading = null;
    throw error;
  });
  return upgrading;
}

async function get(key: string): Promise<string | null> {
  await upgraded();
  return SecureStore.getItemAsync(key);
}

async function set(key: string, value: string): Promise<void> {
  await upgraded();
  await SecureStore.setItemAsync(key, value, KEYCHAIN);
}

async function del(key: string): Promise<void> {
  await upgraded();
  await SecureStore.deleteItemAsync(key);
}

/** A paired host as the app lists it. */
export interface SavedHost {
  /** The host id, base64url: stable, and what routes use. */
  id: string;
  name: string;
  relay: string;
  /** Addresses most recently advertised by a 1.17 or newer host. */
  addresses?: string[];
  /** Addresses this phone's owner added. */
  manualAddresses?: string[];
  /** Address that most recently connected. */
  lastAddress?: string;
  hostPublic: string;
  pairedAt: number;
  /** Where the last session left off, for resuming. */
  cursor?: { epoch: string; seq: string };
}

/** This phone's key pair, made the first time it is asked for. */
export async function phoneKey(): Promise<Keypair> {
  const stored = await get(KEY);
  if (stored) return keypairFrom(fromBase64Url(stored));
  const keys = generateKeypair();
  await set(KEY, toBase64Url(keys.privateKey));
  return keys;
}

export async function listHosts(): Promise<SavedHost[]> {
  const ids: string[] = JSON.parse((await get(HOSTS)) ?? '[]');
  const hosts = await Promise.all(ids.map((id) => getHost(id)));
  return hosts.filter((h): h is SavedHost => h !== null);
}

export async function getHost(id: string): Promise<SavedHost | null> {
  const stored = await get(hostEntry(id));
  if (!stored) return null;
  const host = JSON.parse(stored) as SavedHost;
  const migrated = normalizeHost(host);
  if (!host.addresses || !host.manualAddresses) await set(hostEntry(id), JSON.stringify(migrated));
  return migrated;
}

export async function saveHost(host: SavedHost): Promise<void> {
  await set(hostEntry(host.id), JSON.stringify(normalizeHost(host)));
  const ids: string[] = JSON.parse((await get(HOSTS)) ?? '[]');
  if (!ids.includes(host.id)) {
    ids.push(host.id);
    await set(HOSTS, JSON.stringify(ids));
  }
}

export async function forgetHost(id: string): Promise<void> {
  await del(hostEntry(id));
  await del(collapsedEntry(id));
  await del(lastWorkEntry(id));
  const ids: string[] = JSON.parse((await get(HOSTS)) ?? '[]');
  await set(HOSTS, JSON.stringify(ids.filter((i) => i !== id)));
}

/** The desktop the Worktrees screen shows, when more than one is paired. */
export async function saveDesktop(id: string): Promise<void> {
  await set(DESKTOP, id);
}

export async function getDesktop(): Promise<string | null> {
  return (await get(DESKTOP)) ?? null;
}

/** The projects folded on the Worktrees screen, per desktop. The phone's own:
 * the desktop's sidebar folds what fits a window, not what fits a phone. */
export async function saveCollapsed(host: string, projects: string[]): Promise<void> {
  await set(collapsedEntry(host), JSON.stringify(projects));
}

export async function getCollapsed(host: string): Promise<string[]> {
  return JSON.parse((await get(collapsedEntry(host))) ?? '[]') as string[];
}

/** The project and agent New task last started work with on a desktop, so the
 * next task starts there too. An empty agent is the project's own. */
export interface LastWork {
  project: string;
  agent: string;
}

export async function saveLastWork(host: string, last: LastWork): Promise<void> {
  await set(lastWorkEntry(host), JSON.stringify(last));
}

export async function getLastWork(host: string): Promise<LastWork | null> {
  const stored = await get(lastWorkEntry(host));
  return stored ? (JSON.parse(stored) as LastWork) : null;
}

/** Which notices the phone posts — see lib/notices.ts. */
export interface NotifyConfig {
  /** The switch over all of them. */
  on: boolean;
  /** An agent has stopped on a prompt. */
  waiting: boolean;
  /** An agent's task finished, or failed. */
  finished: boolean;
  /** A check failed on a worktree's pull request. */
  checks: boolean;
}

/** Off until it is turned on: that is when the phone asks to show them. */
export const DEFAULT_NOTIFY: NotifyConfig = { on: false, waiting: true, finished: true, checks: true };

export async function getNotify(): Promise<NotifyConfig> {
  const stored = await get(NOTIFY);
  return stored ? { ...DEFAULT_NOTIFY, ...(JSON.parse(stored) as Partial<NotifyConfig>) } : DEFAULT_NOTIFY;
}

export async function saveNotify(config: NotifyConfig): Promise<void> {
  await set(NOTIFY, JSON.stringify(config));
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
  const stored = await get(LOCK);
  return stored ? (JSON.parse(stored) as LockConfig) : null;
}

export async function saveLock(config: LockConfig | null): Promise<void> {
  if (config) await set(LOCK, JSON.stringify(config));
  else await del(LOCK);
}

export function toPaired(host: SavedHost): PairedHost {
  return {
    hostId: fromBase64Url(host.id),
    hostPublic: fromBase64Url(host.hostPublic),
    relay: host.relay,
    addresses: dialAddresses(host),
  };
}
