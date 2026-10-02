//! Whether a branch can be deleted without losing anything.
//!
//! `git branch -d` decides this by ancestry: is the branch's tip reachable
//! from the base? That is the right question for a merge commit and the wrong
//! one for almost everything a forge does. A squash-merge replaces the whole
//! branch with a single new commit; a rebase-merge gives every commit a new
//! id. In both cases every line of the branch is already on the base and `-d`
//! still refuses, because no commit id matches.
//!
//! Refusing is the safe direction to be wrong in, so `-d` stays the first
//! thing tried. But being wrong here is not free: the branch survives, and so
//! does the worktree it belongs to, and for a Rust checkout that is several
//! gigabytes of `target/` that nobody ever reclaims. That is how this
//! repository accumulated eighty gigabytes of merged worktrees.
//!
//! So when `-d` refuses, ask the question by content instead of by id.
//!
//! # Two questions, not one
//!
//! "Would deleting this branch lose anything?" and "has this branch's work
//! landed?" are the same question for every branch that has commits, and
//! opposite answers for one that has none. A worktree created a minute ago is
//! a branch sitting exactly on its base: deleting it loses nothing, and
//! nothing about it has been merged. [`branch_is_fully_merged`] answers the
//! first and is what the cleanup uses; [`branch_has_landed`] answers the
//! second and is what a badge reading "merged" must use, because the first
//! one passes vacuously on an empty branch and put that badge on every
//! worktree the moment it was created. The second needs something git does
//! not have — where the branch started — so it is given it.

use crate::git::Git;

/// Whether `branch` contains no change its base does not already have.
///
/// Only ever called after `git branch -d` has refused, and only to decide
/// whether to override that refusal. Every probe it runs fails closed: an
/// unreadable repository, a git too old for `merge-tree --write-tree`, a ref
/// that does not resolve, a merge that conflicts — all of them mean "not
/// proved", which preserves the branch.
pub fn branch_is_fully_merged(git: &Git, branch: &str) -> bool {
    let branch_ref = format!("refs/heads/{branch}");

    target_refs(git, branch).iter().any(|target| {
        git.commit_of(target).is_some_and(|target_oid| {
            merges_without_tree_change(git, &target_oid, &branch_ref)
                || every_commit_is_upstream(git, &target_oid, &branch_ref)
        })
    })
}

/// Whether `branch`'s own work has landed on its base.
///
/// The question a "merged" badge is asking, and *not* the one
/// [`branch_is_fully_merged`] answers. That one exists to override a refusal
/// from `git branch -d`, where a branch with no commits of its own is a
/// perfectly good thing to delete — so it says yes to one, correctly and
/// vacuously: no commit of the branch's is missing from the base because
/// there are no commits. Read as "merged" that is plainly wrong, and it put
/// the badge on every worktree from the moment it was created.
///
/// Git alone cannot tell the two apart. A branch cut an hour ago and never
/// committed to has no commits the base lacks; so has one whose commits were
/// merged and left behind. Both are `git cherry`-silent and both are
/// ancestors of the base. What separates them is not in the repository at
/// all — it is `created_at`, the commit the base resolved to when ket cut the
/// worktree (see [`crate::worktree::Worktree::base_commit`]). A branch still
/// sitting on it has not done anything yet.
///
/// `created_at` is `None` for worktrees recorded before ket kept it. There is
/// nothing to distinguish them with, so they fall through to the old answer:
/// a genuinely merged one keeps its badge, and an untouched one keeps the
/// wrong badge it already had.
pub fn branch_has_landed(git: &Git, branch: &str, created_at: Option<&str>) -> bool {
    if let Some(created_at) = created_at
        && git
            .commit_of(&format!("refs/heads/{branch}"))
            .is_some_and(|tip| tip == created_at)
    {
        return false;
    }

    branch_is_fully_merged(git, branch)
}

/// The refs a branch might have been merged into, best guess first.
///
/// `branch.<name>.base` is what ket recorded when it created the worktree, so
/// it is the one that knows; `origin/HEAD` is the repository's own idea of its
/// default branch; `HEAD` is where `git branch -d` would have looked.
fn target_refs(git: &Git, branch: &str) -> Vec<String> {
    let candidates = [
        git.config_get(&format!("branch.{branch}.base")),
        git.symbolic_ref("refs/remotes/origin/HEAD"),
        Some("HEAD".to_owned()),
    ];

    let mut refs: Vec<String> = Vec::new();
    for candidate in candidates.into_iter().flatten() {
        // A value beginning `-` would be read as an option by every command
        // these are handed to, so it never becomes one.
        if candidate.starts_with('-') || refs.contains(&candidate) {
            continue;
        }
        refs.push(candidate);
    }
    refs
}

/// Whether merging the branch into the target would change the target's tree.
///
/// This is the check that sees a squash-merge. The squashed commit has none of
/// the branch's commit ids, but it has all of its content, so merging the
/// branch back in produces exactly the tree the target already has.
fn merges_without_tree_change(git: &Git, target_oid: &str, branch_ref: &str) -> bool {
    let Some(merged) = git.merge_tree(target_oid, branch_ref) else {
        return false;
    };
    git.tree_of(target_oid)
        .is_some_and(|target| target == merged)
}

/// Whether every commit on the branch already exists upstream as a patch.
///
/// This is the check that sees a rebase-merge: `git cherry` marks a commit `-`
/// when an equivalent patch is already on the target, whatever its id. No
/// output at all means the branch is not ahead, which is the same answer.
fn every_commit_is_upstream(git: &Git, target_oid: &str, branch_ref: &str) -> bool {
    let Some(lines) = git.cherry(target_oid, branch_ref) else {
        return false;
    };
    lines.iter().all(|line| line.starts_with('-'))
}
