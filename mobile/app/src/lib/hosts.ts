// Screens' view of the paired desktops: each one's live connection, redrawn
// whenever any of them changes.
//
// A connection is one long-lived object whose fields change in place, so a
// screen that reads it opts out of the React Compiler with 'use no memo': the
// compiler caches what it derives from an unchanged reference, and the
// redraw these hooks ask for then drew the old status — Worktrees sat at
// "Connecting…" while Settings, drawn later, said connected.

import { useFocusEffect } from 'expo-router';
import { useCallback, useEffect, useState } from 'react';

import { connection, type HostConnection } from './connection';
import { getHost, listHosts } from './store';
import { adoptPalette } from './theme';

/** Every paired desktop, connected; `null` until the list has been read. */
export function useHosts(): HostConnection[] | null {
  const [conns, setConns] = useState<HostConnection[] | null>(null);
  const [, redraw] = useState(0);

  useFocusEffect(
    useCallback(() => {
      void listHosts().then((hosts) => setConns(hosts.map((host) => connection(host))));
    }, []),
  );

  useEffect(() => {
    const offs = (conns ?? []).map((c) => c.onChange(() => redraw((n) => n + 1)));
    return () => offs.forEach((off) => off());
  }, [conns]);

  return conns;
}

/** One paired desktop, connected; `null` until it has been found. */
export function useHost(id: string): HostConnection | null {
  const [conn, setConn] = useState<HostConnection | null>(null);
  const [, redraw] = useState(0);

  useEffect(() => {
    let off = () => {};
    let live = true;
    void getHost(id).then((host) => {
      if (!host || !live) return;
      const c = connection(host);
      setConn(c);
      off = c.onChange(() => redraw((n) => n + 1));
    });
    return () => {
      live = false;
      off();
    };
  }, [id]);

  return conn;
}

/** What a connection's state reads as under a desktop's name. */
export function statusLine(conn: HostConnection | null): { text: string; connected: boolean } {
  switch (conn?.status) {
    case 'connected':
      return { text: 'Connected through the relay', connected: true };
    case 'offline':
      return { text: conn.error ? `Offline · ${conn.error}` : 'Offline · retrying', connected: false };
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
      const palette = conns.map((c) => c.snapshot?.palette).find((p) => p !== undefined);
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
