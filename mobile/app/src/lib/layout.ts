// The app on a wide screen — an iPad, or a browser window wide enough for
// two columns: the worktree list stays on the left as a sidebar, and what
// it opens is drawn beside it rather than over it. Narrower, every screen is
// the whole screen, as on a phone.
//
// Either way a screen's own text never runs wider than READABLE_WIDTH: a
// conversation or a settings card stretched across an iPad's width is
// harder to read, not easier.

import { router } from 'expo-router';
import { createContext, useContext } from 'react';
import { useWindowDimensions } from 'react-native';
import type { HostSnapshot } from '@ket/remote';

/** The narrowest window that gets the sidebar: an 11-inch iPad on its side,
 * a 13-inch one either way. An iPad in portrait under 13 inches, and every
 * phone, keeps one column. */
export const SPLIT_AT = 900;

/** The sidebar's width. Wide enough for the tab bar's four tabs. */
export const SIDEBAR_WIDTH = 340;

/** The widest a screen's content is drawn: a comfortable line of prose. */
export const READABLE_WIDTH = 720;

/** Whether the window is wide enough for the sidebar. */
export function useWide(): boolean {
  return useWindowDimensions().width >= SPLIT_AT;
}

/** Whether the sidebar is showing: a wide window with a desktop paired. Set
 * by the root layout; screens drawn beside the sidebar leave out what it
 * already has — the tab bar, the worktree list. */
export const SplitContext = createContext(false);

export function useSplit(): boolean {
  return useContext(SplitContext);
}

/** Opens a screen beside the sidebar in place of whatever was open there:
 * picking another worktree is not a step deeper, so Back never walks through
 * the ones picked before it. */
export function showBeside(href: string): void {
  if (router.canDismiss()) router.dismissAll();
  router.replace(href);
}

/** The worktree a screen's path is about, by its id, when it is one of
 * `hostId`'s — what the sidebar marks as open. `null` for any other screen. */
export function worktreeAt(path: string, hostId: string, snapshot: HostSnapshot | null): string | null {
  const [, kind, host, id] = path.split('/');
  if (!snapshot || host !== hostId || id === undefined) return null;
  if (kind === 'worktree' || kind === 'diff') return decodeURIComponent(id);
  if (kind !== 'session' && kind !== 'terminal') return null;
  for (const project of snapshot.projects) {
    for (const worktree of project.worktrees) {
      if (worktree.terminals.some((terminal) => terminal.id.toString() === id)) return worktree.id;
    }
  }
  return null;
}

/** The tab a path belongs to, for the sidebar's tab bar: a worktree's
 * screens are Worktrees', a desktop's devices and pairing are Settings'. */
export function tabAt(path: string): 'worktrees' | 'needs' | 'usage' | 'settings' {
  const first = path.split('/')[1];
  if (first === 'needs' || first === 'usage' || first === 'settings') return first;
  if (first === 'devices' || first === 'pair') return 'settings';
  return 'worktrees';
}
