//! Per-project preferences.
//!
//! These are this user's choices about a project on this machine — a name, a
//! colour, what to hide — and they matter only if they survive the process,
//! refuse values every reader would otherwise have to guard against, and
//! actually change what ket does next.

use std::fs;

use ket_core::KetError;
use ket_core::config::Config;
use ket_core::event::Event;
use ket_core::id::ProjectId;
use ket_core::project::{MAX_ICON_BYTES, ProjectSettings};
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

#[test]
fn a_project_with_nothing_set_has_default_settings() {
    let sandbox = Sandbox::new("settings-default");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let settings = ws.project_settings(&project.id).unwrap();
    assert_eq!(settings, ProjectSettings::default());
    assert_eq!(settings.name_for(&project), "repo");
}

#[test]
fn settings_survive_the_process_that_wrote_them() {
    let sandbox = Sandbox::new("settings-durable");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let project = workspace(&sandbox).add_project(&repo).unwrap();
    workspace(&sandbox)
        .set_project_settings(
            &project.id,
            ProjectSettings {
                display_name: Some("Ready Set Reading".to_owned()),
                color: Some("#EAB308".to_owned()),
                icon: Some("📚".to_owned()),
                hide_discovered_worktrees: true,
                build_dirs: None,
                agents: Default::default(),
                backlog_in_repo: false,
            },
        )
        .unwrap();

    // Tomorrow morning.
    let settings = workspace(&sandbox).project_settings(&project.id).unwrap();
    assert_eq!(settings.display_name.as_deref(), Some("Ready Set Reading"));
    assert_eq!(settings.name_for(&project), "Ready Set Reading");
    // Normalised on the way in, so two clients never disagree on case.
    assert_eq!(settings.color.as_deref(), Some("#eab308"));
    assert_eq!(settings.icon.as_deref(), Some("📚"));
    assert!(settings.hide_discovered_worktrees);
}

#[test]
fn a_blank_name_means_the_directory_name() {
    // Three spaces is not a name. Storing it would render a project with no
    // label and no way to see why.
    let sandbox = Sandbox::new("settings-blank");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            display_name: Some("   ".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();

    let settings = ws.project_settings(&project.id).unwrap();
    assert_eq!(settings.display_name, None);
    assert_eq!(settings.name_for(&project), "repo");
}

#[test]
fn a_colour_that_will_not_parse_is_refused_before_anything_is_written() {
    let sandbox = Sandbox::new("settings-colour");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let bad = ProjectSettings {
        color: Some("teal".to_owned()),
        display_name: Some("kept out".to_owned()),
        ..Default::default()
    };

    let err = ws.set_project_settings(&project.id, bad).unwrap_err();
    assert!(matches!(err, KetError::Config(_)), "{err}");
    assert!(err.to_string().contains("teal"), "{err}");
    // Nothing from the refused write landed — not even the valid field.
    assert_eq!(
        ws.project_settings(&project.id).unwrap(),
        ProjectSettings::default()
    );
}

#[test]
fn an_oversized_icon_is_refused() {
    let sandbox = Sandbox::new("settings-icon");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let huge = ProjectSettings {
        icon: Some("x".repeat(MAX_ICON_BYTES + 1)),
        ..Default::default()
    };

    let err = ws.set_project_settings(&project.id, huge).unwrap_err();
    assert!(err.to_string().contains("icon"), "{err}");

    // Exactly at the limit is fine: the bound is a ceiling, not a cliff.
    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            icon: Some("x".repeat(MAX_ICON_BYTES)),
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn settings_for_an_unknown_project_are_an_error_not_defaults() {
    // A stale id must not masquerade as a project with default settings.
    let sandbox = Sandbox::new("settings-unknown");
    let ws = workspace(&sandbox);
    let ghost = ProjectId::new("ghost-000000000000".to_owned());

    assert!(matches!(
        ws.project_settings(&ghost),
        Err(KetError::UnknownProject(_))
    ));
    assert!(matches!(
        ws.set_project_settings(&ghost, ProjectSettings::default()),
        Err(KetError::UnknownProject(_))
    ));
}

#[test]
fn restoring_defaults_leaves_no_entry_behind() {
    let sandbox = Sandbox::new("settings-reset");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            color: Some("#ef4444".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    ws.set_project_settings(&project.id, ProjectSettings::default())
        .unwrap();

    let state = Store::at(sandbox.path("state.json")).load().unwrap();
    assert!(state.project_settings.is_empty());
}

#[test]
fn removing_a_project_takes_its_settings_with_it() {
    let sandbox = Sandbox::new("settings-remove");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            display_name: Some("gone soon".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    ws.remove_project(&project.id, false).unwrap();

    let state = Store::at(sandbox.path("state.json")).load().unwrap();
    assert!(state.project_settings.is_empty());
}

#[test]
fn a_settings_entry_with_an_unknown_field_still_loads() {
    // A newer ket writing a field this one does not know must not make this
    // one refuse to start — the same policy the layout blob follows.
    let sandbox = Sandbox::new("settings-forward");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let path = sandbox.path("state.json");
    let mut state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    state["project_settings"] = serde_json::json!({
        project.id.as_str(): {
            "displayName": "from the future",
            "fromTheFuture": true
        }
    });
    fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();

    let settings = ws.project_settings(&project.id).unwrap();
    assert_eq!(settings.display_name.as_deref(), Some("from the future"));
}

#[test]
fn changing_the_default_base_changes_what_the_next_worktree_is_based_on() {
    // The difference between a setting and a stored string nobody reads.
    let sandbox = Sandbox::new("settings-base");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "develop"]);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    assert_eq!(project.default_base, "main");

    ws.set_default_base(&project.id, "develop").unwrap();

    let worktree = ws
        .create_worktree(&project.id, "attempt", None, None)
        .unwrap();
    assert_eq!(worktree.base, "develop");
}

#[test]
fn a_default_base_that_is_not_a_branch_is_refused() {
    // Otherwise the mistake surfaces at the next `worktree add`, with git's
    // message, far from the dialog it was made in.
    let sandbox = Sandbox::new("settings-base-missing");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let err = ws
        .set_default_base(&project.id, "no-such-branch")
        .unwrap_err();
    assert!(err.to_string().contains("no-such-branch"), "{err}");
    assert!(ws.set_default_base(&project.id, "  ").is_err());

    let unchanged = ws.projects().unwrap().into_iter().next().unwrap();
    assert_eq!(unchanged.default_base, "main");
}

#[test]
fn the_preferred_agent_must_be_a_configured_one() {
    let sandbox = Sandbox::new("settings-agent");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let config = Config::default();
    let known = config
        .agents
        .first()
        .map(|agent| agent.name.clone())
        .expect("the default config ships at least one agent");
    let ws = Workspace::with_config(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        config,
    );
    let project = ws.add_project(&repo).unwrap();

    let err = ws
        .set_preferred_agent(&project.id, Some("hal9000"))
        .unwrap_err();
    assert!(err.to_string().contains("hal9000"), "{err}");

    ws.set_preferred_agent(&project.id, Some(&known)).unwrap();
    let project = ws.projects().unwrap().into_iter().next().unwrap();
    assert_eq!(project.preferred_agent.as_deref(), Some(known.as_str()));

    // Clearing is a first-class outcome, not an error.
    ws.set_preferred_agent(&project.id, None).unwrap();
    let project = ws.projects().unwrap().into_iter().next().unwrap();
    assert_eq!(project.preferred_agent, None);
}

#[test]
fn every_change_announces_itself() {
    // A second client — or the sidebar — re-reads on this rather than
    // polling the store.
    let sandbox = Sandbox::new("settings-event");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let mut events = ws.bus().subscribe();

    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            icon: Some("🐙".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    ws.set_default_base(&project.id, "main").unwrap();
    ws.set_preferred_agent(&project.id, None).unwrap();

    let changes = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|envelope| {
            matches!(
                envelope.event,
                Event::ProjectSettingsChanged { ref project_id } if project_id == &project.id
            )
        })
        .count();
    assert_eq!(changes, 3);
}
