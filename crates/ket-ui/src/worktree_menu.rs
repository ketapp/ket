//! The worktree row's right-click menu, and the one thing it can do.
//!
//! Built on [`crate::ui::menu`] rather than beside it: the `+` button's menu, the
//! palette and this one all want the same keyboard, the same hairlines and the
//! same hint column, and a second menu implementation is how those three drift
//! apart.
//!
//! **Everything but Sleep is wired.** Sleep is drawn because the shape of the
//! menu is itself information — someone who learns where it sits can find it
//! again the day it works — and it is disabled rather than doing nothing. A
//! menu item that silently no-ops is indistinguishable from a bug, and this
//! project has already spent an afternoon on exactly that confusion.
//!
//! **Delete asks first.** `Workspace::remove_worktree` refuses to discard
//! uncommitted work unless forced, which is the right default and a poor
//! dialog: the answer arrives as an error after the reader has already
//! committed to the action. So the first attempt is unforced, and a refusal
//! turns the dialog into the question it should have been — the reason is
//! shown, and the button becomes **Delete anyway**. Nothing is destroyed
//! without the reader seeing what they are destroying.

use std::time::Duration;

use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, Entity, KeyDownEvent, MouseButton, PathBuilder,
    Pixels, Point, Rgba, SharedString, WeakEntity, Window, canvas, div, fill, point, prelude::*,
    px, size,
};
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
const MENU_WIDTH: Pixels = px(220.0);

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

/// What picking a row in a worktree's context menu does.
///
/// The menu hands one of these back without interpreting it — see
/// [`crate::ui::menu`]. Unimplemented picks report that fact rather than
/// silently doing nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeAction {
    /// Change the worktree row's display name without renaming its branch.
    Rename,
    /// Put its path on the clipboard.
    CopyPath,
    /// Keep it at the top of the list.
    Pin,
    /// Suspend its agent without removing it.
    Sleep,
    /// Change how hard the agents launched here are asked to cut their own
    /// token usage. Opens a second menu of levels.
    TokenReduction,
    /// Send one of the saved prompt snippets to its agent. Opens a second
    /// menu of snippets.
    Snippets,
    /// Delete the regenerable build output inside it.
    ClearBuildOutput,
    /// Remove the worktree.
    Delete,
}

impl WorktreeAction {
    /// The label, for the "not wired up" note.
    ///
    /// Duplicating the menu's own titles is deliberate: the note should name
    /// what the reader clicked, in their words, and reaching back into the
    /// open menu for it would break the moment the menu closed first.
    fn label(self) -> &'static str {
        match self {
            Self::Rename => "Rename",
            Self::CopyPath => "Copy Path",
            Self::Pin => "Pin",
            Self::Sleep => "Sleep",
            Self::TokenReduction => "Economy",
            Self::Snippets => "Send snippet",
            Self::ClearBuildOutput => "Clear build output",
            Self::Delete => "Delete",
        }
    }
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
    /// The dialog as it first opens: asking, nothing refused yet.
    pub(crate) fn new(target: WorktreeTarget) -> Self {
        Self {
            target,
            refusal: None,
            pending: false,
            name_hovered: false,
            quiet: false,
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

/// How long the branch takes to drain back into its base while the delete
/// runs, and the pulse of what is on it.
const DRAIN: Duration = Duration::from_millis(1600);

/// The menu for one worktree row.
///
/// `primary` disables **Delete**: the repository's own checkout is not a
/// worktree ket created, and removing it would take the project with it. It
/// stays visible so the menu has one shape rather than two.
pub(crate) fn worktree_menu(
    at: Point<Pixels>,
    primary: bool,
    pinned: bool,
    token_reduction: u8,
    build_bytes: Option<u64>,
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

    let delete = MenuItem::new(WorktreeAction::Delete, "Delete")
        .icon(Icon::Trash)
        .danger()
        .chord(Chord::parse("cmd+shift+backspace").expect("the shipped chord parses"));
    let rename = MenuItem::new(WorktreeAction::Rename, "Rename…").icon(Icon::Pencil);
    let rename = if primary { rename.disabled() } else { rename };

    OpenMenu::new(
        ORIGIN.into(),
        // No search row. A short curated list is read, not searched.
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
            MenuEntry::Item(
                MenuItem::new(WorktreeAction::Sleep, "Sleep")
                    .icon(Icon::Moon)
                    .disabled(),
            ),
            MenuEntry::Item(if primary { delete.disabled() } else { delete }),
        ],
    )
    .with_header("Workspace")
    .at(at)
}

impl Shell {
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
        self.worktree_menu = Some(worktree_menu(
            at,
            node.primary,
            node.pinned,
            node.token_reduction,
            build_bytes,
            &self.theme,
        ));
    }

    /// Runs one pick.
    pub(crate) fn run_worktree_action(&mut self, action: WorktreeAction, cx: &mut Context<Self>) {
        self.worktree_menu = None;

        let Some(target) = self.worktree_target.clone() else {
            return;
        };

        match action {
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
            WorktreeAction::Pin => self.set_worktree_pinned(&target, !target.pinned, cx),
            WorktreeAction::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(target.path.to_string()));
                self.toast_detail(Tone::Info, "Copied path", target.path, cx);
            }
            // Said rather than swallowed. See the module docs.
            other => {
                self.note = Some(format!("{} is not wired up yet", other.label()).into());
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
        Some(menu.view(
            &self.theme,
            cx,
            |shell, action, cx| shell.run_worktree_action(*action, cx),
            |shell, cx| {
                shell.worktree_menu = None;
                cx.notify();
            },
        ))
    }

    /// Opens the level picker for `target`, over where its menu was.
    ///
    /// A second menu rather than a submenu of the first: `MenuItem::submenu`
    /// is a chevron and nothing more today, and a picker that opens in the
    /// same place the reader just clicked reads the same way a submenu would.
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
        let ask = ket_core::config::Config::load().map_or(true, |c| c.general.confirm_delete);
        let mut confirm = RemoveWorktreeConfirm::new(target);
        confirm.quiet = !ask;
        self.confirm_remove_worktree = Some(confirm);
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
                // Every terminal the worktree had, not "the" one: they are
                // per tab now, and a handle left behind is a shell nothing can
                // reach and nothing will ever kill. Read before the layout
                // goes, since the layout is what lists them.
                for terminal in self
                    .spaces
                    .get(&id)
                    .map(|space| space.terminal_ids())
                    .unwrap_or_default()
                {
                    if let Some(handle) = self.terminals.remove(&terminal) {
                        handle.term.kill();
                    }
                }
                // Its pane layout and its running shell go with it. Left
                // behind, they would be resurrected by the next worktree that
                // happened to be given the same id.
                self.spaces.remove(&id);
                // The same argument for its last usage reading: shown against a
                // new worktree, it would be another session's spend under this
                // one's name.
                self.session_usage.remove(&id);
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

        let state = match (pending, escalated) {
            (true, _) => Prune::Draining,
            (false, true) => Prune::Refused,
            (false, false) => Prune::Asking,
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
    /// Before the button: the branch in the danger ink, cut at its base.
    Asking,
    /// The delete running: the branch drains back into its base.
    Draining,
    /// Git refused: the branch greys, and the cut becomes a warning.
    Refused,
}

/// The drawing: a base line with its commits, the worktree's branch leaving
/// it with two of its own, a mark where it leaves, and the checkout — a
/// folder and the name — at its tip.
fn prune(state: Prune, name: AnyElement, t: &Theme) -> AnyElement {
    let phase = if state == Prune::Draining {
        crate::motion::phase(DRAIN)
    } else {
        0.0
    };
    // 1 at rest, dipping to a third and back once a cycle while draining.
    let pulse = if state == Prune::Draining {
        0.35 + 0.65 * (0.5 + 0.5 * (phase * std::f32::consts::TAU).cos())
    } else {
        1.0
    };
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
            for x in [36.0, 70.0, right - 110.0, right - 44.0] {
                dot(at(x, 92.0), commit, ground, window);
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
            let shown = if state == Prune::Draining {
                1.0 - phase
            } else {
                1.0
            };
            stroke_prefix(&points, shown, branch, window);

            let mut faded = branch;
            faded.a *= pulse;
            for x in [148.0, 172.0] {
                dot(at(x, 44.0), faded, ground, window);
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
        .opacity(pulse)
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

/// Strokes the first `fraction` (0 to 1) of the polyline through `points`,
/// by length.
fn stroke_prefix(points: &[Point<Pixels>], fraction: f32, colour: Rgba, window: &mut Window) {
    let length = |a: Point<Pixels>, b: Point<Pixels>| {
        let (dx, dy) = (f32::from(b.x - a.x), f32::from(b.y - a.y));
        (dx * dx + dy * dy).sqrt()
    };
    let total: f32 = points.windows(2).map(|w| length(w[0], w[1])).sum();
    let mut left = total * fraction.clamp(0.0, 1.0);
    if left <= 0.5 || points.len() < 2 {
        return;
    }
    let mut path = PathBuilder::stroke(px(2.0));
    path.move_to(points[0]);
    for pair in points.windows(2) {
        let step = length(pair[0], pair[1]);
        if step >= left {
            let k = left / step;
            path.line_to(point(
                pair[0].x + (pair[1].x - pair[0].x) * k,
                pair[0].y + (pair[1].y - pair[0].y) * k,
            ));
            break;
        }
        path.line_to(pair[1]);
        left -= step;
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, colour);
    }
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
            &Theme::dark(),
        )
    }

    #[test]
    fn sleep_is_visible_but_cannot_be_picked() {
        // Drawn dim rather than dropped: a menu whose shape changes with
        // context is one nobody can aim at from memory.
        let menu = default_menu(false);
        let sleep = menu
            .sections()
            .into_iter()
            .flatten()
            .find(|item| item.title == "Sleep")
            .expect("Sleep is in the menu");

        assert!(!sleep.enabled);
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

        assert_eq!(submenus, ["Send snippet", "Economy"]);
    }

    #[test]
    fn every_unwired_action_names_itself() {
        // The note the shell shows has to say what was clicked, so a label
        // that went missing would leave "is not wired up yet" on its own.
        for action in [WorktreeAction::CopyPath, WorktreeAction::Sleep] {
            assert!(!action.label().is_empty());
        }
    }
}
