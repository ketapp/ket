//! Projects as a person manages them: adding one, tuning how it appears,
//! removing it. `tree.rs` *shows* projects; this module owns what happens when
//! you act on one.
//!
//! The flow keeps to the shape a person already knows: the content area is a
//! landing page until a project exists; `+` in the sidebar header opens the
//! add-project dialog; right-clicking a project's heading opens its menu —
//! **New Worktree** and **Backlog** first, **Pin to Top** and **Hide
//! Project** as the phone's hold menu has them, what the project weighs on
//! disk and what Economy has saved it, then **Project Settings**, **Hide
//! non-ket worktrees** and **Remove Project**. A hidden project waits behind
//! a line under the list, whose menu shows it again.
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
mod github;
mod settings;

pub(crate) use add::AddProjectDialog;
pub(crate) use form::Field;
pub(crate) use settings::SettingsDialog;

use gpui::{
    AnyElement, Context, KeyDownEvent, Pixels, Point, Rgba, SharedString, Window, div, prelude::*,
    px, relative,
};
use ket_core::KetError;
use ket_core::id::ProjectId;
use ket_core::keybinding::Chord;
use ket_core::project::ProjectSettings;
use ket_core::storage::format_bytes;
use ket_core::theme::{Color, Theme, contrast_ratio};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::paint::paint;
use crate::ui::banner::banner;
use crate::ui::button::button;
use crate::ui::chip::{caption, kbd};
use crate::ui::dialog::{body, card, centered, footer, header};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu};
use crate::ui::toast::Tone;
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

/// The rail a selected worktree wears: its project's colour, so the eye that
/// finds where you are also learns whose it is.
///
/// A project left at the default keeps the theme's marker. Graphite is what
/// "no colour" looks like, and a grey rail on a grey fill would lose the one
/// thing the rail is for. A colour too dark to stand off the selected fill is
/// moved towards the text until it clears 3:1, the bar WCAG sets for a
/// non-text mark — Slate and Indigo both need it on the dark theme.
pub(crate) fn rail_colour(project: Color, t: &Theme) -> Color {
    if project == default_color() {
        return t.marker;
    }
    (0..=10)
        .map(|step| project.mix(t.text.primary, f64::from(step) / 10.0))
        .find(|candidate| contrast_ratio(*candidate, t.hover) >= 3.0)
        .unwrap_or(t.text.primary)
}

/// How the project menu names itself, so an opener can tell whose menu is up.
const PROJECT_MENU_ORIGIN: &str = "project-context";

/// How the hidden-projects menu names itself.
const TUCKED_MENU_ORIGIN: &str = "hidden-projects";

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
    /// Pin it to the top of the sidebar, or unpin it.
    Pin,
    /// Leave it out of the sidebar.
    Hide,
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
    pinned: bool,
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
            // The phone's two, in its words.
            MenuEntry::Item(if pinned {
                MenuItem::new(ProjectAction::Pin, "Unpin").icon(Icon::PinOff)
            } else {
                MenuItem::new(ProjectAction::Pin, "Pin to Top").icon(Icon::Pin)
            }),
            MenuEntry::Item(
                MenuItem::new(ProjectAction::Hide, "Hide Project")
                    .subtitle("Back in the list if an agent there needs you")
                    .icon(Icon::EyeOff),
            ),
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
    /// Why the last press stopped short, when a worktree would have lost
    /// uncommitted work. The next press removes anyway.
    pub(crate) blocked: Option<SharedString>,
    /// Why the last press failed for any other reason.
    pub(crate) error: Option<SharedString>,
}

impl Shell {
    /// Run a Project menu command for the project containing the current selection.
    pub(crate) fn native_project_action(&mut self, action: ProjectAction, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(node) = self.projects.get(selection.project) else {
            return;
        };
        let id = node.id.clone();
        match action {
            ProjectAction::NewWorktree => self.open_new_worktree(selection.project, false, cx),
            ProjectAction::Backlog => self.open_backlog(&id, cx),
            ProjectAction::Pin => self.toggle_project_pin(&id, cx),
            ProjectAction::Hide => self.hide_project(&id, cx),
            ProjectAction::Settings => self.open_project_settings(&id, Field::Name, cx),
            ProjectAction::ToggleDiscovered => self.toggle_discovered_worktrees(&id, cx),
            ProjectAction::Remove => self.confirm_remove_project(&id),
            ProjectAction::Storage | ProjectAction::Economy => {}
        }
        cx.notify();
    }
    /// Flips whether a project shows worktrees ket did not create.
    ///
    /// Written straight through rather than via the dialog: it is a one-click
    /// toggle in the menu, and a menu item that opened a dialog to flip a
    /// switch would be a step nobody asked for.
    pub(crate) fn toggle_discovered_worktrees(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        self.change_projects(std::slice::from_ref(id), cx, |settings| {
            settings.hide_discovered_worktrees = !settings.hide_discovered_worktrees;
        });
    }

    /// Pins a project to the top of the sidebar, or unpins it.
    pub(crate) fn toggle_project_pin(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        self.change_projects(std::slice::from_ref(id), cx, |settings| {
            settings.pinned = !settings.pinned;
        });
    }

    /// Leaves a project out of the sidebar. Nothing else about it changes:
    /// its worktrees and agents carry on, a selection in it stays, and it is
    /// back in the list while an agent there waits on you.
    pub(crate) fn hide_project(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        self.change_projects(std::slice::from_ref(id), cx, |settings| {
            settings.hidden = true;
        });
    }

    /// Puts hidden projects back in the sidebar.
    pub(crate) fn show_projects(&mut self, ids: &[ProjectId], cx: &mut Context<Self>) {
        self.change_projects(ids, cx, |settings| settings.hidden = false);
    }

    /// Writes one change to each project's settings, then reloads once.
    ///
    /// Written straight through rather than via the dialog: these are
    /// one-click picks in a menu, and a pick that opened a dialog to flip a
    /// switch would be a step nobody asked for.
    fn change_projects(
        &mut self,
        ids: &[ProjectId],
        cx: &mut Context<Self>,
        change: impl Fn(&mut ProjectSettings),
    ) {
        self.project_menu = None;
        self.tucked_menu = None;
        let result = Workspace::open().and_then(|workspace| {
            ids.iter().try_for_each(|id| {
                let mut settings = workspace.project_settings(id)?;
                change(&mut settings);
                workspace.set_project_settings(id, settings)
            })
        });
        if let Err(e) = result {
            self.note = Some(format!("could not change the project: {e}").into());
        }
        // After a failure too: the projects changed before it are changed.
        self.reload(cx);
        // The menu bar's Pin to Top check follows the pick, wherever it was made.
        self.menu_dirty = true;
    }

    // ---- removing -------------------------------------------------------------

    /// Asks before removing. Reachable from a menu in one click, so it must ask.
    pub(crate) fn confirm_remove_project(&mut self, id: &ProjectId) {
        self.project_menu = None;
        self.tucked_menu = None;
        self.settings = None;
        self.popup = None;
        let Some(node) = self.projects.iter().find(|p| &p.id == id) else {
            return;
        };
        self.confirm_remove = Some(RemoveConfirm {
            id: id.clone(),
            name: node.name.clone(),
            worktrees: node.worktrees.len(),
            blocked: None,
            error: None,
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
    ///
    /// A refusal keeps the dialog up and says which worktree stopped it — the
    /// dialog closing on a note nobody could see read as Remove doing
    /// nothing. The press after a refusal removes anyway, the way Delete
    /// worktree's does.
    pub(crate) fn remove_project_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_remove.as_mut() else {
            return;
        };
        let force = confirm.blocked.take().is_some();
        confirm.error = None;
        let (id, name) = (confirm.id.clone(), confirm.name.clone());

        let workspace = match Workspace::open() {
            Ok(workspace) => workspace,
            Err(e) => {
                return self.remove_project_failed(format!("Could not remove {name}: {e}"), cx);
            }
        };
        let worktrees = match workspace.worktrees(Some(&id)) {
            Ok(worktrees) => worktrees,
            Err(e) => {
                return self.remove_project_failed(format!("Could not remove {name}: {e}"), cx);
            }
        };
        for worktree in worktrees {
            match workspace.remove_worktree(&worktree.id, force, true) {
                Ok(_) => {}
                // Work that would be lost: say whose, and offer to go on.
                Err(KetError::Conflict(_)) if !force => {
                    if let Some(confirm) = self.confirm_remove.as_mut() {
                        confirm.blocked = Some(
                            format!(
                                "{} has uncommitted changes. Remove anyway deletes them.",
                                worktree.branch
                            )
                            .into(),
                        );
                    }
                    self.reload(cx);
                    return;
                }
                Err(e) => {
                    return self.remove_project_failed(
                        format!("Could not remove {}: {e}", worktree.branch),
                        cx,
                    );
                }
            }
        }
        match workspace.remove_project(&id, false) {
            Ok(()) => {
                self.confirm_remove = None;
                self.toast(Tone::Success, format!("Removed {name}"), cx);
                self.reload(cx);
            }
            Err(e) => self.remove_project_failed(format!("Could not remove {name}: {e}"), cx),
        }
    }

    /// Keeps the remove dialog up with `message` in it.
    fn remove_project_failed(&mut self, message: String, cx: &mut Context<Self>) {
        if let Some(confirm) = self.confirm_remove.as_mut() {
            confirm.error = Some(message.into());
        }
        self.reload(cx);
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
    pub(crate) fn landing_view(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.setup_open || !self.setup_ready() || self.projects.is_empty() {
            return self.setup_view(window, cx);
        }
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
            ProjectAction::Pin => self.toggle_project_pin(&id, cx),
            ProjectAction::Hide => self.hide_project(&id, cx),
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
                    .children(
                        confirm
                            .blocked
                            .clone()
                            .map(|why| banner("confirm-remove-blocked", Tone::Warning, why, t)),
                    )
                    .children(
                        confirm
                            .error
                            .clone()
                            .map(|why| banner("confirm-remove-error", Tone::Error, why, t)),
                    )
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
                                button(
                                    "confirm-remove-go",
                                    if confirm.blocked.is_some() {
                                        "Remove anyway"
                                    } else {
                                        "Remove"
                                    },
                                )
                                .danger()
                                .leading(Icon::Trash)
                                .render(t)
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.remove_project_confirmed(cx);
                                        cx.notify();
                                    },
                                )),
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
        self.tucked_menu = None;
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
            menu: project_menu(at, node.hide_discovered, node.pinned, facts, &self.theme)
                .with_header(node.name.clone()),
        });
    }

    // ---- hidden projects ------------------------------------------------------

    /// The line under the projects to the ones hidden from the sidebar, when
    /// any are — the phone's "hidden on this phone". A press opens the menu
    /// that shows them again.
    pub(crate) fn tucked_line(&self, now: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.tucked_projects(now).len();
        if count == 0 {
            return None;
        }
        let t = &self.theme;
        let open = self.tucked_menu.is_some();
        let label = if count == 1 {
            "1 hidden project".to_owned()
        } else {
            format!("{count} hidden projects")
        };
        Some(
            // The shared row, selected while its menu is up; placed in the
            // list's margin and set in the quiet ink of a line that is not
            // itself a project.
            crate::ui::row::sized_row("tucked-projects", open, px(28.0), t)
                .mx(px(6.0))
                .mb(px(6.0))
                .text_size(crate::ui::CAPTION)
                .text_color(paint(t.text.dim))
                .cursor_pointer()
                .child(sized_icon(Icon::EyeOff, px(13.0), paint(t.text.dim)))
                .child(label)
                // On the press, as the project heading's right-click is: the
                // shell closes every menu on a press, and this one has to
                // open after that rather than be closed by it.
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        if this.tucked_menu.take().is_none() {
                            this.open_tucked_menu(event.position);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    /// Opens the menu of hidden projects at `at`: each one, to show it again,
    /// then all of them.
    fn open_tucked_menu(&mut self, at: Point<Pixels>) {
        // One menu at a time: two open is two things claiming the keyboard.
        self.menu = None;
        self.worktree_menu = None;
        self.path_menu = None;
        self.project_menu = None;
        self.tucked_menu = None;
        self.sidebar_menu = None;
        self.popup = None;

        let tucked: Vec<&crate::tree::ProjectNode> = self
            .tucked_projects(ket_core::now_ms())
            .into_iter()
            .filter_map(|index| self.projects.get(index))
            .collect();
        if tucked.is_empty() {
            return;
        }
        let mut entries: Vec<MenuEntry<Option<ProjectId>>> = tucked
            .iter()
            .map(|project| {
                let worktrees: Vec<String> = project
                    .worktrees
                    .iter()
                    .map(|node| node.label().to_string())
                    .collect();
                MenuEntry::Item(
                    MenuItem::new(Some(project.id.clone()), project.name.clone())
                        .subtitle(if worktrees.is_empty() {
                            "No worktrees".to_owned()
                        } else {
                            worktrees.join(" \u{b7} ")
                        })
                        .icon(Icon::Eye)
                        .detail("Show", None),
                )
            })
            .collect();
        if tucked.len() > 1 {
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::Item(
                MenuItem::new(None, format!("Show All {}", tucked.len())).icon(Icon::Eye),
            ));
        }
        self.tucked_menu = Some(
            OpenMenu::new(TUCKED_MENU_ORIGIN.into(), None, px(264.0), entries)
                .with_header("Hidden projects")
                .at(at),
        );
    }

    /// The hidden-projects menu, when it is open.
    pub(crate) fn tucked_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.tucked_menu.as_ref()?;
        Some(menu.view(
            &self.theme,
            cx,
            |shell, pick, cx| shell.run_tucked_pick(pick.clone(), cx),
            |shell, cx| {
                shell.tucked_menu = None;
                cx.notify();
            },
        ))
    }

    /// Shows the picked project again, or every hidden one for Show All.
    fn run_tucked_pick(&mut self, pick: Option<ProjectId>, cx: &mut Context<Self>) {
        let ids: Vec<ProjectId> = match pick {
            Some(id) => vec![id],
            None => self
                .tucked_projects(ket_core::now_ms())
                .into_iter()
                .filter_map(|index| self.projects.get(index))
                .map(|project| project.id.clone())
                .collect(),
        };
        self.show_projects(&ids, cx);
        cx.notify();
    }

    /// Handles a key while the hidden-projects menu is open.
    pub(crate) fn tucked_menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.tucked_menu.as_mut() else {
            return false;
        };
        match menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => {}
            MenuKey::Close => self.tucked_menu = None,
            MenuKey::Run(pick) => self.run_tucked_pick(pick, cx),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_left_at_the_default_keeps_the_theme_marker() {
        for t in [Theme::dark(), Theme::light()] {
            assert_eq!(rail_colour(default_color(), &t), t.marker);
        }
    }

    #[test]
    fn every_palette_colour_makes_a_rail_that_stands_off_the_selected_fill() {
        for t in [Theme::dark(), Theme::light()] {
            for (name, hex) in &PROJECT_COLORS[1..] {
                let colour = Color::from_hex(hex).expect("palette entries parse");
                let rail = rail_colour(colour, &t);
                assert!(
                    contrast_ratio(rail, t.hover) >= 3.0,
                    "{name} rails at {:.2}:1",
                    contrast_ratio(rail, t.hover)
                );
            }
        }
    }

    #[test]
    fn a_colour_that_already_stands_out_is_drawn_as_chosen() {
        let t = Theme::dark();
        let red = Color::from_hex("#ef4444").expect("parses");
        assert_eq!(rail_colour(red, &t), red);
    }
}
