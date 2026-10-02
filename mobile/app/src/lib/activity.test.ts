import {
  Activity,
  type HostSnapshot,
  type ProjectSummary,
  type TerminalSummary,
  type WorktreeSummary,
} from '@ket/remote';
import { describe, expect, jest, test } from '@jest/globals';

import {
  agentTerminal,
  detail,
  detailColor,
  everyWorktree,
  filterOf,
  projectColor,
  railColor,
  stateOf,
} from './activity';
import { PROJECT_COLORS, t } from './theme';

jest.mock('@ket/remote', () => ({
  Activity: { QUIET: 0, RUNNING: 1, WORKING: 2, MERGING: 3, BLOCKED: 4, FAILED: 5 },
}));

const terminal = (agent: string, closed = false): TerminalSummary =>
  ({ agent, closed }) as TerminalSummary;

type WorktreeFields = Partial<Pick<WorktreeSummary, 'id' | 'agentState' | 'terminals' | 'agent'>>;

const worktree = (activity: Activity, fields: WorktreeFields = {}): WorktreeSummary =>
  ({ activity, agentState: '', agent: '', terminals: [], ...fields }) as WorktreeSummary;

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
});
