use super::*;
use gpui::{Action as _, MenuItem};

#[test]
fn the_app_menu_offers_quit() {
    // A menu bar without a Quit item is the bug: macOS resolves Cmd-Q
    // against the menu's key equivalents, so no Quit item means no way to
    // quit from the keyboard. The item's equivalent comes from the
    // QUIT_CHORD binding, which is asserted separately below.
    let menus = app_menus(&crate::app_menu::MenuState {
        status_bar_open: true,
        ..Default::default()
    });
    let app_menu = menus
        .iter()
        .find(|menu| menu.name.as_ref() == "ket")
        .expect("the app menu exists");

    assert!(
        app_menu.items.iter().any(|item| match item {
            MenuItem::Action { action, .. } => action.name() == Quit.name(),
            _ => false,
        }),
        "the app menu has a Quit item"
    );
}

#[test]
fn the_view_menu_offers_zoom() {
    // The chords are bound in `main`; the items are how someone finds
    // out they exist, and how a native view that took the chord — a web
    // page — still hands it on.
    let menus = app_menus(&crate::app_menu::MenuState {
        status_bar_open: true,
        ..Default::default()
    });
    let view = menus
        .iter()
        .find(|menu| menu.name.as_ref() == "View")
        .expect("the View menu exists");
    for wanted in [ZoomIn.name(), ZoomOut.name(), ActualSize.name()] {
        assert!(
            view.items.iter().any(|item| match item {
                MenuItem::Action { action, .. } => action.name() == wanted,
                _ => false,
            }),
            "the View menu has {wanted}"
        );
    }
}

#[test]
fn switch_worktree_actions_keep_ids_when_rows_reorder() {
    use crate::app_menu::{MenuState, MenuWorktree, SwitchWorktree};
    let first = WorktreeId::new("first");
    let second = WorktreeId::new("second");
    let menu_for = |ids: &[WorktreeId]| {
        let state = MenuState {
            selected_worktree: Some(second.clone()),
            worktrees: ids
                .iter()
                .map(|id| MenuWorktree {
                    id: id.clone(),
                    name: id.to_string(),
                })
                .collect(),
            ..Default::default()
        };
        app_menus(&state)
            .into_iter()
            .find(|menu| menu.name.as_ref() == "Worktree")
            .unwrap()
            .items
            .into_iter()
            .find_map(|item| match item {
                MenuItem::Submenu(menu) if menu.name.as_ref() == "Switch Worktree" => {
                    Some(menu.items)
                }
                _ => None,
            })
            .unwrap()
            .into_iter()
            .map(|item| match item {
                MenuItem::Action {
                    action, checked, ..
                } => (
                    action
                        .as_any()
                        .downcast_ref::<SwitchWorktree>()
                        .unwrap()
                        .id
                        .clone(),
                    checked,
                ),
                _ => panic!("expected worktree action"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        menu_for(&[first.clone(), second.clone()]),
        vec![(first.clone(), false), (second.clone(), true)]
    );
    assert_eq!(
        menu_for(&[second.clone(), first.clone()]),
        vec![(second, true), (first, false)]
    );
}

#[test]
fn agent_menu_shortcuts_follow_runnable_order() {
    let state = crate::app_menu::MenuState {
        agents: vec!["claude".into(), "codex".into()],
        ..Default::default()
    };
    let file = app_menus(&state)
        .into_iter()
        .find(|menu| menu.name.as_ref() == "File")
        .unwrap();
    let agent_items = file
        .items
        .into_iter()
        .find_map(|item| match item {
            MenuItem::Submenu(menu) if menu.name.as_ref() == "New Agent Session" => {
                Some(menu.items)
            }
            _ => None,
        })
        .unwrap();
    for (index, item) in agent_items.into_iter().enumerate() {
        let MenuItem::Action { key_equivalent, .. } = item else {
            panic!("expected agent action");
        };
        assert_eq!(key_equivalent, crate::shortcuts::agent_session_chord(index));
    }
}

#[test]
fn modal_disables_commands_behind_its_fields() {
    fn find_action<'a>(items: &'a [MenuItem], wanted: &str) -> Option<&'a MenuItem> {
        items.iter().find_map(|item| match item {
            MenuItem::Action { name, .. } if name.as_ref() == wanted => Some(item),
            MenuItem::Submenu(menu) => find_action(&menu.items, wanted),
            _ => None,
        })
    }

    let state = crate::app_menu::MenuState {
        modal_open: true,
        selected_project: Some(ProjectId::new("project")),
        selected_worktree: Some(WorktreeId::new("worktree")),
        projects: vec![crate::app_menu::MenuProject {
            id: ProjectId::new("project"),
            name: "Project A".into(),
        }],
        worktrees: vec![crate::app_menu::MenuWorktree {
            id: WorktreeId::new("worktree"),
            name: "Worktree A".into(),
        }],
        economy_levels: vec![crate::app_menu::MenuEconomyLevel {
            level: 1,
            name: "Economy Level".into(),
        }],
        has_tab: true,
        has_multiple_panes: true,
        can_new_worktree: true,
        ..Default::default()
    };
    let menus = app_menus(&state);
    for wanted in [
        "Change Tab Title…",
        "Split Pane Right",
        "Close Pane",
        "Close Other Tabs",
        "Pin/Unpin Tab",
        "New Worktree…",
        "Project A",
        "Worktree A",
        "Economy Level",
    ] {
        assert!(
            menus.iter().any(|menu| matches!(
                find_action(&menu.items, wanted),
                Some(MenuItem::Action { enabled: false, .. })
            )),
            "{wanted} remains enabled behind a modal"
        );
    }
    #[cfg(target_os = "macos")]
    assert!(menus.iter().any(|menu| matches!(
        find_action(&menu.items, "Paste"),
        Some(MenuItem::Action { enabled: true, .. })
    )));
    for wanted in ["Minimize", "Zoom", "Enter Full Screen"] {
        assert!(
            menus.iter().any(|menu| matches!(
                find_action(&menu.items, wanted),
                Some(MenuItem::Action { enabled: true, .. })
            )),
            "{wanted} is disabled by a modal"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn unsupported_edit_history_is_disabled() {
    let state = crate::app_menu::MenuState::default();
    let edit = app_menus(&state)
        .into_iter()
        .find(|menu| menu.name.as_ref() == "Edit")
        .unwrap();
    for name in ["Undo", "Redo", "Cut"] {
        assert!(edit.items.iter().any(|item| matches!(item,
            MenuItem::Action { name: item_name, enabled: false, .. } if item_name == name
        )));
    }
}

#[test]
fn the_quit_chord_is_the_platforms_own() {
    if cfg!(target_os = "macos") {
        assert_eq!(QUIT_CHORD, "cmd-q");
    } else {
        assert_eq!(QUIT_CHORD, "alt-f4");
    }
}

#[test]
fn no_overlay_defers_itself() {
    // `render` above defers every overlay exactly once. A view that also
    // defers itself nests a deferred draw inside a deferred draw, and gpui
    // does not merely misdraw that — it aborts the process the moment the
    // overlay opens ("cannot call defer_draw during deferred drawing").
    // That shipped once, in the project menu, so it is worth a tripwire:
    // these files supply overlays, and none of them may call `deferred`.
    // If you need a deferred element in one of them for something that is
    // *not* an overlay, this test is the place to say so.
    let sources = [
        ("projects.rs", include_str!("projects.rs")),
        ("palette.rs", include_str!("palette.rs")),
        ("explorer.rs", include_str!("explorer.rs")),
        ("worktree_menu.rs", include_str!("worktree_menu.rs")),
        ("worktree_dialog.rs", include_str!("worktree_dialog.rs")),
    ];

    for (name, source) in sources {
        // Skip the prose: the doc comments here explain the rule.
        let code = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("deferred("),
            "{name} defers an element itself; the shell defers overlays"
        );
    }
}
