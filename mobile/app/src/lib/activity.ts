// How a worktree's state is drawn: the word and colour ket's sidebar gives
// it, the colour its detail line takes, and a phrase when the agent said
// nothing.

import {
  Activity,
  type AsleepTerminal,
  type HostSnapshot,
  type ProjectSummary,
  type TerminalSummary,
  type WorktreeSummary,
} from '@ket/remote';

import { PROJECT_COLORS, agentLabel, t } from './theme';

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

/** A worktree whose agent tab the desktop put to sleep: no terminal, the
 * session kept. Faint, like a row that is only starting. */
export const ASLEEP = { word: 'asleep', color: t.faint };
/** One the phone asked the desktop to wake, until its terminal is there. */
export const WAKING = { word: 'waking', color: t.faint };

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
      return worktree.terminals.some((x) => !x.closed) ? 'Ready' : worktree.asleep.length > 0 ? 'Asleep' : 'Nothing running';
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

/** The agent a worktree's row is marked with: the one whose state it
 * reports, else the agent in one of its terminals — an agent that has not
 * said anything yet still runs in one. Empty for a worktree of shells. */
export function agentOf(worktree: WorktreeSummary): string {
  return worktree.agent || agentTerminal(worktree)?.agent || worktree.asleep[0]?.agent || '';
}

/** The asleep tab a worktree's row wakes: the one of its reporting agent,
 * else the first. */
export function asleepTab(worktree: WorktreeSummary): AsleepTerminal | undefined {
  const agent = worktree.agent.toLowerCase();
  return worktree.asleep.find((x) => agent !== '' && x.agent.toLowerCase() === agent) ?? worktree.asleep[0];
}

/** What an asleep tab is called, as `terminalName` names a terminal. */
export function asleepName(tab: AsleepTerminal): string {
  const title = tab.title.replace(LEADING_GLYPHS, '').trim();
  return title || (tab.agent ? agentLabel(tab.agent) : 'Terminal');
}

/** The terminal running a worktree's reporting agent, if there is one. A
 * tab in Claude's agent view runs `claude` too, but what the worktree reports
 * comes from another: it is the last choice, never the first. */
export function agentTerminal(worktree: WorktreeSummary) {
  const agent = worktree.agent.toLowerCase();
  const open = worktree.terminals.filter((x) => !x.closed && x.agent !== '');
  const ranked = [...open.filter((x) => !x.agentView), ...open.filter((x) => x.agentView)];
  return ranked.find((x) => agent !== '' && x.agent.toLowerCase() === agent) ?? ranked[0];
}

/** The glyphs an agent leads its title with, and the space after them:
 * Claude Code's ✳, a spinner's braille frame, a bullet, an emoji. Symbols
 * and punctuation from U+2000 to U+2BFF and the astral emoji, never ASCII,
 * so a title that starts `~/` or `[1]` keeps its first character. */
const LEADING_GLYPHS = /^\s*(?:[\u2000-\u2bff\ufe0f\u200d]|[\ud83c-\ud83e][\udc00-\udfff])+\s+/;

/** What a terminal is called: the title its program set, else its agent's
 * name. An agent's title loses the glyph it leads with, since the agent's
 * mark already stands there and a spinner's frames would make the label
 * flicker. */
export function terminalName(terminal: TerminalSummary): string {
  // Claude's agent view titles itself after whichever session it last had up.
  if (terminal.agentView) return 'Agent view';
  const title = terminal.agent ? terminal.title.replace(LEADING_GLYPHS, '') : terminal.title;
  return title.trim() || (terminal.agent ? agentLabel(terminal.agent) : 'Terminal');
}
