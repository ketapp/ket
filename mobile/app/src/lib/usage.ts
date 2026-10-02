// The agents' plan usage, as the desktop's usage popover draws it
// (crates/ket-ui/src/status_bar.rs): the tightest window per agent, a ramp
// that only colours a quota worth noticing, and a failed read that is never
// shown as a percentage.

import { type HostSnapshot, type ProviderUsage, type UsageWindow, UsageStatus } from '@ket/remote';
import { useEffect, useState } from 'react';

import { t } from './theme';

/** A quota's colour at `used` percent, or none while it is nothing to note. */
export function pressure(used: number): string | undefined {
  if (used >= 90) return t.quotaCritical;
  if (used >= 80) return t.quotaHot;
  if (used >= 70) return t.quotaWarm;
  return undefined;
}

/** A window's usage, safe to draw: 0 to 100. */
export function usedOf(window: UsageWindow): number {
  return Number.isFinite(window.usedPercent) ? Math.min(100, Math.max(0, window.usedPercent)) : 0;
}

/** The window that will stop the agent first. */
export function tightest(provider: ProviderUsage): UsageWindow | undefined {
  return provider.windows.reduce<UsageWindow | undefined>((a, b) => (a && usedOf(a) >= usedOf(b) ? a : b), undefined);
}

/** A duration as the two units anyone reads: `42m`, `3h 12m`, `6d 23h`. */
export function compactDuration(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

/** How long until a window resets, or nothing when unknown or past. */
export function resetsIn(window: UsageWindow, now: number): string | undefined {
  const at = Number(window.resetsAtMs);
  return at > now ? compactDuration(at - now) : undefined;
}

/** The plan it was read on and how old the reading is. */
export function metadata(provider: ProviderUsage, now: number): string | undefined {
  const parts: string[] = [];
  if (provider.plan) parts.push(provider.plan);
  const fetched = Number(provider.fetchedAtMs);
  if (fetched > 0) {
    const age = Math.max(0, now - fetched);
    parts.push(age < 60_000 ? 'just now' : `${compactDuration(age)} ago`);
  }
  return parts.length > 0 ? parts.join(' · ') : undefined;
}

/** Whether a provider has numbers to show, current or not. */
export function hasReading(provider: ProviderUsage): boolean {
  return provider.status === UsageStatus.FRESH || provider.status === UsageStatus.STALE;
}

/**
 * The providers worth a line on the strip: an agent with no quota to report
 * has nothing to say there, and is left to the full view to explain.
 */
export function reporting(snapshot: HostSnapshot | null): ProviderUsage[] {
  return (snapshot?.usage ?? []).filter((p) => p.status !== UsageStatus.UNSUPPORTED && p.status !== UsageStatus.UNSPECIFIED);
}

/** Sessions of this agent running on the desktop now. */
export function sessions(snapshot: HostSnapshot | null, provider: string): number {
  const name = provider.toLowerCase();
  let n = 0;
  for (const project of snapshot?.projects ?? []) {
    for (const worktree of project.worktrees) {
      n += worktree.terminals.filter((x) => !x.closed && x.agent.toLowerCase() === name).length;
    }
  }
  return n;
}

/** The time, again every half minute: resets count down between snapshots. */
export function useNow(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}
