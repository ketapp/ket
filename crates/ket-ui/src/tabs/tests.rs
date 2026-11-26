use super::*;

fn leaf_ids(space: &Space) -> Vec<PaneId> {
    let mut ids = Vec::new();
    space.root.leaf_ids(&mut ids);
    ids
}

fn assert_sizes(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
    }
}

fn sample_agent(name: &str) -> AgentSpec {
    AgentSpec {
        name: name.to_owned(),
        transport: ket_core::config::Transport::Pty,
        command: name.to_owned(),
        args: Vec::new(),
        env: Default::default(),
        env_remove: Vec::new(),
        launch: None,
    }
}

#[test]
fn the_new_tab_menu_puts_the_agent_session_after_the_only_separator() {
    let pane = PaneId(3);
    let agent = sample_agent("claude");
    let menu = new_tab_menu(pane, &[agent], false, false, &Theme::dark());

    assert!(menu.opened_by(new_tab_origin(pane).as_ref()));

    let sections = menu.sections();
    assert_eq!(
        sections.len(),
        2,
        "exactly one heading, ahead of the agent rows"
    );
    let titles: Vec<_> = sections
        .iter()
        .flatten()
        .map(|item| item.title.as_ref())
        .collect();
    assert_eq!(titles, ["File", "Terminal", "Browser", "Claude Code"]);

    let actions: Vec<_> = sections
        .iter()
        .flatten()
        .map(|item| item.action.clone())
        .collect();
    assert_eq!(
        actions,
        [
            MenuAction::File(pane),
            MenuAction::Terminal(pane),
            MenuAction::NewBrowser(pane),
            MenuAction::Agent(pane, "claude".into()),
        ],
        "every item acts on the pane whose menu was opened"
    );

    // The hint on the terminal row is the chord the shell really
    // dispatches, so the menu cannot promise a shortcut it does not own.
    let terminal = &sections[0][1];
    assert_eq!(
        terminal.chord.as_ref(),
        Some(&Chord::parse("cmd+t").unwrap())
    );
}

#[test]
fn the_new_tab_menu_lists_every_runnable_agent() {
    let pane = PaneId(3);
    let agents = [sample_agent("claude"), sample_agent("codex")];
    let menu = new_tab_menu(pane, &agents, false, false, &Theme::dark());

    let sections = menu.sections();
    let titles: Vec<_> = sections
        .iter()
        .flatten()
        .map(|item| item.title.as_ref())
        .collect();
    assert_eq!(
        titles,
        ["File", "Terminal", "Browser", "Claude Code", "Codex"],
        "an enabled agent gets its own row, not just the worktree's assigned one"
    );
}

#[test]
fn the_new_tab_menu_has_no_agent_section_without_a_runnable_agent() {
    let pane = PaneId(3);
    let menu = new_tab_menu(pane, &[], false, false, &Theme::dark());

    let sections = menu.sections();
    assert_eq!(
        sections.len(),
        1,
        "nothing to start means no dangling heading either"
    );
    let titles: Vec<_> = sections
        .iter()
        .flatten()
        .map(|item| item.title.as_ref())
        .collect();
    assert_eq!(titles, ["File", "Terminal", "Browser"]);
}

#[test]
fn the_new_tab_menu_filters_by_what_was_typed() {
    use gpui::{KeyDownEvent, Keystroke, Modifiers};

    let mut menu = new_tab_menu(PaneId(0), &[], false, false, &Theme::dark());
    for character in "term".chars() {
        menu.key(&KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::default(),
                key: character.to_string(),
                key_char: Some(character.to_string()),
            },
            is_held: false,
        });
    }

    let sections = menu.sections();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0][0].title.as_ref(), "Terminal");
}

fn image_tab(path: &str) -> Tab {
    Tab {
        title: path.rsplit('/').next().unwrap_or(path).to_owned().into(),
        kind: TabKind::Image(path.to_owned().into()),
        renamed: false,
        pinned: false,
        preview: false,
    }
}

fn preview_editor(path: &str) -> Tab {
    Tab {
        title: path.rsplit('/').next().unwrap_or(path).to_owned().into(),
        kind: TabKind::Editor(path.to_owned().into()),
        renamed: false,
        pinned: false,
        preview: true,
    }
}

#[test]
fn git_previews_replace_only_the_preview_in_the_focused_pane() {
    let mut space = Space::default();
    space.open_tab(Pane::test_tab());
    space.open_preview_tab(preview_editor("/repo/a.rs"));
    space.open_preview_tab(preview_editor("/repo/b.rs"));

    let (tabs, active) = space.root.leaf(PaneId(0)).unwrap();
    assert_eq!(tabs.len(), 2);
    assert!(tabs[0].kind == TabKind::Editor("/tmp/file.rs".into()));
    assert!(tabs[active].kind == TabKind::Editor("/repo/b.rs".into()));
    assert!(tabs[active].preview);
}

#[test]
fn explicitly_opening_a_preview_promotes_it() {
    let mut space = Space::default();
    space.open_preview_tab(preview_editor("/repo/a.rs"));
    let mut lasting = preview_editor("/repo/a.rs");
    lasting.preview = false;
    space.open_tab(lasting);
    space.open_preview_tab(preview_editor("/repo/b.rs"));

    let (tabs, active) = space.root.leaf(PaneId(0)).unwrap();
    assert_eq!(tabs.len(), 2);
    assert!(!tabs[0].preview);
    assert!(tabs[active].kind == TabKind::Editor("/repo/b.rs".into()));
}

#[test]
fn open_file_replaces_a_preview_in_its_slot() {
    let mut space = Space::default();
    space.open_tab(Pane::test_tab());
    space.open_preview_tab(Tab {
        title: "file.rs (diff)".into(),
        kind: TabKind::Diff(crate::diff_view::DiffKey {
            root: "/repo".into(),
            rel: "file.rs".into(),
            base: None,
        }),
        renamed: false,
        pinned: false,
        preview: true,
    });
    let mut file = preview_editor("/repo/file.rs");
    file.preview = false;
    space.open_tab_replacing_preview(file);

    let (tabs, active) = space.root.leaf(PaneId(0)).unwrap();
    assert_eq!(tabs.len(), 2);
    assert_eq!(active, 1);
    assert!(tabs[active].kind == TabKind::Editor("/repo/file.rs".into()));
    assert!(!tabs[active].preview);
}

#[test]
fn an_edited_preview_stays_open_when_another_preview_arrives() {
    let mut space = Space::default();
    space.open_preview_tab(preview_editor("/repo/conflict.rs"));
    assert!(space.promote_preview(&TabKind::Editor("/repo/conflict.rs".into())));
    space.open_preview_tab(preview_editor("/repo/next.rs"));

    let (tabs, _) = space.root.leaf(PaneId(0)).unwrap();
    assert_eq!(tabs.len(), 2);
    assert!(!tabs[0].preview);
    assert!(tabs[1].preview);
}

#[test]
fn previews_in_separate_panes_do_not_replace_each_other() {
    let mut space = Space::default();
    space.open_preview_tab(preview_editor("/repo/left.rs"));
    space.split(Axis::Horizontal);
    space.open_preview_tab(preview_editor("/repo/right.rs"));

    assert_eq!(space.root.leaf(PaneId(0)).unwrap().0.len(), 1);
    assert_eq!(space.root.leaf(PaneId(1)).unwrap().0.len(), 1);
    assert!(space.root.leaf(PaneId(0)).unwrap().0[0].preview);
    assert!(space.root.leaf(PaneId(1)).unwrap().0[0].preview);
}

#[test]
fn opening_the_same_image_twice_focuses_the_one_tab_it_already_has() {
    let mut space = Space::default();
    space.open_tab(image_tab("/repo/shot.png"));
    space.open_tab(Pane::test_tab());
    space.open_tab(image_tab("/repo/shot.png"));

    let (tabs, active) = space.root.leaf(PaneId(0)).unwrap();
    assert_eq!(tabs.len(), 2);
    assert_eq!(active, 0, "the second open focuses the first tab");
}

#[test]
fn an_image_open_in_another_pane_is_focused_there_rather_than_opened_again() {
    let mut space = Space::default();
    space.open_tab(image_tab("/repo/shot.png"));
    space.split(Axis::Horizontal);
    assert_eq!(space.focused, PaneId(1));

    space.open_tab(image_tab("/repo/shot.png"));

    assert_eq!(space.focused, PaneId(0));
    let (tabs, _) = space.root.leaf(PaneId(1)).unwrap();
    assert!(tabs.is_empty(), "the new pane did not get a second copy");
}

#[test]
fn two_images_are_two_tabs() {
    let mut space = Space::default();
    space.open_tab(image_tab("/repo/a.png"));
    space.open_tab(image_tab("/repo/b.png"));

    let (tabs, active) = space.root.leaf(PaneId(0)).unwrap();
    assert_eq!(tabs.len(), 2);
    assert_eq!(active, 1);
    assert_eq!(tabs[1].title.as_ref(), "b.png");
}

#[test]
fn splitting_a_leaf_creates_two_equal_children_and_focuses_the_new_one() {
    let mut space = Space::default();
    space.split(Axis::Horizontal);

    assert_eq!(leaf_ids(&space), vec![PaneId(0), PaneId(1)]);
    assert_eq!(space.focused, PaneId(1));
    match &space.root {
        Pane::Split { axis, sizes, .. } => {
            assert_eq!(*axis, Axis::Horizontal);
            assert_eq!(sizes, &[0.5, 0.5]);
        }
        Pane::Leaf { .. } => panic!("split left a leaf at the root"),
    }
}

#[test]
fn closing_a_pane_restores_the_surviving_leaf_as_the_root() {
    let mut space = Space::default();
    space.split(Axis::Vertical);

    assert!(space.close_focused_pane());
    assert_eq!(leaf_ids(&space), vec![PaneId(0)]);
    assert_eq!(space.focused, PaneId(0));
    assert!(matches!(space.root, Pane::Leaf { .. }));
}

#[test]
fn closing_the_last_tab_collapses_its_leaf() {
    let mut space = Space::default();
    space.split(Axis::Horizontal);
    // Panes open empty now, so give this one the tab the test is about
    // to close.
    space.open_tab_in(PaneId(1), Pane::test_tab());

    space.close_tab(PaneId(1), 0);

    assert_eq!(leaf_ids(&space), vec![PaneId(0)]);
    assert_eq!(space.focused, PaneId(0));
}

#[test]
fn a_split_with_one_child_collapses_into_its_parent() {
    let mut space = Space::default();
    space.split(Axis::Horizontal);
    space.split(Axis::Vertical);

    assert!(space.close_focused_pane());

    assert_eq!(leaf_ids(&space), vec![PaneId(0), PaneId(1)]);
    assert!(matches!(
        space.root,
        Pane::Split {
            axis: Axis::Horizontal,
            ..
        }
    ));
}

#[test]
fn focus_moves_to_the_next_leaf_when_the_focused_one_disappears() {
    let mut space = Space::default();
    space.split(Axis::Horizontal);
    space.focused = PaneId(0);

    assert!(space.close_focused_pane());
    assert_eq!(space.focused, PaneId(1));
}

#[test]
fn the_only_leaf_cannot_be_closed() {
    let mut space = Space::default();
    space.open_tab_in(PaneId(0), Pane::test_tab());

    assert!(!space.close_focused_pane());
    space.close_tab(PaneId(0), 0);

    // The leaf survives losing its last tab when it is the only one:
    // collapsing it would leave the worktree with no pane at all.
    assert_eq!(leaf_ids(&space), vec![PaneId(0)]);
}

#[test]
fn dragging_a_divider_changes_only_its_adjacent_shares() {
    let mut space = Space {
        root: Pane::Split {
            axis: Axis::Horizontal,
            children: vec![
                Pane::empty(PaneId(0)),
                Pane::empty(PaneId(1)),
                Pane::empty(PaneId(2)),
            ],
            sizes: vec![0.2, 0.3, 0.5],
        },
        focused: PaneId(0),
        next_id: 3,
    };

    // A thousand pixels of panes between two dividers, and the second
    // divider dragged to 700 of them.
    let extent = px(1000.0) + DIVIDER_SIZE * 2;
    assert!(space.resize_split(&[], 1, px(700.0) + DIVIDER_SIZE, extent));

    let Pane::Split { sizes, .. } = &space.root else {
        panic!("root is not split");
    };
    assert_sizes(sizes, &[0.2, 0.5, 0.3]);
}

#[test]
fn dragging_a_divider_stops_before_a_pane_becomes_unreachable() {
    let mut space = Space::default();
    space.split(Axis::Horizontal);

    assert!(space.resize_split(&[], 0, px(10.0), px(1000.0) + DIVIDER_SIZE));

    let Pane::Split { sizes, .. } = &space.root else {
        panic!("root is not split");
    };
    let least = MIN_PANE_SIZE / px(1000.0);
    assert_sizes(sizes, &[least, 1.0 - least]);
}

#[test]
fn a_nested_split_reserves_room_for_every_leaf_on_the_same_axis() {
    let mut space = Space::default();
    space.split(Axis::Horizontal);
    space.focused = PaneId(0);
    space.split(Axis::Horizontal);

    assert!(space.resize_split(&[], 0, px(10.0), px(1000.0) + DIVIDER_SIZE));

    let Pane::Split { sizes, .. } = &space.root else {
        panic!("root is not split");
    };
    // The nested split keeps room for both its panes and the divider
    // between them.
    let least = (MIN_PANE_SIZE * 2.0 + DIVIDER_SIZE) / px(1000.0);
    assert_sizes(sizes, &[least, 1.0 - least]);
}

#[test]
fn closing_a_pane_preserves_the_remaining_resize_proportions() {
    let mut space = Space {
        root: Pane::Split {
            axis: Axis::Horizontal,
            children: vec![
                Pane::empty(PaneId(0)),
                Pane::empty(PaneId(1)),
                Pane::empty(PaneId(2)),
            ],
            sizes: vec![0.2, 0.3, 0.5],
        },
        focused: PaneId(1),
        next_id: 3,
    };

    assert!(space.close_focused_pane());

    let Pane::Split { sizes, .. } = &space.root else {
        panic!("root is not split");
    };
    assert_sizes(sizes, &[2.0 / 7.0, 5.0 / 7.0]);
}
