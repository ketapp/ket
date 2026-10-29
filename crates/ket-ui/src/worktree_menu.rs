//! The worktree row's right-click menu, and the one thing it can do.
//!
//! Built on [`crate::ui::menu`] rather than beside it: the `+` button's menu, the
//! palette and this one all want the same keyboard, the same hairlines and the
//! same hint column, and a second menu implementation is how those three drift
//! apart.
//!
//! Sleep keeps agent tabs and their exact sessions available to wake later.
//!
//! **Delete asks first.** `Workspace::remove_worktree` refuses to discard
//! uncommitted work unless forced, which is the right default and a poor
//! dialog: the answer arrives as an error after the reader has already
//! committed to the action. So the first attempt is unforced, and a refusal
//! turns the dialog into the question it should have been — the reason is
//! shown, and the button becomes **Delete anyway**. Nothing is destroyed
//! without the reader seeing what they are destroying.
//!
//! **Shift-click skips the asking**, not the safety: while shift is held the
//! row reads **Delete, don't ask**, and picking it deletes at once, unforced,
//! the way Settings → General's confirmation-off does. Work that would be
//! lost still stops it and brings the dialog up to say what.

use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, Entity, KeyDownEvent, MouseButton, PathBuilder,
    Pixels, Point, Rgba, SharedString, WeakEntity, Window, canvas, div, fill, point, prelude::*,
    px, size,
};
use ket_core::github::PullRequestStatus;
use ket_core::id::WorktreeId;
use ket_core::keybinding::Chord;
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::input::{Style as InputStyle, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::snippets::{SnippetMenu, SnippetPick};
use crate::ui::banner::banner;
use crate::ui::button::button;
use crate::ui::chip::{caption, stroke_arc};
use crate::ui::dialog::{card, centered, footer, header};
use crate::ui::icon::{Icon, icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu};
use crate::ui::toast::Tone;
use crate::ui::tooltip::{Side, tooltip};
use crate::ui::{CARD_PAD, LABEL, RADIUS_MD, RADIUS_SM};

/// The menu's width.
const MENU_WIDTH: Pixels = px(240.0);

/// Room for Git action labels and their status subtitles.
const GIT_MENU_WIDTH: Pixels = px(280.0);

/// The Git submenu's name, distinct from its parent.
const GIT_ORIGIN: &str = "worktree-git";

/// Keep a submenu open while the pointer crosses a neighbouring row.
const SUBMENU_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// How the menu names itself, so an opener can tell whose menu is up.
const ORIGIN: &str = "worktree-context";

/// The level picker's own name, kept apart from [`ORIGIN`] because the two are
/// open at different moments and a dismissal has to know which it closed.
const LEVEL_ORIGIN: &str = "worktree-token-reduction";

/// Wider than the menu it opens from: every row carries the level's one-line
/// description, and a 280pt column would clip the sentence that is the whole
/// reason the row is legible. Wider again for the saving each row carries on
/// its right edge, which has to sit beside that sentence rather than over it.
const LEVEL_MENU_WIDTH: Pixels = px(470.0);

/// The snippet picker's own name, for the same reason as [`LEVEL_ORIGIN`].
const SNIPPET_ORIGIN: &str = "worktree-snippets";

/// The Delete row's title, and what it reads while shift is held: the pick
/// then skips the dialog, and the row says so before it is clicked. Short
/// enough to sit beside the row's chord hint at [`MENU_WIDTH`].
const DELETE: &str = "Delete";
const DELETE_WITHOUT_ASKING: &str = "Delete, don't ask";

/// Whether a click on `action`'s row with shift held or not skips the delete
/// dialog: only Delete, and only under shift. For a click alone — the row's
/// chord holds shift too, but it is the app menu's own Delete Worktree
/// command and goes its own way, through the dialog as it always has.
fn skips_asking(action: WorktreeAction, shift_held: bool) -> bool {
    action == WorktreeAction::Delete && shift_held
}

/// Retitles `menu`'s Delete row for shift held or not — see the module docs
/// — and says whether that changed anything, so the window redraws only
/// when it did. A menu without the row is left alone.
fn retitle_delete(menu: &mut OpenMenu<WorktreeAction>, shift_held: bool) -> bool {
    let title = if shift_held {
        DELETE_WITHOUT_ASKING
    } else {
        DELETE
    };
    let Some(delete) = menu.item_mut(|action| *action == WorktreeAction::Delete) else {
        return false;
    };
    if delete.title == title {
        return false;
    }
    delete.title = title.into();
    true
}

/// What picking a row in a worktree's context menu does.
///
/// The menu hands one of these back without interpreting it — see
/// [`crate::ui::menu`]. Unimplemented picks report that fact rather than
/// silently doing nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeAction {
    /// Open the worktree's Git actions.
    Git,
    /// Change the worktree row's display name without renaming its branch.
    Rename,
    /// Put its path on the clipboard.
    CopyPath,
    /// Keep it at the top of the list.
    Pin,
    /// Suspend its agent without removing it.
    Sleep,
    /// Resume the saved sessions in this worktree's asleep tabs.
    Wake,
    /// Change how hard the agents launched here are asked to cut their own
    /// token usage. Opens a second menu of levels.
    TokenReduction,
    /// Send one of the saved prompt snippets to its agent. Opens a second
    /// menu of snippets.
    Snippets,
    /// Delete the regenerable build output inside it.
    ClearBuildOutput,
    /// Commit its work, push its branch and open a pull request for it.
    PullRequest,
    /// Give its pull request's failed checks to its agent.
    FixChecks,
    /// Give its pull request's unresolved review comments to its agent.
    AddressComments,
    /// Merge its GitHub pull request.
    MergePullRequest,
    /// Update its pull request branch from the base.
    UpdatePullRequest,
    /// Mark its draft pull request ready for review.
    ReadyPullRequest,
    /// Reopen its closed unmerged pull request.
    ReopenPullRequest,
    /// Hand a local agent's task to a cloud agent, in a new tab here: the
    /// one explicit step that lets a local task leave the Mac.
    Escalate,
    /// Remove the worktree.
    Delete,
}

/// The worktree a menu or a confirmation is about.
///
/// Carried rather than looked up by index: the list is rebuilt on every
/// reload, and an index captured when the menu opened can point at a
/// different row by the time it is picked.
#[derive(Clone)]
pub(crate) struct WorktreeTarget {
    /// Which worktree.
    pub(crate) id: WorktreeId,
    /// Its branch, for the dialog's copy.
    pub(crate) branch: SharedString,
    /// Current row label, which may differ from the branch.
    pub(crate) label: SharedString,
    /// Where it lives, so the reader can see what is about to go.
    pub(crate) path: SharedString,
    /// Its token-reduction level, for the picker the menu can open onto.
    pub(crate) token_reduction: u8,
    /// Agent whose provider-specific policy the picker describes.
    pub(crate) agent: Option<SharedString>,
    /// Whether it is pinned now, so the pick can flip it.
    pub(crate) pinned: bool,
    /// Where the menu itself was opened, so a second menu can appear in the
    /// same place rather than in a corner: the row it belongs to is behind an
    /// open menu by then, and there is nothing else to anchor to.
    pub(crate) at: Point<Pixels>,
}

/// The worktree display-name editor.
pub(crate) struct WorktreeRename {
    target: WorktreeTarget,
    input: Entity<TextInput>,
    error: Option<SharedString>,
}

/// The "are you sure" step before a worktree is removed.
pub(crate) struct RemoveWorktreeConfirm {
    /// What is about to be removed.
    pub(crate) target: WorktreeTarget,
    /// Why the unforced attempt was refused, if it was.
    ///
    /// `Some` turns the dialog into a second, harder question — see the
    /// module docs.
    pub(crate) refusal: Option<SharedString>,
    /// Whether the delete is running, in the background.
    pub(crate) pending: bool,
    /// Whether the pointer is on the worktree's name, which shows its path.
    pub(crate) name_hovered: bool,
    /// Whether the dialog is kept off screen: Settings said not to ask, so
    /// the delete is already running, and the dialog appears only if git
    /// refuses it.
    pub(crate) quiet: bool,
}

impl RemoveWorktreeConfirm {
    /// The same, or — `ask` false — kept off screen while the delete runs,
    /// to appear only if git refuses it.
    pub(crate) fn asking(target: WorktreeTarget, ask: bool) -> Self {
        Self {
            target,
            refusal: None,
            pending: false,
            name_hovered: false,
            quiet: !ask,
        }
    }
}

/// The delete dialog's width, and the least it will be: a long branch name
/// widens it rather than running off the drawing.
const REMOVE_WIDTH: Pixels = px(420.0);

/// The most the delete dialog widens to fit the name; past it the name
/// truncates and the tooltip carries the rest.
const REMOVE_MAX_WIDTH: Pixels = px(640.0);

/// How tall the drawing of the branch is.
const PRUNE_H: Pixels = px(118.0);

/// What a cloud agent is handed with a local agent's task, typed into its
/// input for the person to finish and send.
const HANDOFF: &str = "A local model has been working on a task in this worktree. Its changes \
                       are in the working tree: read `git status` and `git diff` first. Then \
                       continue the task: ";

/// The menu for one worktree row.
///
/// `primary` disables **Delete**: the repository's own checkout is not a
/// worktree ket created, and removing it would take the project with it. It
/// stays visible so the menu has one shape rather than two.
///
/// `has_agent_tab` and `has_asleep_tab` choose Sleep or Wake. Git operations
/// live in the submenu built by [`git_menu`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn worktree_menu(
    at: Point<Pixels>,
    primary: bool,
    pinned: bool,
    token_reduction: u8,
    build_bytes: Option<u64>,
    escalate_to: Option<SharedString>,
    has_agent_tab: bool,
    has_asleep_tab: bool,
    t: &Theme,
) -> OpenMenu<WorktreeAction> {
    let clear_build = match build_bytes.filter(|bytes| *bytes > 0) {
        Some(bytes) => MenuItem::new(WorktreeAction::ClearBuildOutput, "Clear build output")
            .icon(Icon::Eraser)
            .subtitle(ket_core::storage::format_bytes(bytes)),
        None => MenuItem::new(WorktreeAction::ClearBuildOutput, "Clear build output")
            .icon(Icon::Eraser)
            .disabled(),
    };

    let delete = MenuItem::new(WorktreeAction::Delete, DELETE)
        .icon(Icon::Trash)
        .danger()
        .chord(Chord::parse("cmd+shift+backspace").expect("the shipped chord parses"));
    let rename = MenuItem::new(WorktreeAction::Rename, "Rename…").icon(Icon::Pencil);
    let rename = if primary { rename.disabled() } else { rename };

    OpenMenu::new(
        ORIGIN.into(),
        None,
        MENU_WIDTH,
        vec![
            // Named with what it would reclaim, because that number is the
            // entire reason to pick it — and disabled, rather than hidden,
            // when there is nothing to reclaim or nothing has measured it yet.
            MenuEntry::Item(clear_build),
            MenuEntry::Item(
                MenuItem::new(WorktreeAction::Snippets, "Send snippet")
                    .icon(Icon::Layers)
                    .submenu(),
            ),
        ]
        .into_iter()
        // Only on a worktree a local agent runs in, and only when there is a
        // cloud agent to hand to: the step that lets its task leave the Mac.
        .chain(escalate_to.map(|label| {
            MenuEntry::Item(
                MenuItem::new(WorktreeAction::Escalate, format!("Hand to {label}\u{2026}"))
                    .icon(Icon::Launch)
                    .subtitle("cloud".to_owned()),
            )
        }))
        .chain([
            MenuEntry::Item(
                MenuItem::new(WorktreeAction::Git, "Git")
                    .icon(Icon::GitBranch)
                    .submenu(),
            ),
            // Named with its current level rather than just "Economy":
            // this is the only place a worktree's level can be read after it
            // was created, and a reader opening the menu to change it wants to
            // know what it is now first.
            MenuEntry::Item({
                let economy = MenuItem::new(WorktreeAction::TokenReduction, "Economy")
                    .subtitle(
                        ket_core::worktree::token_reduction(token_reduction)
                            .name()
                            .to_owned(),
                    )
                    .tinted_icon(
                        Icon::Sliders,
                        crate::worktree_dialog::token_reduction_tint(token_reduction, t),
                    )
                    .submenu();
                if primary { economy.disabled() } else { economy }
            }),
            MenuEntry::Item(rename),
            // Disabled on the primary checkout: it has no record of its own to
            // carry the flag, and it is already the first row of its project.
            MenuEntry::Item({
                let pin = if pinned {
                    MenuItem::new(WorktreeAction::Pin, "Unpin").icon(Icon::PinOff)
                } else {
                    MenuItem::new(WorktreeAction::Pin, "Pin").icon(Icon::Pin)
                };
                if primary { pin.disabled() } else { pin }
            }),
            MenuEntry::Item(MenuItem::new(WorktreeAction::CopyPath, "Copy Path").icon(Icon::Copy)),
            MenuEntry::Separator,
            MenuEntry::Item(if has_asleep_tab {
                MenuItem::new(WorktreeAction::Wake, "Wake").icon(Icon::Moon)
            } else if has_agent_tab {
                MenuItem::new(WorktreeAction::Sleep, "Sleep").icon(Icon::Moon)
            } else {
                MenuItem::new(WorktreeAction::Sleep, "Sleep")
                    .icon(Icon::Moon)
                    .disabled()
            }),
            MenuEntry::Item(if primary { delete.disabled() } else { delete }),
        ])
        .collect(),
    )
    .with_header("Worktree")
    .at(at)
}

/// Git operations for the selected worktree, with the same availability rules
/// as the commands in the app menu.
fn git_menu(
    at: Point<Pixels>,
    primary: bool,
    pull_request: Option<&PullRequestStatus>,
) -> OpenMenu<WorktreeAction> {
    let failed_checks = pull_request.map_or(0, |open| open.failed().count());
    OpenMenu::new(
        GIT_ORIGIN.into(),
        None,
        GIT_MENU_WIDTH,
        vec![
            // Disabled on the primary checkout: it is the base a pull
            // request asks to merge into, so it cannot also be the head.
            MenuEntry::Item({
                let pull_request =
                    MenuItem::new(WorktreeAction::PullRequest, "Push and open pull request…")
                        .icon(Icon::GitHub);
                if primary {
                    pull_request.disabled()
                } else {
                    pull_request
                }
            }),
            // Beside it, since it is about the pull request that opens, and
            // disabled until one of its checks has failed.
            MenuEntry::Item({
                let fix = MenuItem::new(WorktreeAction::FixChecks, "Fix checks with agent")
                    .icon(Icon::CircleX);
                if failed_checks == 0 {
                    fix.disabled()
                } else {
                    fix.subtitle(format!("{failed_checks} failed"))
                }
            }),
            MenuEntry::Item({
                let comments = MenuItem::new(
                    WorktreeAction::AddressComments,
                    "Address comments with agent",
                )
                .icon(Icon::MessageSquare);
                match pull_request {
                    Some(open) if open.can_address_comments() => {
                        if open.unresolved_comments > 0 {
                            comments.subtitle(format!(
                                "{} comment{}",
                                open.unresolved_comments,
                                if open.unresolved_comments == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ))
                        } else {
                            comments
                        }
                    }
                    _ => comments.disabled(),
                }
            }),
            MenuEntry::Item({
                let merge = MenuItem::new(WorktreeAction::MergePullRequest, "Merge pull request…")
                    .icon(Icon::GitMerge);
                match pull_request {
                    Some(open) if open.can_merge() => {
                        if open.base.is_empty() {
                            merge
                        } else {
                            merge.subtitle(format!("into {}", open.base))
                        }
                    }
                    _ => merge.disabled(),
                }
            }),
            MenuEntry::Item({
                let update = MenuItem::new(WorktreeAction::UpdatePullRequest, "Update branch")
                    .icon(Icon::Refresh);
                match pull_request {
                    Some(open) if open.can_update() => {
                        if open.base.is_empty() {
                            update
                        } else {
                            update.subtitle(format!("from {}", open.base))
                        }
                    }
                    _ => update.disabled(),
                }
            }),
            MenuEntry::Item({
                let ready = MenuItem::new(WorktreeAction::ReadyPullRequest, "Ready for review")
                    .icon(Icon::CircleCheck);
                match pull_request {
                    Some(open) if open.can_ready() => ready,
                    _ => ready.disabled(),
                }
            }),
            MenuEntry::Item({
                let reopen =
                    MenuItem::new(WorktreeAction::ReopenPullRequest, "Reopen pull request")
                        .icon(Icon::GitBranch);
                match pull_request {
                    Some(open) if open.can_reopen() => reopen,
                    _ => reopen.disabled(),
                }
            }),
        ],
    )
    .with_header("Git")
    .at(at)
}

/// A worktree context menu and the Git branch that belongs to it. Keeping
/// them together makes every existing dismissal close both panels.
pub(crate) struct WorktreeMenu {
    menu: OpenMenu<WorktreeAction>,
    git: Option<OpenMenu<WorktreeAction>>,
    at: Point<Pixels>,
    primary: bool,
    pull_request: Option<PullRequestStatus>,
    git_held: bool,
}

impl WorktreeMenu {
    fn new(
        menu: OpenMenu<WorktreeAction>,
        at: Point<Pixels>,
        primary: bool,
        pull_request: Option<&PullRequestStatus>,
    ) -> Self {
        Self {
            menu,
            git: None,
            at,
            primary,
            pull_request: pull_request.cloned(),
            git_held: false,
        }
    }

    fn open_git(&mut self) {
        if self.git.is_none() {
            let row = self
                .menu
                .sections()
                .iter()
                .flatten()
                .position(|item| item.action == WorktreeAction::Git)
                .unwrap_or(0);
            let at = point(
                self.at.x + MENU_WIDTH - px(4.0),
                self.at.y + self.menu.row_top(row),
            );
            self.git = Some(git_menu(at, self.primary, self.pull_request.as_ref()));
        }
        self.git_held = true;
    }

    fn key(&mut self, event: &KeyDownEvent) -> MenuKey<WorktreeAction> {
        if let Some(git) = self.git.as_mut() {
            if matches!(event.keystroke.key.as_str(), "left" | "escape") {
                self.git = None;
                return MenuKey::Consumed;
            }
            return git.key(event);
        }
        if event.keystroke.key == "right" {
            let mut enter = event.clone();
            enter.keystroke.key = "enter".into();
            if let MenuKey::Run(WorktreeAction::Git) = self.menu.key(&enter) {
                self.open_git();
            }
            return MenuKey::Consumed;
        }
        self.menu.key(event)
    }
}

impl Shell {
    /// Reveal the selected worktree directory through the same file-manager handoff.
    pub(crate) fn native_reveal_worktree(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(node) = self
            .projects
            .get(selection.project)
            .and_then(|project| project.worktrees.get(selection.worktree))
        else {
            return;
        };
        if node.missing {
            return;
        }
        self.note = Some(
            match ket_core::surface::reveal_in_file_manager(&node.path) {
                Ok(()) => format!("showed {} in Finder", node.label()).into(),
                Err(error) => format!("could not reveal worktree: {error}").into(),
            },
        );
    }
    /// Apply a native Economy pick only to the id and pack level it displayed.
    pub(crate) fn native_economy(&mut self, id: &WorktreeId, level: u8, cx: &mut Context<Self>) {
        if self.selected_id().as_ref() != Some(id)
            || !ket_core::worktree::token_reduction_levels()
                .iter()
                .any(|item| item.level == level)
        {
            return;
        }
        let Some(selection) = self.selection else {
            return;
        };
        let Some(node) = self
            .projects
            .get(selection.project)
            .and_then(|project| project.worktrees.get(selection.worktree))
        else {
            return;
        };
        if node.primary || node.missing {
            return;
        }
        self.worktree_target = Some(WorktreeTarget {
            id: node.id.clone(),
            branch: node.branch.clone(),
            label: node.label(),
            path: node.path.display().to_string().into(),
            token_reduction: node.token_reduction,
            agent: node.agent.clone(),
            pinned: node.pinned,
            at: point(px(0.0), px(0.0)),
        });
        self.set_token_reduction(level, cx);
        cx.notify();
    }
    /// Run a native Worktree command for the selected row, rechecking restrictions.
    pub(crate) fn native_worktree_action(
        &mut self,
        action: WorktreeAction,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(node) = self
            .projects
            .get(selection.project)
            .and_then(|project| project.worktrees.get(selection.worktree))
        else {
            return;
        };
        if node.primary
            && matches!(
                action,
                WorktreeAction::Rename
                    | WorktreeAction::Pin
                    | WorktreeAction::TokenReduction
                    | WorktreeAction::PullRequest
                    | WorktreeAction::Delete
            )
        {
            return;
        }
        if node.missing && !matches!(action, WorktreeAction::Delete | WorktreeAction::CopyPath) {
            return;
        }
        if action == WorktreeAction::ClearBuildOutput
            && !self.footprint(&node.id).is_some_and(|size| size.build > 0)
        {
            return;
        }
        self.worktree_target = Some(WorktreeTarget {
            id: node.id.clone(),
            branch: node.branch.clone(),
            label: node.label(),
            path: node.path.display().to_string().into(),
            token_reduction: node.token_reduction,
            agent: node.agent.clone(),
            pinned: node.pinned,
            at: point(px(0.0), px(0.0)),
        });
        self.run_worktree_action(action, cx);
        cx.notify();
    }
    /// The cloud agent a local agent's task can be handed to: the default
    /// agent when it is not local, else the first runnable agent that is not.
    /// `None` when `agent` is not local, or there is nothing to hand to.
    pub(crate) fn escalation_target(&self, agent: Option<&str>) -> Option<String> {
        let local = |name: &str| ket_core::local_models::model_tag(name).is_some();
        if !agent.is_some_and(local) {
            return None;
        }
        let cloud = |name: &str| !local(name) && ket_core::agents::harness(name) != "ollama";
        let runnable: Vec<String> = self
            .runnable_agents()
            .iter()
            .map(|spec| spec.name.clone())
            .collect();
        let default = ket_core::config::Config::load()
            .ok()
            .and_then(|config| match config.agent.default {
                ket_core::agents::DefaultAgent::Named(name) => Some(name),
                _ => None,
            })
            .filter(|name| cloud(name) && runnable.contains(name));
        default.or_else(|| runnable.into_iter().find(|name| cloud(name)))
    }

    /// Opens the cloud agent in a new tab in `target`, with a handoff note
    /// typed into its input but not sent: nothing about the task leaves the
    /// Mac until the person presses Enter. The local agent's tab and its
    /// changes stay as they are.
    fn escalate_worktree(&mut self, target: WorktreeTarget, cx: &mut Context<Self>) {
        let Some(agent) = self.escalation_target(target.agent.as_ref().map(|agent| agent.as_ref()))
        else {
            self.toast(Tone::Error, "No cloud agent is set up to hand this to.", cx);
            return;
        };
        if let Some(selection) = self.position_of(&target.id) {
            self.select(selection, cx);
        }
        let pane = self.spaces.entry(target.id.clone()).or_default().focused;
        let terminal = crate::terminal::TerminalId(self.next_terminal);
        self.open_agent_tab_in(pane, &agent, cx);
        if self.next_terminal == terminal.0 {
            return;
        }
        self.toast_detail(
            Tone::Info,
            format!("Opened {} here", crate::ui::agent::agent_label(&agent)),
            "A handoff note is typed in, not sent. Nothing about this task leaves the Mac \
             until you press Enter.",
            cx,
        );
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            cx.background_executor()
                .timer(crate::phone_work::AGENT_START)
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.paste_into_terminal(terminal, HANDOFF, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the context menu for the row at `project`/`worktree`.
    pub(crate) fn open_worktree_menu(
        &mut self,
        project: usize,
        worktree: usize,
        at: Point<Pixels>,
    ) {
        let Some(node) = self
            .projects
            .get(project)
            .and_then(|p| p.worktrees.get(worktree))
        else {
            return;
        };

        // Every other menu closes: two menus open at once is two things
        // claiming the keyboard.
        self.menu = None;
        self.project_menu = None;
        self.path_menu = None;
        self.popup = None;

        self.worktree_target = Some(WorktreeTarget {
            id: node.id.clone(),
            branch: node.branch.clone(),
            label: node.label(),
            path: node.path.display().to_string().into(),
            token_reduction: node.token_reduction,
            agent: node.agent.clone(),
            pinned: node.pinned,
            at,
        });
        let build_bytes = self.footprint(&node.id).map(|footprint| footprint.build);
        let escalate_to = self
            .escalation_target(node.agent.as_ref().map(|agent| agent.as_ref()))
            .map(|agent| crate::ui::agent::agent_label(&agent).into());
        let pull_request = self.pull_requests.of(&node.id);
        let ids = self
            .spaces
            .get(&node.id)
            .map_or_else(Vec::new, |space| space.terminal_ids());
        let has_agent_tab = ids.iter().any(|id| {
            self.terminals
                .get(id)
                .is_some_and(|handle| handle.agent.is_some())
        });
        let has_asleep_tab = ids.iter().any(|id| self.asleep_terminals.contains_key(id));
        let menu = worktree_menu(
            at,
            node.primary,
            node.pinned,
            node.token_reduction,
            build_bytes,
            escalate_to,
            has_agent_tab,
            has_asleep_tab,
            &self.theme,
        );
        self.worktree_menu = Some(WorktreeMenu::new(menu, at, node.primary, pull_request));
        // Shift may already be down as the menu opens.
        self.worktree_menu_modifiers_changed_quietly();
    }

    /// Retitles the open menu's Delete row for the modifiers now held — see
    /// the module docs — and redraws when that changed anything.
    pub(crate) fn worktree_menu_modifiers_changed(&mut self, cx: &mut Context<Self>) {
        if self.worktree_menu_modifiers_changed_quietly() {
            cx.notify();
        }
    }

    fn worktree_menu_modifiers_changed_quietly(&mut self) -> bool {
        let shift_held = self.shift_held;
        self.worktree_menu
            .as_mut()
            .is_some_and(|open| retitle_delete(&mut open.menu, shift_held))
    }

    /// Runs one pick.
    pub(crate) fn run_worktree_action(&mut self, action: WorktreeAction, cx: &mut Context<Self>) {
        if action == WorktreeAction::Git {
            if let Some(open) = self.worktree_menu.as_mut() {
                open.open_git();
            }
            cx.notify();
            return;
        }
        self.worktree_menu = None;

        let Some(target) = self.worktree_target.clone() else {
            return;
        };

        match action {
            WorktreeAction::Git => {}
            WorktreeAction::Rename => self.begin_worktree_rename(target, cx),
            WorktreeAction::TokenReduction => {
                self.popup = None;
                self.open_token_reduction_menu(&target);
            }
            WorktreeAction::Snippets => {
                self.popup = None;
                self.open_worktree_snippet_menu(&target, cx);
            }
            WorktreeAction::ClearBuildOutput => {
                self.popup = None;
                self.request_clear_build(target, cx);
            }
            WorktreeAction::Delete => {
                self.popup = None;
                self.request_remove_worktree(target, cx);
            }
            WorktreeAction::PullRequest => {
                self.popup = None;
                self.request_pull_request(target, cx);
            }
            WorktreeAction::FixChecks => {
                self.popup = None;
                self.fix_checks(target.id, cx);
            }
            WorktreeAction::AddressComments => {
                self.popup = None;
                self.address_comments(target.id, cx);
            }
            WorktreeAction::MergePullRequest => {
                self.popup = None;
                self.request_merge_pull_request(target, cx);
            }
            WorktreeAction::UpdatePullRequest => {
                self.popup = None;
                self.update_pull_request(target.id, cx);
            }
            WorktreeAction::ReadyPullRequest => {
                self.popup = None;
                self.ready_pull_request(target.id, cx);
            }
            WorktreeAction::ReopenPullRequest => {
                self.popup = None;
                self.reopen_pull_request(target.id, cx);
            }
            WorktreeAction::Escalate => {
                self.popup = None;
                self.escalate_worktree(target, cx);
            }
            WorktreeAction::Pin => self.set_worktree_pinned(&target, !target.pinned, cx),
            WorktreeAction::Sleep => self.sleep_worktree(&target.id, cx),
            WorktreeAction::Wake => self.wake_worktree(&target.id, cx),
            WorktreeAction::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(target.path.to_string()));
                self.toast_detail(Tone::Info, "Copied path", target.path, cx);
            }
        }
    }

    fn begin_worktree_rename(&mut self, target: WorktreeTarget, cx: &mut Context<Self>) {
        let input = TextInput::with_text("Worktree name", target.label.as_ref(), cx);
        input.update(cx, |input, _| input.request_focus());
        self.watch_field(&input, cx);
        self.worktree_rename = Some(WorktreeRename {
            target,
            input,
            error: None,
        });
    }

    fn save_worktree_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rename) = self.worktree_rename.as_ref() else {
            return;
        };
        let id = rename.target.id.clone();
        let name = rename.input.read(cx).text();
        match Workspace::open().and_then(|workspace| workspace.set_worktree_name(&id, &name)) {
            Ok(()) => {
                self.worktree_rename = None;
                self.reload(cx);
            }
            Err(error) => {
                if let Some(rename) = self.worktree_rename.as_mut() {
                    rename.error = Some(error.to_string().into());
                }
            }
        }
    }

    pub(crate) fn worktree_rename_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(input) = self
            .worktree_rename
            .as_ref()
            .map(|rename| rename.input.clone())
        else {
            return false;
        };
        if is_text(&event.keystroke) {
            return true;
        }
        if input.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.worktree_rename = None,
            "enter" => self.save_worktree_rename(cx),
            _ => {}
        }
        true
    }

    pub(crate) fn worktree_rename_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rename = self.worktree_rename.as_ref()?;
        let t = &self.theme;
        let cancel = button("cancel-worktree-rename", "Cancel")
            .ghost()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.worktree_rename = None;
                cx.notify();
            }));
        let save = button("save-worktree-rename", "Rename")
            .primary()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.save_worktree_rename(cx);
                cx.notify();
            }));
        let field = text_field(
            &rename.input,
            "worktree-rename-name",
            rename.error.is_some(),
            InputStyle::new(t, self.caret.visible),
            window,
            cx,
        );
        Some(
            centered(
                card("rename-worktree", px(420.0), t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(header("Rename worktree", t))
                    .child(field)
                    .child(caption(format!("Git branch: {}", rename.target.branch), t))
                    .children(
                        rename
                            .error
                            .clone()
                            .map(|error| caption(error, t).text_color(paint(t.status.failed))),
                    )
                    .child(footer().child(cancel).child(save)),
            )
            .into_any_element(),
        )
    }

    /// Handles a key while the context menu is open.
    pub(crate) fn worktree_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.worktree_menu.as_mut() else {
            return false;
        };

        match menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.worktree_menu = None,
            MenuKey::Run(action) => self.run_worktree_action(action, cx),
        }
        true
    }

    /// The menu panel, or nothing when it is closed.
    pub(crate) fn worktree_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.worktree_menu.as_ref()?;
        let parent = menu.menu.view_with_hover(
            &self.theme,
            cx,
            |shell, action, cx| {
                if skips_asking(*action, shell.shift_held) {
                    shell.worktree_menu = None;
                    shell.popup = None;
                    if let Some(target) = shell.worktree_target.clone() {
                        shell.remove_worktree_without_asking(target, cx);
                    }
                } else {
                    shell.run_worktree_action(*action, cx);
                }
            },
            |shell, cx| {
                shell.worktree_menu = None;
                cx.notify();
            },
            |shell, action, cx| shell.hover_worktree_row(*action, cx),
        );
        let git = menu.git.as_ref().map(|git| {
            git.view_with_hover(
                &self.theme,
                cx,
                |shell, action, cx| shell.run_worktree_action(*action, cx),
                |shell, cx| {
                    shell.worktree_menu = None;
                    cx.notify();
                },
                |shell, _, _| {
                    if let Some(open) = shell.worktree_menu.as_mut() {
                        open.git_held = true;
                    }
                },
            )
        });
        Some(div().child(parent).children(git).into_any_element())
    }

    /// Open Git on hover, allowing a diagonal crossing into its panel.
    fn hover_worktree_row(&mut self, action: WorktreeAction, cx: &mut Context<Self>) {
        let Some(open) = self.worktree_menu.as_mut() else {
            return;
        };
        if action == WorktreeAction::Git {
            open.open_git();
            cx.notify();
        } else if open.git.is_some() {
            open.git_held = false;
            cx.spawn(async move |this: WeakEntity<Shell>, cx| {
                cx.background_executor().timer(SUBMENU_GRACE).await;
                let _ = this.update(cx, |shell, cx| {
                    if let Some(open) = shell.worktree_menu.as_mut()
                        && open.git.is_some()
                        && !open.git_held
                    {
                        open.git = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// Opens the level picker for `target`, over where its menu was.
    ///
    /// This picker replaces the parent menu so its longer descriptions have
    /// room, while Git actions stay beside the parent as a submenu.
    fn open_token_reduction_menu(&mut self, target: &WorktreeTarget) {
        self.token_reduction_menu = Some(
            OpenMenu::new(
                LEVEL_ORIGIN.into(),
                // Five rows, read rather than searched — the same call the
                // dialog's picker makes.
                None,
                LEVEL_MENU_WIDTH,
                crate::worktree_dialog::token_reduction_items(
                    target.token_reduction,
                    target.agent.as_deref().map(|agent| &**agent),
                    &self.theme,
                ),
            )
            .with_header(target.branch.clone())
            .selected(crate::worktree_dialog::token_reduction_index(
                target.token_reduction,
            ))
            .at(target.at),
        );
    }

    /// Opens the snippet picker for `target`, over where its menu was — a
    /// second menu rather than a submenu, for the reason
    /// [`Shell::open_token_reduction_menu`] gives.
    fn open_worktree_snippet_menu(&mut self, target: &WorktreeTarget, cx: &mut Context<Self>) {
        let snippets = self.load_snippets(cx);
        let mut open = SnippetMenu::new(SNIPPET_ORIGIN, snippets, true, None);
        open.menu = open.menu.with_header(target.branch.clone()).at(target.at);
        self.worktree_snippet_menu = Some(open);
    }

    /// Runs a pick from the snippet picker: sends the snippet to the
    /// worktree's agent, the way the quick prompt would.
    ///
    /// Into its open agent tab when there is one, and to a fresh headless run
    /// when there is not — [`Shell::send_prompt_to_worktree`] makes that call.
    pub(crate) fn run_worktree_snippet(&mut self, pick: SnippetPick, cx: &mut Context<Self>) {
        let Some(open) = self.worktree_snippet_menu.take() else {
            return;
        };
        if pick == SnippetPick::Manage {
            self.manage_snippets(cx);
            return;
        }
        let (Some(snippet), Some(target)) = (open.picked(pick), self.worktree_target.clone())
        else {
            return;
        };
        self.toast(
            Tone::Info,
            format!("Sent “{}” to {}", snippet.name, target.branch),
            cx,
        );
        self.send_prompt_to_worktree(target.id, snippet.body.clone(), cx);
    }

    /// Handles a key while the snippet picker is open.
    pub(crate) fn worktree_snippet_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.worktree_snippet_menu.as_mut() else {
            return false;
        };

        match open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.worktree_snippet_menu = None,
            MenuKey::Run(pick) => self.run_worktree_snippet(pick, cx),
        }
        true
    }

    /// The snippet picker's panel, or nothing when it is closed.
    pub(crate) fn worktree_snippet_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.worktree_snippet_menu.as_ref()?;
        Some(open.menu.view(
            &self.theme,
            cx,
            |shell, pick, cx| shell.run_worktree_snippet(*pick, cx),
            |shell, cx| {
                shell.worktree_snippet_menu = None;
                cx.notify();
            },
        ))
    }

    /// Pins or unpins the worktree the menu was opened on.
    ///
    /// The primary checkout has no record to carry the flag, so it says so
    /// rather than appearing to succeed.
    fn set_worktree_pinned(
        &mut self,
        target: &WorktreeTarget,
        pinned: bool,
        cx: &mut Context<Self>,
    ) {
        let workspace = match Workspace::open() {
            Ok(workspace) => workspace,
            Err(e) => {
                self.note = Some(format!("could not open the workspace: {e}").into());
                return;
            }
        };

        if let Err(e) = workspace.set_worktree_pinned(&target.id, pinned) {
            self.note = Some(
                format!(
                    "could not {} {}: {e}",
                    if pinned { "pin" } else { "unpin" },
                    target.branch
                )
                .into(),
            );
            return;
        }

        self.reload(cx);
    }

    /// Records a new level for the worktree the picker is about.
    ///
    /// Says that running agents keep the old one, because they do: the level
    /// reaches an agent as an environment variable on the process ket launched
    /// (see `ket_core::worktree::TOKEN_REDUCTION_ENV`), and nothing can reach
    /// back into a session that has already read it. Silently leaving that out
    /// would make the setting look broken in the one case a reader is most
    /// likely to try it — on the worktree they are working in.
    pub(crate) fn set_token_reduction(&mut self, level: u8, cx: &mut Context<Self>) {
        self.token_reduction_menu = None;

        let Some(target) = self.worktree_target.clone() else {
            return;
        };

        let workspace = match Workspace::open() {
            Ok(workspace) => workspace,
            Err(e) => {
                self.note = Some(format!("could not open the workspace: {e}").into());
                return;
            }
        };

        let level_id = ket_core::worktree::token_reduction(level).id.clone();
        if let Err(e) = workspace.set_economy_level(&target.id, &level_id) {
            self.note = Some(format!("could not change economy: {e}").into());
            return;
        }

        self.note = Some(
            format!(
                "{}: economy {} — agents already running keep the level they started with",
                target.branch,
                ket_core::worktree::token_reduction(level).name(),
            )
            .into(),
        );
        // So the row's ring, and the menu's own subtitle next time, come from
        // what was just stored rather than from what the sidebar last read.
        self.reload(cx);
    }

    /// Handles a key while the level picker is open.
    pub(crate) fn token_reduction_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.token_reduction_menu.as_mut() else {
            return false;
        };

        match menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.token_reduction_menu = None,
            MenuKey::Run(level) => self.set_token_reduction(level, cx),
        }
        true
    }

    /// The level picker's panel, or nothing when it is closed.
    pub(crate) fn token_reduction_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.token_reduction_menu.as_ref()?;
        Some(menu.view(
            &self.theme,
            cx,
            |shell, level, cx| shell.set_token_reduction(*level, cx),
            |shell, cx| {
                shell.token_reduction_menu = None;
                cx.notify();
            },
        ))
    }

    /// Asks to delete `target`: the dialog, or — with Settings → General's
    /// confirmation off — the delete at once, unforced, so work that would be
    /// lost still stops it and brings the dialog up to say what.
    pub(crate) fn request_remove_worktree(
        &mut self,
        target: WorktreeTarget,
        cx: &mut Context<Self>,
    ) {
        let ask = ket_core::config::Config::load().map_or(true, |c| c.general.confirm_delete);
        self.remove_worktree_asking(target, ask, cx);
    }

    /// Deletes `target` without the dialog, whatever Settings says: a
    /// shift-click on the menu's Delete row. Unforced, so work that would be
    /// lost still stops it and asks — see the module docs.
    pub(crate) fn remove_worktree_without_asking(
        &mut self,
        target: WorktreeTarget,
        cx: &mut Context<Self>,
    ) {
        self.remove_worktree_asking(target, false, cx);
    }

    fn remove_worktree_asking(
        &mut self,
        target: WorktreeTarget,
        ask: bool,
        cx: &mut Context<Self>,
    ) {
        // One at a time: a quiet delete is still running behind no dialog,
        // and its answer would land on this one's.
        if let Some(running) = self
            .confirm_remove_worktree
            .as_ref()
            .filter(|confirm| confirm.pending)
        {
            let branch = running.target.branch.clone();
            self.toast_detail(Tone::Info, "Still deleting", branch, cx);
            return;
        }
        self.confirm_remove_worktree = Some(RemoveWorktreeConfirm::asking(target, ask));
        if !ask {
            self.remove_worktree_confirmed(false, cx);
        }
    }

    /// Removes the worktree the dialog is about, in the background.
    ///
    /// `force` discards uncommitted work, and is only ever reached by a reader
    /// who has been shown what would be lost and asked again. Off the
    /// window's thread because a delete walks the checkout and asks git
    /// twice, which takes seconds on a big tree; the dialog stays up with its
    /// button busy until the answer comes back.
    pub(crate) fn remove_worktree_confirmed(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_remove_worktree.as_mut() else {
            return;
        };
        if confirm.pending {
            return;
        }
        confirm.pending = true;
        let id = confirm.target.id.clone();
        // Taken now, because the dialog it lives on is closed before the
        // outcome is reported and the toast still has to name what went.
        let branch = confirm.target.branch.clone();

        // The branch goes with the checkout. Leaving it was the safer-looking
        // default and turned out to be the expensive one: the branches piled
        // up, and with them the worktrees nobody could tell were finished.
        // Nothing is lost by it — `Workspace::remove_worktree` keeps any branch
        // whose commits are not already on its base, and says which.
        let work = cx.background_executor().spawn({
            let id = id.clone();
            async move {
                let workspace =
                    Workspace::open().map_err(|e| format!("could not open the workspace: {e}"))?;
                workspace
                    .remove_worktree(&id, force, true)
                    .map_err(|e| e.to_string())
            }
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.remove_worktree_finished(id, branch, force, outcome, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Puts a delete's answer on the window.
    fn remove_worktree_finished(
        &mut self,
        id: WorktreeId,
        branch: SharedString,
        force: bool,
        outcome: Result<ket_core::workspace::RemovalReport, String>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(report) => {
                self.confirm_remove_worktree = None;
                self.worktree_target = None;
                self.forget_deleted_worktree(&id);
                // A kept branch is said out loud, and in the tone that says
                // "not all of it". Someone who asked for the branch to go and
                // was quietly given half of that would find out much later,
                // from a branch list they did not expect.
                match report.preserved_branch {
                    Some(_) => self.toast_detail(
                        Tone::Warning,
                        "Worktree deleted",
                        format!("Kept the branch {branch}, it has unmerged commits"),
                        cx,
                    ),
                    None => {
                        self.toast_detail(Tone::Success, "Worktree deleted", branch.clone(), cx)
                    }
                }
                self.reload(cx);
            }
            Err(e) => {
                // An unforced refusal is a question rather than a result: the
                // dialog stays up, says exactly what stopped it, and offers to
                // go again as "Delete anyway" — see the module docs. Reporting
                // that as a toast as well would be saying it twice, once in the
                // place that can act on it and once in a place that cannot.
                //
                // A *forced* attempt that still failed has nothing left to
                // offer. Asking "Delete anyway" a second time is a loop, so the
                // dialog goes and the outcome is reported like any other.
                if force {
                    tracing::warn!(%e, branch = %branch, "forced worktree delete failed");
                    self.confirm_remove_worktree = None;
                    self.worktree_target = None;
                    self.toast(Tone::Error, format!("Couldn't delete {branch}"), cx);
                } else if let Some(confirm) = self.confirm_remove_worktree.as_mut() {
                    confirm.pending = false;
                    confirm.refusal = Some(e.into());
                    // A delete nobody was asked about comes out from behind
                    // no dialog here: this is the asking it skipped.
                    confirm.quiet = false;
                }
            }
        }
    }

    /// What a deleted worktree leaves behind in the window, let go of.
    fn forget_deleted_worktree(&mut self, id: &WorktreeId) {
        // Every terminal the worktree had, not "the" one: they are per tab
        // now, and a handle left behind is a shell nothing can reach and
        // nothing will ever kill. Read before the layout goes, since the
        // layout is what lists them.
        for terminal in self
            .spaces
            .get(id)
            .map(|space| space.terminal_ids())
            .unwrap_or_default()
        {
            if let Some(handle) = self.terminals.remove(&terminal) {
                handle.term.kill();
            }
        }
        // Its pane layout and its running shell go with it. Left behind, they
        // would be resurrected by the next worktree that happened to be given
        // the same id.
        self.spaces.remove(id);
        // The same argument for its last usage reading: shown against a new
        // worktree, it would be another session's spend under this one's
        // name.
        self.session_usage.remove(id);
    }

    /// Deletes a worktree a phone asked to delete — see
    /// `ket_core::host::take_phone_delete`. The phone asked its person
    /// first, as this window's dialog does, so there is no dialog here: an
    /// unforced refusal goes back to the phone, which offers Delete anyway.
    pub(crate) fn delete_worktree_from_phone(
        &mut self,
        ask: ket_core::host::PhoneDelete,
        cx: &mut Context<Self>,
    ) {
        let req = ask.req;
        let id = WorktreeId::from(ask.worktree.as_str());
        let Some(node) = self.position_of(&id).and_then(|at| {
            self.projects
                .get(at.project)
                .and_then(|project| project.worktrees.get(at.worktree))
        }) else {
            ket_core::host::finish_phone_delete(
                req,
                Err("ket on your Mac doesn't have that worktree".into()),
            );
            return;
        };
        if node.primary {
            ket_core::host::finish_phone_delete(
                req,
                Err("the repository's own checkout can't be deleted".into()),
            );
            return;
        }
        // This window's own delete of it, running behind its dialog.
        if self
            .confirm_remove_worktree
            .as_ref()
            .is_some_and(|confirm| confirm.pending && confirm.target.id == id)
        {
            ket_core::host::finish_phone_delete(
                req,
                Err("ket on your Mac is already deleting this worktree".into()),
            );
            return;
        }
        let branch = node.branch.clone();
        let force = ask.force;
        let work = cx.background_executor().spawn({
            let id = id.clone();
            async move {
                let workspace =
                    Workspace::open().map_err(|e| format!("could not open the workspace: {e}"))?;
                workspace
                    .remove_worktree(&id, force, true)
                    .map_err(|e| e.to_string())
            }
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let answer = match &outcome {
                Ok(report) => Ok(report.preserved_branch.as_ref().map(|_| branch.to_string())),
                Err(why) => Err(why.clone()),
            };
            let _ = shell.update(cx, |shell, cx| {
                if outcome.is_ok() {
                    // A dialog this window had up about it has nothing left
                    // to ask.
                    if shell
                        .confirm_remove_worktree
                        .as_ref()
                        .is_some_and(|confirm| confirm.target.id == id)
                    {
                        shell.confirm_remove_worktree = None;
                    }
                    if shell
                        .worktree_target
                        .as_ref()
                        .is_some_and(|target| target.id == id)
                    {
                        shell.worktree_target = None;
                    }
                    shell.forget_deleted_worktree(&id);
                    shell.toast_detail(Tone::Info, "Deleted from your phone", branch.clone(), cx);
                    shell.reload(cx);
                }
                cx.notify();
            });
            ket_core::host::finish_phone_delete(req, answer);
        })
        .detach();
    }

    /// Handles a key while the confirmation is up.
    pub(crate) fn remove_worktree_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(confirm) = self
            .confirm_remove_worktree
            .as_ref()
            .filter(|confirm| !confirm.quiet)
        else {
            return false;
        };
        let escalated = confirm.refusal.is_some();
        // A delete under way is not called back by a key: the dialog holds
        // until the answer arrives.
        if confirm.pending {
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => self.confirm_remove_worktree = None,
            // Enter never escalates on its own. Discarding work has to be
            // clicked, or typed again after reading why the first attempt
            // stopped.
            "enter" => self.remove_worktree_confirmed(escalated, cx),
            _ => {}
        }

        true
    }

    /// The confirmation dialog, or nothing when it is closed.
    ///
    /// A drawing more than a paragraph: the worktree's branch leaving its
    /// base, cut where it leaves. The name is all it spells out; its path is
    /// a tooltip on the name.
    pub(crate) fn remove_worktree_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self
            .confirm_remove_worktree
            .as_ref()
            .filter(|confirm| !confirm.quiet)?;
        let t = &self.theme;
        let escalated = confirm.refusal.is_some();
        let pending = confirm.pending;

        let cancel = button("cancel-remove-worktree", "Cancel")
            .ghost()
            .enabled(!pending)
            .render(t)
            .when(!pending, |el| {
                el.on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_remove_worktree = None;
                    cx.notify();
                }))
            });

        // Never primary, however many times it is asked for: an action that
        // destroys a checkout should not also look like the obvious next step.
        let delete = button(
            "confirm-remove-worktree",
            if escalated { "Delete anyway" } else { "Delete" },
        )
        .danger()
        .leading(Icon::Trash)
        .loading_if(pending, "Deleting…")
        .render(t)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.remove_worktree_confirmed(escalated, cx);
            cx.notify();
        }));

        // Held still while the delete runs: the button already says it is
        // busy, and a drawing that moved under it read as something breaking.
        let state = if escalated {
            Prune::Refused
        } else {
            Prune::Asking
        };

        let name = tooltip(
            "remove-worktree-name",
            div()
                .font_family(self.font_family.clone())
                .text_size(px(12.0))
                .text_color(paint(t.text.primary))
                .truncate()
                .child(confirm.target.branch.clone())
                .into_any_element(),
            tilde(&confirm.target.path),
            Side::Top,
            confirm.name_hovered,
            t,
        )
        .min_w_0()
        // The tooltip wrapper is `flex_none`; the name has to be allowed to
        // give way to the chip, or `truncate` never gets a width to cut at.
        .flex_shrink()
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if let Some(confirm) = this.confirm_remove_worktree.as_mut() {
                confirm.name_hovered = *hovered;
            }
            cx.notify();
        }));

        let under = match &confirm.refusal {
            Some(why) => {
                banner("remove-worktree-refusal", Tone::Warning, why.clone(), t).into_any_element()
            }
            None => div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(LABEL)
                .text_color(paint(t.text.dim))
                .child(icon(Icon::ShieldCheck, paint(t.status.running)))
                .child("Unmerged commits keep their branch.")
                .into_any_element(),
        };

        Some(
            centered(
                card("remove-worktree", REMOVE_WIDTH, t)
                    .w_auto()
                    .min_w(REMOVE_WIDTH)
                    .max_w(REMOVE_MAX_WIDTH)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(header(
                        if pending {
                            "Deleting worktree"
                        } else {
                            "Delete worktree"
                        },
                        t,
                    ))
                    .child(prune(state, name.into_any_element(), t))
                    .child(under)
                    .child(footer().child(cancel).child(delete)),
            )
            .into_any_element(),
        )
    }
}

/// `path` with the home directory written `~`, which is how a path reads in a
/// tooltip that has one line to give it.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

/// Where the delete dialog's drawing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prune {
    /// Before the button, and while the delete runs: the branch in the
    /// danger ink, cut at its base.
    Asking,
    /// Git refused: the branch greys, and the cut becomes a warning.
    Refused,
}

/// The drawing: a base line with its commits, the worktree's branch leaving
/// it with two of its own, a mark where it leaves, and the checkout — a
/// folder and the name — at its tip.
fn prune(state: Prune, name: AnyElement, t: &Theme) -> AnyElement {
    let warning = Tone::Warning.colour(t);
    let danger = paint(t.status.failed);
    let branch = if state == Prune::Refused {
        paint(t.text.dim)
    } else {
        danger
    };
    let mark = if state == Prune::Refused {
        warning
    } else {
        danger
    };
    let base = paint(t.border);
    let commit = paint(t.text.dim);
    let ground = paint(t.sunken);
    let lid = paint(t.elevated);

    let drawing = canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let at = |x: f32, y: f32| point(bounds.left() + px(x), bounds.top() + px(y));
            let right = f32::from(bounds.size.width) - 16.0;

            let mut line = PathBuilder::stroke(px(2.0));
            line.move_to(at(16.0, 92.0));
            line.line_to(at(right, 92.0));
            if let Ok(path) = line.build() {
                window.paint_path(path, base);
            }

            // The branch, as points: the curve up off the base, then level.
            let mut points: Vec<Point<Pixels>> = (0..=24)
                .map(|i| {
                    let u = i as f32 / 24.0;
                    let v = 1.0 - u;
                    let x = v * v * v * 70.0
                        + 3.0 * v * v * u * 110.0
                        + 3.0 * v * u * u * 110.0
                        + u * u * u * 150.0;
                    let y = v * v * v * 92.0
                        + 3.0 * v * v * u * 92.0
                        + 3.0 * v * u * u * 44.0
                        + u * u * u * 44.0;
                    at(x, y)
                })
                .collect();
            points.push(at(186.0, 44.0));
            let mut stem = PathBuilder::stroke(px(2.0));
            stem.move_to(points[0]);
            for &p in &points[1..] {
                stem.line_to(p);
            }
            if let Ok(path) = stem.build() {
                window.paint_path(path, branch);
            }

            // The base's commits go on after the branch: it leaves from the
            // centre of the second one, and painted first that ring sat
            // under the red line instead of on it.
            for x in [36.0, 70.0, right - 110.0, right - 44.0] {
                dot(at(x, 92.0), commit, ground, window);
            }

            for x in [148.0, 172.0] {
                dot(at(x, 44.0), branch, ground, window);
            }

            // The cut: a disc on the curve where it leaves the base.
            let centre = at(97.0, 80.0);
            let disc = Bounds::centered_at(centre, size(px(18.0), px(18.0)));
            window.paint_quad(fill(disc, lid).corner_radii(px(9.0)));
            stroke_arc(disc, 0.0, 1.0, mark, px(1.5), window);
            let mut glyph = PathBuilder::stroke(px(1.5));
            if state == Prune::Refused {
                glyph.move_to(at(97.0, 75.5));
                glyph.line_to(at(97.0, 80.5));
                glyph.move_to(at(97.0, 83.4));
                glyph.line_to(at(97.0, 83.8));
            } else {
                glyph.move_to(at(93.5, 76.5));
                glyph.line_to(at(100.5, 83.5));
                glyph.move_to(at(100.5, 76.5));
                glyph.line_to(at(93.5, 83.5));
            }
            if let Ok(path) = glyph.build() {
                window.paint_path(path, mark);
            }
        },
    )
    .absolute()
    .inset_0();

    let mut edge = mark;
    edge.a = 0.45;
    // In flow rather than absolute, so the name's width reaches the dialog
    // and widens it; the drawing reads its bounds and stretches to match.
    let checkout = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .px(px(9.0))
        .min_w_0()
        .max_w_full()
        .rounded(RADIUS_MD)
        .bg(paint(t.hover))
        .border_1()
        .border_color(edge)
        .child(sized_icon(Icon::Folder, px(14.0), mark))
        .child(name);

    div()
        .relative()
        .flex()
        .flex_col()
        .items_start()
        .h(PRUNE_H)
        .min_w(REMOVE_WIDTH - CARD_PAD * 2.0)
        .pl(px(186.0))
        .pt(px(31.0))
        .pr(px(12.0))
        .rounded(RADIUS_SM * 2.0)
        .bg(ground)
        .child(drawing)
        .child(
            div()
                .absolute()
                .left(px(16.0))
                .bottom(px(6.0))
                .child(caption("main", t)),
        )
        .child(checkout)
        .into_any_element()
}

/// A commit: a ring of `ink` on the drawing's `ground`.
fn dot(centre: Point<Pixels>, ink: Rgba, ground: Rgba, window: &mut Window) {
    let bounds = Bounds::centered_at(centre, size(px(9.0), px(9.0)));
    window.paint_quad(fill(bounds, ground).corner_radii(px(4.5)));
    stroke_arc(bounds, 0.0, 1.0, ink, px(1.6), window);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu under test, at the default token-reduction level.
    fn default_menu(primary: bool) -> OpenMenu<WorktreeAction> {
        worktree_menu(
            Point::default(),
            primary,
            false,
            ket_core::worktree::default_token_reduction(),
            None,
            None,
            false,
            false,
            &Theme::dark(),
        )
    }

    fn menu_with_failed_checks(primary: bool, failed: usize) -> OpenMenu<WorktreeAction> {
        let pull_request = (failed > 0).then(|| status_with_failed(failed));
        git_menu(Point::default(), primary, pull_request.as_ref())
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
    fn sleep_and_wake_follow_agent_tabs() {
        let menu = worktree_menu(
            Point::default(),
            false,
            false,
            ket_core::worktree::default_token_reduction(),
            None,
            None,
            true,
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
            ket_core::worktree::default_token_reduction(),
            None,
            None,
            false,
            true,
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

        let idle = git_menu(Point::default(), false, None);
        assert!(!item(&idle, "Merge pull request…").enabled);
        assert!(!item(&idle, "Update branch").enabled);
        assert!(!item(&idle, "Ready for review").enabled);
        assert!(!item(&idle, "Reopen pull request").enabled);
        assert!(!item(&idle, "Address comments with agent").enabled);

        let mut open = status_with_failed(0);
        open.draft = false;
        let merge = git_menu(Point::default(), false, Some(&open));
        let merge_item = item(&merge, "Merge pull request…");
        assert!(merge_item.enabled);
        assert_eq!(
            merge_item.subtitle.as_ref().map(ToString::to_string),
            Some("into main".to_owned())
        );
        assert!(!item(&merge, "Ready for review").enabled);

        open.behind = true;
        let update = git_menu(Point::default(), false, Some(&open));
        assert!(item(&update, "Update branch").enabled);

        open.draft = true;
        let draft = git_menu(Point::default(), false, Some(&open));
        assert!(!item(&draft, "Merge pull request…").enabled);
        assert!(item(&draft, "Ready for review").enabled);

        open.draft = false;
        open.behind = false;
        open.review = Review::ChangesRequested;
        open.unresolved_comments = 2;
        let comments = git_menu(Point::default(), false, Some(&open));
        let comments_item = item(&comments, "Address comments with agent");
        assert!(comments_item.enabled);
        assert_eq!(
            comments_item.subtitle.as_ref().map(ToString::to_string),
            Some("2 comments".to_owned())
        );

        open.state = PullRequestState::Closed;
        open.review = Review::None;
        open.unresolved_comments = 0;
        let closed = git_menu(Point::default(), false, Some(&open));
        assert!(item(&closed, "Reopen pull request").enabled);
        assert!(!item(&closed, "Merge pull request…").enabled);
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
        let git = git_menu(Point::default(), false, None);
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
        assert!(item(&git, "Push and open pull request…").enabled);
        assert!(
            !item(
                &git_menu(Point::default(), true, None),
                "Push and open pull request…"
            )
            .enabled
        );
    }

    #[test]
    fn keyboard_opens_git_picks_an_action_and_returns_to_the_parent() {
        let mut open = WorktreeMenu::new(default_menu(false), Point::default(), false, None);
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
}
