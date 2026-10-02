// How a worktree's state is drawn: the word and colour ket's sidebar gives
// it, the colour its detail line takes, and a phrase when the agent said
// nothing.

import { Activity, type HostSnapshot, type ProjectSummary, type WorktreeSummary } from '@ket/remote';

import { PROJECT_COLORS, t } from './theme';

/**
 * The word a row ends in, in its colour — the desktop sidebar's words
 * (crates/ket-ui/src/tree.rs, `state_word`). Nothing when the worktree is
 * quiet: a quiet row ends in nothing rather than in "idle".
 */
export function stateOf(activity: Activity): { word: string; color: string } | undefined {
  switch (activity) {
    case Activity.WORKING:
      return { word: 'running', color: t.running };
    case Activity.BLOCKED:
      return { word: 'waiting', color: t.attention };
    case Activity.FAILED:
      return { word: 'failed', color: t.failed };
    case Activity.MERGING:
      return { word: 'merging', color: t.merging };
    case Activity.RUNNING:
      return { word: 'busy', color: t.dim };
    default:
      return undefined;
  }
}

/** A filter over worktrees, as the desktop sidebar's filter has them. */
export type Filter = 'all' | 'running' | 'waiting' | 'idle';

/**
 * The one filter other than `all` a worktree falls in — the sidebar's
 * `SidebarFilter::of`: anything moving is running, a failure is idle.
 */
export function filterOf(activity: Activity): Exclude<Filter, 'all'> {
  switch (activity) {
    case Activity.WORKING:
    case Activity.RUNNING:
    case Activity.MERGING:
      return 'running';
    case Activity.BLOCKED:
      return 'waiting';
    default:
      return 'idle';
  }
}

/** The colour a worktree's state is drawn in; nothing when it is quiet. */
export function railColor(activity: Activity): string | undefined {
  return stateOf(activity)?.color;
}

/** The detail line takes a colour only when it asks for somebody. */
export function detailColor(activity: Activity): string {
  switch (activity) {
    case Activity.BLOCKED:
      return t.attention;
    case Activity.FAILED:
      return t.failed;
    case Activity.MERGING:
      return t.merging;
    default:
      return t.dim;
  }
}

/** What the row says: the agent's own words, else the state itself. */
export function detail(worktree: WorktreeSummary): string {
  if (worktree.agentState) return sentence(worktree.agentState);
  switch (worktree.activity) {
    case Activity.WORKING:
      return 'Working';
    case Activity.MERGING:
      return 'Merging';
    case Activity.BLOCKED:
      return 'Waiting on you';
    case Activity.FAILED:
      return 'Failed';
    case Activity.RUNNING:
      return 'Running';
    default:
      return worktree.terminals.some((x) => !x.closed) ? 'Ready' : 'Nothing running';
  }
}

function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** A project's colour: the one chosen in ket, else ket's default. */
export function projectColor(project: ProjectSummary): string {
  return /^#[0-9a-f]{6}$/i.test(project.color) ? project.color : PROJECT_COLORS[0];
}

/** A worktree and the project it belongs to, flattened out of a snapshot. */
export interface Located {
  project: ProjectSummary;
  worktree: WorktreeSummary;
}

export function everyWorktree(snapshot: HostSnapshot | null): Located[] {
  return (snapshot?.projects ?? []).flatMap((project) => project.worktrees.map((worktree) => ({ project, worktree })));
}

/** The terminal running a worktree's reporting agent, if there is one. */
export function agentTerminal(worktree: WorktreeSummary) {
  const agent = worktree.agent.toLowerCase();
  return (
    worktree.terminals.find((x) => !x.closed && agent !== '' && x.agent.toLowerCase() === agent) ??
    worktree.terminals.find((x) => !x.closed && x.agent !== '')
  );
}
