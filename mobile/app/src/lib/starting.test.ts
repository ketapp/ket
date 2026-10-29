import { describe, expect, test } from '@jest/globals';

import { branchStem, elapsed, firstLine, progressOf, segments, stageText, startingAt, worktreeOf } from './starting';
import type { Starting } from './connection';

const work: Starting = { project: 'proj', prompt: 'Fix the login redirect\nKeep the path', agent: 'claude', at: 1000, before: ['old'] };

const snapshot = (worktrees: object[]) =>
  ({ projects: [{ id: 'proj', name: 'Project', worktrees }], usage: [] }) as never;

describe('branchStem', () => {
  test('names the branch as the desktop does: five words, lower case, forty characters', () => {
    expect(branchStem('Fix the login redirect: after signing in with Google')).toBe('fix-the-login-redirect-after');
    expect(branchStem('  Add   tests!  ')).toBe('add-tests');
    expect(branchStem('Rename everything in src/components/Attachments.tsx to use the new icons')).toBe(
      'rename-everything-in-src-components',
    );
    // Cut at forty characters, and never left ending in a dash.
    expect(branchStem('abcdefghijklmnopqrst abcdefghijklmnopqrs x')).toBe('abcdefghijklmnopqrst-abcdefghijklmnopqrs');
    expect(branchStem('abcdefghijklmnopqrstu abcdefghijklmnopqr x')).toBe('abcdefghijklmnopqrstu-abcdefghijklmnopqr');
    expect(branchStem('!!! ???')).toBe('phone-task');
  });
});

describe('worktreeOf and progressOf', () => {
  const branch = 'fix-the-login-redirect-keep-k3f9';

  test('is asked until the project lists a worktree on the prompt\'s branch that it did not have', () => {
    expect(progressOf(work, null)).toEqual({ step: 'asked' });
    expect(progressOf(work, snapshot([{ id: 'old', branch, terminals: [] }]))).toEqual({ step: 'asked' });
    // Another new worktree — the desktop's, or another task's — is not this one.
    expect(progressOf(work, snapshot([{ id: 'other', branch: 'other-thing-a1b2', terminals: [] }]))).toEqual({ step: 'asked' });
    expect(progressOf(work, snapshot([{ id: 'other', branch: 'fix-the-login-redirect-keep-k3f9-extra', terminals: [] }]))).toEqual({ step: 'asked' });
    const made = { id: 'new', branch, terminals: [] };
    const project = { id: 'proj', name: 'Project', worktrees: [{ id: 'other', branch: 'other-thing-a1b2', terminals: [] }, made] } as never;
    expect(worktreeOf(work, project)).toBe(made);
  });

  test('is the new worktree until an agent terminal is open in it, a shell not counting', () => {
    const made = { id: 'new', name: 'fix-the-login', branch, agent: '', terminals: [{ id: 3n, agent: '', closed: false }] };
    expect(progressOf(work, snapshot([{ id: 'old', branch: 'main', terminals: [] }, made])).step).toBe('worktree');
    const bare = { ...made, terminals: [] };
    expect(progressOf(work, snapshot([bare])).step).toBe('worktree');
    const closed = { ...made, terminals: [{ id: 3n, agent: 'claude', closed: true }] };
    expect(progressOf(work, snapshot([closed])).step).toBe('worktree');
  });

  test('is ready at the agent terminal, not a shell beside it', () => {
    const made = {
      id: 'new',
      name: 'fix-the-login',
      branch,
      agent: 'claude',
      terminals: [
        { id: 3n, agent: '', closed: false },
        { id: 4n, agent: 'claude', closed: false },
      ],
    };
    expect(progressOf(work, snapshot([made]))).toMatchObject({ step: 'ready', terminal: 4n });
  });
});

describe('stageText and segments', () => {
  test('say what the desktop is doing, by step', () => {
    expect(stageText({ step: 'asked' }, 'Studio Mac', 'claude')).toBe('Making the worktree on Studio Mac');
    const worktree = { id: 'new', name: 'n', agent: 'codex', terminals: [] } as never;
    const project = { id: 'proj', name: 'Project', worktrees: [worktree] } as never;
    expect(stageText({ step: 'worktree', project, worktree }, 'Mac', '')).toBe('Starting Codex');
    expect(segments({ step: 'asked' }, false)).toEqual(['done', 'live', 'todo']);
    expect(segments({ step: 'worktree', project, worktree }, false)).toEqual(['done', 'done', 'live']);
    expect(segments({ step: 'asked' }, true)).toEqual(['done', 'todo', 'todo']);
  });

  test('names the row by the first line, and counts whole seconds', () => {
    expect(firstLine(work.prompt)).toBe('Fix the login redirect');
    expect(firstLine(`${'x'.repeat(80)}\nmore`)).toBe(`${'x'.repeat(71)}…`);
    expect(elapsed(1000, 4900)).toBe('3 s');
    expect(elapsed(5000, 4000)).toBe('0 s');
  });
});

describe('startingAt', () => {
  test('is the worktree a starting screen is about, once it is listed', () => {
    const made = snapshot([{ id: 'new', branch: 'fix-the-login-redirect-keep-k3f9', terminals: [] }]);
    expect(startingAt('/starting/host/1000', 'host', [work], made)).toBe('new');
    // Not made yet, another request, another desktop, another screen.
    expect(startingAt('/starting/host/1000', 'host', [work], snapshot([]))).toBeNull();
    expect(startingAt('/starting/host/2000', 'host', [work], made)).toBeNull();
    expect(startingAt('/starting/other/1000', 'host', [work], made)).toBeNull();
    expect(startingAt('/session/host/1000', 'host', [work], made)).toBeNull();
    // Become its terminal's chat and loaded again: the request is gone, the
    // terminal in the address is not.
    const opened = snapshot([{ id: 'new', branch: 'x', terminals: [{ id: 3n }] }]);
    expect(startingAt('/starting/host/1000', 'host', [], opened, '3')).toBe('new');
    expect(startingAt('/starting/host/1000', 'host', [], opened, '4')).toBeNull();
  });
});
