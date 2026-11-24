// Screens' view of the paired desktops: each one's connection, for what it
// does, and its state, for what it is now (see `ConnectionState` in
// lib/connection.ts).
//
// A connection is one long-lived object that changes in place, so a screen
// never reads it while it draws: it draws from `useHostState`, a new state
// object whenever anything in it changes, which React and the React
// Compiler can tell from the last. The connection itself is for asking the
// desktop.

import { useFocusEffect } from 'expo-router';
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from 'react';

import { type AddressKind, WAY_PHRASE, addressKind, wayOf } from './address-kind';
import { type ConnectionState, connection, heldConnections, type HostConnection, liveConnection } from './connection';
import { getHost, listHosts } from './store';
import { adoptPalette } from './theme';

/** Every paired desktop's connection; `null` until the list has been read.
 * Read again each time the screen is focused, and when `refresh` changes.
 * Their states are `useHostStates`'. */
export function useHosts(refresh?: string): HostConnection[] | null {
  const [conns, setConns] = useState<HostConnection[] | null>(null);

  const read = useCallback(() => {
    void listHosts().then((hosts) => setConns(hosts.map((host) => connection(host))));
  }, []);
  useFocusEffect(read);
  useEffect(() => {
    if (refresh !== undefined) read();
  }, [refresh, read]);

  return conns;
}

/** `useHosts`, but with the connections this app already holds until the
 * Keychain gives the list back: a screen opened from Worktrees has its
 * desktops on its first draw, rather than drawing none for a moment. `null`
 * only while none is held and the list has not been read. */
export function useHeldHosts(): HostConnection[] | null {
  const [held] = useState(() => {
    const now = heldConnections();
    return now.length > 0 ? now : null;
  });
  return useHosts() ?? held;
}

/** One paired desktop's connection; `null` until it has been found. A desktop
 * the app is already connected to — any screen opened from Worktrees — is
 * there on the first draw; only one not met yet waits for the Keychain. Its
 * state is `useHostState`'s. */
export function useHost(id: string): HostConnection | null {
  const held = liveConnection(id);
  // One not met yet, once the Keychain has given it back.
  const [found, setFound] = useState<HostConnection | null>(null);

  useEffect(() => {
    // Only when none was held as this drew: one that went live since is the
    // connection `connection(host)` hands back.
    if (held) return;
    let live = true;
    void getHost(id).then((host) => {
      if (host && live) setFound(connection(host));
    });
    return () => {
      live = false;
    };
  }, [id, held]);

  return held ?? (found?.id === id ? found : null);
}

/** What `conn` is now, drawn again whenever it changes; `null` without a
 * connection.
 *
 * `shown` false holds it as it was until it is true again, then draws it as
 * it is now. A screen others open on top of passes `useIsFocused()`: the
 * stack keeps every screen under the top one mounted and running, and a
 * chat, the worktree opened from its header and the chat opened again from
 * there were each drawn again for every snapshot out of sight, so the app
 * slowed with every screen the stack held. */
export function useHostState(conn: HostConnection, shown?: boolean): ConnectionState;
export function useHostState(conn: HostConnection | null, shown?: boolean): ConnectionState | null;
export function useHostState(conn: HostConnection | null, shown = true): ConnectionState | null {
  const subscribe = useCallback(
    (changed: () => void) => (conn && shown ? conn.onChange(changed) : () => {}),
    [conn, shown],
  );
  return useSyncExternalStore(subscribe, () => conn?.state ?? null);
}

/** The states `useHostStates` last handed out, by the list of connections they
 * are of: the same array until one of them changes. */
const statesOf = new WeakMap<readonly HostConnection[], readonly ConnectionState[]>();

/** What each of `conns` is now, in their order, drawn again whenever any of
 * them changes; `null` without the list. */
export function useHostStates(conns: readonly HostConnection[] | null): readonly ConnectionState[] | null {
  const subscribe = useCallback(
    (changed: () => void) => {
      const offs = (conns ?? []).map((c) => c.onChange(changed));
      return () => offs.forEach((off) => off());
    },
    [conns],
  );
  const read = () => {
    if (!conns) return null;
    const now = conns.map((c) => c.state);
    const was = statesOf.get(conns);
    if (was && was.length === now.length && was.every((state, at) => state === now[at])) return was;
    statesOf.set(conns, now);
    return now;
  };
  return useSyncExternalStore(subscribe, read);
}

/** A paired desktop as a screen listing them all has it: its connection, for
 * asking it, and its state, for drawing it. */
export type PairedHost = { readonly conn: HostConnection; readonly hostState: ConnectionState };

/** Every paired desktop with its state, in the order they were paired; `null`
 * until the list has been read. A new list whenever any of them changes. See
 * `useHosts` for `refresh`. */
export function usePairedHosts(refresh?: string): readonly PairedHost[] | null {
  const conns = useHosts(refresh);
  const hostStates = useHostStates(conns);
  return useMemo(
    () => (conns && hostStates ? conns.map((conn, at) => ({ conn, hostState: hostStates[at] })) : null),
    [conns, hostStates],
  );
}

/** What a connection's state reads as under a desktop's name: once it is
 * up, how — on the local network, over Tailscale, through a relay — in
 * words, and as the kind of address the Secure pill draws a mark for. */
export function statusLine(live: ConnectionState | null): { text: string; connected: boolean; kind?: AddressKind } {
  switch (live?.status) {
    case 'connected': {
      const address = live.activeAddress;
      if (!address) return { text: 'Connected', connected: true };
      const way = wayOf(address, live.host.manualAddresses?.includes(address));
      return { text: WAY_PHRASE[way], connected: true, kind: addressKind(address) };
    }
    case 'offline':
      return { text: live.error ? `Offline · ${live.error}` : 'Offline · retrying', connected: false };
    default:
      return { text: 'Connecting…', connected: false };
  }
}

/** How often the paired desktops are listed again, so one paired since the
 * app started is followed too. */
const RELIST_MS = 5000;

/** Follows the desktop's theme: the first paired desktop that has sent one,
 * so with two desktops the phone does not flip between them. See
 * `adoptPalette`, which reloads the app when the palette has changed. */
export function useDesktopPalette(): void {
  useEffect(() => {
    let live = true;
    let ids = '';
    let offs: (() => void)[] = [];
    const follow = (conns: HostConnection[]) => {
      const palette = conns.map((c) => c.state.snapshot?.palette).find((p) => p !== undefined);
      if (palette) adoptPalette(palette);
    };
    const look = () => {
      void listHosts().then((hosts) => {
        if (!live) return;
        const next = hosts.map((host) => host.id).join(',');
        if (next === ids) return;
        ids = next;
        offs.forEach((off) => off());
        const conns = hosts.map((host) => connection(host));
        offs = conns.map((c) => c.onChange(() => follow(conns)));
        follow(conns);
      }).catch(() => {
        // The Keychain unreadable for now: the next look tries again.
      });
    };
    look();
    const timer = setInterval(look, RELIST_MS);
    return () => {
      live = false;
      clearInterval(timer);
      offs.forEach((off) => off());
    };
  }, []);
}
