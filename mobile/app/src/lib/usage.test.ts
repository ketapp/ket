import {
  UsageStatus,
  type HostSnapshot,
  type ProjectSummary,
  type ProviderUsage,
  type TerminalSummary,
  type UsageWindow,
  type WorktreeSummary,
} from '@ket/remote';
import { describe, expect, jest, test } from '@jest/globals';

import {
  compactDuration,
  hasReading,
  metadata,
  pressure,
  reporting,
  resetsIn,
  sessions,
  tightest,
  usedOf,
} from './usage';
import { t } from './theme';

jest.mock('@ket/remote', () => ({
  UsageStatus: { UNSPECIFIED: 0, FRESH: 1, STALE: 2, UNAVAILABLE: 3, UNSUPPORTED: 4 },
}));

const usageWindow = (usedPercent: number, resetsAtMs = 0n) =>
  ({ usedPercent, resetsAtMs }) as UsageWindow;

const provider = (status = UsageStatus.FRESH, used: number[] = []) =>
  ({ status, windows: used.map((value) => usageWindow(value)) }) as ProviderUsage;

describe('usage values', () => {
  test.each([
    [69.9, undefined],
    [70, t.quotaWarm],
    [80, t.quotaHot],
    [90, t.quotaCritical],
  ])('colours pressure at %s percent', (used, color) => {
    expect(pressure(used)).toBe(color);
  });

  test.each([
    [-1, 0],
    [42.5, 42.5],
    [101, 100],
    [Number.NaN, 0],
    [Number.POSITIVE_INFINITY, 0],
  ])('normalizes %s to %s', (used, safe) => {
    expect(usedOf(usageWindow(used))).toBe(safe);
  });

  test('selects the window with the highest safe use', () => {
    const item = provider(UsageStatus.FRESH, [20, 91, 80]);
    expect(tightest(item)).toBe(item.windows[1]);
    expect(tightest(provider())).toBeUndefined();
  });
});

describe('usage labels', () => {
  test.each([
    [42 * 60_000, '42m'],
    [(3 * 60 + 12) * 60_000, '3h 12m'],
    [(6 * 24 * 60 + 23 * 60) * 60_000, '6d 23h'],
  ])('formats %s ms as %s', (ms, label) => {
    expect(compactDuration(ms)).toBe(label);
  });

  test('shows only future resets', () => {
    expect(resetsIn(usageWindow(0, 1_180_000n), 1_000_000)).toBe('3m');
    expect(resetsIn(usageWindow(0, 999_999n), 1_000_000)).toBeUndefined();
  });

  test('combines plan and reading age', () => {
    const item = { plan: 'Pro', fetchedAtMs: 820_000n } as ProviderUsage;
    expect(metadata(item, 1_000_000)).toBe('Pro · 3m ago');
    expect(metadata({ plan: '', fetchedAtMs: 999_999n } as ProviderUsage, 1_000_000)).toBe('just now');
    expect(metadata({ plan: '', fetchedAtMs: 0n } as ProviderUsage, 1_000_000)).toBeUndefined();
  });
});

describe('snapshot usage', () => {
  test.each([
    [UsageStatus.FRESH, true],
    [UsageStatus.STALE, true],
    [UsageStatus.UNAVAILABLE, false],
    [UsageStatus.UNSUPPORTED, false],
  ])('recognizes readable status %s', (status, expected) => {
    expect(hasReading(provider(status))).toBe(expected);
  });

  test('reports supported statuses, including unavailable reads', () => {
    const fresh = provider(UsageStatus.FRESH);
    const unavailable = provider(UsageStatus.UNAVAILABLE);
    const unsupported = provider(UsageStatus.UNSUPPORTED);
    const snapshot = { usage: [fresh, unavailable, unsupported] } as HostSnapshot;
    expect(reporting(snapshot)).toEqual([fresh, unavailable]);
    expect(reporting(null)).toEqual([]);
  });

  test('counts only open terminals for the requested agent', () => {
    const worktree = {
      terminals: [
        { agent: 'Codex', closed: false } as TerminalSummary,
        { agent: 'codex', closed: true } as TerminalSummary,
        { agent: 'claude', closed: false } as TerminalSummary,
      ],
    } as WorktreeSummary;
    const project = { worktrees: [worktree] } as ProjectSummary;
    const snapshot = { projects: [project] } as HostSnapshot;
    expect(sessions(snapshot, 'CODEX')).toBe(1);
    expect(sessions(snapshot, 'gemini')).toBe(0);
    expect(sessions(null, 'codex')).toBe(0);
  });
});
