import { describe, expect, jest, test } from '@jest/globals';
import type { PullRequestSummary } from '@ket/remote';

import {
  checkWord,
  checksColor,
  checksInOrder,
  checksState,
  checksSummary,
  failedChecks,
  mergeMethods,
  prIsClosed,
  prIsOpen,
  prLine,
  reviewWords,
} from './checks';
import { t } from './theme';

jest.mock('@ket/remote', () => ({
  CheckSummary_State: { STATE_UNSPECIFIED: 0, PASSED: 1, FAILED: 2, PENDING: 3, SKIPPED: 4 },
  PullRequestMergeMethod: { UNSPECIFIED: 0, MERGE_COMMIT: 1, SQUASH: 2, REBASE: 3 },
  PullRequestSummary_Review: { REVIEW_UNSPECIFIED: 0, APPROVED: 1, CHANGES_REQUESTED: 2, REVIEW_REQUIRED: 3 },
  PullRequestSummary_State: { OPEN: 0, CLOSED: 1, MERGED: 2 },
}));

const PASSED = 1;
const FAILED = 2;
const PENDING = 3;
const SKIPPED = 4;

const pr = (states: number[], review = 0): PullRequestSummary =>
  ({
    number: 17n,
    url: 'https://github.com/o/r/pull/17',
    draft: false,
    review,
    checks: states.map((state, at) => ({ name: `check ${at}`, state, url: '' })),
  }) as unknown as PullRequestSummary;

describe('pull request checks', () => {
  test('are failed before pending before passed, and skipped says nothing', () => {
    expect(checksState(pr([PASSED, PENDING, FAILED]))).toBe('failed');
    expect(checksState(pr([PASSED, PENDING]))).toBe('pending');
    expect(checksState(pr([PASSED, SKIPPED]))).toBe('passed');
    expect(checksState(pr([SKIPPED]))).toBeNull();
    expect(checksState(pr([]))).toBeNull();
  });

  test('take the desktop mark’s colours', () => {
    expect(checksColor('failed')).toBe(t.failed);
    expect(checksColor('passed')).toBe(t.running);
    expect(checksColor('pending')).toBe(t.dim);
    expect(checksColor(null)).toBe(t.dim);
  });

  test('are summed up in the desktop’s words', () => {
    expect(checksSummary(pr([FAILED, PASSED, PENDING, SKIPPED]))).toBe('1 of 3 checks failing');
    expect(checksSummary(pr([FAILED, FAILED]))).toBe('2 checks failing');
    expect(checksSummary(pr([FAILED]))).toBe('1 check failing');
    expect(checksSummary(pr([PENDING, PASSED]))).toBe('1 check running');
    expect(checksSummary(pr([PASSED, PASSED, SKIPPED]))).toBe('all 2 checks passed');
    expect(checksSummary(pr([PASSED]))).toBe('1 check passed');
    expect(checksSummary(pr([SKIPPED]))).toBe('');
    expect(failedChecks(pr([FAILED, PASSED, FAILED]))).toBe(2);
  });

  test('list what failed first, keeping GitHub’s order within each state', () => {
    const ordered = checksInOrder(pr([PASSED, SKIPPED, FAILED, PENDING, FAILED]));
    expect(ordered.map((check) => check.name)).toEqual(['check 2', 'check 4', 'check 3', 'check 0', 'check 1']);
  });

  test('name each state and review in words', () => {
    expect(checkWord(FAILED)).toEqual({ word: 'failed', color: t.failed });
    expect(checkWord(PENDING).word).toBe('running');
    expect(checkWord(SKIPPED).word).toBe('skipped');
    expect(reviewWords(pr([], 1))).toBe('approved');
    expect(reviewWords(pr([], 2))).toBe('changes requested');
    expect(reviewWords(pr([], 3))).toBe('review required');
    expect(reviewWords(pr([], 0))).toBe('');
  });

  test('names closed and behind pull requests on the card line', () => {
    expect(prLine({ ...pr([FAILED]), draft: true } as PullRequestSummary)).toBe('Draft · 1 check failing');
    expect(prIsOpen(pr([]))).toBe(true);
    const closed = { ...pr([]), state: 1, behind: true, base: 'main' } as unknown as PullRequestSummary;
    expect(prIsClosed(closed)).toBe(true);
    expect(prIsOpen(closed)).toBe(false);
    expect(prLine(closed)).toBe('Closed · behind main');
    expect(mergeMethods(pr([])).map((method) => method.label)).toEqual(['Merge commit', 'Squash', 'Rebase']);
    expect(
      mergeMethods({ ...pr([]), squashMergeAllowed: true } as PullRequestSummary).map((method) => method.label),
    ).toEqual(['Squash']);
  });
});
