// A worktree marked unread from its menu (1.35) is read once someone comes
// to it: here, by opening its screen or one of its terminals.

import { type WorktreeSummary } from '@ket/remote';
import { useIsFocused } from 'expo-router';
import { useEffect, useRef } from 'react';

import type { HostConnection } from './connection';

/** Takes the mark off `worktree` as this screen comes to it — once each time
 * it comes into focus, not whenever the mark is there. One marked while it is
 * already showing, from its own menu here or on the desktop, keeps its dot
 * until it is left and come back to. */
export function useReadOnArrival(conn: HostConnection | null, worktree: WorktreeSummary | undefined) {
  const showing = useIsFocused();
  // The worktree this look has come to, until the screen goes out of focus.
  const cameTo = useRef<string | null>(null);
  const id = worktree?.id;
  const marked = worktree?.unread === true;
  useEffect(() => {
    if (!showing) {
      cameTo.current = null;
      return;
    }
    if (!conn || id === undefined || cameTo.current === id) return;
    cameTo.current = id;
    if (marked) conn.markUnread(id, false);
  }, [conn, showing, id, marked]);
}
