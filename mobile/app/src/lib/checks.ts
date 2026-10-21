// A worktree's pull request: how its checks stand together, in the colour
// ket's sidebar tints its GitHub mark, and in the words its tooltip uses.
// The same rules as `ket_core::github::PullRequestStatus` on the desktop.

import {
  CheckSummary_State,
  type PullRequestSummary,
  PullRequestMergeMethod,
  PullRequestSummary_Review,
  PullRequestSummary_State,
} from '@ket/remote';

import { t } from './theme';

/** A pull request's checks taken together. */
export type ChecksState = 'failed' | 'pending' | 'passed';

/** Failed if any failed, else pending if any is still going, else passed if
 * any passed; `null` when none says anything. */
export function checksState(pr: PullRequestSummary): ChecksState | null {
  const any = (state: CheckSummary_State) => pr.checks.some((check) => check.state === state);
  if (any(CheckSummary_State.FAILED)) return 'failed';
  if (any(CheckSummary_State.PENDING)) return 'pending';
  if (any(CheckSummary_State.PASSED)) return 'passed';
  return null;
}

/** The ink a state is drawn in: failed, running green when they passed,
 * and plain dim ink while they run, as the desktop's mark has them. */
export function checksColor(state: ChecksState | null): string {
  if (state === 'failed') return t.failed;
  if (state === 'passed') return t.running;
  return t.dim;
}

/** How many of its checks failed. */
export function failedChecks(pr: PullRequestSummary): number {
  return pr.checks.filter((check) => check.state === CheckSummary_State.FAILED).length;
}

/** Its checks in a few words: "2 of 9 checks failing", "3 checks running",
 * "all 9 checks passed". Empty when none says anything. */
export function checksSummary(pr: PullRequestSummary): string {
  const counted = pr.checks.filter((check) => check.state !== CheckSummary_State.SKIPPED);
  const count = (state: CheckSummary_State) => counted.filter((check) => check.state === state).length;
  const checks = (n: number) => (n === 1 ? 'check' : 'checks');
  switch (checksState(pr)) {
    case 'failed': {
      const failed = count(CheckSummary_State.FAILED);
      return failed === counted.length
        ? `${failed} ${checks(failed)} failing`
        : `${failed} of ${counted.length} ${checks(counted.length)} failing`;
    }
    case 'pending': {
      const pending = count(CheckSummary_State.PENDING);
      return `${pending} ${checks(pending)} running`;
    }
    case 'passed':
      return counted.length === 1 ? '1 check passed' : `all ${counted.length} checks passed`;
    default:
      return '';
  }
}

/** Where its review stands, in words; empty when there is nothing to say. */
export function reviewWords(pr: PullRequestSummary): string {
  switch (pr.review) {
    case PullRequestSummary_Review.APPROVED:
      return 'approved';
    case PullRequestSummary_Review.CHANGES_REQUESTED:
      return 'changes requested';
    case PullRequestSummary_Review.REVIEW_REQUIRED:
      return 'review required';
    default:
      return '';
  }
}

/** One check's state as its row says it, and the colour it says it in. */
export function checkWord(state: CheckSummary_State): { word: string; color: string } {
  switch (state) {
    case CheckSummary_State.FAILED:
      return { word: 'failed', color: t.failed };
    case CheckSummary_State.PASSED:
      return { word: 'passed', color: t.running };
    case CheckSummary_State.PENDING:
      return { word: 'running', color: t.dim };
    default:
      return { word: 'skipped', color: t.faint };
  }
}

/** Whether the pull request is still open. Missing on an older desktop,
 * which only sent open ones. */
export function prIsOpen(pr: PullRequestSummary): boolean {
  return pr.state !== PullRequestSummary_State.CLOSED && pr.state !== PullRequestSummary_State.MERGED;
}

/** Closed without merging. */
export function prIsClosed(pr: PullRequestSummary): boolean {
  return pr.state === PullRequestSummary_State.CLOSED;
}

/** Merge methods the repository allows. An older desktop sent none of the
 * flags, which proto3 reads as all false — treat that as all three. */
export function mergeMethods(pr: PullRequestSummary): { method: PullRequestMergeMethod; label: string }[] {
  const allowed: { method: PullRequestMergeMethod; label: string }[] = [];
  if (pr.mergeCommitAllowed) allowed.push({ method: PullRequestMergeMethod.MERGE_COMMIT, label: 'Merge commit' });
  if (pr.squashMergeAllowed) allowed.push({ method: PullRequestMergeMethod.SQUASH, label: 'Squash' });
  if (pr.rebaseMergeAllowed) allowed.push({ method: PullRequestMergeMethod.REBASE, label: 'Rebase' });
  if (allowed.length > 0) return allowed;
  return [
    { method: PullRequestMergeMethod.MERGE_COMMIT, label: 'Merge commit' },
    { method: PullRequestMergeMethod.SQUASH, label: 'Squash' },
    { method: PullRequestMergeMethod.REBASE, label: 'Rebase' },
  ];
}

/** The status line on the pull request card: draft or closed, behind, review,
 * checks. */
export function prLine(pr: PullRequestSummary): string {
  const state =
    pr.state === PullRequestSummary_State.CLOSED
      ? 'Closed'
      : pr.state === PullRequestSummary_State.MERGED
        ? 'Merged'
        : pr.draft
          ? 'Draft'
          : '';
  const behind = pr.behind ? `behind ${pr.base || 'base'}` : '';
  return [state, behind, reviewWords(pr), checksSummary(pr)].filter(Boolean).join(' · ');
}

/** Its checks in the order a person wants them: what failed, what is still
 * running, what passed, what said nothing — GitHub's order within each. */
export function checksInOrder(pr: PullRequestSummary): PullRequestSummary['checks'] {
  const order = [CheckSummary_State.FAILED, CheckSummary_State.PENDING, CheckSummary_State.PASSED];
  const rank = (state: CheckSummary_State) => {
    const at = order.indexOf(state);
    return at === -1 ? order.length : at;
  };
  return [...pr.checks].sort((a, b) => rank(a.state) - rank(b.state));
}
