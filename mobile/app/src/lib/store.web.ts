// What the phone keeps, in a browser: the same entries `store.ts` keeps in the
// Keychain, in this origin's localStorage instead. Weaker — anything running
// on the page's origin can read the key — which is acceptable for trying ket
// from a phone's browser, not for shipping.

import './polyfills';

import { type Keypair, type PairedHost, fromBase64Url, generateKeypair, keypairFrom, toBase64Url } from '@ket/remote';

import type { LastWork, LockConfig, SavedHost } from './store';

export type { LastWork, LockConfig, SavedHost } from './store';

const KEY = 'ket.phone.key';
const HOSTS = 'ket.hosts';
const hostEntry = (id: string) => `ket.host.${id}`;
const DESKTOP = 'ket.desktop';
const FIT = 'ket.fit';
const LOCK = 'ket.lock';
const collapsedEntry = (host: string) => `ket.collapsed.${host}`;
const lastWorkEntry = (host: string) => `ket.lastwork.${host}`;

const get = (key: string) => localStorage.getItem(key);
const set = (key: string, value: string) => localStorage.setItem(key, value);

export async function phoneKey(): Promise<Keypair> {
  const stored = get(KEY);
  if (stored) return keypairFrom(fromBase64Url(stored));
  const keys = generateKeypair();
  set(KEY, toBase64Url(keys.privateKey));
  return keys;
}

export async function listHosts(): Promise<SavedHost[]> {
  const ids: string[] = JSON.parse(get(HOSTS) ?? '[]');
  const hosts = await Promise.all(ids.map((id) => getHost(id)));
  return hosts.filter((h): h is SavedHost => h !== null);
}

export async function getHost(id: string): Promise<SavedHost | null> {
  const stored = get(hostEntry(id));
  return stored ? (JSON.parse(stored) as SavedHost) : null;
}

export async function saveHost(host: SavedHost): Promise<void> {
  set(hostEntry(host.id), JSON.stringify(host));
  const ids: string[] = JSON.parse(get(HOSTS) ?? '[]');
  if (!ids.includes(host.id)) {
    ids.push(host.id);
    set(HOSTS, JSON.stringify(ids));
  }
}

export async function forgetHost(id: string): Promise<void> {
  localStorage.removeItem(hostEntry(id));
  localStorage.removeItem(collapsedEntry(id));
  localStorage.removeItem(lastWorkEntry(id));
  const ids: string[] = JSON.parse(get(HOSTS) ?? '[]');
  set(HOSTS, JSON.stringify(ids.filter((i) => i !== id)));
}

/** The desktop the Worktrees screen shows, when more than one is paired. */
export async function saveDesktop(id: string): Promise<void> {
  set(DESKTOP, id);
}

export async function getDesktop(): Promise<string | null> {
  return get(DESKTOP);
}

export async function saveCollapsed(host: string, projects: string[]): Promise<void> {
  set(collapsedEntry(host), JSON.stringify(projects));
}

export async function getCollapsed(host: string): Promise<string[]> {
  return JSON.parse(get(collapsedEntry(host)) ?? '[]') as string[];
}

export async function saveLastWork(host: string, last: LastWork): Promise<void> {
  set(lastWorkEntry(host), JSON.stringify(last));
}

export async function getLastWork(host: string): Promise<LastWork | null> {
  const stored = get(lastWorkEntry(host));
  return stored ? (JSON.parse(stored) as LastWork) : null;
}

/** Whether a terminal is fitted to the phone while the phone has control. */
export async function saveFit(on: boolean): Promise<void> {
  set(FIT, on ? '1' : '0');
}

export async function getFit(): Promise<boolean> {
  return get(FIT) === '1';
}

export function toPaired(host: SavedHost): PairedHost {
  return {
    hostId: fromBase64Url(host.id),
    hostPublic: fromBase64Url(host.hostPublic),
    relay: host.relay,
  };
}

export async function getLock(): Promise<LockConfig | null> {
  const stored = get(LOCK);
  return stored ? (JSON.parse(stored) as LockConfig) : null;
}

export async function saveLock(config: LockConfig | null): Promise<void> {
  if (config) set(LOCK, JSON.stringify(config));
  else localStorage.removeItem(LOCK);
}
