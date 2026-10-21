//! The small per-worktree setters: name, pin, Economy level, and the diff
//! against a worktree's base.

use std::fs;

use ket_core::store::Store;
use ket_core::workspace::Workspace;
use ket_core::worktree::token_reduction_levels;

mod common;
use common::{Sandbox, commit_file, init_repo};

/// A workspace whose state and worktrees live entirely inside `sandbox`.
fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

#[test]
fn set_worktree_name_overrides_the_branch_label() {
    let sandbox = Sandbox::new("meta-name");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    assert!(a.name.is_none());

    ws.set_worktree_name(&a.id, "  My Attempt  ").unwrap();
    let updated = ws.worktrees(Some(&project.id)).unwrap();
    assert_eq!(updated[0].name.as_deref(), Some("My Attempt"));

    // Blank restores the branch as the label.
    ws.set_worktree_name(&a.id, "   ").unwrap();
    let updated = ws.worktrees(Some(&project.id)).unwrap();
    assert!(updated[0].name.is_none());
}

#[test]
fn set_worktree_name_on_an_unknown_worktree_errors() {
    let sandbox = Sandbox::new("meta-name-unknown");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    ws.add_project(&repo).expect("add project");

    let bogus = ket_core::id::WorktreeId::new("nope");
    assert!(ws.set_worktree_name(&bogus, "x").is_err());
}

#[test]
fn saved_name_survives_legacy_agent_title() {
    let sandbox = Sandbox::new("meta-legacy-agent-title");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    ws.set_worktree_name(&worktree.id, "Saved label").unwrap();

    let mut previous_state = serde_json::to_value(&ws.worktrees(Some(&project.id)).unwrap()[0])
        .expect("serialize worktree");
    previous_state["agentTitle"] = serde_json::json!("Old agent title");
    let restored: ket_core::worktree::Worktree =
        serde_json::from_value(previous_state).expect("load previous worktree");

    assert_eq!(restored.name.as_deref(), Some("Saved label"));
}

#[test]
fn set_worktree_pinned_toggles_the_flag() {
    let sandbox = Sandbox::new("meta-pin");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    assert!(!a.pinned);

    ws.set_worktree_pinned(&a.id, true).unwrap();
    assert!(ws.worktrees(Some(&project.id)).unwrap()[0].pinned);

    ws.set_worktree_pinned(&a.id, false).unwrap();
    assert!(!ws.worktrees(Some(&project.id)).unwrap()[0].pinned);
}

#[test]
fn set_economy_level_updates_the_level_and_pack_atomically() {
    let sandbox = Sandbox::new("meta-economy");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    let levels = token_reduction_levels();
    let target = levels
        .iter()
        .find(|l| l.level != a.token_reduction)
        .expect("more than one level exists");

    ws.set_economy_level(&a.id, &target.id).unwrap();

    let updated = &ws.worktrees(Some(&project.id)).unwrap()[0];
    assert_eq!(updated.token_reduction, target.level);
    assert_eq!(
        updated.token_reduction_id.as_deref(),
        Some(target.id.as_str())
    );
    assert!(updated.token_reduction_pack_id.is_some());
}

#[test]
fn set_economy_level_rejects_an_unknown_id() {
    let sandbox = Sandbox::new("meta-economy-unknown");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    let err = ws
        .set_economy_level(&a.id, "not-a-real-level")
        .expect_err("should refuse");
    assert!(err.to_string().contains("unknown Economy level"), "{err}");
}

#[test]
fn set_token_reduction_resolves_a_level_number_to_its_id() {
    let sandbox = Sandbox::new("meta-token-reduction");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    let levels = token_reduction_levels();
    let target = levels
        .iter()
        .find(|l| l.level != a.token_reduction)
        .expect("more than one level exists");

    ws.set_token_reduction(&a.id, target.level).unwrap();

    let updated = &ws.worktrees(Some(&project.id)).unwrap()[0];
    assert_eq!(updated.token_reduction, target.level);
    assert_eq!(
        updated.token_reduction_id.as_deref(),
        Some(target.id.as_str())
    );
}

#[test]
fn set_token_reduction_rejects_an_unknown_level_number() {
    let sandbox = Sandbox::new("meta-token-reduction-unknown");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    let err = ws
        .set_token_reduction(&a.id, 255)
        .expect_err("should refuse");
    assert!(err.to_string().contains("unknown Economy level"), "{err}");
}

#[test]
fn worktree_diff_reports_uncommitted_and_committed_changes_against_the_base() {
    let sandbox = Sandbox::new("meta-diff");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    commit_file(&a.path, "committed.txt", "in the branch\n", "a: commit");
    fs::write(a.path.join("uncommitted.txt"), "not yet committed\n").unwrap();

    let diff = ws.worktree_diff(&a.id).unwrap();

    assert_eq!(diff.base, "main");
    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"committed.txt"), "{paths:?}");
    assert!(paths.contains(&"uncommitted.txt"), "{paths:?}");
}

#[test]
fn worktree_diff_on_a_missing_directory_is_refused() {
    let sandbox = Sandbox::new("meta-diff-missing");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    fs::remove_dir_all(&a.path).unwrap();

    let err = ws.worktree_diff(&a.id).expect_err("should refuse");
    assert!(err.to_string().contains("gone"), "{err}");
}

#[test]
fn worktree_diff_on_an_unknown_worktree_is_an_error() {
    let sandbox = Sandbox::new("meta-diff-unknown");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    ws.add_project(&repo).expect("add project");

    let bogus = ket_core::id::WorktreeId::new("nope");
    assert!(ws.worktree_diff(&bogus).is_err());
}
