//! `ket collapse` — merging the winner and discarding the rest.
//!
//! This is the only destructive command in Epic 4, so most of what is checked
//! here is what it *refuses* to do. Every refusal below exists because the
//! alternative silently loses work.

use std::fs;

use ket_core::store::Store;
use ket_core::workspace::{CollapseOptions, Workspace};

mod common;
use common::{Sandbox, commit_file, git, init_repo};

/// A workspace whose state and worktrees live entirely inside `sandbox`.
fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

#[test]
fn the_winner_is_merged_and_the_losers_are_discarded() {
    let sandbox = Sandbox::new("collapse-happy");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    let b = ws
        .create_worktree(&project.id, "attempt-b", None, None)
        .unwrap();
    let c = ws
        .create_worktree(&project.id, "attempt-c", None, None)
        .unwrap();

    commit_file(&a.path, "winner.txt", "kept\n", "a: work");
    commit_file(&b.path, "loser-b.txt", "discarded\n", "b: work");
    commit_file(&c.path, "loser-c.txt", "discarded\n", "c: work");

    let report = ws
        .collapse(&a.id, CollapseOptions::default())
        .expect("collapse");

    assert_eq!(report.merged, "attempt-a");
    assert_eq!(report.into, "main");
    assert!(!report.already_merged);
    assert_eq!(report.discarded.len(), 2);

    // The winner's work is in the base.
    assert!(repo.join("winner.txt").is_file());
    // The losers' work is not, and their worktrees are gone.
    assert!(!repo.join("loser-b.txt").exists());
    assert!(!b.path.exists());
    assert!(!c.path.exists());
    assert!(!a.path.exists(), "the winner is consumed too, by default");
    assert!(ws.worktrees(Some(&project.id)).unwrap().is_empty());
}

#[test]
fn an_uncommitted_winner_is_refused_rather_than_half_merged() {
    let sandbox = Sandbox::new("collapse-dirty");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    commit_file(&a.path, "committed.txt", "safe\n", "a: commit");
    fs::write(a.path.join("uncommitted.txt"), "at risk\n").expect("write");

    let err = ws
        .collapse(&a.id, CollapseOptions::default())
        .expect_err("should refuse");

    // Merging a branch takes only what was committed. Proceeding would drop the
    // uncommitted file with no warning, which is the worst outcome available.
    assert!(
        err.to_string().contains("uncommitted"),
        "unhelpful error: {err}"
    );
    assert!(a.path.exists(), "nothing removed on a refusal");
    assert!(
        !repo.join("committed.txt").exists(),
        "nothing merged either"
    );
}

#[test]
fn force_merges_what_was_committed_and_leaves_the_rest_behind() {
    let sandbox = Sandbox::new("collapse-force");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    commit_file(&a.path, "committed.txt", "safe\n", "a: commit");
    fs::write(a.path.join("uncommitted.txt"), "lost\n").expect("write");

    let opts = CollapseOptions {
        force: true,
        ..CollapseOptions::default()
    };
    ws.collapse(&a.id, opts).expect("collapse with force");

    // --force waives the refusal; it does not widen the merge.
    assert!(repo.join("committed.txt").is_file());
    assert!(!repo.join("uncommitted.txt").exists());
}

#[test]
fn keep_leaves_the_winning_worktree_in_place() {
    let sandbox = Sandbox::new("collapse-keep");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    let b = ws
        .create_worktree(&project.id, "attempt-b", None, None)
        .unwrap();

    commit_file(&a.path, "winner.txt", "kept\n", "a: work");

    let opts = CollapseOptions {
        keep_winner: true,
        ..CollapseOptions::default()
    };
    let report = ws.collapse(&a.id, opts).expect("collapse");

    assert!(report.winner_kept);
    assert!(a.path.exists(), "winner kept");
    assert!(!b.path.exists(), "loser still discarded");
    assert_eq!(ws.worktrees(Some(&project.id)).unwrap().len(), 1);
}

#[test]
fn an_already_merged_branch_is_reported_rather_than_merged_again() {
    let sandbox = Sandbox::new("collapse-already");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    // The agent produced nothing, so the branch is still exactly the base.
    let report = ws
        .collapse(&a.id, CollapseOptions::default())
        .expect("collapse");

    // "Already contained" and "merged just now" are both success to git, and
    // different answers to someone asking whether their agent did anything.
    assert!(report.already_merged);
}

#[test]
fn a_conflicting_merge_is_aborted_and_leaves_the_repository_usable() {
    let sandbox = Sandbox::new("collapse-conflict");
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
        .collapse(&a.id, CollapseOptions::default())
        .expect_err("should conflict");
    assert!(!err.to_string().is_empty());

    // Invariant 4: no state you cannot get out of. A half-applied merge would
    // leave the primary checkout wedged, which is worse than not merging.
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
fn a_worktree_based_on_a_tag_cannot_be_collapsed() {
    let sandbox = Sandbox::new("collapse-tag");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["tag", "v1"]);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "from-tag", Some("v1"), None)
        .unwrap();

    commit_file(&a.path, "work.txt", "done\n", "work");

    let err = ws
        .collapse(&a.id, CollapseOptions::default())
        .expect_err("should refuse");

    // A tag is a fine thing to branch from and an impossible thing to merge into.
    assert!(err.to_string().contains("not a branch"), "unhelpful: {err}");
    assert!(a.path.exists());
}

#[test]
fn collapsing_while_the_checkout_is_on_another_branch_is_refused() {
    let sandbox = Sandbox::new("collapse-elsewhere");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "work");

    git(&repo, &["checkout", "--quiet", "-b", "somewhere-else"]);

    let err = ws
        .collapse(&a.id, CollapseOptions::default())
        .expect_err("should refuse");

    // Merging into whatever happens to be checked out would put the work on the
    // wrong branch, silently.
    assert!(
        err.to_string().contains("somewhere-else"),
        "unhelpful: {err}"
    );
    assert!(a.path.exists());
}

#[test]
fn worktrees_in_other_projects_are_untouched() {
    let sandbox = Sandbox::new("collapse-scoped");

    let repo_one = sandbox.path("one");
    let repo_two = sandbox.path("two");
    init_repo(&repo_one);
    init_repo(&repo_two);

    let ws = workspace(&sandbox);
    let p1 = ws.add_project(&repo_one).expect("add one");
    let p2 = ws.add_project(&repo_two).expect("add two");

    let winner = ws.create_worktree(&p1.id, "attempt-a", None, None).unwrap();
    let loser = ws.create_worktree(&p1.id, "attempt-b", None, None).unwrap();
    let bystander = ws.create_worktree(&p2.id, "unrelated", None, None).unwrap();

    commit_file(&winner.path, "w.txt", "won\n", "work");

    let report = ws
        .collapse(&winner.id, CollapseOptions::default())
        .expect("collapse");

    assert_eq!(report.discarded, vec![loser.id]);
    assert!(!loser.path.exists());
    assert!(
        bystander.path.exists(),
        "collapse must not reach into another project"
    );
    assert_eq!(ws.worktrees(Some(&p2.id)).unwrap().len(), 1);
}
