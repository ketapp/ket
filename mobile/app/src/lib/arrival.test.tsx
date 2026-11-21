import { type WorktreeSummary } from '@ket/remote';
import { describe, expect, jest, test } from '@jest/globals';
import { renderHook } from '@testing-library/react-native';

import { useReadOnArrival } from './arrival';
import type { HostConnection } from './connection';

let mockFocused = true;
jest.mock('expo-router', () => ({ useIsFocused: () => mockFocused }));

const worktree = (unread: boolean, id = 'w1') => ({ id, unread }) as WorktreeSummary;

const look = async (first: WorktreeSummary | undefined) => {
  const markUnread = jest.fn();
  const conn = { markUnread } as unknown as HostConnection;
  const hook = await renderHook(({ at }: { at: WorktreeSummary | undefined }) => useReadOnArrival(conn, at), {
    initialProps: { at: first },
  });
  return {
    markUnread,
    rerender: async (at: WorktreeSummary | undefined) => {
      await hook.rerender({ at });
    },
  };
};

describe('a worktree marked unread', () => {
  test('is read as its screen comes to it', async () => {
    mockFocused = true;
    const { markUnread } = await look(worktree(true));
    expect(markUnread).toHaveBeenCalledWith('w1', false);
  });

  test('once the worktree is known, when the screen drew before the snapshot', async () => {
    mockFocused = true;
    const { markUnread, rerender } = await look(undefined);
    expect(markUnread).not.toHaveBeenCalled();
    await rerender(worktree(true));
    expect(markUnread).toHaveBeenCalledWith('w1', false);
  });

  test('keeps its mark when it is marked while already showing', async () => {
    mockFocused = true;
    const { markUnread, rerender } = await look(worktree(false));
    await rerender(worktree(true));
    expect(markUnread).not.toHaveBeenCalled();
  });

  test('is read again after the screen is left and come back to', async () => {
    mockFocused = true;
    const { markUnread, rerender } = await look(worktree(false));
    await rerender(worktree(true));
    mockFocused = false;
    await rerender(worktree(true));
    expect(markUnread).not.toHaveBeenCalled();
    mockFocused = true;
    await rerender(worktree(true));
    expect(markUnread).toHaveBeenCalledWith('w1', false);
  });

  test('leaves a worktree with no mark alone', async () => {
    mockFocused = true;
    const { markUnread } = await look(worktree(false));
    expect(markUnread).not.toHaveBeenCalled();
  });
});
