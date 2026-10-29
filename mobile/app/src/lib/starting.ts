// New work this phone asked for, between asking and its chat being real.
//
// The desktop answers as soon as it has the request, then makes the worktree
// — fetching the base, checking out, opening the agent's tab — which can take
// a while, and it says nothing of its progress. What the phone can tell is
// read off the snapshot: a worktree the project did not have when it asked,
// then that worktree's agent terminal. Two stages, and then the chat.

import type { HostSnapshot, ProjectSummary, WorktreeSummary } from '@ket/remote';

import type { Starting } from './connection';
import { agentLabel } from './theme';

/** Where new work has got to, as far as the snapshot says. */
export type Progress =
  /** The desktop has the request; no worktree yet. */
  | { step: 'asked' }
  /** The worktree is there, and its agent's tab is not open yet. */
  | { step: 'worktree'; project: ProjectSummary; worktree: WorktreeSummary }
  /** Its agent's tab is open: the chat is that terminal's. */
  | { step: 'ready'; project: ProjectSummary; worktree: WorktreeSummary; terminal: bigint };

/** The branch the desktop names new work's worktree after: the prompt's
 * first few words, then four characters that keep two alike apart. The same
 * rule as `branch_for` in ket-ui's phone_work.rs — what lets the phone tell
 * its worktree from one made on the desktop meanwhile, or from another task
 * it asked for in the same breath. */
export function branchStem(prompt: string): string {
  const words = prompt
    .split(/[^A-Za-z0-9]+/)
    .filter((word) => word !== '')
    .slice(0, 5)
    .map((word) => word.toLowerCase());
  const stem = words.join('-').slice(0, 40).replace(/-+$/, '');
  return stem === '' ? 'phone-task' : stem;
}

/** The worktree `work` asked for, once `project` lists it: one that was not
 * there when it asked, on the branch its prompt names. */
export function worktreeOf(work: Starting, project: ProjectSummary | undefined): WorktreeSummary | undefined {
  const stem = `${branchStem(work.prompt)}-`;
  return project?.worktrees.find(
    (w) => !work.before.includes(w.id) && w.branch.startsWith(stem) && w.branch.length === stem.length + 4,
  );
}

/** Where `work` has got to, as far as the snapshot says. */
export function progressOf(work: Starting, snapshot: HostSnapshot | null): Progress {
  const project = snapshot?.projects.find((p) => p.id === work.project);
  const worktree = worktreeOf(work, project);
  if (!project || !worktree) return { step: 'asked' };
  // Its agent's tab, not a shell the desktop opened alongside: the chat for
  // a shell reads no conversation, and a message typed there would go to
  // the shell rather than the agent still opening.
  const terminal = worktree.terminals.find((x) => !x.closed && x.agent !== '');
  if (!terminal) return { step: 'worktree', project, worktree };
  return { step: 'ready', project, worktree, terminal: terminal.id };
}

/** What the desktop is doing, in a few words, for the working row and the
 * list's row: `mac` is the desktop's name. */
export function stageText(progress: Progress, mac: string, agent: string): string {
  if (progress.step === 'asked') return `Making the worktree on ${mac}`;
  const name = progress.worktree.agent || agent;
  return `Starting ${name ? agentLabel(name) : 'the agent'}`;
}

/** Which of the three steps — asked, worktree, agent — each segment of a
 * progress line stands for is `done`, `live` or still to come. */
export type Segment = 'done' | 'live' | 'todo';

export function segments(progress: Progress, stalled: boolean): [Segment, Segment, Segment] {
  if (stalled) return ['done', 'todo', 'todo'];
  if (progress.step === 'asked') return ['done', 'live', 'todo'];
  if (progress.step === 'worktree') return ['done', 'done', 'live'];
  return ['done', 'done', 'done'];
}

/** The first line of what an agent was asked, to name the row — and the
 * chat — standing in for its worktree. */
export function firstLine(prompt: string): string {
  const line = prompt.trim().split('\n')[0] ?? '';
  return line.length > 72 ? `${line.slice(0, 71)}…` : line;
}

/** How long since `at`, in whole seconds, as the row and the chat count it. */
export function elapsed(at: number, now: number): string {
  return `${Math.max(0, Math.floor((now - at) / 1000))} s`;
}

/** A message already on its way to a chat as it opens — the task, when the
 * chat that stood in for the work becomes the worktree's own — and the draft
 * typed meanwhile. */
export interface Seed {
  text: string;
  draft: string;
}

/** The worktree the starting screen at `path` is about, once `snapshot`
 * lists it: what the sidebar marks as open, as it does any other screen of a
 * worktree's (see `worktreeAt` in lib/layout). `terminal` is the one its
 * address names once it is that terminal's chat, which outlasts the request
 * when the page is loaded again. `null` for any other screen. */
export function startingAt(
  path: string,
  hostId: string,
  asked: Starting[],
  snapshot: HostSnapshot | null,
  terminal?: string,
): string | null {
  const [, kind, host, at] = path.split('/');
  if (kind !== 'starting' || host !== hostId) return null;
  if (terminal) {
    const worktrees = snapshot?.projects.flatMap((p) => p.worktrees) ?? [];
    return worktrees.find((w) => w.terminals.some((x) => x.id.toString() === terminal))?.id ?? null;
  }
  const work = asked.find((w) => String(w.at) === at);
  const progress = work ? progressOf(work, snapshot) : undefined;
  return progress && progress.step !== 'asked' ? progress.worktree.id : null;
}
