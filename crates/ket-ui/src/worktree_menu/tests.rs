use super::*;

/// The menu under test, at the default token-reduction level.
fn default_menu(primary: bool) -> OpenMenu<WorktreeAction> {
    worktree_menu(
        Point::default(),
        primary,
        false,
        false,
        ket_core::worktree::default_token_reduction(),
        None,
        None,
        false,
        false,
        false,
        &Theme::dark(),
    )
}

fn menu_with_failed_checks(primary: bool, failed: usize) -> OpenMenu<WorktreeAction> {
    let pull_request = (failed > 0).then(|| status_with_failed(failed));
    git_menu(primary, pull_request.as_ref())
}

fn status_with_failed(failed: usize) -> ket_core::github::PullRequestStatus {
    use ket_core::github::{Check, CheckState, MergeMethods, PullRequestState, Review};
    ket_core::github::PullRequestStatus {
        number: 17,
        url: "https://github.com/o/r/pull/17".to_owned(),
        branch: "fix".to_owned(),
        draft: false,
        review: Review::None,
        checks: (0..failed)
            .map(|at| Check {
                name: format!("check {at}"),
                state: CheckState::Failed,
                url: String::new(),
            })
            .collect(),
        state: PullRequestState::Open,
        base: "main".to_owned(),
        behind: false,
        unresolved_comments: 0,
        merge_methods: MergeMethods::default(),
    }
}

fn item<'a>(
    menu: &'a OpenMenu<WorktreeAction>,
    title: &str,
) -> &'a crate::ui::menu::MenuItem<WorktreeAction> {
    menu.sections()
        .into_iter()
        .flatten()
        .find(|item| item.title == title)
        .unwrap_or_else(|| panic!("{title} is in the menu"))
}

#[test]
fn fixing_checks_waits_for_one_to_fail_and_says_how_many() {
    // Whether it can be picked, and what it says beside its name.
    let fix = |failed: usize| {
        let menu = menu_with_failed_checks(false, failed);
        let item = menu
            .sections()
            .into_iter()
            .flatten()
            .find(|item| item.title == "Fix checks with agent")
            .expect("Fix checks with agent is in the menu");
        (
            item.enabled,
            item.subtitle.as_ref().map(ToString::to_string),
        )
    };
    assert_eq!(fix(0), (false, None));
    assert_eq!(fix(2), (true, Some("2 failed".to_owned())));
}

#[test]
fn mark_as_unread_waits_for_a_row_without_its_dot() {
    let menu = |unread: bool| {
        worktree_menu(
            Point::default(),
            false,
            false,
            unread,
            ket_core::worktree::default_token_reduction(),
            None,
            None,
            false,
            false,
            false,
            &Theme::dark(),
        )
    };
    assert!(item(&menu(false), "Mark as Unread").enabled);
    assert!(!item(&menu(true), "Mark as Unread").enabled);
    // The repository's own checkout has agents too.
    assert!(item(&default_menu(true), "Mark as Unread").enabled);
}

#[test]
fn sleep_and_wake_follow_agent_tabs() {
    let menu = worktree_menu(
        Point::default(),
        false,
        false,
        false,
        ket_core::worktree::default_token_reduction(),
        None,
        None,
        true,
        false,
        false,
        &Theme::dark(),
    );
    assert!(
        menu.sections()
            .into_iter()
            .flatten()
            .any(|item| item.title == "Sleep" && item.enabled)
    );
    let menu = worktree_menu(
        Point::default(),
        false,
        false,
        false,
        ket_core::worktree::default_token_reduction(),
        None,
        None,
        false,
        true,
        false,
        &Theme::dark(),
    );
    assert!(
        menu.sections()
            .into_iter()
            .flatten()
            .any(|item| item.title == "Wake" && item.enabled)
    );
}

#[test]
fn pull_request_actions_wait_for_a_pull_request_that_allows_them() {
    use ket_core::github::{PullRequestState, Review};

    let idle = git_menu(false, None);
    assert!(!item(&idle, "Merge pull request").enabled);
    assert!(!item(&idle, "Update branch").enabled);
    assert!(!item(&idle, "Ready for review").enabled);
    assert!(!item(&idle, "Reopen pull request").enabled);
    assert!(!item(&idle, "Address comments with agent").enabled);

    let mut open = status_with_failed(0);
    open.draft = false;
    let merge = git_menu(false, Some(&open));
    let merge_item = item(&merge, "Merge pull request");
    assert!(merge_item.enabled);
    assert_eq!(
        merge_item.subtitle.as_ref().map(ToString::to_string),
        Some("into main".to_owned())
    );
    assert!(!item(&merge, "Ready for review").enabled);

    open.behind = true;
    let update = git_menu(false, Some(&open));
    assert!(item(&update, "Update branch").enabled);

    open.draft = true;
    let draft = git_menu(false, Some(&open));
    assert!(!item(&draft, "Merge pull request").enabled);
    assert!(item(&draft, "Ready for review").enabled);

    open.draft = false;
    open.behind = false;
    open.review = Review::ChangesRequested;
    open.unresolved_comments = 2;
    let comments = git_menu(false, Some(&open));
    let comments_item = item(&comments, "Address comments with agent");
    assert!(comments_item.enabled);
    assert_eq!(
        comments_item.subtitle.as_ref().map(ToString::to_string),
        Some("2 comments".to_owned())
    );

    open.state = PullRequestState::Closed;
    open.review = Review::None;
    open.unresolved_comments = 0;
    let closed = git_menu(false, Some(&open));
    assert!(item(&closed, "Reopen pull request").enabled);
    assert!(!item(&closed, "Merge pull request").enabled);
}

#[test]
fn the_checkout_cannot_be_deleted_but_still_shows_the_row() {
    // Removing the repository's own checkout would take the project with
    // it, and a menu that silently omits the row teaches the reader the
    // wrong shape.
    let menu = default_menu(true);
    let delete = menu
        .sections()
        .into_iter()
        .flatten()
        .find(|item| item.title == "Delete")
        .expect("Delete is in the menu");

    assert!(!delete.enabled);
    assert!(delete.danger);
}

#[test]
fn a_real_worktree_can_be_deleted() {
    let menu = default_menu(false);
    let delete = menu
        .sections()
        .into_iter()
        .flatten()
        .find(|item| item.title == "Delete")
        .expect("Delete is in the menu");

    assert!(delete.enabled);
    // The hint is the shortcut the design promises.
    assert_eq!(
        crate::ui::menu::glyphs(delete.chord.as_ref().expect("Delete has a chord")).as_ref(),
        "⇧⌘⌫"
    );
}

fn titles(menu: &OpenMenu<WorktreeAction>) -> Vec<String> {
    menu.sections()
        .iter()
        .flatten()
        .map(|item| item.title.to_string())
        .collect()
}

#[test]
fn shift_retitles_the_delete_row_and_letting_go_restores_it() {
    let mut menu = default_menu(false);

    assert!(retitle_delete(&mut menu, true));
    assert!(titles(&menu).contains(&"Delete, don't ask".to_owned()));
    assert!(!titles(&menu).contains(&"Delete".to_owned()));
    // Shift still held on the next modifiers change: nothing to redraw.
    assert!(!retitle_delete(&mut menu, true));

    // Still the danger row with its chord, and still enabled: only the
    // words changed, so it is picked and drawn as before.
    let delete = menu
        .item_mut(|action| *action == WorktreeAction::Delete)
        .expect("Delete is in the menu");
    assert!(delete.danger);
    assert!(delete.enabled);
    assert!(delete.chord.is_some());

    assert!(retitle_delete(&mut menu, false));
    assert!(titles(&menu).contains(&"Delete".to_owned()));
    assert!(!retitle_delete(&mut menu, false));
}

#[test]
fn the_checkouts_delete_row_stays_disabled_whatever_shift_says() {
    let mut menu = default_menu(true);
    assert!(retitle_delete(&mut menu, true));
    let delete = menu
        .item_mut(|action| *action == WorktreeAction::Delete)
        .expect("Delete is in the menu");
    assert_eq!(delete.title.as_ref(), "Delete, don't ask");
    assert!(!delete.enabled);
}

#[test]
fn only_a_shift_click_on_delete_skips_asking() {
    assert!(skips_asking(WorktreeAction::Delete, true));
    assert!(!skips_asking(WorktreeAction::Delete, false));
    // Shift means nothing to the other rows.
    assert!(!skips_asking(WorktreeAction::Rename, true));
    assert!(!skips_asking(WorktreeAction::ClearBuildOutput, true));
    assert!(!skips_asking(WorktreeAction::Pin, true));
}

#[test]
fn a_delete_told_not_to_ask_runs_behind_no_dialog() {
    let target = WorktreeTarget {
        id: WorktreeId::from("wt"),
        branch: "fix-the-login".into(),
        label: "fix-the-login".into(),
        path: "/tmp/fix-the-login".into(),
        token_reduction: ket_core::worktree::default_token_reduction(),
        agent: None,
        pinned: false,
        at: Point::default(),
    };

    let quiet = RemoveWorktreeConfirm::asking(target.clone(), false);
    assert!(quiet.quiet);
    assert!(!quiet.pending);
    assert!(quiet.refusal.is_none());

    // The ordinary dialog asks before deleting.
    let asked = RemoveWorktreeConfirm::asking(target, true);
    assert!(!asked.quiet);
    assert!(!asked.pending);
}

fn key(key: &str) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: gpui::Keystroke {
            key: key.into(),
            key_char: None,
            modifiers: gpui::Modifiers::default(),
        },
        is_held: false,
    }
}

#[test]
fn git_actions_are_grouped_and_keep_the_primary_checkout_restriction() {
    let parent = default_menu(false);
    let git = git_menu(false, None);
    let parent_actions: Vec<_> = parent
        .sections()
        .into_iter()
        .flatten()
        .map(|item| item.action)
        .collect();
    assert!(item(&parent, "Git").submenu);
    assert_eq!(git.sections().into_iter().flatten().count(), 7);
    assert!(
        git.sections()
            .into_iter()
            .flatten()
            .all(|item| !parent_actions.contains(&item.action))
    );
    assert!(item(&git, "Push and open pull request").enabled);
    assert!(!item(&git_menu(true, None), "Push and open pull request").enabled);
}

#[test]
fn keyboard_opens_git_picks_an_action_and_returns_to_the_parent() {
    let mut open = WorktreeMenu::new(default_menu(false), false, None);
    // Clear build output, snippets, then Git.
    open.key(&key("down"));
    open.key(&key("down"));
    assert!(matches!(open.key(&key("right")), MenuKey::Consumed));
    assert!(open.git.is_some());
    assert!(matches!(
        open.key(&key("enter")),
        MenuKey::Run(WorktreeAction::PullRequest)
    ));
    assert!(matches!(open.key(&key("left")), MenuKey::Consumed));
    assert!(open.git.is_none());
    // Selection remains on Git after returning to the parent.
    assert!(matches!(
        open.key(&key("enter")),
        MenuKey::Run(WorktreeAction::Git)
    ));
    open.open_git();
    assert!(matches!(open.key(&key("escape")), MenuKey::Consumed));
    assert!(open.git.is_none());
    assert!(matches!(open.key(&key("escape")), MenuKey::Close));
}

#[test]
fn the_rows_that_lead_somewhere_say_so() {
    let menu = default_menu(false);
    let submenus: Vec<String> = menu
        .sections()
        .iter()
        .flatten()
        .filter(|item| item.submenu)
        .map(|item| item.title.to_string())
        .collect();

    assert_eq!(submenus, ["Send snippet", "Git", "Economy"]);
}
