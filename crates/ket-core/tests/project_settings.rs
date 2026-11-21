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
use ket_core::project_image::{self, MAX_IMAGE_BYTES};
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
                image: None,
                hide_discovered_worktrees: true,
                build_dirs: None,
                agents: Default::default(),
                backlog_in_repo: false,
                github_issues: false,
                github_account: None,
                pinned: false,
                hidden: false,
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
    // Nor does one written before there were badge images.
    assert_eq!(settings.image, None);
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

// ---- badge images -----------------------------------------------------------

/// A real one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

const SVG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="8" fill="#22c55e"/></svg>
"##;

/// Imports `source` and writes the settings with it, as a dialog's Save does.
fn choose_image(ws: &Workspace, project: &ProjectId, source: &std::path::Path) {
    let image = ws.import_project_image(project, source).unwrap();
    let mut settings = ws.project_settings(project).unwrap();
    settings.image = Some(image);
    ws.set_project_settings(project, settings).unwrap();
}

/// What is in the project's directory of images, by name.
fn copies(sandbox: &Sandbox, project: &ProjectId) -> Vec<String> {
    let dir = sandbox.path("project-images").join(project.as_str());
    let mut names: Vec<String> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

#[test]
fn a_chosen_image_is_copied_in_and_outlives_the_original() {
    let sandbox = Sandbox::new("image-copy");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let original = sandbox.path("Downloads/acme-logo.png");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, PNG).unwrap();
    choose_image(&ws, &project.id, &original);

    // The Downloads folder is tidied.
    fs::remove_file(&original).unwrap();

    let settings = workspace(&sandbox).project_settings(&project.id).unwrap();
    let path = ws
        .project_image_path(&project.id, &settings)
        .expect("the project has an image");
    // Kept beside the store, under the project, by contents.
    assert!(
        path.starts_with(sandbox.path("project-images").join(project.id.as_str())),
        "{}",
        path.display()
    );
    assert_eq!(path.extension().unwrap(), "png");
    assert!(!settings.image.as_deref().unwrap().contains("acme-logo"));
    assert_eq!(fs::read(&path).unwrap(), PNG);
}

#[test]
fn clearing_an_image_deletes_the_copy_and_leaves_the_icon_and_colour_alone() {
    let sandbox = Sandbox::new("image-keeps-icon");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            color: Some("#22c55e".to_owned()),
            icon: Some("glyph:leaf".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();

    let source = sandbox.path("acme.svg");
    fs::write(&source, SVG).unwrap();
    choose_image(&ws, &project.id, &source);
    let chosen = ws.project_settings(&project.id).unwrap();
    assert!(chosen.image.as_deref().unwrap().ends_with(".svg"));
    assert_eq!(chosen.color.as_deref(), Some("#22c55e"));
    assert_eq!(chosen.icon.as_deref(), Some("glyph:leaf"));

    let mut cleared = chosen;
    cleared.image = None;
    ws.set_project_settings(&project.id, cleared).unwrap();
    let after = ws.project_settings(&project.id).unwrap();
    assert_eq!(after.image, None);
    assert_eq!(after.color.as_deref(), Some("#22c55e"));
    assert_eq!(after.icon.as_deref(), Some("glyph:leaf"));
    // Clearing deleted the copy, and the project's directory with it.
    assert!(
        !sandbox
            .path("project-images")
            .join(project.id.as_str())
            .exists()
    );
}

#[test]
fn a_new_image_replaces_the_old_copy_under_a_new_name() {
    // A new name for a new picture is what keeps an image cache keyed by
    // path from drawing the old one.
    let sandbox = Sandbox::new("image-replace");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let first = sandbox.path("acme.png");
    fs::write(&first, PNG).unwrap();
    choose_image(&ws, &project.id, &first);
    let before = ws
        .project_image_path(&project.id, &ws.project_settings(&project.id).unwrap())
        .unwrap();

    // The same file name, new contents.
    fs::write(&first, SVG).unwrap();
    choose_image(&ws, &project.id, &first);
    let after = ws
        .project_image_path(&project.id, &ws.project_settings(&project.id).unwrap())
        .unwrap();

    assert_ne!(before, after);
    assert!(!before.exists());
    assert_eq!(fs::read_to_string(&after).unwrap(), SVG);
    assert_eq!(copies(&sandbox, &project.id).len(), 1);

    // The same picture again is the same copy.
    let again = ws.import_project_image(&project.id, &first).unwrap();
    assert_eq!(Some(again.as_str()), after.file_name().unwrap().to_str());
}

#[test]
fn a_file_a_badge_cannot_show_is_refused_by_name_with_the_reason() {
    let sandbox = Sandbox::new("image-refused");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let bmp = sandbox.path("acme.bmp");
    fs::write(&bmp, b"BM\x36\0\0\0\0\0\0\0\x36\0\0\0").unwrap();
    // Named .png, but it is not one: gpui decodes by contents, and so does this.
    let renamed = sandbox.path("acme-renamed.png");
    fs::write(&renamed, b"GIF8 not quite").unwrap();
    let huge = sandbox.path("acme-huge.png");
    let mut big = PNG.to_vec();
    big.resize(usize::try_from(MAX_IMAGE_BYTES).unwrap() + 1, 0);
    fs::write(&huge, &big).unwrap();
    let empty = sandbox.path("acme-empty.png");
    fs::write(&empty, b"").unwrap();
    let folder = sandbox.path("acme-folder");
    fs::create_dir_all(&folder).unwrap();
    let missing = sandbox.path("acme-missing.png");

    for (source, why) in [
        (&bmp, "is not an image a badge can show"),
        (&renamed, "PNG, JPEG, GIF, WebP, SVG or ICO"),
        (&huge, "a project image can be at most 1 MB"),
        (&empty, "is empty"),
        (&folder, "is a folder"),
        (&missing, "is not there"),
    ] {
        let name = source.file_name().unwrap().to_string_lossy().into_owned();
        let checked = project_image::check(source).unwrap_err().to_string();
        assert!(checked.contains(&name), "{checked}");
        assert!(checked.contains(why), "{checked}");

        let imported = ws
            .import_project_image(&project.id, source)
            .unwrap_err()
            .to_string();
        assert_eq!(imported, checked);
    }
    assert!(copies(&sandbox, &project.id).is_empty());
    assert_eq!(
        ws.project_settings(&project.id).unwrap(),
        ProjectSettings::default()
    );

    // Exactly at the cap is fine.
    big.truncate(usize::try_from(MAX_IMAGE_BYTES).unwrap());
    fs::write(&huge, &big).unwrap();
    assert_eq!(
        project_image::check(&huge).unwrap(),
        project_image::ImageFormat::Png
    );
}

#[test]
fn an_image_name_that_was_never_imported_is_refused() {
    let sandbox = Sandbox::new("image-unimported");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let err = ws
        .set_project_settings(
            &project.id,
            ProjectSettings {
                image: Some("0123456789abcdef.png".to_owned()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("0123456789abcdef.png"), "{err}");

    // Nor may it be a way out of the project's directory.
    for path in ["../../state.json", "/etc/hosts", "sub/acme.png"] {
        let err = ws
            .set_project_settings(
                &project.id,
                ProjectSettings {
                    image: Some(path.to_owned()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, KetError::Config(_)), "{path}: {err}");
    }
    assert_eq!(
        ws.project_settings(&project.id).unwrap(),
        ProjectSettings::default()
    );
}

#[test]
fn other_changes_carry_the_image_through_even_once_its_copy_is_gone() {
    // A menu's Pin to Top reads the settings, flips one field and writes them
    // back. It must neither drop the picture nor fail because someone deleted
    // the copy by hand.
    let sandbox = Sandbox::new("image-carried");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let source = sandbox.path("acme.png");
    fs::write(&source, PNG).unwrap();
    choose_image(&ws, &project.id, &source);
    let image = ws.project_settings(&project.id).unwrap().image;

    let mut settings = ws.project_settings(&project.id).unwrap();
    settings.pinned = true;
    ws.set_project_settings(&project.id, settings).unwrap();
    assert_eq!(ws.project_settings(&project.id).unwrap().image, image);

    fs::remove_dir_all(sandbox.path("project-images")).unwrap();
    let mut settings = ws.project_settings(&project.id).unwrap();
    settings.hidden = true;
    ws.set_project_settings(&project.id, settings).unwrap();
    let settings = ws.project_settings(&project.id).unwrap();
    assert_eq!(settings.image, image);
    assert!(settings.pinned && settings.hidden);
}

#[test]
fn a_copy_no_save_came_to_name_is_deleted_by_the_next_write() {
    // A dialog imports at Save, then writes; if the write never happens, the
    // copy is a stray, and the project's next write tidies it.
    let sandbox = Sandbox::new("image-stray");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let source = sandbox.path("acme.png");
    fs::write(&source, PNG).unwrap();
    choose_image(&ws, &project.id, &source);
    let kept = ws.project_settings(&project.id).unwrap().image.unwrap();

    let other = sandbox.path("acme.svg");
    fs::write(&other, SVG).unwrap();
    ws.import_project_image(&project.id, &other).unwrap();
    assert_eq!(copies(&sandbox, &project.id).len(), 2);

    let mut settings = ws.project_settings(&project.id).unwrap();
    settings.pinned = true;
    ws.set_project_settings(&project.id, settings).unwrap();
    assert_eq!(copies(&sandbox, &project.id), vec![kept]);
}

#[test]
fn removing_a_project_takes_its_image_with_it() {
    let sandbox = Sandbox::new("image-remove");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let source = sandbox.path("acme.png");
    fs::write(&source, PNG).unwrap();
    choose_image(&ws, &project.id, &source);

    ws.remove_project(&project.id, false).unwrap();
    assert!(
        !sandbox
            .path("project-images")
            .join(project.id.as_str())
            .exists()
    );

    // Added again, it starts without one.
    let again = ws.add_project(&repo).unwrap();
    let settings = ws.project_settings(&again.id).unwrap();
    assert_eq!(settings.image, None);
    assert_eq!(ws.project_image_path(&again.id, &settings), None);
}

#[test]
fn an_image_for_an_unknown_project_is_refused_before_anything_is_copied() {
    let sandbox = Sandbox::new("image-unknown");
    let ws = workspace(&sandbox);
    let ghost = ProjectId::new("ghost-000000000000".to_owned());
    let source = sandbox.path("acme.png");
    fs::write(&source, PNG).unwrap();

    assert!(matches!(
        ws.import_project_image(&ghost, &source),
        Err(KetError::UnknownProject(_))
    ));
    assert!(!sandbox.path("project-images").exists());
}

#[test]
fn a_project_without_an_image_is_written_as_it_was_before_there_were_images() {
    let text = serde_json::to_string(&ProjectSettings {
        icon: Some("glyph:leaf".to_owned()),
        ..Default::default()
    })
    .unwrap();
    assert!(!text.contains("image"), "{text}");
}
