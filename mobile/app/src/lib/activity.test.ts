import {
  Activity,
  type AsleepTerminal,
  type HostSnapshot,
  type ProjectSummary,
  type TerminalSummary,
  type WorktreeSummary,
} from '@ket/remote';
import { describe, expect, jest, test } from '@jest/globals';

import {
  agentOf,
  agentTerminal,
  asleepName,
  asleepTab,
  detail,
  detailColor,
  everyWorktree,
  filterOf,
  projectColor,
  railColor,
  stateOf,
  terminalName,
} from './activity';
import { PROJECT_COLORS, agentLabel, t } from './theme';

jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
}));

const terminal = (agent: string, closed = false): TerminalSummary =>
  ({ agent, closed }) as TerminalSummary;

type WorktreeFields = Partial<Pick<WorktreeSummary, 'id' | 'agentState' | 'terminals' | 'agent' | 'asleep'>>;

const worktree = (activity: Activity, fields: WorktreeFields = {}): WorktreeSummary =>
  ({ activity, agentState: '', agent: '', terminals: [], asleep: [], ...fields }) as WorktreeSummary;

const asleep = (agent: string, title = '', key = agent): AsleepTerminal =>
  ({ key, title, agent, sinceMs: 0n, freedBytes: 0n }) as AsleepTerminal;

const filterCases: [Activity, ReturnType<typeof filterOf>][] = [
  [Activity.WORKING, 'running'],
  [Activity.RUNNING, 'running'],
  [Activity.MERGING, 'running'],
  [Activity.BLOCKED, 'waiting'],
  [Activity.FAILED, 'idle'],
  [Activity.QUIET, 'idle'],
];

describe('activity presentation', () => {
  test.each([
    [Activity.WORKING, 'running', t.running],
    [Activity.BLOCKED, 'waiting', t.attention],
    [Activity.FAILED, 'failed', t.failed],
    [Activity.MERGING, 'merging', t.merging],
    [Activity.RUNNING, 'busy', t.dim],
  ])('maps %s to its label and colour', (activity, word, color) => {
    expect(stateOf(activity)).toEqual({ word, color });
    expect(railColor(activity)).toBe(color);
  });

  test('keeps quiet worktrees unlit', () => {
    expect(stateOf(Activity.QUIET)).toBeUndefined();
    expect(railColor(Activity.QUIET)).toBeUndefined();
  });

  test.each(filterCases)('puts %s in the %s filter', (activity, filter) => {
    expect(filterOf(activity)).toBe(filter);
  });

  test('uses agent text before fallback state text', () => {
    expect(detail(worktree(Activity.BLOCKED, { agentState: 'waiting for approval' }))).toBe('Waiting for approval');
    expect(detail(worktree(Activity.BLOCKED))).toBe('Waiting on you');
    expect(detail(worktree(Activity.QUIET, { terminals: [terminal('', false)] }))).toBe('Ready');
    expect(detail(worktree(Activity.QUIET, { terminals: [terminal('', true)] }))).toBe('Nothing running');
  });

  test('colours only noteworthy detail states', () => {
    expect(detailColor(Activity.BLOCKED)).toBe(t.attention);
    expect(detailColor(Activity.FAILED)).toBe(t.failed);
    expect(detailColor(Activity.MERGING)).toBe(t.merging);
    expect(detailColor(Activity.WORKING)).toBe(t.dim);
  });

  test('accepts six-digit project colours and rejects malformed values', () => {
    expect(projectColor({ color: '#aBc123' } as ProjectSummary)).toBe('#aBc123');
    expect(projectColor({ color: '#abc' } as ProjectSummary)).toBe(PROJECT_COLORS[0]);
    expect(projectColor({ color: 'red' } as ProjectSummary)).toBe(PROJECT_COLORS[0]);
  });
});

describe('snapshot helpers', () => {
  test('flattens worktrees with their projects in protocol order', () => {
    const first = worktree(Activity.QUIET, { id: 'w1' });
    const second = worktree(Activity.WORKING, { id: 'w2' });
    const project = { id: 'p1', worktrees: [first, second] } as ProjectSummary;
    const snapshot = { projects: [project] } as HostSnapshot;

    expect(everyWorktree(snapshot)).toEqual([
      { project, worktree: first },
      { project, worktree: second },
    ]);
    expect(everyWorktree(null)).toEqual([]);
  });

  test('finds the open terminal for the reporting agent case-insensitively', () => {
    const fallback = terminal('claude');
    const match = terminal('CoDeX');
    const tree = worktree(Activity.WORKING, {
      agent: 'codex',
      terminals: [terminal('codex', true), fallback, match],
    });

    expect(agentTerminal(tree)).toBe(match);
  });

  test('falls back to any open agent terminal, never a shell or closed terminal', () => {
    const fallback = terminal('claude');
    expect(agentTerminal(worktree(Activity.WORKING, { agent: 'codex', terminals: [terminal(''), fallback] }))).toBe(
      fallback,
    );
    expect(agentTerminal(worktree(Activity.QUIET, { terminals: [terminal(''), terminal('codex', true)] }))).toBeUndefined();
  });

  test('marks a row with the reporting agent, else the one in its terminals', () => {
    expect(agentOf(worktree(Activity.WORKING, { agent: 'claude', terminals: [terminal('codex')] }))).toBe('claude');
    expect(agentOf(worktree(Activity.QUIET, { terminals: [terminal(''), terminal('codex')] }))).toBe('codex');
    expect(agentOf(worktree(Activity.QUIET, { terminals: [terminal('')] }))).toBe('');
  });

  test('names a terminal without the glyph its agent leads the title with', () => {
    const named = (agent: string, title: string) => terminalName({ agent, title } as TerminalSummary);
    expect(named('claude', '✳ design canvas')).toBe('design canvas');
    expect(named('claude', '✳\ufe0f design canvas')).toBe('design canvas');
    expect(named('codex', '⠹ Review changes | ket')).toBe('Review changes | ket');
    expect(named('codex', '🤖 Review')).toBe('Review');
    // Nothing but the glyph: the agent's own name.
    expect(named('claude', '✳ ')).toBe('Claude');
    expect(named('opencode', '')).toBe('OpenCode');
    // A glyph with no space after it is part of the title, and a shell's
    // title is its own, whatever it starts with.
    expect(named('claude', '→next')).toBe('→next');
    expect(named('', '⠹ make')).toBe('⠹ make');
    expect(named('', '~/dev/ket')).toBe('~/dev/ket');
    expect(named('', '')).toBe('Terminal');
  });
});

describe('asleep tabs', () => {
  test('says a worktree is asleep when nothing runs but a tab sleeps', () => {
    expect(detail(worktree(Activity.QUIET))).toBe('Nothing running');
    expect(detail(worktree(Activity.QUIET, { asleep: [asleep('claude')] }))).toBe('Asleep');
    expect(detail(worktree(Activity.QUIET, { asleep: [asleep('claude')], terminals: [terminal('claude')] }))).toBe('Ready');
    expect(agentOf(worktree(Activity.QUIET, { asleep: [asleep('claude')] }))).toBe('claude');
  });

  test("wakes the reporting agent's tab first, else the first", () => {
    const tabs = [asleep('codex', '', 'c'), asleep('claude', '', 'k')];
    expect(asleepTab(worktree(Activity.QUIET, { agent: 'Claude', asleep: tabs }))?.key).toBe('k');
    expect(asleepTab(worktree(Activity.QUIET, { asleep: tabs }))?.key).toBe('c');
    expect(asleepTab(worktree(Activity.QUIET))).toBeUndefined();
  });

  test('names an asleep tab as a terminal is named', () => {
    expect(asleepName(asleep('claude', '✳ Fixing the build'))).toBe('Fixing the build');
    expect(asleepName(asleep('codex'))).toBe(agentLabel('codex'));
    expect(asleepName(asleep('', ''))).toBe('Terminal');
  });
});
