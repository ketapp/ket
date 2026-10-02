//! `Workspace::merge_worktree` — merging a single attempt into its base
//! without touching its siblings.
//!
//! [`crate::workspace::collapse`] shares the same `prepare_merge` checks and
//! is covered in `collapse.rs`; this file covers what is specific to the
//! single-worktree path: committing uncommitted work before merging, and the
//! `keep_after_merge` config that decides whether the checkout survives.

use std::fs;

use ket_core::config::Config;
use ket_core::store::Store;
use ket_core::workspace::{MergeError, Workspace};

mod common;
use common::{Sandbox, commit_file, git, init_repo};

/// A workspace whose state and worktrees live entirely inside `sandbox`.
fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

/// Same, but with `keep_after_merge` forced off so the checkout is removed.
fn workspace_removing_after_merge(sandbox: &Sandbox) -> Workspace {
    let mut config = Config::default();
    config.merge.keep_after_merge = false;
    Workspace::with_config(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        config,
    )
}

#[test]
fn uncommitted_work_is_committed_then_merged_and_the_checkout_survives() {
    let sandbox = Sandbox::new("merge-happy");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    fs::write(a.path.join("work.txt"), "done\n").expect("write");

    let report = ws.merge_worktree(&a.id, false).expect("merge");

    assert_eq!(report.worktree, a.id);
    assert_eq!(report.branch, "attempt-a");
    assert_eq!(report.into, "main");
    assert!(report.committed.is_some(), "uncommitted work was committed");
    assert!(!report.already_merged);

    // `keep_after_merge` defaults to true, so a quick merge does not also
    // silently remove the thing you were looking at.
    assert!(!report.removed);
    assert!(a.path.exists());
    assert!(repo.join("work.txt").is_file());
}

#[test]
fn an_already_merged_branch_is_reported_without_a_second_commit() {
    let sandbox = Sandbox::new("merge-already");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    // The agent produced nothing, so the branch is still exactly the base.
    let report = ws.merge_worktree(&a.id, false).expect("merge");

    assert!(report.already_merged);
    assert!(report.committed.is_none());
}

#[test]
fn keep_after_merge_false_removes_the_checkout() {
    let sandbox = Sandbox::new("merge-remove");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace_removing_after_merge(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "a: work");

    let report = ws.merge_worktree(&a.id, false).expect("merge");

    assert!(report.removed);
    assert!(report.remove_failed.is_none());
    assert!(!a.path.exists());
    assert!(ws.worktrees(Some(&project.id)).unwrap().is_empty());
}

#[test]
fn a_dirty_primary_checkout_is_refused_as_waivable() {
    let sandbox = Sandbox::new("merge-dirty-primary");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "a: work");

    fs::write(repo.join("uncommitted.txt"), "in the way\n").expect("write");

    let err = ws.merge_worktree(&a.id, false).expect_err("should refuse");

    assert!(
        matches!(err, MergeError::Waivable(_)),
        "a dirty primary checkout should be a waivable refusal, got: {err}"
    );
    assert!(!repo.join("work.txt").exists(), "nothing merged");
}

#[test]
fn force_merges_on_top_of_a_dirty_primary_checkout() {
    let sandbox = Sandbox::new("merge-force-dirty-primary");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "a: work");

    fs::write(repo.join("uncommitted.txt"), "in the way\n").expect("write");

    ws.merge_worktree(&a.id, true).expect("merge with force");

    assert!(repo.join("work.txt").is_file());
    // The waiver is for the refusal, not a stash: the dirty file is untouched.
    assert!(repo.join("uncommitted.txt").exists());
}

#[test]
fn merging_while_the_checkout_is_on_another_branch_is_refused() {
    let sandbox = Sandbox::new("merge-elsewhere");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "a: work");

    git(&repo, &["checkout", "--quiet", "-b", "somewhere-else"]);

    let err = ws.merge_worktree(&a.id, false).expect_err("should refuse");

    assert!(
        matches!(err, MergeError::Failed(_)),
        "not waivable by --force, got: {err}"
    );
    assert!(err.to_string().contains("somewhere-else"), "{err}");
    assert!(a.path.exists());
}

#[test]
fn a_conflicting_merge_is_aborted_and_leaves_the_repository_usable() {
    let sandbox = Sandbox::new("merge-conflict");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "shared.txt", "original\n", "add shared");

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    commit_file(&a.path, "shared.txt", "from the agent\n", "a: edit shared");
    // The base moves underneath, incompatibly.
    commit_file(&repo, "shared.txt", "from main\n", "main: edit shared");

    let err = ws
        .merge_worktree(&a.id, false)
        .expect_err("should conflict");
    assert!(
        matches!(err, MergeError::Failed(_)),
        "a real conflict is not waivable, got: {err}"
    );

    let status = git(&repo, &["status", "--porcelain"]);
    assert!(status.trim().is_empty(), "left dirty: {status}");
    assert_eq!(
        fs::read_to_string(repo.join("shared.txt")).unwrap(),
        "from main\n"
    );
    assert!(
        a.path.exists(),
        "the attempt survives so it can be resolved"
    );
}

#[test]
fn a_worktree_based_on_a_tag_cannot_be_merged() {
    let sandbox = Sandbox::new("merge-tag");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["tag", "v1"]);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "from-tag", Some("v1"), None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "work");

    let err = ws.merge_worktree(&a.id, false).expect_err("should refuse");

    assert!(err.to_string().contains("not a branch"), "unhelpful: {err}");
    assert!(a.path.exists());
}

#[test]
fn merging_an_unknown_worktree_is_an_error() {
    let sandbox = Sandbox::new("merge-unknown");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    ws.add_project(&repo).expect("add project");

    let bogus = ket_core::id::WorktreeId::new("does-not-exist");
    let err = ws.merge_worktree(&bogus, false).expect_err("should error");
    assert!(matches!(err, MergeError::Failed(_)));
}
