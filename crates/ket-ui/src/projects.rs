//! Projects as a person manages them: adding one, tuning how it appears,
//! removing it. `tree.rs` *shows* projects; this module owns what happens when
//! you act on one.
//!
//! The flow keeps to the shape a person already knows: the content area is a
//! landing page until a project exists; `+` in the sidebar header opens the
//! add-project dialog; right-clicking a project's heading opens its menu —
//! **New Worktree** and **Backlog** first, what the project weighs on disk
//! and what Economy has saved it, then **Project Settings**, **Hide non-ket
//! worktrees** and **Remove Project**.
//!
//! The two dialogs are separate components over one form:
//!
//! - [`form`] — the drafts both dialogs edit (name, icon, colour, base
//!   branch, agent and its command, the two switches) and the parts they are
//!   drawn as.
//! - [`add`] — the folder line over the form; registers nothing until Add.
//! - [`settings`] — the form under the project's path, with Remove.
//!
//! This file keeps the menu, removal and the landing page.

mod add;
mod form;
mod settings;

pub(crate) use add::AddProjectDialog;
pub(crate) use form::Field;
pub(crate) use settings::SettingsDialog;

use gpui::{
    AnyElement, Context, KeyDownEvent, Pixels, Point, Rgba, SharedString, Window, div, prelude::*,
    px, relative,
};
use ket_core::id::ProjectId;
use ket_core::keybinding::Chord;
use ket_core::storage::format_bytes;
use ket_core::theme::{Color, Theme};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::paint::paint;
use crate::ui::button::button;
use crate::ui::chip::{caption, kbd};
use crate::ui::dialog::{body, card, centered, footer, header};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu};
use form::FormKey;

/// The badge colours a project can pick from, with the name the picker shows
/// for each.
///
/// Eighteen, in display order — two neutrals, then once round the hue wheel —
/// with the first as the neutral default a project has before anyone chooses.
/// The picker lays them out in two rows of nine.
///
/// There is no cyan. Cyan is what a project wears when it launches its agents
/// with a command of its own (see [`crate::agent_override`]), ring around the
/// badge included, and a cyan badge would read as that ring at a glance.
pub(crate) const PROJECT_COLORS: [(&str, &str); 18] = [
    ("Graphite", "#737373"),
    ("Slate", "#64748b"),
    ("Red", "#ef4444"),
    ("Orange", "#f97316"),
    ("Amber", "#f59e0b"),
    ("Yellow", "#eab308"),
    ("Lime", "#84cc16"),
    ("Green", "#22c55e"),
    ("Emerald", "#10b981"),
    ("Teal", "#14b8a6"),
    ("Sky", "#0ea5e9"),
    ("Blue", "#3b82f6"),
    ("Indigo", "#6366f1"),
    ("Violet", "#8b5cf6"),
    ("Purple", "#a855f7"),
    ("Fuchsia", "#d946ef"),
    ("Pink", "#ec4899"),
    ("Rose", "#f43f5e"),
];

/// The colour a project has when none was chosen.
pub(crate) fn default_color() -> Color {
    Color::from_hex(PROJECT_COLORS[0].1).expect("the default palette entry parses")
}

/// How the project menu names itself, so an opener can tell whose menu is up.
const PROJECT_MENU_ORIGIN: &str = "project-context";

/// What a project's menu can do.
///
/// The menu itself never interprets these — see [`crate::ui::menu`] — so the
/// rows stay a list of labels and [`Shell::run_project_action`] owns what each
/// one means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectAction {
    /// Start a worktree in this project.
    NewWorktree,
    /// The project's footprint on disk. A read-out: never picked.
    Storage,
    /// What Economy has saved it. A read-out: never picked.
    Economy,
    /// Open the project's backlog.
    Backlog,
    /// Open the settings dialog.
    Settings,
    /// Show or hide worktrees ket did not create.
    ToggleDiscovered,
    /// Ask before forgetting the project.
    Remove,
}

/// The menu a project heading opens when it is right-clicked.
///
/// Built on [`OpenMenu`] like every other menu in the shell. It used to be a
/// second, hand-rolled menu with its own width, row height, radius and border
/// — a menu that looked like the others from a distance and matched none of
/// their metrics, and that no keyboard could drive.
pub(crate) struct ProjectMenu {
    /// Index into [`Shell::projects`].
    pub(crate) project: usize,
    /// The menu itself.
    pub(crate) menu: OpenMenu<ProjectAction>,
}

/// What the menu states about its project, worked out by the opener.
struct Facts {
    /// Total and build output on disk, once something has measured them.
    storage: Option<(u64, u64)>,
    /// Economy's figure, its tint and the clause under it; `None` while no
    /// worktree in the project is reduced.
    economy: Option<(SharedString, Rgba, SharedString)>,
    /// Backlog notes not started yet.
    backlog: usize,
}

/// The rows a project's menu offers: the action it exists for first, with
/// the backlog of work waiting for it, then what the project costs, then the
/// rest of what can be done to it.
fn project_menu(
    at: Point<Pixels>,
    hidden: bool,
    facts: Facts,
    t: &Theme,
) -> OpenMenu<ProjectAction> {
    // Read-outs rather than choices, so neither takes a click or the
    // keyboard's selection; the figure keeps its own ink.
    let storage = match facts.storage {
        Some((total, build)) => MenuItem::new(ProjectAction::Storage, "Storage")
            .subtitle(if build > 0 {
                format!("{} is build output", format_bytes(build))
            } else {
                "No build output".to_owned()
            })
            .detail(format_bytes(total), Some(paint(t.text.primary))),
        None => MenuItem::new(ProjectAction::Storage, "Storage")
            .subtitle("Not measured yet")
            .detail("—", None),
    }
    .icon(Icon::Layers)
    .disabled();

    let economy = match facts.economy {
        Some((figure, tint, note)) => MenuItem::new(ProjectAction::Economy, "Economy")
            .subtitle(note)
            .detail(figure, Some(tint)),
        None => MenuItem::new(ProjectAction::Economy, "Economy")
            .subtitle("No worktree here is reduced")
            .detail("Off", None),
    }
    .icon(Icon::PiggyBank)
    .disabled();

    let backlog = MenuItem::new(ProjectAction::Backlog, "Backlog").icon(Icon::ListTodo);
    let backlog = match facts.backlog {
        0 => backlog,
        open => backlog.detail(open.to_string(), None),
    };

    OpenMenu::new(
        PROJECT_MENU_ORIGIN.into(),
        // No search row: a handful of curated items are read, not searched.
        None,
        px(264.0),
        vec![
            MenuEntry::Item(
                MenuItem::new(ProjectAction::NewWorktree, "New Worktree")
                    .icon(Icon::Plus)
                    // Drawn where the action lives so the chord is
                    // discoverable, and honoured while the menu is open —
                    // see `MenuItem::chord`.
                    .chord(
                        Chord::parse(crate::shortcuts::NEW_WORKTREE_CHORD)
                            .expect("the shipped chord parses"),
                    ),
            ),
            MenuEntry::Item(backlog),
            MenuEntry::Heading("Footprint".into()),
            MenuEntry::Item(storage),
            MenuEntry::Item(economy),
            MenuEntry::Heading("Manage".into()),
            MenuEntry::Item(
                MenuItem::new(ProjectAction::Settings, "Project Settings…").icon(Icon::Sliders),
            ),
            MenuEntry::Item(
                MenuItem::new(
                    ProjectAction::ToggleDiscovered,
                    if hidden {
                        "Show hidden worktrees"
                    } else {
                        "Hide non-ket worktrees"
                    },
                )
                .icon(Icon::List),
            ),
            MenuEntry::Item(
                MenuItem::new(ProjectAction::Remove, "Remove Project")
                    .icon(Icon::Trash)
                    .danger(),
            ),
        ],
    )
    .at(at)
    // New Worktree starts lit: it is what the menu is for, and Enter takes it.
    .selected(0)
}

/// The "are you sure" step before a project is removed.
pub(crate) struct RemoveConfirm {
    /// The project.
    pub(crate) id: ProjectId,
    /// What the sidebar calls it.
    pub(crate) name: SharedString,
    /// How many ket-managed worktrees go with it.
    pub(crate) worktrees: usize,
}

impl Shell {
    /// Flips whether a project shows worktrees ket did not create.
    ///
    /// Written straight through rather than via the dialog: it is a one-click
    /// toggle in the menu, and a menu item that opened a dialog to flip a
    /// switch would be a step nobody asked for.
    pub(crate) fn toggle_discovered_worktrees(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        self.project_menu = None;
        let result = Workspace::open().and_then(|workspace| {
            let mut settings = workspace.project_settings(id)?;
            settings.hide_discovered_worktrees = !settings.hide_discovered_worktrees;
            workspace.set_project_settings(id, settings)
        });
        match result {
            Ok(()) => self.reload(cx),
            Err(e) => self.note = Some(format!("could not change the project: {e}").into()),
        }
    }

    // ---- removing -------------------------------------------------------------

    /// Asks before removing. Reachable from a menu in one click, so it must ask.
    pub(crate) fn confirm_remove_project(&mut self, id: &ProjectId) {
        self.project_menu = None;
        self.settings = None;
        self.popup = None;
        let Some(node) = self.projects.iter().find(|p| &p.id == id) else {
            return;
        };
        self.confirm_remove = Some(RemoveConfirm {
            id: id.clone(),
            name: node.name.clone(),
            worktrees: node.worktrees.len(),
        });
    }

    /// Removes the project the user just confirmed.
    ///
    /// Its ket-managed worktrees go first, one at a time and never forced:
    /// core refuses to remove a worktree with uncommitted changes, and that
    /// refusal is what keeps "remove project" from being a way to lose an
    /// agent's afternoon. The project is only deregistered once every
    /// worktree is gone, so a refusal leaves a project with fewer worktrees
    /// rather than orphaned directories nothing knows how to clean up.
    pub(crate) fn remove_project_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_remove.take() else {
            return;
        };

        let result = Workspace::open().and_then(|workspace| {
            for worktree in workspace.worktrees(Some(&confirm.id))? {
                workspace.remove_worktree(&worktree.id, false, true)?;
            }
            workspace.remove_project(&confirm.id, false)
        });

        match result {
            Ok(()) => {
                self.note = Some(format!("removed {}", confirm.name).into());
                self.reload(cx);
            }
            Err(e) => {
                self.note = Some(format!("could not remove {}: {e}", confirm.name).into());
                self.reload(cx);
            }
        }
    }

    // ---- keys -----------------------------------------------------------------

    /// Handles a keystroke while a project dialog or menu is open.
    ///
    /// Returns whether it was consumed. A dialog takes every key while it is
    /// up — typing a display name must not also drive the shell's shortcuts
    /// underneath it — which is the same rule the palette follows.
    pub(crate) fn project_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();

        if self.confirm_remove.is_some() {
            match key {
                "escape" => self.confirm_remove = None,
                "enter" => self.remove_project_confirmed(cx),
                _ => {}
            }
            return true;
        }

        if self.project_form().is_some() {
            match self.project_form_key(event, window, cx) {
                FormKey::Handled => {}
                FormKey::Close => {
                    self.settings = None;
                    self.adding_project = None;
                }
                FormKey::Submit if self.settings.is_some() => self.save_project_settings(cx),
                FormKey::Submit => self.add_project_confirmed(cx),
            }
            return true;
        }

        false
    }

    // ---- views ----------------------------------------------------------------

    /// What the content area shows when nothing is selected.
    ///
    /// Before any project exists, the only thing to do is add one, and the
    /// page says so with the button that does it. With projects but no
    /// selection, it points at the sidebar. In both cases the shell's `note`
    /// is shown here, because this is where the eye is when adding fails.
    pub(crate) fn landing_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let has_projects = !self.projects.is_empty();
        let message: SharedString = if has_projects {
            "Select a worktree from the sidebar to begin.".into()
        } else {
            "Add a project to get started.".into()
        };
        // The brand's green, as the header's mark wears it.
        let ink = paint(t.status.running);

        let add_button = button("landing-add-project", "Add Project")
            .primary()
            .leading(Icon::Plus)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.add_project(cx);
                cx.notify();
            }));

        let shortcut = |keys: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(kbd(keys, self.font_family.clone(), t))
                .child(caption(what, t))
        };

        let content = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .max_w(px(560.0))
            .child(sized_icon(Icon::KetMark, px(40.0), ink))
            .child(
                div()
                    .text_3xl()
                    .text_color(paint(t.text.primary))
                    .child("ket"),
            )
            .child(div().text_sm().text_color(paint(t.text.dim)).child(message))
            .child(div().h(px(4.0)))
            .child(add_button)
            .child(div().h(px(12.0)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(22.0))
                    .child(shortcut("\u{2318}K", "Command palette"))
                    .child(shortcut("\u{2303}`", "Terminal in the selected worktree")),
            )
            .children(self.note.clone().map(|note| {
                div()
                    .mt_4()
                    .text_sm()
                    .text_color(paint(t.text.dim))
                    .child(note)
            }));

        // The planet rises under the words; they sit in the sky above it.
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(crate::horizon::horizon(ink)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(relative(crate::horizon::HORIZON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(24.0))
                    .child(content),
            )
            .into_any_element()
    }

    /// The project menu, when one is open.
    pub(crate) fn project_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.project_menu.as_ref()?;
        Some(open.menu.view(
            &self.theme,
            cx,
            |shell, action, cx| shell.run_project_action(*action, cx),
            |shell, cx| {
                shell.close_project_menu();
                cx.notify();
            },
        ))
    }

    /// Runs one pick from a project's menu.
    pub(crate) fn run_project_action(&mut self, action: ProjectAction, cx: &mut Context<Self>) {
        let Some(open) = self.project_menu.take() else {
            return;
        };
        let Some(id) = self.projects.get(open.project).map(|node| node.id.clone()) else {
            return;
        };

        match action {
            ProjectAction::NewWorktree => self.open_new_worktree(open.project, false, cx),
            ProjectAction::Backlog => self.open_backlog(&id, cx),
            ProjectAction::Settings => self.open_project_settings(&id, Field::Name, cx),
            ProjectAction::ToggleDiscovered => self.toggle_discovered_worktrees(&id, cx),
            ProjectAction::Remove => self.confirm_remove_project(&id),
            // Disabled rows: the menu never hands these back.
            ProjectAction::Storage | ProjectAction::Economy => {}
        }
    }

    /// Handles a key while a project's menu is open.
    pub(crate) fn project_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.project_menu.as_mut() else {
            return false;
        };

        match open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.close_project_menu(),
            MenuKey::Run(action) => self.run_project_action(action, cx),
        }
        true
    }

    /// The remove confirmation, when one is up.
    pub(crate) fn confirm_remove_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm_remove.as_ref()?;
        let t = &self.theme;

        let explanation: SharedString = match confirm.worktrees {
            0 => "The repository stays on disk; ket just stops listing it.".into(),
            1 => "The repository stays on disk. Its one ket worktree is removed; \
                  a worktree with uncommitted changes stops the removal."
                .into(),
            n => format!(
                "The repository stays on disk. Its {n} ket worktrees are removed; \
                 a worktree with uncommitted changes stops the removal."
            )
            .into(),
        };

        Some(
            centered(
                card("confirm-remove", px(440.0), t)
                    .child(header(format!("Remove {}?", confirm.name), t))
                    .child(body(explanation, t))
                    .child(
                        footer()
                            .child(
                                button("confirm-cancel", "Cancel")
                                    .ghost()
                                    .render(t)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_remove = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button("confirm-remove-go", "Remove")
                                    .danger()
                                    .leading(Icon::Trash)
                                    .render(t)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.remove_project_confirmed(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }

    /// Whether the open project menu belongs to the project at `index`.
    pub(crate) fn project_menu_is_for(&self, index: usize) -> bool {
        self.project_menu
            .as_ref()
            .is_some_and(|open| open.project == index)
    }

    /// Closes the project menu.
    fn close_project_menu(&mut self) {
        self.project_menu = None;
    }

    /// Opens the menu for the project at `index` at `at`, where its heading
    /// was right-clicked.
    pub(crate) fn open_project_menu(&mut self, index: usize, at: Point<Pixels>) {
        // Every other menu closes: two menus open at once is two things
        // claiming the keyboard.
        self.menu = None;
        self.worktree_menu = None;
        self.path_menu = None;
        self.popup = None;
        if !self.project_menu_is_for(index) {
            self.close_project_menu();
        }

        let Some(node) = self.projects.get(index) else {
            return;
        };
        let facts = Facts {
            storage: self.project_footprint(index),
            economy: self.project_economy(&node.id),
            backlog: ket_core::backlog::Backlog::load(&node.id)
                .map(|backlog| backlog.open_count())
                .unwrap_or(0),
        };
        self.project_menu = Some(ProjectMenu {
            project: index,
            menu: project_menu(at, node.hide_discovered, facts, &self.theme)
                .with_header(node.name.clone()),
        });
    }
}
