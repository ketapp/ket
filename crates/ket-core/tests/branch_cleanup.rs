//! Whether a branch can be deleted without losing anything — the squash- and
//! rebase-merge detection `git branch -d` cannot see.

use ket_core::branch_cleanup::{branch_has_landed, branch_is_fully_merged};
use ket_core::git::Git;

mod common;
use common::{Sandbox, commit_file, git, init_repo};

#[test]
fn an_unmerged_branch_is_not_fully_merged() {
    let sandbox = Sandbox::new("cleanup-unmerged");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "new\n", "feature work");
    git(&repo, &["checkout", "--quiet", "main"]);

    assert!(!branch_is_fully_merged(&Git::new(&repo), "feature"));
}

#[test]
fn a_branch_merged_by_a_real_merge_commit_is_fully_merged() {
    let sandbox = Sandbox::new("cleanup-real-merge");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "new\n", "feature work");
    git(&repo, &["checkout", "--quiet", "main"]);
    git(&repo, &["merge", "--quiet", "--no-ff", "feature"]);

    assert!(branch_is_fully_merged(&Git::new(&repo), "feature"));
}

#[test]
fn a_squash_merged_branch_is_recognised_though_its_commit_id_is_gone() {
    let sandbox = Sandbox::new("cleanup-squash");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "new\n", "feature work");
    git(&repo, &["checkout", "--quiet", "main"]);
    git(&repo, &["merge", "--quiet", "--squash", "feature"]);
    git(&repo, &["commit", "--quiet", "-m", "squashed feature"]);

    // `git branch -d` refuses this: no commit id on `feature` is reachable
    // from `main`. The content is there, which is what this checks instead.
    assert!(branch_is_fully_merged(&Git::new(&repo), "feature"));
}

#[test]
fn a_rebase_merged_branch_is_recognised_by_patch_equivalence() {
    let sandbox = Sandbox::new("cleanup-rebase");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "new\n", "feature work");

    // Cherry-picking the same patch onto `main` is what a rebase-merge leaves
    // behind: `main` has an equivalent commit under a different id, while
    // `feature` keeps its original one. `git cherry` sees past the id.
    git(&repo, &["checkout", "--quiet", "main"]);
    git(&repo, &["cherry-pick", "feature"]);

    assert!(branch_is_fully_merged(&Git::new(&repo), "feature"));
}

#[test]
fn a_branch_with_no_commits_of_its_own_is_vacuously_fully_merged() {
    let sandbox = Sandbox::new("cleanup-empty-branch");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "fresh"]);

    assert!(branch_is_fully_merged(&Git::new(&repo), "fresh"));
}

#[test]
fn branch_has_landed_is_false_for_a_branch_still_sitting_on_its_creation_point() {
    let sandbox = Sandbox::new("cleanup-landed-fresh");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let created_at = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    git(&repo, &["branch", "fresh"]);

    // Vacuously "fully merged", but nothing has landed: it has done nothing yet.
    assert!(!branch_has_landed(
        &Git::new(&repo),
        "fresh",
        Some(&created_at)
    ));
}

#[test]
fn branch_has_landed_is_true_once_its_own_work_is_on_the_base() {
    let sandbox = Sandbox::new("cleanup-landed-true");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let created_at = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "work.txt", "new\n", "feature work");
    git(&repo, &["checkout", "--quiet", "main"]);
    git(&repo, &["merge", "--quiet", "--no-ff", "feature"]);

    assert!(branch_has_landed(
        &Git::new(&repo),
        "feature",
        Some(&created_at)
    ));
}

#[test]
fn branch_has_landed_falls_back_to_fully_merged_when_created_at_is_unknown() {
    let sandbox = Sandbox::new("cleanup-landed-unknown");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "fresh"]);

    // With no `created_at` to tell the two apart, an empty branch reads as
    // landed — the old, imprecise answer for worktrees recorded before ket
    // kept the distinction.
    assert!(branch_has_landed(&Git::new(&repo), "fresh", None));
}

#[test]
fn an_unknown_branch_fails_closed_as_not_merged() {
    let sandbox = Sandbox::new("cleanup-unknown-branch");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    assert!(!branch_is_fully_merged(&Git::new(&repo), "does-not-exist"));
}
