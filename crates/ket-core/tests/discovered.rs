//! Reconciling ket's registry against what git actually knows about.
//!
//! `Workspace::worktrees` only ever reports what ket itself created — see
//! `worktree_lifecycle.rs`'s `ket_and_git_agree_on_what_worktrees_exist` for
//! that contract. `Workspace::worktree_report` is the other half: it tells a
//! caller about everything else git knows too, distinguished from what ket
//! manages, so a UI can show both without doing set arithmetic of its own.
//! Checked against the real `git` binary throughout, per invariant 5.

use std::fs;

use ket_core::store::Store;
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, git, init_repo};

/// A workspace whose state and worktrees live entirely inside `sandbox`.
fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

#[test]
fn a_worktree_made_by_plain_git_worktree_add_shows_up_as_discovered() {
    let sandbox = Sandbox::new("discovered-manual");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let manual = sandbox.path("manual");
    git(
        &repo,
        &["worktree", "add", "-b", "by-hand", manual.to_str().unwrap()],
    );

    let report = ws.worktree_report(Some(&project.id)).unwrap();
    assert!(report.managed.is_empty());
    assert_eq!(report.discovered.len(), 1);

    let found = &report.discovered[0];
    assert_eq!(found.project_id, project.id);
    assert_eq!(found.branch.as_deref(), Some("by-hand"));
    assert_eq!(found.path, manual.canonicalize().unwrap());
}

#[test]
fn a_worktree_ket_created_shows_up_as_managed_not_discovered() {
    let sandbox = Sandbox::new("discovered-managed");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "ket-made", None, None)
        .expect("create worktree");

    let report = ws.worktree_report(Some(&project.id)).unwrap();
    assert_eq!(report.managed.len(), 1);
    assert_eq!(report.managed[0].id, worktree.id);
    assert!(
        report.discovered.is_empty(),
        "a worktree ket created must not also show up as discovered: {:?}",
        report.discovered
    );
}

#[test]
fn the_primary_checkout_is_never_reported_as_discovered() {
    let sandbox = Sandbox::new("discovered-primary");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    // git always reports the primary checkout as the first `worktree list`
    // entry, even though no linked worktree has ever been created.
    let report = ws.worktree_report(Some(&project.id)).unwrap();
    assert!(
        report.discovered.is_empty(),
        "the primary checkout leaked into discovered: {:?}",
        report.discovered
    );

    // It stays excluded even once other worktrees exist alongside it.
    ws.create_worktree(&project.id, "ket-made", None, None)
        .unwrap();
    let manual = sandbox.path("manual");
    git(
        &repo,
        &["worktree", "add", "-b", "by-hand", manual.to_str().unwrap()],
    );

    let report = ws.worktree_report(Some(&project.id)).unwrap();
    assert!(
        report
            .discovered
            .iter()
            .all(|w| w.path != repo.canonicalize().unwrap()),
        "the primary checkout leaked into discovered: {:?}",
        report.discovered
    );
    assert_eq!(report.discovered.len(), 1);
}

#[test]
fn a_project_whose_repository_is_gone_degrades_instead_of_failing() {
    let sandbox = Sandbox::new("discovered-vanished");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "orphan", None, None)
        .expect("create worktree");

    fs::remove_dir_all(&repo).unwrap();

    let report = ws
        .worktree_report(Some(&project.id))
        .expect("must not fail just because the repository is gone");

    // The registry entry is still reported...
    assert_eq!(report.managed.len(), 1);
    assert_eq!(report.managed[0].id, worktree.id);
    // ...but nothing can be discovered without a repository to ask git about.
    assert!(report.discovered.is_empty());
}

#[test]
fn a_discovered_worktree_that_is_later_registered_stops_being_discovered() {
    let sandbox = Sandbox::new("discovered-adopted");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let adopted = sandbox.path("adopted");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "to-be-adopted",
            adopted.to_str().unwrap(),
        ],
    );

    let before = ws.worktree_report(Some(&project.id)).unwrap();
    assert_eq!(before.discovered.len(), 1);
    assert!(before.managed.is_empty());

    // Simulate ket adopting it into the registry: a record with a matching path
    // is what "registered" means, whatever operation eventually produces one.
    let store = Store::at(sandbox.path("state.json"));
    let worktree = ket_core::worktree::Worktree {
        agent: None,
        id: ket_core::worktree::id_for(&project.id, "to-be-adopted"),
        project_id: project.id.clone(),
        branch: "to-be-adopted".to_owned(),
        name: None,
        agent_title: None,
        path: adopted.canonicalize().unwrap(),
        base: "main".to_owned(),
        base_commit: None,
        created_at_ms: ket_core::now_ms(),
        provisioned_at_ms: None,
        provisioned_paths: Vec::new(),
        token_reduction: ket_core::worktree::default_token_reduction(),
        token_reduction_id: None,
        token_reduction_pack_id: None,
        pinned: false,
    };
    store
        .update(move |state| {
            state.worktrees.push(worktree.clone());
            Ok(())
        })
        .unwrap();

    let after = ws.worktree_report(Some(&project.id)).unwrap();
    assert_eq!(after.managed.len(), 1);
    assert_eq!(after.managed[0].branch, "to-be-adopted");
    assert!(
        after.discovered.is_empty(),
        "a now-registered worktree must not still be reported as discovered: {:?}",
        after.discovered
    );
}

#[test]
fn scoping_to_one_project_excludes_another_projects_discovered_worktrees() {
    let sandbox = Sandbox::new("discovered-scoped");
    let a = sandbox.path("a");
    let b = sandbox.path("b");
    init_repo(&a);
    init_repo(&b);

    let ws = workspace(&sandbox);
    let pa = ws.add_project(&a).unwrap();
    let pb = ws.add_project(&b).unwrap();

    let manual_a = sandbox.path("manual-a");
    git(
        &a,
        &["worktree", "add", "-b", "in-a", manual_a.to_str().unwrap()],
    );
    let manual_b = sandbox.path("manual-b");
    git(
        &b,
        &["worktree", "add", "-b", "in-b", manual_b.to_str().unwrap()],
    );

    let only_a = ws.worktree_report(Some(&pa.id)).unwrap();
    assert_eq!(only_a.discovered.len(), 1);
    assert_eq!(only_a.discovered[0].project_id, pa.id);

    let only_b = ws.worktree_report(Some(&pb.id)).unwrap();
    assert_eq!(only_b.discovered.len(), 1);
    assert_eq!(only_b.discovered[0].project_id, pb.id);

    // Unscoped reports both.
    let all = ws.worktree_report(None).unwrap();
    assert_eq!(all.discovered.len(), 2);
}
