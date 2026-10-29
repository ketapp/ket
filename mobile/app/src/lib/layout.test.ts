import { beforeEach, describe, expect, jest, test } from '@jest/globals';
import type { HostSnapshot } from '@ket/remote';
import { showBeside, tabAt, worktreeAt } from './layout';

const mockRouter = { canDismiss: jest.fn(() => true), dismissAll: jest.fn(), replace: jest.fn() };
jest.mock('expo-router', () => ({
  router: {
    canDismiss: () => mockRouter.canDismiss(),
    dismissAll: () => mockRouter.dismissAll(),
    replace: (href: string) => mockRouter.replace(href),
  },
}));

const snapshot = {
  projects: [
    { worktrees: [{ id: 'one', terminals: [{ id: 3n }] }] },
    { worktrees: [{ id: 'a/b', terminals: [{ id: 7n }, { id: 9n }] }] },
  ],
} as unknown as HostSnapshot;

beforeEach(() => {
  jest.clearAllMocks();
});

describe('the wide layout', () => {
  test('finds the worktree a screen is about', () => {
    expect(worktreeAt('/session/host/9', 'host', snapshot)).toBe('a/b');
    expect(worktreeAt('/terminal/host/3', 'host', snapshot)).toBe('one');
    expect(worktreeAt('/agent-session/host/9/abc-123', 'host', snapshot)).toBe('a/b');
    expect(worktreeAt('/worktree/host/a%2Fb', 'host', snapshot)).toBe('a/b');
    expect(worktreeAt('/diff/host/one', 'host', snapshot)).toBe('one');
    expect(worktreeAt('/session/host/42', 'host', snapshot)).toBeNull();
    expect(worktreeAt('/session/other/9', 'host', snapshot)).toBeNull();
    expect(worktreeAt('/needs', 'host', snapshot)).toBeNull();
    expect(worktreeAt('/backlog/host/project', 'host', snapshot)).toBeNull();
    expect(worktreeAt('/session/host/9', 'host', null)).toBeNull();
  });

  test('puts every path under a tab', () => {
    expect(tabAt('/')).toBe('worktrees');
    expect(tabAt('/session/host/9')).toBe('worktrees');
    expect(tabAt('/needs')).toBe('needs');
    expect(tabAt('/usage')).toBe('usage');
    expect(tabAt('/settings')).toBe('settings');
    expect(tabAt('/devices/host')).toBe('settings');
    expect(tabAt('/pair')).toBe('settings');
  });

  test('opens beside the sidebar in place of what was there', () => {
    showBeside('/session/host/9');
    expect(mockRouter.dismissAll).toHaveBeenCalled();
    expect(mockRouter.replace).toHaveBeenCalledWith('/session/host/9');

    mockRouter.canDismiss.mockReturnValueOnce(false);
    showBeside('/needs');
    expect(mockRouter.dismissAll).toHaveBeenCalledTimes(1);
    expect(mockRouter.replace).toHaveBeenLastCalledWith('/needs');
  });
});
