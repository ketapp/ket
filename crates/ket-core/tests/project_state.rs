//! Per-project durable state.
//!
//! Projects sit above worktrees precisely so that coming back to one restores
//! where you were rather than starting over. That only works if the state
//! survives the process, and only stays trustworthy if it can never point at a
//! worktree that is gone.

use std::fs;

use ket_core::event::Event;
use ket_core::store::{MAX_LAYOUT_BYTES, Store};
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, init_repo};

fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

#[test]
fn creating_a_worktree_opens_it_and_gives_it_focus() {
    // A worktree you just made is one you are working in. Anything else means
    // creating three worktrees leaves the project pointing at nothing.
    let sandbox = Sandbox::new("state-create");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let first = ws
        .create_worktree(&project.id, "first", None, None)
        .unwrap();
    let second = ws
        .create_worktree(&project.id, "second", None, None)
        .unwrap();

    let state = ws.project_state(&project.id).unwrap();
    assert_eq!(state.open, vec![first.id.clone(), second.id.clone()]);
    assert_eq!(state.active, Some(second.id));
}

#[test]
fn state_survives_the_process_that_wrote_it() {
    // The whole point. A second `Workspace` over the same store stands in for
    // tomorrow morning.
    let sandbox = Sandbox::new("state-durable");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let worktree_id = {
        let ws = workspace(&sandbox);
        let project = ws.add_project(&repo).unwrap();
        let worktree = ws
            .create_worktree(&project.id, "yesterday", None, None)
            .unwrap();
        ws.set_layout(&project.id, Some(serde_json::json!({ "split": 0.4 })))
            .unwrap();
        worktree.id
    };

    let ws = workspace(&sandbox);
    let project = ws.projects().unwrap().into_iter().next().unwrap();
    let state = ws.project_state(&project.id).unwrap();

    assert_eq!(state.active, Some(worktree_id));
    assert_eq!(state.layout, Some(serde_json::json!({ "split": 0.4 })));
}

#[test]
fn closing_the_active_worktree_moves_focus_to_something_still_open() {
    let sandbox = Sandbox::new("state-close");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let first = ws
        .create_worktree(&project.id, "first", None, None)
        .unwrap();
    let second = ws
        .create_worktree(&project.id, "second", None, None)
        .unwrap();

    ws.close_worktree(&second.id).unwrap();

    let state = ws.project_state(&project.id).unwrap();
    assert_eq!(state.open, vec![first.id.clone()]);
    assert_eq!(state.active, Some(first.id.clone()));

    // Closing the last one leaves nothing focused rather than a dangling id.
    ws.close_worktree(&first.id).unwrap();
    let state = ws.project_state(&project.id).unwrap();
    assert!(state.open.is_empty());
    assert_eq!(state.active, None);
}

#[test]
fn reopening_a_closed_worktree_refocuses_it_without_duplicating_it() {
    let sandbox = Sandbox::new("state-reopen");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let first = ws
        .create_worktree(&project.id, "first", None, None)
        .unwrap();
    let second = ws
        .create_worktree(&project.id, "second", None, None)
        .unwrap();

    ws.open_worktree(&first.id).unwrap();
    ws.open_worktree(&first.id).unwrap();

    let state = ws.project_state(&project.id).unwrap();
    assert_eq!(state.open, vec![first.id.clone(), second.id]);
    assert_eq!(state.active, Some(first.id));
}

#[test]
fn removing_a_worktree_drops_it_from_the_project_that_held_it() {
    let sandbox = Sandbox::new("state-remove");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let kept = ws.create_worktree(&project.id, "kept", None, None).unwrap();
    let removed = ws
        .create_worktree(&project.id, "removed", None, None)
        .unwrap();

    ws.remove_worktree(&removed.id, false, true).unwrap();

    let state = ws.project_state(&project.id).unwrap();
    assert_eq!(state.open, vec![kept.id.clone()]);
    assert_eq!(state.active, Some(kept.id));
}

#[test]
fn a_worktree_deleted_behind_our_back_does_not_linger_in_project_state() {
    // Directories vanish for ordinary reasons — a manual `rm -rf`, a git
    // operation elsewhere. Reconciling on read means the state cannot outlive
    // what it points at even before `prune` runs.
    let sandbox = Sandbox::new("state-vanish");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let gone = ws.create_worktree(&project.id, "gone", None, None).unwrap();

    fs::remove_dir_all(&gone.path).unwrap();
    assert_eq!(ws.prune().unwrap(), vec![gone.id]);

    let state = ws.project_state(&project.id).unwrap();
    assert!(state.open.is_empty(), "{state:?}");
    assert_eq!(state.active, None);
}

#[test]
fn deregistering_a_project_forgets_its_state() {
    let sandbox = Sandbox::new("state-forget");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.create_worktree(&project.id, "work", None, None).unwrap();
    ws.set_layout(&project.id, Some(serde_json::json!({ "tab": "diff" })))
        .unwrap();

    ws.remove_project(&project.id, true).unwrap();

    let state = ws.project_state(&project.id).unwrap();
    assert_eq!(state, Default::default());
}

#[test]
fn two_projects_keep_entirely_separate_state() {
    // Nothing in the data model may assume there is one repository.
    let sandbox = Sandbox::new("state-two");
    let alpha = sandbox.path("alpha");
    let beta = sandbox.path("beta");
    init_repo(&alpha);
    init_repo(&beta);

    let ws = workspace(&sandbox);
    let a = ws.add_project(&alpha).unwrap();
    let b = ws.add_project(&beta).unwrap();

    let a_work = ws.create_worktree(&a.id, "a-work", None, None).unwrap();
    let b_work = ws.create_worktree(&b.id, "b-work", None, None).unwrap();
    ws.set_layout(&a.id, Some(serde_json::json!({ "who": "alpha" })))
        .unwrap();

    let a_state = ws.project_state(&a.id).unwrap();
    let b_state = ws.project_state(&b.id).unwrap();

    assert_eq!(a_state.active, Some(a_work.id));
    assert_eq!(b_state.active, Some(b_work.id));
    assert_eq!(a_state.layout, Some(serde_json::json!({ "who": "alpha" })));
    assert_eq!(b_state.layout, None);
}

#[test]
fn a_layout_is_stored_verbatim_and_never_interpreted() {
    // Core has no layout schema, on purpose: one here would be exactly the
    // UI-specific knowledge invariant 3 keeps out of `ket-core`.
    let sandbox = Sandbox::new("state-layout");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let layout = serde_json::json!({
        "panes": [{ "kind": "terminal", "size": 0.3 }, { "kind": "diff" }],
        "nonsense-the-shell-invented": { "deeply": { "nested": true } },
    });

    ws.set_layout(&project.id, Some(layout.clone())).unwrap();
    assert_eq!(ws.project_state(&project.id).unwrap().layout, Some(layout));

    ws.set_layout(&project.id, None).unwrap();
    assert_eq!(ws.project_state(&project.id).unwrap().layout, None);
}

#[test]
fn an_oversized_layout_is_refused_rather_than_growing_the_state_file() {
    // Nothing reads inside a layout, so nothing else would ever notice a client
    // growing one without bound — and the state file is read in full on every
    // command.
    let sandbox = Sandbox::new("state-huge");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let huge = serde_json::json!({ "blob": "x".repeat(MAX_LAYOUT_BYTES + 1) });
    let message = ws
        .set_layout(&project.id, Some(huge))
        .expect_err("expected a refusal")
        .to_string();

    assert!(message.contains("limit"), "unhelpful message: {message}");
    assert_eq!(ws.project_state(&project.id).unwrap().layout, None);
}

#[test]
fn focus_changes_are_announced_on_the_bus() {
    // A second client — another window, `ket events --follow` — has to be able
    // to see this without polling the state file.
    let sandbox = Sandbox::new("state-events");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let first = ws
        .create_worktree(&project.id, "first", None, None)
        .unwrap();

    let mut events = ws.bus().subscribe();
    let second = ws
        .create_worktree(&project.id, "second", None, None)
        .unwrap();

    let announced = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|envelope| match envelope.event {
            Event::ProjectStateChanged { open, active, .. } => Some((open, active)),
            _ => None,
        })
        .last()
        .expect("a project state change should have been published");

    assert_eq!(announced.0, vec![first.id, second.id.clone()]);
    assert_eq!(announced.1, Some(second.id));
}

#[test]
fn build_dirs_falls_back_to_the_global_default_until_a_project_sets_its_own() {
    let sandbox = Sandbox::new("state-build-dirs-default");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    assert_eq!(ws.build_dirs(&project.id), ws.config().storage.build_dirs);

    let mut settings = ws.project_settings(&project.id).unwrap();
    settings.build_dirs = Some(vec!["out".to_owned()]);
    ws.set_project_settings(&project.id, settings).unwrap();

    assert_eq!(ws.build_dirs(&project.id), vec!["out".to_owned()]);
}

#[test]
fn an_explicit_empty_build_dirs_list_is_kept_rather_than_falling_back() {
    // A repository that commits its own build output has nothing here that is
    // safe to clear, and falling back to the global default would be ket
    // overriding a person who said so explicitly.
    let sandbox = Sandbox::new("state-build-dirs-empty");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let mut settings = ws.project_settings(&project.id).unwrap();
    settings.build_dirs = Some(Vec::new());
    ws.set_project_settings(&project.id, settings).unwrap();

    assert!(ws.build_dirs(&project.id).is_empty());
}

#[test]
fn project_order_is_empty_until_set_and_then_round_trips() {
    let sandbox = Sandbox::new("state-project-order");
    let repo_a = sandbox.path("a");
    let repo_b = sandbox.path("b");
    init_repo(&repo_a);
    init_repo(&repo_b);

    let ws = workspace(&sandbox);
    let a = ws.add_project(&repo_a).unwrap();
    let b = ws.add_project(&repo_b).unwrap();

    assert!(ws.project_order().unwrap().is_empty());

    ws.set_project_order(&[b.id.clone(), a.id.clone()]).unwrap();
    assert_eq!(ws.project_order().unwrap(), vec![b.id.clone(), a.id]);
}

#[test]
fn set_project_order_drops_unknown_and_duplicate_ids() {
    let sandbox = Sandbox::new("state-project-order-dedup");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let bogus = ket_core::id::ProjectId::new("does-not-exist");

    ws.set_project_order(&[project.id.clone(), bogus, project.id.clone()])
        .unwrap();

    assert_eq!(ws.project_order().unwrap(), vec![project.id]);
}

#[test]
fn an_order_naming_nothing_clears_the_record() {
    let sandbox = Sandbox::new("state-project-order-clear");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.set_project_order(std::slice::from_ref(&project.id))
        .unwrap();
    assert!(!ws.project_order().unwrap().is_empty());

    ws.set_project_order(&[]).unwrap();
    assert!(ws.project_order().unwrap().is_empty());
}
