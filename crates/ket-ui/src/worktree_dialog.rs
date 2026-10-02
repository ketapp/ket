//! The **Create worktree** dialog, behind the `+` beside a project's name.
//!
//! Deliberately spare. "Run on" is absent because everything is local — a
//! picker with one entry is a question with one answer, and asking it is
//! worse than not asking. The name field is one field rather than four tabs:
//! ket has no GitHub or Jira integration to resolve an issue number against,
//! and tabs that do nothing are a promise the app does not keep.
//!
//! What is left is what a worktree actually needs: which project, what the
//! branch is called, and which agent it is for. The project is stated in the
//! title when the `+` was on one, and asked for when the dialog came from the
//! sidebar's foot with more than one project to choose between. The agent is recorded on the
//! worktree (see `ket_core::worktree::Worktree::agent`) so the sidebar can say
//! what a worktree is *for* before anything has run in it.
//!
//! The two things it asks for are two focusable controls, and which one has
//! the keyboard is state the dialog keeps. That is not ceremony: the arrow
//! keys mean "move the caret" in one and "change the agent" in the other, and
//! without somewhere to record which is live they mean both at once.

use gpui::{
    AnyElement, Context, Div, Entity, FocusHandle, KeyDownEvent, MouseButton, Pixels, SharedString,
    Window, div, prelude::*, px,
};
use ket_core::KetError;
use ket_core::agents::{Catalogue, DefaultAgent, ShellProber};
use ket_core::git::PreparedBase;
use ket_core::id::ProjectId;
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;
use ket_core::worktree::Worktree;

use crate::Shell;
use crate::input::{Key, Style, TextInput, is_text, text_field};
use crate::paint::{alpha, mix, paint};
use crate::tabs::TabKind;
use crate::terminal::TerminalId;
use crate::ui::agent::{agent_label, mark, provider};
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{badge, caption, tag, token_reduction_ring};
use crate::ui::dialog::{card, centered, footer, header, header_with};
use crate::ui::field::{error, label};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;
use crate::ui::{CAPTION, CARD_PAD};

/// The picker's name for whichever level is currently chosen.
fn token_reduction_label(level: u8) -> &'static str {
    ket_core::worktree::token_reduction(level).name()
}

/// The level rows every Economy picker shows, with the current one ticked
/// and each one's measured saving down the right edge.
///
/// Shared with the sidebar's own picker (see
/// `crate::worktree_menu::open_token_reduction_menu`): a worktree's level can
/// be chosen when it is created and changed afterwards, and those are two
/// places to draw the same five rows.
///
/// Each row's second line is what the level changes — [`policy`] — when a
/// pack gives it any, and its description when it only asks for shorter
/// answers.
///
/// Each reduced level's detail is the share of output it is assumed to trim —
/// see [`saving_figure`].
pub(crate) fn token_reduction_items(
    current: u8,
    agent: Option<&str>,
    t: &Theme,
) -> Vec<MenuEntry<u8>> {
    token_reduction_entries(Some(current), agent, t, |level| level)
}

/// [`token_reduction_items`] for a menu that picks something other than the
/// bare level — `wrap` makes each row's value — and that may have none of
/// them ticked yet, when `current` is `None`.
pub(crate) fn token_reduction_entries<T>(
    current: Option<u8>,
    agent: Option<&str>,
    t: &Theme,
    wrap: impl Fn(u8) -> T,
) -> Vec<MenuEntry<T>> {
    ket_core::worktree::token_reduction_levels()
        .iter()
        .map(|level| {
            let changes = policy(level, agent);
            let subtitle = if changes.is_empty() {
                level.description.clone()
            } else {
                changes.join(" · ")
            };
            let mut item = MenuItem::new(wrap(level.level), level.name().to_owned())
                .subtitle(subtitle)
                .tinted_icon(Icon::Sliders, token_reduction_tint(level.level, t));
            if let Some(text) = saving_figure(level.level) {
                item = item.detail(text, None);
            }
            MenuEntry::Item(if Some(level.level) == current {
                item.checked()
            } else {
                item
            })
        })
        .collect()
}

/// What a level saves, as the picker and the dialog's footer both print it:
/// the share of output it is assumed to trim — see
/// [`ket_core::usage::ASSUMED_OUTPUT_REDUCTION`]. `None` at the default level,
/// which trims nothing.
fn saving_figure(level: u8) -> Option<String> {
    let trim = ket_core::usage::assumed_output_reduction(level);
    (trim > 0.0).then(|| format!("~{:.0}% less output", trim * 100.0))
}

/// What a level changes besides the sentence it asks with, each in the few
/// words a tag holds: "sonnet", "low effort", "1h cache".
///
/// Empty for every built-in level, which names no policy — and that is the
/// reason the setting is Economy rather than token reduction: once a pack
/// fills these in, a level is a spending policy, not a request for brevity.
/// Written as the pack wrote each value, the way the launch passes them
/// through (see `crate::terminal`).
fn policy(level: &ket_core::worktree::TokenReduction, agent: Option<&str>) -> Vec<String> {
    let policy = level.policy_for(agent.unwrap_or("opencode"));
    let worded = |value: &Option<String>, words: fn(&str) -> String| value.as_deref().map(words);
    [
        worded(&policy.model, str::to_owned),
        worded(&policy.effort, |v| format!("{v} effort")),
        worded(&policy.cache_ttl, |v| format!("{v} cache")),
        worded(&policy.subagent_model, |v| format!("{v} subagents")),
        worded(&policy.subagent_cache_ttl, |v| {
            format!("{v} subagent cache")
        }),
        worded(&policy.autocompact, |v| format!("autocompact {v}")),
        policy
            .agent_teams
            .map(|on| if on { "agent teams" } else { "no agent teams" }.to_owned()),
        worded(&policy.cross_session_inbound, |v| {
            format!("{v} other sessions")
        }),
        worded(&policy.verbosity, |v| format!("{v} verbosity")),
        worded(&policy.reasoning_summary, |v| format!("{v} summaries")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Where in those rows the current level sits, so the menu opens with it
/// under the keyboard rather than at the top.
pub(crate) fn token_reduction_index(current: u8) -> usize {
    ket_core::worktree::token_reduction_levels()
        .iter()
        .position(|l| l.level == current)
        .unwrap_or(0)
}

/// Tints the picker's icon more strongly as the level cuts deeper — a plain
/// slider glyph would say nothing about which end of the scale it is on.
///
/// `pub(crate)` because the sidebar's token-reduction ring (see
/// `crate::ui::chip::token_reduction_ring`) paints its arc in the exact same
/// colour — one function owning the tint keeps the picker's icon and the
/// row's ring from drifting apart the next time the depth curve changes.
pub(crate) fn token_reduction_tint(level: u8, t: &Theme) -> gpui::Rgba {
    let depth = ket_core::worktree::level_position(level)
        .map(|(position, count)| {
            if count <= 1 {
                0.0
            } else {
                position as f32 / (count - 1) as f32
            }
        })
        .unwrap_or(0.0);
    mix(paint(t.text.dim), paint(t.accent), depth)
}

/// The line under the Agent and Economy row: what the chosen level changes,
/// as tags, or its one-line description when it changes nothing but the
/// sentence the agent is handed.
///
/// One line where there were a paragraph and three caveats. The mechanism
/// note ("same model, same tools") stopped being true the day a pack could
/// set the model, and the caveats — which agents it reaches, that it can be
/// changed later — are things the worktree's own menu already shows.
fn economy_note(level: u8, agent: Option<&str>, t: &Theme) -> Div {
    let level = ket_core::worktree::token_reduction(level);
    let changes = policy(level, agent);
    if changes.is_empty() {
        return caption(level.description.clone(), t);
    }
    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .children(changes.into_iter().map(|words| tag(words, t)))
}

/// How wide the dialog is.
const DIALOG_WIDTH: Pixels = px(520.0);

/// The space between the Agent and Economy columns.
const COLUMN_GAP: Pixels = px(10.0);

/// How wide the dialog's fields run: the card less its padding and its
/// one-pixel edge on each side. Both dropdowns open at this width, so a menu
/// hung from a half-width select still lines up with the form.
fn form_width() -> Pixels {
    DIALOG_WIDTH - CARD_PAD * 2.0 - px(2.0)
}

/// How wide each of the Agent and Economy columns is.
fn column_width() -> Pixels {
    (form_width() - COLUMN_GAP) / 2.0
}

/// The dialog's state while it is open.
pub(crate) struct NewWorktree {
    /// The project the worktree will belong to.
    pub(crate) project: ProjectId,
    /// What the sidebar calls that project.
    pub(crate) project_name: SharedString,
    /// Whether the project is a field rather than part of the title: opened
    /// from the sidebar's foot, which belongs to no project, with more than
    /// one to put the worktree in.
    pub(crate) choose_project: bool,
    /// The project picker's keyboard focus.
    pub(crate) project_focus: FocusHandle,
    /// The shared dropdown while the project field is expanded.
    pub(crate) project_menu: Option<OpenMenu<ProjectId>>,
    /// Draft branch name. An entity because a text field is one — see
    /// [`crate::input`] for why the input method needs it to be.
    pub(crate) name: Entity<TextInput>,
    /// The chosen agent, an index into `agents`.
    pub(crate) agent: usize,
    /// The agent the dialog started on for the current project. Switching
    /// project moves an untouched pick to the new project's own start, and
    /// leaves one a person made alone.
    pub(crate) default_agent: usize,
    /// The agents the config knows, for the picker.
    pub(crate) agents: Vec<String>,
    /// How aggressively the agent launched here should cut its own token
    /// usage, `4` (no reduction) down to `0` (maximum) — see
    /// [`ket_core::worktree::Worktree::token_reduction`].
    pub(crate) token_reduction: u8,
    /// The token-reduction picker's keyboard focus.
    pub(crate) token_reduction_focus: FocusHandle,
    /// The shared dropdown while the token-reduction field is expanded.
    pub(crate) token_reduction_menu: Option<OpenMenu<u8>>,
    /// The agent picker's keyboard focus.
    ///
    /// Its own handle rather than a `Focus` enum beside the name field's:
    /// which control has the keyboard is a fact gpui already holds, and a
    /// second copy of it is a copy that goes stale the moment anything
    /// focuses or blurs without telling the dialog.
    pub(crate) agent_focus: FocusHandle,
    /// The shared dropdown while the agent field is expanded.
    pub(crate) agent_menu: Option<OpenMenu<usize>>,
    /// Why the last attempt was refused, if it was.
    pub(crate) error: Option<SharedString>,
    /// What the dialog is waiting on while a create runs off the window's
    /// thread — "Fetching origin…" — and the reason Enter and the button do
    /// nothing until it comes back.
    pub(crate) busy: Option<SharedString>,
    /// The dialog's own keyboard focus.
    ///
    /// Its own rather than the shell's, and taken when the dialog opens, so
    /// typing reaches the name field however the window was focused before —
    /// a terminal pane installs a text-input handler on the shell's handle,
    /// and a dialog sharing that handle is a dialog you cannot type into.
    pub(crate) focus: FocusHandle,
}

/// Where in `agents` a new worktree in `project` starts: the project's
/// preferred agent, which is the one a person who set a preference meant,
/// else the configured default.
fn starting_agent(workspace: &Workspace, agents: &[String], project: &ProjectId) -> usize {
    preferred_agent(workspace, agents, project)
        .and_then(|wanted| agents.iter().position(|name| name == &wanted))
        .unwrap_or(0)
}

/// The agent among `agents` new work in `project` is for: the project's
/// preferred one, else the configured default — under `Auto`, the first
/// agent there is. `None` when neither is among them. An empty name in
/// `agents` stands for no agent, and is never the answer.
pub(crate) fn preferred_agent(
    workspace: &Workspace,
    agents: &[String],
    project: &ProjectId,
) -> Option<String> {
    let preferred = workspace
        .projects()
        .ok()
        .and_then(|projects| projects.into_iter().find(|p| &p.id == project))
        .and_then(|project| project.preferred_agent);
    let preferred = preferred.filter(|wanted| agents.contains(wanted));
    let fallback = match &workspace.config().agent.default {
        DefaultAgent::None => None,
        DefaultAgent::Named(name) => agents.contains(name).then(|| name.clone()),
        DefaultAgent::Auto => agents.iter().find(|name| !name.is_empty()).cloned(),
    };
    preferred.or(fallback)
}

impl Shell {
    /// Opens the dialog for the project at `index` in the sidebar.
    ///
    /// `choose_project` is for a caller that belongs to no project — the
    /// sidebar's foot — where `index` is only where the picker starts. With
    /// a single project there is nothing to choose, and it is stated instead.
    pub(crate) fn open_new_worktree(
        &mut self,
        index: usize,
        choose_project: bool,
        cx: &mut Context<Self>,
    ) {
        self.project_menu = None;
        let Some(node) = self.projects.get(index) else {
            return;
        };
        let id = node.id.clone();
        let name = node.name.clone();

        let Ok(workspace) = Workspace::open() else {
            return;
        };
        let catalogue = Catalogue::detect(workspace.config(), &ShellProber);
        let agents: Vec<String> = std::iter::once(String::new())
            .chain(catalogue.runnable().map(|entry| entry.name.clone()))
            .collect();
        let agent = starting_agent(&workspace, &agents, &id);

        // The dialog exists to be typed into, so the name asks for the
        // keyboard as it is built. Granted on the first frame — see
        // `TextInput::request_focus`.
        let branch = TextInput::new("Branch name", cx);
        branch.update(cx, |input, _| input.request_focus());
        self.watch_field(&branch, cx);

        self.popup = None;
        self.new_worktree = Some(NewWorktree {
            project: id,
            project_name: name,
            choose_project: choose_project && self.projects.len() > 1,
            project_focus: cx.focus_handle(),
            project_menu: None,
            name: branch,
            agent,
            default_agent: agent,
            agents,
            agent_focus: cx.focus_handle(),
            agent_menu: None,
            token_reduction: ket_core::worktree::default_token_reduction(),
            token_reduction_focus: cx.focus_handle(),
            token_reduction_menu: None,
            error: None,
            busy: None,
            focus: cx.focus_handle(),
        });
    }

    /// Closes it without creating anything.
    pub(crate) fn close_new_worktree(&mut self) {
        self.new_worktree = None;
    }

    /// Opens the dialog for the project in context, for the New Worktree
    /// chord.
    ///
    /// The project in context is the one the current terminal belongs to: a
    /// terminal lives in a worktree's space, and the project is whichever one
    /// holds that worktree. Without one — nothing open yet, or the active tab
    /// is not a terminal — the first expanded project in the sidebar is the
    /// one being looked at, so that is the context instead. Neither
    /// anywhere: there is nothing to open for, and the chord does nothing.
    pub(crate) fn new_worktree_from_shortcut(&mut self, cx: &mut Context<Self>) -> bool {
        let project = self
            .active_terminal_id()
            .and_then(|terminal| self.project_of_terminal(terminal))
            .or_else(|| self.projects.iter().position(|project| project.expanded));
        let Some(index) = project else {
            return false;
        };
        self.open_new_worktree(index, false, cx);
        true
    }

    /// The project whose worktree holds `terminal`, if one still does.
    ///
    /// Which space holds a terminal is not tracked, and does not need to be:
    /// the same walk [`Shell::close_terminal`] makes, then one more step from
    /// the worktree to the project that lists it.
    pub(crate) fn project_of_terminal(&self, terminal: TerminalId) -> Option<usize> {
        let worktree = self.worktree_of_terminal(terminal)?;
        self.projects
            .iter()
            .position(|project| project.worktrees.iter().any(|node| node.id == worktree))
    }

    /// The worktree whose space holds `terminal`, if one still does.
    pub(crate) fn worktree_of_terminal(
        &self,
        terminal: TerminalId,
    ) -> Option<ket_core::id::WorktreeId> {
        self.spaces
            .iter()
            .find(|(_, space)| space.root.find_tab(&TabKind::Terminal(terminal)).is_some())
            .map(|(id, _)| id.clone())
    }

    /// Moves the dialog to another project, carrying the agent along with it
    /// unless it was still the one the last project started on.
    fn set_new_worktree_project(&mut self, id: ProjectId) {
        let Some(node) = self.projects.iter().find(|node| node.id == id) else {
            return;
        };
        let name = node.name.clone();
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        dialog.project_menu = None;
        if dialog.project == id {
            return;
        }
        if let Ok(workspace) = Workspace::open() {
            let start = starting_agent(&workspace, &dialog.agents, &id);
            if dialog.agent == dialog.default_agent {
                dialog.agent = start;
            }
            dialog.default_agent = start;
        }
        dialog.project = id;
        dialog.project_name = name;
        dialog.error = None;
    }

    /// Opens or closes the project dropdown.
    fn toggle_new_worktree_project_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.project_menu.take().is_some() {
            return;
        }
        dialog.agent_menu = None;
        dialog.token_reduction_menu = None;

        let selected = self
            .projects
            .iter()
            .position(|node| node.id == dialog.project)
            .unwrap_or(0);
        let entries = self
            .projects
            .iter()
            .map(|node| {
                let item = MenuItem::new(node.id.clone(), node.name.clone())
                    .tinted_icon(Icon::Folder, paint(node.color));
                MenuEntry::Item(if node.id == dialog.project {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        dialog.project_menu = Some(
            OpenMenu::new("new-worktree-project".into(), None, form_width(), entries)
                .selected(selected)
                .offset(px(0.0), px(6.0)),
        );
    }

    /// Lets the shared dropdown take keys before the dialog interprets them.
    fn new_worktree_project_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let Some(menu) = self
            .new_worktree
            .as_mut()
            .and_then(|dialog| dialog.project_menu.as_mut())
        else {
            return false;
        };

        match menu.key(event) {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.project_menu = None;
                }
                true
            }
            MenuKey::Run(project) => {
                self.set_new_worktree_project(project);
                true
            }
        }
    }

    /// Opens or closes the configured-agent dropdown.
    fn toggle_agent_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.agent_menu.take().is_some() {
            return;
        }
        // Only one dropdown is open at a time: opening this one closes
        // whichever other the dialog had open.
        dialog.token_reduction_menu = None;
        dialog.project_menu = None;

        dialog.agent_menu = Some(
            OpenMenu::new(
                "new-worktree-agent".into(),
                Some("Choose an agent…".into()),
                form_width(),
                dialog
                    .agents
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        let (which, colour) = provider(name, &self.theme);
                        let label = if name.is_empty() {
                            "No agent (blank terminal)".to_owned()
                        } else {
                            agent_label(name)
                        };
                        let item = MenuItem::new(index, label).tinted_icon(which, colour);
                        MenuEntry::Item(if index == dialog.agent {
                            item.checked()
                        } else {
                            item
                        })
                    })
                    .collect(),
            )
            .selected(dialog.agent)
            .offset(px(0.0), px(6.0)),
        );
    }

    /// Opens or closes the token-reduction dropdown.
    fn toggle_token_reduction_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.token_reduction_menu.take().is_some() {
            return;
        }
        dialog.agent_menu = None;
        dialog.project_menu = None;

        let current = dialog.token_reduction;
        let agent = dialog.agents.get(dialog.agent).cloned();
        dialog.token_reduction_menu = Some(
            OpenMenu::new(
                "new-worktree-token-reduction".into(),
                None,
                form_width(),
                token_reduction_items(current, agent.as_deref(), &self.theme),
            )
            .selected(token_reduction_index(current))
            // Shifted back over the Agent column so the panel spans the form
            // rather than hanging off the dialog's right edge.
            .offset(-(column_width() + COLUMN_GAP), px(6.0)),
        );
    }

    /// Lets the shared dropdown take keys before the dialog interprets them.
    fn new_worktree_token_reduction_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let Some(menu) = self
            .new_worktree
            .as_mut()
            .and_then(|dialog| dialog.token_reduction_menu.as_mut())
        else {
            return false;
        };

        match menu.key(event) {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.token_reduction_menu = None;
                }
                true
            }
            MenuKey::Run(level) => {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.token_reduction = level;
                    dialog.token_reduction_menu = None;
                }
                true
            }
        }
    }

    /// Lets the shared dropdown take keys before the dialog interprets them.
    fn new_worktree_agent_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let Some(menu) = self
            .new_worktree
            .as_mut()
            .and_then(|dialog| dialog.agent_menu.as_mut())
        else {
            return false;
        };

        match menu.key(event) {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.agent_menu = None;
                }
                true
            }
            MenuKey::Run(agent) => {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.agent = agent;
                    dialog.agent_menu = None;
                }
                true
            }
        }
    }

    /// Creates the worktree and selects it.
    /// Creates the worktree, off the window's thread.
    ///
    /// Creation now fetches the remote first so the branch is cut from the
    /// main everyone else can see, and a fetch is a network round trip that
    /// may take seconds or time out. The dialog stays up and says what it is
    /// waiting on; the result lands in [`Shell::finish_create_worktree`].
    pub(crate) fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_worktree.as_ref() else {
            return;
        };
        if dialog.busy.is_some() {
            return;
        }
        let branch = dialog.name.read(cx).text().trim().to_owned();
        if branch.is_empty() {
            if let Some(dialog) = self.new_worktree.as_mut() {
                dialog.error = Some("a worktree needs a branch name".into());
                let name = dialog.name.clone();
                name.update(cx, |input, _| input.request_focus());
            }
            return;
        }
        let project = dialog.project.clone();
        let agent = dialog
            .agents
            .get(dialog.agent)
            .filter(|name| !name.is_empty())
            .cloned();
        let token_reduction = dialog.token_reduction;
        let economy_level_id = ket_core::worktree::token_reduction(token_reduction)
            .id
            .clone();

        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.error = None;
            dialog.busy = Some("Fetching the remote and creating the worktree…".into());
        }

        // `Workspace` is opened inside the task rather than passed in, as the
        // merge does: it owns a store handle and a mutex of live sessions.
        let work = cx.background_executor().spawn(async move {
            let workspace = Workspace::open()?;
            let (created, prepared) = workspace.create_worktree_prepared_with_economy(
                &project,
                &branch,
                None,
                agent.as_deref(),
                Some(&economy_level_id),
            )?;
            Ok::<_, KetError>((created, prepared))
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = work.await;
            let _ = shell.update(cx, |shell, cx| shell.finish_create_worktree(result, cx));
        })
        .detach();
        cx.notify();
    }

    /// Puts the result of [`Shell::create_worktree`] where it can be seen.
    fn finish_create_worktree(
        &mut self,
        result: Result<(Worktree, PreparedBase), KetError>,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.busy = None;
        }
        match result {
            Ok((created, prepared)) => {
                if let Some(notice) = prepared.notice() {
                    tracing::info!(worktree = %created.id, "{notice}");
                }
                self.new_worktree = None;
                self.reload(cx);
                // Otherwise the sidebar shows a worktree with no size at all
                // until the next launch's sweep or someone opens the storage
                // card — leaving the project's total looking like it counted
                // rows the header itself is silently missing.
                self.remeasure_worktree(created.id.clone(), cx);
                // Land in it. `select` opens a terminal on a first visit, so
                // creating a worktree puts you in a shell inside it — and
                // when Settings has selecting open none, its session opens
                // here anyway: a worktree made with an agent is asking for it.
                if let Some(selection) = self.position_of(&created.id) {
                    self.select(selection, cx);
                    if self
                        .spaces
                        .get(&created.id)
                        .is_none_or(|space| space.terminal_ids().is_empty())
                    {
                        self.open_worktree_session_tab(cx);
                    }
                }
            }
            // Kept open on failure: the name that was refused is still in the
            // field, which is where a person fixes it. Selected, too — the
            // usual next move is to replace it, not to edit one character of
            // it, and a selection makes both one keystroke away.
            Err(e) => {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = Some(e.to_string().into());
                    let name = dialog.name.clone();
                    name.update(cx, |input, _| {
                        input.select_all();
                        input.request_focus();
                    });
                }
            }
        }
        cx.notify();
    }

    /// Handles a key while the dialog is open. Returns whether it consumed one.
    ///
    /// The name field gets first refusal whenever it has the keyboard, and
    /// what it declines — Enter, Escape, Tab and the vertical arrows — is what
    /// is left here. That ordering is the whole fix for the arrows editing the
    /// agent while you were typing a name: the field takes Left and Right, and
    /// only the keys it does not want ever reach the picker.
    pub(crate) fn new_worktree_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        self.new_worktree.as_ref()?;
        // Whichever menu is open is the topmost thing on screen, so it
        // answers before the name field does.
        if self.new_worktree_project_menu_key(event)
            || self.new_worktree_agent_menu_key(event)
            || self.new_worktree_token_reduction_menu_key(event)
        {
            return Some(Key::Taken);
        }
        let dialog = self.new_worktree.as_ref()?;
        let on_name = dialog.name.read(cx).is_focused(window);
        let name = dialog.name.clone();

        if on_name {
            // Text is never taken. It has to keep travelling until macOS's
            // input context sees it, or there are no dead keys and no input
            // methods — see [`crate::input`].
            if is_text(&event.keystroke) {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = None;
                }
                return Some(Key::Text);
            }
            if name.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
                // Editing is the answer to whatever the last refusal said.
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = None;
                }
                return Some(Key::Taken);
            }
        }

        let on_agent = self
            .new_worktree
            .as_ref()
            .is_some_and(|dialog| dialog.agent_focus.is_focused(window));
        let on_token_reduction = self
            .new_worktree
            .as_ref()
            .is_some_and(|dialog| dialog.token_reduction_focus.is_focused(window));
        let on_project = self
            .new_worktree
            .as_ref()
            .is_some_and(|dialog| dialog.project_focus.is_focused(window));
        match event.keystroke.key.as_str() {
            "escape" => self.close_new_worktree(),
            "enter" if on_project => self.toggle_new_worktree_project_menu(),
            "enter" if on_agent => self.toggle_agent_menu(),
            "enter" if on_token_reduction => self.toggle_token_reduction_menu(),
            "enter" => self.create_worktree(cx),
            "tab" => {
                if let Some(dialog) = self.new_worktree.as_ref() {
                    let name = dialog.name.clone();
                    let agent = dialog.agent_focus.clone();
                    let token_reduction = dialog.token_reduction_focus.clone();
                    let project = dialog.project_focus.clone();
                    // The project comes first on screen but last in the
                    // cycle: the dialog opens on the name, and Tab walks on
                    // from there and round.
                    if on_name {
                        window.focus(&agent);
                    } else if on_agent {
                        window.focus(&token_reduction);
                    } else if on_token_reduction && dialog.choose_project {
                        window.focus(&project);
                    } else {
                        // Landing on the name selects what is there, the way
                        // tabbing into a field does everywhere else.
                        name.update(cx, |input, _| input.select_all());
                        window.focus(name.read(cx).focus_handle());
                    }
                }
            }
            // Opening first gives the shared menu ownership of subsequent
            // arrows, Enter and Escape.
            "up" | "down" | "space" if on_project => {
                self.toggle_new_worktree_project_menu();
            }
            "up" | "down" | "space" if on_agent => {
                self.toggle_agent_menu();
            }
            "up" | "down" | "space" if on_token_reduction => {
                self.toggle_token_reduction_menu();
            }
            // Everything else is swallowed. A modal takes every key it is
            // given, so a stray chord cannot drive the shell behind it — Cmd-K
            // must not open the palette over an open dialog. The app's own
            // chords are unaffected: gpui matches key *bindings* to actions
            // before it runs any `on_key_down`, so Cmd-Q still quits.
            _ => {}
        }

        Some(Key::Taken)
    }

    /// The dialog, or nothing when it is closed.
    pub(crate) fn new_worktree_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.new_worktree.as_ref()?;
        let t = &self.theme;

        // Stated rather than chosen when the `+` was on a project, so the
        // answer is already known — and said in the title, where it reads as
        // which project this is, rather than as a field that looks editable
        // and is not. From the sidebar's foot it is a field, since there it
        // is not known.
        let colour = self
            .projects
            .iter()
            .find(|node| node.id == dialog.project)
            .map(|node| node.color)
            .unwrap_or_else(crate::projects::default_color);
        let initial = dialog
            .project_name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        let title = if dialog.choose_project {
            header("Create worktree", t)
        } else {
            header_with(
                "Create worktree in",
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(badge(colour, initial.clone(), t))
                    .child(dialog.project_name.clone()),
                t,
            )
        };
        let project_select = dialog.choose_project.then(|| {
            let project_menu = dialog.project_menu.as_ref().map(|menu| {
                menu.view(
                    t,
                    cx,
                    |shell, project, cx| {
                        shell.set_new_worktree_project(project.clone());
                        cx.notify();
                    },
                    |shell, cx| {
                        if let Some(dialog) = shell.new_worktree.as_mut() {
                            dialog.project_menu = None;
                        }
                        cx.notify();
                    },
                )
            });
            dropdown(
                "new-worktree-project-select",
                select("new-worktree-project", dialog.project_name.clone())
                    .leading(badge(colour, initial.clone(), t))
                    .open(dialog.project_menu.is_some())
                    .focused(dialog.project_focus.is_focused(window))
                    .render(t),
                project_menu,
            )
            .track_focus(&dialog.project_focus)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window: &mut Window, cx| {
                    if let Some(dialog) = this.new_worktree.as_ref() {
                        window.focus(&dialog.project_focus.clone());
                    }
                    this.toggle_new_worktree_project_menu();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
        });

        let name_row = text_field(
            &dialog.name,
            "new-worktree-name",
            dialog.error.is_some(),
            Style::new(t, self.caret.visible),
            window,
            cx,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.notify()));

        let current: SharedString = dialog
            .agents
            .get(dialog.agent)
            .map(|name| {
                if name.is_empty() {
                    "No agent (blank terminal)".to_owned()
                } else {
                    agent_label(name)
                }
            })
            .unwrap_or_else(|| "No agent (blank terminal)".to_owned())
            .into();
        // The command this project launches the chosen agent with, when it is
        // its own rather than the one in Settings: named in the trigger and
        // edged in the tint, so a worktree is never started on the wrong
        // account by someone who only read "Claude Code".
        let launches = dialog.agents.get(dialog.agent).and_then(|agent| {
            self.projects
                .iter()
                .find(|project| project.id == dialog.project)?
                .agent_overrides
                .get(agent)
                .map(crate::agent_override::short)
        });
        let tint = crate::agent_override::hue(t);
        let agent_lit = dialog.agent_menu.is_some() || dialog.agent_focus.is_focused(window);
        let mut agent_row = select("new-worktree-agent", current).leading(mark(
            dialog
                .agents
                .get(dialog.agent)
                .map(String::as_str)
                .unwrap_or(""),
            t,
        ));
        if let Some(launches) = launches.clone() {
            agent_row = agent_row.trailing(
                div()
                    .flex_none()
                    .font_family(crate::fonts::chrome())
                    .text_size(px(11.5))
                    .text_color(tint)
                    .child(launches),
            );
        }
        let agent_row = agent_row
            .open(dialog.agent_menu.is_some())
            .focused(dialog.agent_focus.is_focused(window))
            .render(t)
            // The ring still wins while the menu is open or Tab is on it: the
            // tint is a fact about the choice, focus is where the keys go.
            .when(launches.is_some() && !agent_lit, |el| {
                el.border_color(alpha(tint, 0.6))
            });

        let agent_menu = dialog.agent_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, agent, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.agent = *agent;
                        dialog.agent_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.agent_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let agent_select = dropdown("new-worktree-agent-select", agent_row, agent_menu)
            .track_focus(&dialog.agent_focus)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window: &mut Window, cx| {
                    if let Some(dialog) = this.new_worktree.as_ref() {
                        window.focus(&dialog.agent_focus.clone());
                    }
                    this.toggle_agent_menu();
                    cx.stop_propagation();
                    cx.notify();
                }),
            );

        let token_reduction_row = select(
            "new-worktree-token-reduction",
            token_reduction_label(dialog.token_reduction),
        )
        .leading(token_reduction_ring(dialog.token_reduction, t))
        .open(dialog.token_reduction_menu.is_some())
        .focused(dialog.token_reduction_focus.is_focused(window))
        .render(t);

        let token_reduction_menu = dialog.token_reduction_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, level, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.token_reduction = *level;
                        dialog.token_reduction_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.token_reduction_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let token_reduction_select = dropdown(
            "new-worktree-token-reduction-select",
            token_reduction_row,
            token_reduction_menu,
        )
        .track_focus(&dialog.token_reduction_focus)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window: &mut Window, cx| {
                if let Some(dialog) = this.new_worktree.as_ref() {
                    window.focus(&dialog.token_reduction_focus.clone());
                }
                this.toggle_token_reduction_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );

        let close = icon_button("new-worktree-close", Icon::Close)
            .circle()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_new_worktree();
                cx.notify();
            }));

        let cancel = button("new-worktree-cancel", "Cancel")
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_new_worktree();
                cx.notify();
            }));

        // Disabled until there is something to name the branch. The dialog
        // still explains *why* if Enter is pressed anyway — see
        // `create_worktree` — so this removes a dead click, not the message.
        let create = button("new-worktree-create", "Create worktree")
            .primary()
            .enabled(!dialog.name.read(cx).text().trim().is_empty() && dialog.busy.is_none())
            .hint("\u{23ce}")
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.create_worktree(cx);
                cx.notify();
            }));

        let row = |title: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_col()
                .child(label(title, t))
                .child(control)
        };

        // Agent and Economy side by side: both are one pick from a short
        // list, and a column each leaves the dialog a row shorter. The note
        // under them is Economy's, but spans both — a tag row cut to half the
        // form would wrap after two tags.
        let choices = div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(
                div()
                    .flex()
                    .gap(COLUMN_GAP)
                    .child(
                        row("Agent", agent_select.into_any_element())
                            .w(column_width())
                            .flex_none(),
                    )
                    .child(
                        row("Economy", token_reduction_select.into_any_element())
                            .w(column_width())
                            .flex_none(),
                    ),
            )
            .child(economy_note(
                dialog.token_reduction,
                dialog.agents.get(dialog.agent).map(String::as_str),
                t,
            ));

        // What the chosen level trims, on the footer's otherwise empty left.
        // None at the default, which trims nothing.
        let saving = saving_figure(dialog.token_reduction)
            .map(|text| caption(text, t).font_family(crate::fonts::chrome()));

        Some(
            centered(
                card("new-worktree", DIALOG_WIDTH, t)
                    .track_focus(&dialog.focus)
                    // Propagation stops for a command and not for text: see
                    // `Key`, and `crate::input`'s module docs for why.
                    .on_key_down(cx.listener(
                        |this, event: &gpui::KeyDownEvent, window: &mut Window, cx| {
                            if this.new_worktree_key(event, window, cx) == Some(Key::Taken) {
                                cx.stop_propagation();
                            }
                            cx.notify();
                        },
                    ))
                    .child(title.child(close))
                    .children(
                        project_select.map(|select| row("Project", select.into_any_element())),
                    )
                    .child(row("Name", name_row.into_any_element()))
                    .child(choices)
                    .children(dialog.error.clone().map(|why| error(why, t)))
                    .children(dialog.busy.clone().map(|what| waiting(what, t)))
                    .child(
                        footer()
                            .children(saving)
                            .child(div().flex_grow())
                            .child(cancel)
                            .child(create),
                    ),
            )
            .into_any_element(),
        )
    }
}

/// The line under the fields while a create runs: the same place and size
/// as an [`error`], in the dim colour of a status rather than the red of a
/// refusal.
fn waiting(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .mt(px(7.0))
        .text_size(CAPTION)
        .text_color(paint(t.text.dim))
        .child(text.into())
}
