// What can be done to a worktree's pull request from the phone, as the
// desktop's worktree menu offers it (crates/ket-ui/src/worktree_menu.rs):
// which actions apply, and the merge question asked before one lands. The
// worktree screen's card and the list's long-press menu both read it, so the
// two never disagree about what a pull request allows.

import { type PullRequestMergeMethod, type PullRequestSummary, PullRequestSummary_Review } from '@ket/remote';
import { Alert } from 'react-native';

import { failedChecks, mergeMethods, prIsClosed, prIsOpen } from './checks';

/** The actions a pull request allows, for a desktop speaking `minor`. */
export interface PrActions {
  fix: boolean;
  address: boolean;
  update: boolean;
  merge: boolean;
  ready: boolean;
  reopen: boolean;
}

export function prActions(pr: PullRequestSummary, minor: number): PrActions {
  // Everything but handing over failed checks came with 1.19.
  const recent = minor >= 19;
  const open = prIsOpen(pr);
  return {
    fix: failedChecks(pr) > 0 && minor >= 11 && open,
    address:
      recent && open && (pr.unresolvedComments > 0 || pr.review === PullRequestSummary_Review.CHANGES_REQUESTED),
    update: recent && open && pr.behind,
    merge: recent && open && !pr.draft,
    ready: recent && open && pr.draft,
    reopen: recent && prIsClosed(pr),
  };
}

/** Asks how to merge, as the desktop's merge dialog does: one button per
 * method the repository allows, then Cancel. `go` runs with the one picked. */
export function askMerge(pr: PullRequestSummary, go: (method: PullRequestMergeMethod) => void): void {
  const extra = pr.behind ? ` This branch is behind ${pr.base || 'its base'}.` : '';
  Alert.alert(`Merge #${pr.number.toString()}?`, `GitHub will land it.${extra}`, [
    ...mergeMethods(pr).map((choice) => ({ text: choice.label, onPress: () => go(choice.method) })),
    { text: 'Cancel', style: 'cancel' as const },
  ]);
}
