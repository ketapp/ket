//! The read side, checked against real repositories.
//!
//! `ket-core::status` is the first place `gix` does the work instead of the
//! `git` binary, so the important test is not that it returns *something* — it
//! is that it agrees with `git status` on a repository where both can be run.

use std::fs;

use ket_core::status::{ChangeKind, MAX_CHANGED_FILES};
use ket_core::store::Store;
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, git, init_repo};

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

/// Paths reported as changed, deduplicated and sorted.
fn changed_paths(status: &ket_core::status::WorktreeStatus) -> Vec<String> {
    let mut paths: Vec<String> = status.files.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    paths.dedup();
    paths
}

/// What `git status --porcelain` reports, as bare paths.
fn git_status_paths(worktree: &std::path::Path) -> Vec<String> {
    let mut paths: Vec<String> = git(worktree, &["status", "--porcelain"])
        .lines()
        .filter_map(|line| line.get(3..))
        .map(|path| path.trim_matches('"').to_owned())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

#[test]
fn a_fresh_worktree_is_clean_and_level_with_its_base() {
    let sandbox = Sandbox::new("status-clean");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "clean", None, None)
        .unwrap();

    let status = ws.worktree_status(&worktree.id).expect("status");
    assert!(!status.is_dirty(), "unexpected changes: {:?}", status.files);
    assert!(!status.truncated);

    let tracking = status.tracking.expect("a base branch that still exists");
    assert_eq!(tracking.base, "main");
    assert_eq!((tracking.ahead, tracking.behind), (0, 0));
}

#[test]
fn each_kind_of_change_is_reported_as_itself() {
    let sandbox = Sandbox::new("status-kinds");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    common::commit_file(&repo, "keep.txt", "keep\n", "add files");
    common::commit_file(&repo, "gone.txt", "gone\n", "add more");

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "kinds", None, None)
        .unwrap();

    fs::write(worktree.path.join("keep.txt"), b"modified\n").unwrap();
    fs::remove_file(worktree.path.join("gone.txt")).unwrap();
    fs::write(worktree.path.join("new.txt"), b"untracked\n").unwrap();
    fs::write(worktree.path.join("staged.txt"), b"staged\n").unwrap();
    git(&worktree.path, &["add", "staged.txt"]);

    let status = ws.worktree_status(&worktree.id).expect("status");

    let kind_of = |path: &str, staged: bool| {
        status
            .files
            .iter()
            .find(|f| f.path == path && f.staged == staged)
            .map(|f| f.kind)
    };

    assert_eq!(kind_of("keep.txt", false), Some(ChangeKind::Modified));
    assert_eq!(kind_of("gone.txt", false), Some(ChangeKind::Deleted));
    assert_eq!(kind_of("new.txt", false), Some(ChangeKind::Untracked));
    assert_eq!(kind_of("staged.txt", true), Some(ChangeKind::Added));
}

#[test]
fn ket_and_git_agree_on_what_changed() {
    // The contract check. `git status` is the authority; ours is the fast path.
    let sandbox = Sandbox::new("status-contract");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    common::commit_file(&repo, "src/lib.rs", "// lib\n", "add source");
    common::commit_file(&repo, "doomed.txt", "bye\n", "add doomed");

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "contract", None, None)
        .unwrap();

    fs::write(worktree.path.join("src/lib.rs"), b"// edited\n").unwrap();
    fs::remove_file(worktree.path.join("doomed.txt")).unwrap();
    fs::write(worktree.path.join("loose.txt"), b"untracked\n").unwrap();
    fs::create_dir_all(worktree.path.join("fresh/nested")).unwrap();
    fs::write(worktree.path.join("fresh/nested/a.txt"), b"a\n").unwrap();
    fs::write(worktree.path.join("staged.txt"), b"staged\n").unwrap();
    git(&worktree.path, &["add", "staged.txt"]);

    let status = ws.worktree_status(&worktree.id).expect("status");

    assert_eq!(
        changed_paths(&status),
        git_status_paths(&worktree.path),
        "ket and git disagree about what changed"
    );
}

#[test]
fn ahead_and_behind_track_the_base_branch_as_it_moves() {
    let sandbox = Sandbox::new("status-diverge");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "diverging", None, None)
        .unwrap();

    common::commit_file(&worktree.path, "a.txt", "one\n", "agent commit 1");
    common::commit_file(&worktree.path, "b.txt", "two\n", "agent commit 2");
    common::commit_file(&repo, "upstream.txt", "moved on\n", "main moved");

    let tracking = ws
        .worktree_status(&worktree.id)
        .expect("status")
        .tracking
        .expect("tracking");

    assert_eq!(
        (tracking.ahead, tracking.behind),
        (2, 1),
        "measured against {}",
        tracking.base
    );
}

#[test]
fn a_worktree_based_on_a_tag_measures_against_the_frozen_commit() {
    // `HEAD` and tags cannot be re-resolved from inside the worktree to mean
    // what they meant outside it, so creation freezes the commit. This is what
    // that field is for.
    let sandbox = Sandbox::new("status-tag");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["tag", "v1.0"]);
    common::commit_file(&repo, "after.txt", "later\n", "main moved past the tag");

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "on-tag", Some("v1.0"), None)
        .unwrap();

    common::commit_file(&worktree.path, "work.txt", "work\n", "agent commit");

    let tracking = ws
        .worktree_status(&worktree.id)
        .expect("status")
        .tracking
        .expect("tracking");

    assert_eq!(tracking.base, "v1.0");
    // One commit past the tag, and the tag has nothing the worktree lacks —
    // main having moved on is not this worktree's concern.
    assert_eq!((tracking.ahead, tracking.behind), (1, 0));
}

#[test]
fn a_base_branch_that_has_since_been_deleted_costs_the_numbers_not_the_status() {
    // Coming back to a week-old worktree whose base branch was merged and
    // deleted is ordinary. It must not be an error.
    let sandbox = Sandbox::new("status-nobase");
    let repo = sandbox.path("repo");
    common::init_repo_on(&repo, "main");
    git(&repo, &["branch", "temporary"]);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "orphaned", Some("temporary"), None)
        .unwrap();

    git(&repo, &["branch", "-D", "temporary"]);
    fs::write(worktree.path.join("work.txt"), b"still here\n").unwrap();

    let status = ws.worktree_status(&worktree.id).expect("status");
    assert!(status.tracking.is_none(), "{:?}", status.tracking);
    assert_eq!(changed_paths(&status), vec!["work.txt".to_owned()]);
}

#[test]
fn provisioned_dependencies_do_not_show_up_as_changes() {
    // A cloned `node_modules` is 80,000 files. If it reached the change list,
    // every status call would cost the size of the dependency tree and the list
    // would be useless.
    let sandbox = Sandbox::new("status-deps");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    common::commit_file(&repo, ".gitignore", "node_modules/\n", "ignore deps");

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "with-deps", None, None)
        .unwrap();

    fs::create_dir_all(worktree.path.join("node_modules/pkg")).unwrap();
    fs::write(worktree.path.join("node_modules/pkg/index.js"), b"x").unwrap();

    let status = ws.worktree_status(&worktree.id).expect("status");
    assert!(
        !status.is_dirty(),
        "ignored files leaked in: {:?}",
        status.files
    );
}

#[test]
fn the_change_list_is_bounded_but_the_count_is_not() {
    // The bound is documented, and a documented bound
    // nobody enforces is a preference. A worktree where an agent ran something
    // catastrophic is exactly when this matters.
    let sandbox = Sandbox::new("status-bound");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "flooded", None, None)
        .unwrap();

    let excess = MAX_CHANGED_FILES + 20;
    for n in 0..excess {
        fs::write(worktree.path.join(format!("f{n:05}.txt")), b"x").unwrap();
    }

    let status = ws.worktree_status(&worktree.id).expect("status");

    assert_eq!(status.files.len(), MAX_CHANGED_FILES);
    assert!(status.truncated);
    assert_eq!(status.total_changes, excess);
}

#[test]
fn a_worktree_whose_directory_vanished_reports_that_rather_than_failing_obscurely() {
    let sandbox = Sandbox::new("status-gone");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "vanishing", None, None)
        .unwrap();
    fs::remove_dir_all(&worktree.path).unwrap();

    let message = ws
        .worktree_status(&worktree.id)
        .expect_err("expected a refusal")
        .to_string();

    assert!(message.contains("prune"), "unhelpful message: {message}");
}
