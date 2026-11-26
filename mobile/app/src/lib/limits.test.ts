import { type SessionLimit, type WorktreeSummary } from '@ket/remote';
import { describe, expect, jest, test } from '@jest/globals';

import {
  aboutDuration,
  clockOf,
  handChoices,
  isOver,
  limitWords,
  pickAgent,
  resetWords,
  stopSentence,
  stoppedAt,
  waitWords,
  waited,
} from './limits';

jest.mock('@ket/remote', () => ({
  SessionLimit_Scope: { SCOPE_UNSPECIFIED: 0, PLAN: 1, MODEL: 2 },
}));

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

const limit = (since: number, resetsAt: number, scope = 1) =>
  ({ scope, window: 0, sinceMs: BigInt(since), resetsAtMs: BigInt(resetsAt), said: '' }) as SessionLimit;

const worktree = (terminals: object[], asleep: object[] = []) => ({ terminals, asleep }) as unknown as WorktreeSummary;

describe('where a worktree is stopped', () => {
  test('is its newest stop, in an open tab or an asleep one', () => {
    expect(stoppedAt(worktree([{ id: 1n, agent: 'codex', closed: false }]))).toBeUndefined();

    const open = { id: 1n, agent: 'codex', closed: false, limit: limit(100, 0) };
    expect(stoppedAt(worktree([open]))).toMatchObject({ agent: 'codex', terminal: 1n });

    const asleep = { key: 'k', agent: 'claude', limit: limit(200, 0) };
    const newest = stoppedAt(worktree([open], [asleep]));
    expect(newest).toMatchObject({ agent: 'claude' });
    expect(newest?.terminal).toBeUndefined();

    // A closed tab's is nobody's any more.
    expect(stoppedAt(worktree([{ ...open, closed: true }]))).toBeUndefined();
  });
});

describe('a limit', () => {
  test('is over once its reset has passed, and never when none was named', () => {
    expect(isOver(limit(0, 5000), 4999)).toBe(false);
    expect(isOver(limit(0, 5000), 5000)).toBe(true);
    expect(isOver(limit(0, 0), Number.MAX_SAFE_INTEGER)).toBe(false);
  });

  test('is worded as the desktop words it', () => {
    const now = 1_000_000;
    expect(limitWords('Codex', limit(now, now + 2 * HOUR), now)).toBe('Codex has reached its usage limit and resets in about 2 hours.');
    expect(limitWords('Codex', limit(now, 0), now)).toBe('Codex has reached its usage limit.');
    expect(limitWords('Codex', limit(now, now - 1), now)).toBe('Codex has reached its usage limit.');
    expect(limitWords('Claude', limit(now, 0, 2), now)).toBe('Claude has reached its limit for this model.');
    expect(resetWords('Codex')).toBe('Codex’s limit has reset. It can carry on.');
  });

  test('says a wait to the nearest unit a person plans by', () => {
    const cases: [number, string][] = [
      [1, '1 minute'],
      [MINUTE, '1 minute'],
      [41 * MINUTE + 1, '42 minutes'],
      [89 * MINUTE, '89 minutes'],
      [90 * MINUTE, '2 hours'],
      [2 * HOUR + 20 * MINUTE, '2 hours'],
      [35 * HOUR, '35 hours'],
      [36 * HOUR, '2 days'],
      [5 * 24 * HOUR + 3 * HOUR, '5 days'],
    ];
    for (const [ms, said] of cases) expect(aboutDuration(ms)).toBe(said);
  });
});

describe('the limit sheet', () => {
  test('says the wait to the minute, rounded up', () => {
    const cases: [number, string][] = [
      [1, '1 minute'],
      [42 * MINUTE, '42 minutes'],
      [2 * HOUR + 5 * MINUTE + 1, '2 hours 6 minutes'],
      [HOUR, '1 hour'],
      [3 * 24 * HOUR + 4 * HOUR, '3 days 4 hours'],
      [24 * HOUR, '1 day'],
    ];
    for (const [ms, said] of cases) expect(waitWords(ms)).toBe(said);
  });

  test('says what stopped and when it resets in one sentence', () => {
    const now = 1_000_000;
    expect(stopSentence('Codex', limit(now, now + 2 * HOUR + 6 * MINUTE), now)).toBe(
      'Codex has reached its usage limit and resets in 2 hours 6 minutes.',
    );
    expect(stopSentence('Claude', limit(now, 0, 2), now)).toBe('Claude has reached its limit for this model.');
    expect(stopSentence('Codex', limit(now, now - 1), now)).toBe('Codex has reached its usage limit.');
  });

  test('fills the bar with the share of the wait gone', () => {
    expect(waited(limit(0, 1000), 250)).toBe(0.25);
    expect(waited(limit(0, 1000), 2000)).toBe(1);
    expect(waited(limit(500, 1000), 0)).toBe(0);
    expect(waited(limit(0, 0), 250)).toBe(0);
  });

  test('reads the reset on this phone\'s clock', () => {
    expect(clockOf(new Date(2026, 10, 20, 16, 37).getTime())).toMatch(/^4:37\s?pm$/);
  });
});

describe('the agent ket offers', () => {
  const NOW = 1_000_000;
  const snapshot = (fields: object) =>
    ({ agents: ['codex', 'claude', 'opencode'], usage: [], projects: [], ...fields }) as never;

  test('is the first the desktop can start that is another program', () => {
    expect(pickAgent(snapshot({}), 'codex', NOW)).toBe('claude');
    expect(pickAgent(snapshot({}), 'claude', NOW)).toBe('codex');
    expect(pickAgent(snapshot({ agents: ['codex', 'codex:work', 'ollama:qwen3'] }), 'codex', NOW)).toBeUndefined();
    expect(pickAgent(null, 'codex', NOW)).toBeUndefined();
  });

  test('comes from a list of every other program, each with how much it has left', () => {
    const full = { provider: 'Claude', windows: [{ name: 'Session', usedPercent: 100, resetsAtMs: 0n }] };
    expect(handChoices(snapshot({ usage: [full] }), 'codex', NOW)).toEqual([
      { agent: 'claude', spent: false, low: true },
      { agent: 'opencode', spent: false, low: false },
    ]);
    expect(handChoices(null, 'codex', NOW)).toEqual([]);
  });

  test('prefers one with plenty left, and still offers one that reads full', () => {
    const full = { provider: 'Claude', windows: [{ name: 'Session', usedPercent: 100, resetsAtMs: 0n }] };
    expect(pickAgent(snapshot({ usage: [full] }), 'codex', NOW)).toBe('opencode');
    expect(pickAgent(snapshot({ agents: ['codex', 'claude'], usage: [full] }), 'codex', NOW)).toBe('claude');
    // A full reading from before its window reset says nothing now.
    const old = { provider: 'Claude', windows: [{ name: 'Session', usedPercent: 100, resetsAtMs: BigInt(NOW - 1) }] };
    expect(handChoices(snapshot({ usage: [old] }), 'codex', NOW)[0]).toEqual({ agent: 'claude', spent: false, low: false });
  });

  test('passes over one stopped at its plan, not one with a model spent', () => {
    const modelOnly = {
      id: 'p',
      worktrees: [worktree([{ id: 2n, agent: 'claude', closed: false, limit: limit(NOW - 1000, NOW + HOUR, 2) }])],
    };
    expect(handChoices(snapshot({ projects: [modelOnly] }), 'codex', NOW)[0]).toEqual({ agent: 'claude', spent: false, low: true });

    const stoppedClaude = {
      id: 'p',
      worktrees: [
        worktree([{ id: 2n, agent: 'claude', closed: false, limit: limit(NOW - 1000, NOW + HOUR) }]),
      ],
    };
    expect(pickAgent(snapshot({ projects: [stoppedClaude] }), 'codex', NOW)).toBe('opencode');
    // Once its limit has reset, it is offered again.
    expect(pickAgent(snapshot({ projects: [stoppedClaude] }), 'codex', NOW + 2 * HOUR)).toBe('claude');
  });
});
