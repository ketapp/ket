/**
 * A session stopped because its plan ran out, read off a desktop's snapshot.
 *
 * The desktop marks the session (`TerminalSummary.limit`, since 1.38) from
 * the turn that was refused until the session gets going again, and says
 * when the limit resets where anything told it. Whether it has reset is that
 * time against the clock, worked out here, so one mark reads as "it's out"
 * and later as "it's back" with nothing more sent.
 *
 * The words are the desktop's own (`limit_words` and `about` in
 * `crates/ket-core/src/host/phones/notices.rs`): a notice the phone posts
 * and one the desktop pushes say the same thing. Change one and change the
 * other.
 */
import { type HostSnapshot, type SessionLimit, SessionLimit_Scope, type WorktreeSummary } from '@ket/remote';

/** The stops whose limit sheet was closed, by `limitKey`, for as long as
 * the app runs: coming back to the chat does not put it up again. */
export const closedSheets = new Set<string>();

/** The key of a stop's sheet (`useLimitSheet`): its desktop, its session,
 * and when it stopped, so the next stop of the same session comes up again. */
export function limitKey(host: string, terminal: bigint, limit: SessionLimit): string {
  return `${host}:${terminal.toString()}:${limit.sinceMs.toString()}`;
}

/** Puts the sheet for the stop named `key` back up when its chat next
 * draws: Switch, on the Usage tab, opens the chat at it. */
export function reopenLimitSheet(key: string): void {
  closedSheets.delete(key);
}

/** Where a worktree is stopped: the limit, whose it is, and its chat while its tab is open. */
export interface StoppedAt {
  limit: SessionLimit;
  agent: string;
  /** The terminal whose chat shows it; none for a tab that is asleep. */
  terminal?: bigint;
}

/** The limit one of `worktree`'s sessions is stopped at: the newest stop, in
 * an open tab or an asleep one. */
export function stoppedAt(worktree: WorktreeSummary): StoppedAt | undefined {
  const stops: StoppedAt[] = [];
  for (const terminal of worktree.terminals) {
    if (!terminal.closed && terminal.limit) {
      stops.push({ limit: terminal.limit, agent: terminal.agent, terminal: terminal.id });
    }
  }
  for (const tab of worktree.asleep) {
    if (tab.limit) stops.push({ limit: tab.limit, agent: tab.agent });
  }
  let newest: StoppedAt | undefined;
  for (const stop of stops) {
    if (!newest || stop.limit.sinceMs >= newest.limit.sinceMs) newest = stop;
  }
  return newest;
}

/** Whether `limit` has reset by `now`. One with no reset named never has. */
export function isOver(limit: SessionLimit, now: number): boolean {
  return limit.resetsAtMs !== 0n && Number(limit.resetsAtMs) <= now;
}

/** What a stop at `limit` says. */
export function limitWords(who: string, limit: SessionLimit, now: number): string {
  const what =
    limit.scope === SessionLimit_Scope.MODEL
      ? `${who} has reached its limit for this model`
      : `${who} has reached its usage limit`;
  const left = Number(limit.resetsAtMs) - now;
  return limit.resetsAtMs !== 0n && left > 0 ? `${what} and resets in about ${aboutDuration(left)}.` : `${what}.`;
}

/** What a limit having reset says. */
export function resetWords(who: string): string {
  return `${who}’s limit has reset. It can carry on.`;
}

/** A wait of `ms`, to the nearest unit a person plans by. */
export function aboutDuration(ms: number): string {
  const minutes = Math.max(1, Math.ceil(ms / 60_000));
  const plural = (count: number, unit: string) => (count === 1 ? `1 ${unit}` : `${count.toString()} ${unit}s`);
  if (minutes < 90) return plural(minutes, 'minute');
  if (minutes < 36 * 60) return plural(Math.floor((minutes + 30) / 60), 'hour');
  return plural(Math.floor((minutes + 12 * 60) / (24 * 60)), 'day');
}

/** A wait of `ms` in words, to the minute, rounded up so it never says
 * nothing is left: `2 hours 6 minutes`, `42 minutes`, `3 days 4 hours`. */
export function waitWords(ms: number): string {
  const minutes = Math.max(1, Math.ceil(ms / 60_000));
  const unit = (count: number, name: string) => (count === 1 ? `1 ${name}` : `${count.toString()} ${name}s`);
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return hours > 0 ? `${unit(days, 'day')} ${unit(hours, 'hour')}` : unit(days, 'day');
  if (hours > 0) return mins > 0 ? `${unit(hours, 'hour')} ${unit(mins, 'minute')}` : unit(hours, 'hour');
  return unit(mins, 'minute');
}

/** The one sentence the limit sheet says: what stopped, and when it resets. */
export function stopSentence(who: string, limit: SessionLimit, now: number): string {
  const what =
    limit.scope === SessionLimit_Scope.MODEL ? `${who} has reached its limit for this model` : `${who} has reached its usage limit`;
  const left = Number(limit.resetsAtMs) - now;
  return limit.resetsAtMs !== 0n && left > 0 ? `${what} and resets in ${waitWords(left)}.` : `${what}.`;
}

/** How much of the wait has gone, 0 to 1, for the bar under that sentence:
 * from the stop to the reset. */
export function waited(limit: SessionLimit, now: number): number {
  const since = Number(limit.sinceMs);
  const resets = Number(limit.resetsAtMs);
  if (limit.resetsAtMs === 0n || resets <= since) return 0;
  return Math.min(1, Math.max(0, (now - since) / (resets - since)));
}

/** The harness of an agent's name: `opencode:ollama/qwen` runs `opencode`. */
function harnessOf(agent: string): string {
  return (agent.split(':')[0] ?? '').toLowerCase();
}

/** How much `agent` has left as far as the desktop knows. `spent`: a
 * session of it is stopped at its plan's limit, which has not reset. `low`:
 * a window of its provider's usage reads full, or one of its models is
 * spent — it may well still take the task, on another model or on a
 * reading from before a reset, so it is still offered, and said. */
function left(snapshot: HostSnapshot, agent: string, now: number): { spent: boolean; low: boolean } {
  const harness = harnessOf(agent);
  const stops = snapshot.projects.flatMap((project) =>
    project.worktrees.flatMap((worktree) => {
      const stop = stoppedAt(worktree);
      return stop !== undefined && harnessOf(stop.agent) === harness && !isOver(stop.limit, now) ? [stop.limit] : [];
    }),
  );
  const spent = stops.some((limit) => limit.scope !== SessionLimit_Scope.MODEL);
  const full = snapshot.usage
    .filter((provider) => provider.provider.toLowerCase() === harness)
    .some((provider) =>
      provider.windows.some(
        (window) => window.usedPercent >= 99 && (window.resetsAtMs === 0n || Number(window.resetsAtMs) > now),
      ),
    );
  return { spent, low: !spent && (full || stops.length > 0) };
}

/** The agents that could take over from `stopped`, in the desktop's own
 * order: every one it can start that is another program, each with how much
 * it has left. A bare Ollama chat is no coding agent, so never listed. */
export function handChoices(
  snapshot: HostSnapshot | null,
  stopped: string,
  now: number = Date.now(),
): { agent: string; spent: boolean; low: boolean }[] {
  if (!snapshot) return [];
  const own = harnessOf(stopped);
  return snapshot.agents
    .filter((agent) => harnessOf(agent) !== own && harnessOf(agent) !== 'ollama')
    .map((agent) => ({ agent, ...left(snapshot, agent, now) }));
}

/** The agent ket offers to take over from `stopped`: the first of
 * `handChoices` with plenty left, else the first not spent. */
export function pickAgent(snapshot: HostSnapshot | null, stopped: string, now: number = Date.now()): string | undefined {
  const choices = handChoices(snapshot, stopped, now);
  return (choices.find((choice) => !choice.spent && !choice.low) ?? choices.find((choice) => !choice.spent))?.agent;
}

/** When a limit resets, as the clock on this phone reads: `4:37 pm`. */
export function clockOf(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }).toLowerCase();
}
