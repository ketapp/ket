// Whether a desktop's worktree list is still filling, so the Worktrees
// screen draws its skeleton rather than saying nothing is running.
//
// The desktop welcomes a phone before it has worked out its list, so for a
// moment the phone is connected with nothing to show. And a list can be
// empty only because the desktop has just started: its windows put their
// terminals back a second or two later. So the list fills from the first
// answer until a snapshot is here, and an empty one waits out SETTLE_MS
// before it is believed. After that, empty is empty.

import { everyWorktree } from './activity';
import type { HostConnection } from './connection';

/** How long after the desktop is first reached, or restarts, an empty list
 * still reads as filling. */
export const SETTLE_MS = 3_000;

type Settling = Pick<HostConnection, 'status' | 'snapshot' | 'starting'> &
  Partial<Pick<HostConnection, 'freshSince' | 'snapshotAt'>>;

/** Whether `conn`'s worktree list is still filling at `now`. Never while it
 * is not connected: the screen shows the reaching instead. */
export function settling(conn: Settling, now: number): boolean {
  if (conn.status !== 'connected') return false;
  if (conn.snapshot === null) return true;
  const fresh = conn.freshSince ?? null;
  if (fresh === null) return false;
  // The list is from before the desktop restarted; its own is on the way.
  if ((conn.snapshotAt ?? 0) < fresh) return true;
  const empty = everyWorktree(conn.snapshot).length === 0 && conn.starting.length === 0;
  return empty && now - fresh < SETTLE_MS;
}

/** When an empty list stops reading as filling, for a screen to look again
 * then; `null` when no wait is running. */
export function settlesAt(conn: Settling): number | null {
  const fresh = conn.freshSince ?? null;
  return fresh === null ? null : fresh + SETTLE_MS;
}
