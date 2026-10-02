//! `Workspace::adopt_worktree` — taking a checkout git already knows about
//! into ket's registry, without touching anything on disk.

use ket_core::store::Store;
use ket_core::workspace::Workspace;

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
fn a_worktree_made_by_hand_becomes_a_registered_one() {
    let sandbox = Sandbox::new("adopt-happy");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    // Made with plain git, not `ws.create_worktree`.
    let hand_made = sandbox.path("hand-made");
    git(
        &repo,
        &[
            "worktree",
            "add",
            hand_made.to_str().unwrap(),
            "-b",
            "by-hand",
        ],
    );

    assert!(
        ws.worktrees(Some(&project.id)).unwrap().is_empty(),
        "not managed until adopted"
    );
    let report = ws.worktree_report(Some(&project.id)).unwrap();
    assert_eq!(report.discovered.len(), 1);

    let adopted = ws.adopt_worktree(&project.id, &hand_made).expect("adopt");

    assert_eq!(adopted.branch, "by-hand");
    assert_eq!(adopted.project_id, project.id);
    assert!(
        adopted.provisioned_at_ms.is_none(),
        "adopting does not provision"
    );

    let managed = ws.worktrees(Some(&project.id)).unwrap();
    assert_eq!(managed.len(), 1);
    assert_eq!(managed[0].id, adopted.id);

    let report = ws.worktree_report(Some(&project.id)).unwrap();
    assert!(
        report.discovered.is_empty(),
        "adopted worktree is no longer reported as discovered"
    );
}

#[test]
fn adopting_the_same_checkout_twice_returns_the_same_record() {
    let sandbox = Sandbox::new("adopt-idempotent");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let hand_made = sandbox.path("hand-made");
    git(
        &repo,
        &[
            "worktree",
            "add",
            hand_made.to_str().unwrap(),
            "-b",
            "by-hand",
        ],
    );

    let first = ws.adopt_worktree(&project.id, &hand_made).expect("adopt");
    let second = ws
        .adopt_worktree(&project.id, &hand_made)
        .expect("adopt again");

    assert_eq!(first.id, second.id);
    assert_eq!(ws.worktrees(Some(&project.id)).unwrap().len(), 1);
}

#[test]
fn a_detached_checkout_cannot_be_adopted() {
    let sandbox = Sandbox::new("adopt-detached");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let detached = sandbox.path("detached");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            &head,
        ],
    );

    let err = ws
        .adopt_worktree(&project.id, &detached)
        .expect_err("should refuse");
    assert!(err.to_string().contains("detached"), "unhelpful: {err}");
    assert!(
        ws.worktrees(Some(&project.id)).unwrap().is_empty(),
        "nothing registered"
    );
}

#[test]
fn a_path_that_is_not_a_worktree_is_refused() {
    let sandbox = Sandbox::new("adopt-not-a-worktree");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let not_a_worktree = sandbox.path("elsewhere");
    std::fs::create_dir_all(&not_a_worktree).unwrap();

    let err = ws
        .adopt_worktree(&project.id, &not_a_worktree)
        .expect_err("should refuse");
    assert!(
        err.to_string().contains("not a worktree"),
        "unhelpful: {err}"
    );
}

#[test]
fn adopting_in_an_unavailable_project_is_refused() {
    let sandbox = Sandbox::new("adopt-unavailable");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    std::fs::remove_dir_all(&repo).unwrap();

    let err = ws
        .adopt_worktree(&project.id, &sandbox.path("anything"))
        .expect_err("should refuse");
    assert!(
        err.to_string().contains("not available"),
        "unhelpful: {err}"
    );
}

#[test]
fn base_commit_is_the_merge_base_not_the_moving_tip() {
    let sandbox = Sandbox::new("adopt-merge-base");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let hand_made = sandbox.path("hand-made");
    git(
        &repo,
        &[
            "worktree",
            "add",
            hand_made.to_str().unwrap(),
            "-b",
            "by-hand",
        ],
    );
    let branch_point = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();

    // The branch diverges...
    commit_file(&hand_made, "on-branch.txt", "work\n", "branch work");
    // ...and main moves on after the fork point.
    commit_file(&repo, "on-main.txt", "later\n", "main moves on");

    let adopted = ws.adopt_worktree(&project.id, &hand_made).expect("adopt");

    assert_eq!(
        adopted.base_commit.as_deref(),
        Some(branch_point.as_str()),
        "recorded the fork point, not main's current tip"
    );
}
