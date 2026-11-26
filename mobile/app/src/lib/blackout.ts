// Blackout hours: the hours a desktop shuts ket off on every paired device,
// set in its Settings › Devices and lifted only there (protocol 1.37).
//
// The desktop is what enforces it: in a blackout it sends a snapshot with
// nothing in it and refuses whatever is asked. This is the phone's half —
// knowing when, so components/Blackout can cover every screen on time:
//
// - a desktop the phone is connected to has said so itself, in the snapshot
//   of this session (`blackout.untilMs`), and its word is taken as it is;
// - one it cannot reach is judged by the hours it last said, which are kept:
//   the blackouts to come as moments, so nothing here knows the desktop's
//   clock or calendar. Two weeks of them, said again on every snapshot.
//
// One desktop in a blackout covers the app, whichever desktop a screen was
// showing: the limit is on the person, not on a Mac.

import type { HostSnapshot } from '@ket/remote';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { AppState } from 'react-native';

import { type ConnectionState, type HostConnection, connection } from './connection';
import { type SavedBlackout, getBlackout, listHosts, saveBlackout } from './store';

/** How long before a blackout the chat says one is coming. */
export const WARN_MS = 10 * 60_000;

/** The longest the phone goes without looking at the clock again, so a
 * clock that was changed is noticed. */
const LOOK_MS = 60_000;

/** How often the paired desktops are listed again, as lib/hosts.ts does. */
const RELIST_MS = 5000;

/** One desktop's blackout hours, as the phone keeps them. */
export interface Hours extends SavedBlackout {
  /** The desktop's id. */
  host: string;
  /** Its name, for saying whose hours they are. */
  name: string;
}

/** A blackout shutting the phone out: when it ends, and whose it is. */
export interface Shut {
  until: number;
  name: string;
}

/** A blackout about to start: when, and whose. */
export interface Soon {
  at: number;
  name: string;
}

/** What the app sees. */
export interface BlackoutState {
  /** The kept hours have been read. Nothing is shown until they are. */
  ready: boolean;
  /** The blackout in force, if one is. */
  shut: Shut | null;
  /** One starting within [`WARN_MS`], while none is in force. */
  soon: Soon | null;
  /** Every paired desktop's hours, for Settings. */
  hours: readonly Hours[];
  /** When this was worked out. */
  now: number;
}

/** The blackout `now` falls in, by `hours`: its start and end. */
export function windowAt(hours: SavedBlackout, now: number): [number, number] | null {
  return hours.windows.find(([start, end]) => start <= now && now < end) ?? null;
}

/** When the blackout shutting the phone out at `now` ends, by `hours`;
 * `null` when it is not shut out — no blackout, or one it was let in
 * during. */
export function shutUntil(hours: SavedBlackout, now: number): number | null {
  const window = windowAt(hours, now);
  return window && now >= hours.letInUntil ? window[1] : null;
}

/** When the next blackout after `now` starts, by `hours`. */
export function nextStart(hours: SavedBlackout, now: number): number | null {
  return hours.windows.find(([start]) => start > now)?.[0] ?? null;
}

/** What a desktop's snapshot says of its blackout hours: the hours to keep,
 * `null` when it has none, and when the blackout it is in ends, or zero. */
export function said(snapshot: HostSnapshot): { hours: SavedBlackout | null; until: number } {
  const told = snapshot.blackout;
  if (!told) return { hours: null, until: 0 };
  return {
    hours: {
      windows: told.windows.map((window) => [Number(window.startMs), Number(window.endMs)]),
      letInUntil: Number(told.letInUntilMs),
      repeat: told.repeat,
    },
    until: Number(told.untilMs),
  };
}

/** `7:00 AM`, on the phone's clock, as the app writes a time. */
export function clockOf(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
}

/** `10 minutes`, `1 minute`: how long until `at`, rounded up. */
export function minutesUntil(at: number, now: number): string {
  const minutes = Math.max(1, Math.ceil((at - now) / 60_000));
  return minutes === 1 ? '1 minute' : `${minutes} minutes`;
}

let ready = false;
/** Each desktop's hours, by its id. */
const kept = new Map<string, Hours>();
/** The hours as last written to the Keychain, to tell a change. */
const stored = new Map<string, string>();
/** Desktops that have spoken in this run of the app: what they said stands
 * over whatever the Keychain held. */
const heard = new Set<string>();
/** The desktops connected now, and when the blackout each said it is in
 * ends — zero for not in one. */
const live = new Map<string, number>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** How things stand at `now`, across every desktop. */
export function standing(now: number): { shut: Shut | null; soon: Soon | null } {
  let shut: Shut | null = null;
  let soon: Soon | null = null;
  for (const id of new Set([...kept.keys(), ...live.keys()])) {
    const hours = kept.get(id);
    const word = live.get(id);
    const name = hours?.name ?? 'your Mac';
    // A connected desktop's own word, until the moment it named.
    const until = word !== undefined ? (word > now ? word : null) : hours ? shutUntil(hours, now) : null;
    if (until !== null && (!shut || until > shut.until)) shut = { until, name };
    const next = hours ? nextStart(hours, now) : null;
    if (next !== null && next - now <= WARN_MS && (!soon || next < soon.at)) soon = { at: next, name };
  }
  return { shut, soon: shut ? null : soon };
}

/** The next moment after `now` at which [`standing`] could change. */
export function nextChange(now: number): number | null {
  const moments: number[] = [...live.values()];
  for (const hours of kept.values()) {
    moments.push(hours.letInUntil);
    for (const [start, end] of hours.windows) moments.push(start - WARN_MS, start, end);
  }
  const ahead = moments.filter((at) => at > now);
  return ahead.length > 0 ? Math.min(...ahead) : null;
}

/** Keeps `hours` as what the desktop `id` says now, and writes them when
 * they have changed. Returns whether they had. */
function keep(id: string, name: string, hours: SavedBlackout | null): boolean {
  heard.add(id);
  const next = hours ? JSON.stringify(hours) : '';
  const renamed = hours !== null && kept.get(id)?.name !== name;
  if ((stored.get(id) ?? '') === next && !renamed) return false;
  if (hours) kept.set(id, { ...hours, host: id, name });
  else kept.delete(id);
  if ((stored.get(id) ?? '') !== next) {
    stored.set(id, next);
    // The Keychain unreadable for now: the next snapshot says them again.
    void saveBlackout(id, hours).catch(() => {});
  }
  return true;
}

/** Takes what a connection's state says. Returns whether anything changed. */
function take(state: ConnectionState): boolean {
  const id = state.host.id;
  // Only a snapshot of the session that is open: the one left from the last
  // session says how things stood before a blackout ended it.
  const current =
    state.status === 'connected' &&
    state.snapshot !== null &&
    state.snapshotAt !== null &&
    state.upSince !== null &&
    state.snapshotAt >= state.upSince;
  if (!current || !state.snapshot) return live.delete(id);
  const { hours, until } = said(state.snapshot);
  const moved = live.get(id) !== until;
  live.set(id, until);
  return keep(id, state.host.name, hours) || moved;
}

/** Reads the kept hours, so the cover can be drawn before any desktop has
 * answered. Once, at launch. */
let started = false;
export function startBlackout(): void {
  if (started) return;
  started = true;
  void listHosts()
    .then(async (hosts) => {
      for (const host of hosts) {
        const hours = await getBlackout(host.id).catch(() => null);
        if (!hours || heard.has(host.id)) continue;
        kept.set(host.id, { ...hours, host: host.id, name: host.name });
        stored.set(host.id, JSON.stringify(hours));
      }
    })
    .catch(() => {
      // The Keychain unreadable: the desktops say their hours as they connect.
    })
    .finally(() => {
      ready = true;
      notify();
    });
}

/** Follows every paired desktop's blackout hours, from the app's root. */
export function useBlackoutFollowing(): void {
  useEffect(() => {
    let on = true;
    let ids = '';
    let offs: (() => void)[] = [];
    const follow = (conns: HostConnection[]) => {
      let changed = false;
      for (const conn of conns) changed = take(conn.state) || changed;
      if (changed) notify();
    };
    const look = () => {
      void listHosts()
        .then((hosts) => {
          if (!on) return;
          const next = hosts.map((host) => host.id).join(',');
          if (next === ids) return;
          ids = next;
          offs.forEach((off) => off());
          // A desktop no longer paired has no say.
          let dropped = false;
          for (const id of new Set([...kept.keys(), ...live.keys()])) {
            if (hosts.some((host) => host.id === id)) continue;
            dropped = kept.delete(id) || dropped;
            dropped = live.delete(id) || dropped;
            stored.delete(id);
            heard.delete(id);
          }
          if (dropped) notify();
          const conns = hosts.map((host) => connection(host));
          offs = conns.map((conn) => conn.onChange(() => follow(conns)));
          follow(conns);
        })
        .catch(() => {
          // The Keychain unreadable for now: the next look tries again.
        });
    };
    look();
    const timer = setInterval(look, RELIST_MS);
    return () => {
      on = false;
      clearInterval(timer);
      offs.forEach((off) => off());
    };
  }, []);
}

/** How the blackout stands, drawn again as it changes: a desktop's word, a
 * blackout's hour coming or going, the app returning to the front. */
export function useBlackout(): BlackoutState {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const look = () => setNow(Date.now());
    listeners.add(look);
    const sub = AppState.addEventListener('change', (next) => {
      if (next === 'active') look();
    });
    return () => {
      listeners.delete(look);
      sub.remove();
    };
  }, []);
  // The clock looked at again at the next moment anything could change.
  const next = nextChange(now);
  useEffect(() => {
    const wait = next === null ? LOOK_MS : Math.min(Math.max(next - Date.now(), 0) + 50, LOOK_MS);
    const timer = setTimeout(() => setNow(Date.now()), wait);
    return () => clearTimeout(timer);
  }, [next, now]);
  const { shut, soon } = ready ? standing(now) : { shut: null, soon: null };
  return { ready, shut, soon, hours: [...kept.values()], now };
}

/** Whether the cover is up — see components/Blackout, which says so. */
let covered = false;
const coverListeners = new Set<() => void>();

/** Said by the cover as it goes up and comes down. */
export function setCovered(next: boolean): void {
  if (covered === next) return;
  covered = next;
  for (const listener of coverListeners) listener();
}

/** Whether the blackout's cover is up, for what is drawn over it: a sheet or
 * a menu is a native modal, above anything the app draws, so it has to take
 * itself down. One flag the cover sets, rather than every sheet keeping the
 * hours' clock for itself. */
export function useCovered(): boolean {
  return useSyncExternalStore(
    (listener) => {
      coverListeners.add(listener);
      return () => {
        coverListeners.delete(listener);
      };
    },
    () => covered,
  );
}
