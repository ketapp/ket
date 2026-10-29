//! The **New worktree** dialog, behind the `+` beside a project's name.
//!
//! It opens on a prompt, the way the phone's New task does: say what the
//! agent should do, and the worktree is made, named from the prompt's first
//! words, with the agent started on it — the same path a phone or `ket
//! start` takes (`crate::phone_work`). The branch fills itself from the
//! prompt and can be typed over; left empty, the prompt is no prompt, and
//! the dialog makes a worktree and a shell as it always did.
//!
//! Deliberately spare otherwise. "Run on" is absent because everything is
//! local — a picker with one entry is a question with one answer, and asking
//! it is worse than not asking. The project is stated in the title when the
//! `+` was on one, and asked for when the dialog came from the sidebar's foot
//! with more than one project to choose between. The agent is recorded on
//! the worktree (see `ket_core::worktree::Worktree::agent`) so the sidebar
//! can say what a worktree is *for* before anything has run in it; the
//! model and effort are for its first session only, and Economy — a spending
//! policy set now and then — sits small in the footer.
//!
//! Each control is focusable, and which one has the keyboard is a fact gpui
//! holds: the arrow keys mean "move the caret" in a field and "change the
//! pick" on a picker, and only the keys a field declines reach the pickers.

use gpui::{
    AnyElement, Context, Entity, FocusHandle, KeyDownEvent, Keystroke, MouseButton, Pixels,
    SharedString, Window, div, prelude::*, px,
};
use ket_core::KetError;
use ket_core::agent_model::Choice;
use ket_core::agents::{Catalogue, DefaultAgent, ShellProber};
use ket_core::git::PreparedBase;
use ket_core::id::ProjectId;
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;
use ket_core::worktree::Worktree;

use crate::Shell;
use crate::editor::NativeEdit;
use crate::input::{Key, Style, TextInput, is_text, text_field};
use crate::paint::{alpha, mix, paint};
use crate::phone_work::{NewWork, WorkOrigin};
use crate::tabs::TabKind;
use crate::terminal::{SessionLaunch, TerminalId};
use crate::ui::CARD_PAD;
use crate::ui::agent::{agent_label, mark, provider};
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{badge, caption, tag, token_reduction_ring};
use crate::ui::dialog::{card, centered, footer, header, header_with};
use crate::ui::field::{error, label};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;
use crate::ui::textarea::{FORM_INSET, textarea};
use crate::ui::toast::Tone;

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
    let agent = agent.unwrap_or("opencode");
    let policy = level.policy_for(agent);
    let worded = |value: &Option<String>, words: fn(&str) -> String| value.as_deref().map(words);
    // The cap in the provider's own unit, the way the pack wrote it.
    let unit = if agent.eq_ignore_ascii_case("codex") {
        "token"
    } else {
        "char"
    };
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
        policy
            .output_limit
            .map(|limit| format!("{} {unit} output", short_count(limit))),
        policy
            .command_filter
            .filter(|on| *on)
            .map(|_| "filtered output".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// A count at tag width: `20k`, `1.5k`, `800`.
pub(crate) fn short_count(count: u32) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=9_999 if !count.is_multiple_of(1_000) => {
            format!("{:.1}k", f64::from(count) / 1_000.0)
        }
        _ => format!("{}k", count / 1_000),
    }
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

/// How wide the dialog is.
const DIALOG_WIDTH: Pixels = px(520.0);

/// The space between the Agent, Model and Effort columns.
const COLUMN_GAP: Pixels = px(10.0);

/// How wide the dialog's fields run: the card less its padding and its
/// one-pixel edge on each side. Both dropdowns open at this width, so a menu
/// hung from a half-width select still lines up with the form.
fn form_width() -> Pixels {
    DIALOG_WIDTH - CARD_PAD * 2.0 - px(2.0)
}

/// How wide each of the Agent, Model and Effort columns is.
fn column_width() -> Pixels {
    (form_width() - COLUMN_GAP * 2.0) / 3.0
}

/// The composer the prompt is written in.
const PROMPT_EDITOR: &str = "new-worktree:prompt";

/// How tall the prompt's well is: a short paragraph before it scrolls.
const PROMPT_HEIGHT: Pixels = px(110.0);

/// The size the prompt is set at: a field's, since it shares the dialog
/// with fields rather than being the one thing in it.
const PROMPT_TEXT: Pixels = px(13.0);

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
    ///
    /// Filled from the prompt as it is typed until somebody types in here,
    /// after which it is theirs — see `branch_edited`.
    pub(crate) name: Entity<TextInput>,
    /// Whether the branch was typed rather than taken from the prompt. Set
    /// when the field's text stops being what `synced_branch` last put
    /// there, cleared when the field is emptied or its reset is clicked.
    pub(crate) branch_edited: bool,
    /// What the dialog last wrote into the branch field from the prompt, so
    /// a change made any other way — typed, pasted from the Edit menu — can
    /// be told from its own.
    pub(crate) synced_branch: String,
    /// The chosen agent, an index into `agents`.
    pub(crate) agent: usize,
    /// The agent the dialog started on for the current project. Switching
    /// project moves an untouched pick to the new project's own start, and
    /// leaves one a person made alone.
    pub(crate) default_agent: usize,
    /// The agents the config knows, for the picker.
    pub(crate) agents: Vec<String>,
    /// The model the first session runs with; `None` is the agent's own
    /// default. An id out of `models`.
    pub(crate) model: Option<String>,
    /// The effort likewise, out of `efforts`.
    pub(crate) effort: Option<String>,
    /// What the chosen agent can be switched to. Empty for no agent, and
    /// for one ket cannot switch — a local model.
    pub(crate) models: Vec<Choice>,
    pub(crate) efforts: Vec<Choice>,
    /// The model picker's keyboard focus.
    pub(crate) model_focus: FocusHandle,
    /// The shared dropdown while the model field is expanded. An index: `0`
    /// is the default, `n` is `models[n - 1]`.
    pub(crate) model_menu: Option<OpenMenu<usize>>,
    /// The effort picker's keyboard focus.
    pub(crate) effort_focus: FocusHandle,
    /// The shared dropdown while the effort field is expanded, indexed as
    /// the model menu is.
    pub(crate) effort_menu: Option<OpenMenu<usize>>,
    /// How aggressively the agent launched here should cut its own token
    /// usage, `4` (no reduction) down to `0` (maximum) — see
    /// [`ket_core::worktree::Worktree::token_reduction`].
    pub(crate) token_reduction: u8,
    /// The token-reduction picker's keyboard focus.
    pub(crate) token_reduction_focus: FocusHandle,
    /// The shared dropdown while the token-reduction trigger is expanded.
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
    /// Whether a create is running off the window's thread, which is why
    /// Enter and the button do nothing until it comes back. Not shown: a
    /// create is usually over before a status line could be read.
    pub(crate) busy: bool,
    /// The dialog's own keyboard focus, which is the prompt's: the composer
    /// has the keys whenever no field or picker has taken them.
    ///
    /// Its own rather than the shell's, and taken when the dialog opens, so
    /// typing reaches the prompt however the window was focused before —
    /// a terminal pane installs a text-input handler on the shell's handle,
    /// and a dialog sharing that handle is a dialog you cannot type into.
    pub(crate) focus: FocusHandle,
    /// Whether the next frame should put the keyboard on the prompt: set
    /// when the dialog opens, where there is no window to ask.
    pub(crate) wants_focus: bool,
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

/// What `agent` can be switched to, for the Model and Effort pickers:
/// nothing for no agent, or one ket cannot switch.
fn agent_choices(agent: &str) -> (Vec<Choice>, Vec<Choice>) {
    if agent.is_empty() {
        return (Vec::new(), Vec::new());
    }
    ket_core::agent_model::settings(agent, None)
        .map(|settings| (settings.models, settings.efforts))
        .unwrap_or_default()
}

/// What a shared dropdown made of a key: `None` when it was not the menu's,
/// `Some(None)` when it was, `Some(Some(value))` when a row was chosen. The
/// menu is closed for a choice and for Escape.
fn dropdown_key<T: Clone + 'static>(
    menu: &mut Option<OpenMenu<T>>,
    event: &KeyDownEvent,
) -> Option<Option<T>> {
    let open = menu.as_mut()?;
    match open.key(event) {
        MenuKey::Ignored => None,
        MenuKey::Consumed => Some(None),
        MenuKey::Close => {
            *menu = None;
            Some(None)
        }
        MenuKey::Run(value) => {
            *menu = None;
            Some(Some(value))
        }
    }
}

/// The rows of a Model or Effort menu: the agent's default first, then each
/// choice, the current one ticked.
fn choice_items(
    choices: &[Choice],
    current: Option<&str>,
    default_hint: &str,
) -> Vec<MenuEntry<usize>> {
    std::iter::once((
        0,
        "Default".to_owned(),
        Some(default_hint.to_owned()),
        current.is_none(),
    ))
    .chain(choices.iter().enumerate().map(|(index, choice)| {
        (
            index + 1,
            choice.label.clone(),
            None,
            current == Some(choice.id.as_str()),
        )
    }))
    .map(|(index, label, subtitle, checked)| {
        let mut item = MenuItem::new(index, label);
        if let Some(subtitle) = subtitle {
            item = item.subtitle(subtitle);
        }
        MenuEntry::Item(if checked { item.checked() } else { item })
    })
    .collect()
}

/// What a Model or Effort trigger says: nothing with nothing to pick, the
/// agent's default until picked, else the pick's label.
fn choice_label(choices: &[Choice], current: Option<&str>) -> SharedString {
    if choices.is_empty() {
        return SharedString::default();
    }
    current
        .and_then(|id| choices.iter().find(|choice| choice.id == id))
        .map_or_else(|| "Default".into(), |choice| choice.label.clone().into())
}

/// The branch a submit makes: what was typed, made into something git will
/// take — spaces become dashes rather than git's refusal landing in the
/// dialog verbatim — or, untyped, the prompt's first words, which are
/// already in that shape. `Err` says why there is none.
fn submitted_branch(prompt: &str, typed: &str, edited: bool) -> Result<String, &'static str> {
    let branch = if edited {
        ket_core::git::branch_name_from(typed)
    } else {
        crate::phone_work::branch_stem(prompt)
    };
    if !branch.is_empty() {
        return Ok(branch);
    }
    Err(if prompt.is_empty() {
        "a worktree needs a prompt or a branch name"
    } else {
        "the prompt needs a word or two to name the branch from"
    })
}

/// `branch` once it is known whether a branch of that name `exists`: a name
/// from the prompt that is taken gets the phone's tag after it, since
/// nobody chose it; a typed one that is taken is refused, since somebody
/// did.
fn branch_when_taken(
    branch: String,
    prompt: &str,
    exists: bool,
    edited: bool,
) -> Result<String, KetError> {
    match (exists, edited) {
        (false, _) => Ok(branch),
        (true, false) => Ok(crate::phone_work::branch_for(prompt, &branch)),
        (true, true) => Err(KetError::Conflict(format!(
            "a branch called {branch} already exists"
        ))),
    }
}

/// Which row of a Model or Effort menu is `current`, for the keyboard
/// selection to start on.
fn choice_index(choices: &[Choice], current: Option<&str>) -> usize {
    current
        .and_then(|id| choices.iter().position(|choice| choice.id == id))
        .map_or(0, |index| index + 1)
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
        let (models, efforts) = agent_choices(agents.get(agent).map_or("", String::as_str));

        // The prompt is what the dialog opens on; the branch fills itself
        // from it until somebody types there instead.
        self.open_composer(PROMPT_EDITOR, "");
        self.set_composer_size(PROMPT_EDITOR, PROMPT_TEXT);
        let branch = TextInput::new("Branch name", cx);
        self.watch_field(&branch, cx);

        self.popup = None;
        self.new_worktree = Some(NewWorktree {
            project: id,
            project_name: name,
            choose_project: choose_project && self.projects.len() > 1,
            project_focus: cx.focus_handle(),
            project_menu: None,
            name: branch,
            branch_edited: false,
            synced_branch: String::new(),
            agent,
            default_agent: agent,
            agents,
            model: None,
            effort: None,
            models,
            efforts,
            model_focus: cx.focus_handle(),
            model_menu: None,
            effort_focus: cx.focus_handle(),
            effort_menu: None,
            agent_focus: cx.focus_handle(),
            agent_menu: None,
            token_reduction: ket_core::worktree::default_token_reduction(),
            token_reduction_focus: cx.focus_handle(),
            token_reduction_menu: None,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            wants_focus: true,
        });
    }

    /// Closes it without creating anything.
    pub(crate) fn close_new_worktree(&mut self) {
        self.close_composer(PROMPT_EDITOR);
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
                Self::set_new_worktree_agent(dialog, start);
            }
            dialog.default_agent = start;
        }
        dialog.project = id;
        dialog.project_name = name;
        dialog.error = None;
    }

    /// Switches the dialog's agent, and the Model and Effort pickers to what
    /// it can be switched to: a model is a fact about one agent, so a pick
    /// made for the last one does not carry.
    fn set_new_worktree_agent(dialog: &mut NewWorktree, agent: usize) {
        if dialog.agent == agent {
            return;
        }
        dialog.agent = agent;
        Self::reset_agent_choices(dialog);
    }

    fn reset_agent_choices(dialog: &mut NewWorktree) {
        let (models, efforts) =
            agent_choices(dialog.agents.get(dialog.agent).map_or("", String::as_str));
        dialog.models = models;
        dialog.efforts = efforts;
        dialog.model = None;
        dialog.effort = None;
    }

    /// Fills the branch from the prompt, unless it has been typed in.
    fn sync_branch_from_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_worktree.as_ref() else {
            return;
        };
        if dialog.branch_edited {
            return;
        }
        let stem = crate::phone_work::branch_stem(&self.composer_text(PROMPT_EDITOR));
        let name = dialog.name.clone();
        if name.read(cx).text() != stem {
            name.update(cx, |input, _| input.set_text(&stem));
        }
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.synced_branch = stem;
        }
    }

    /// Notices a change to the branch field the dialog did not make — a
    /// keystroke, a paste from the Edit menu, the input method — and makes
    /// the branch the person's own, or, emptied, the prompt's again.
    fn note_branch_edit(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_worktree.as_ref() else {
            return;
        };
        let text = dialog.name.read(cx).text();
        if text.is_empty() {
            if dialog.branch_edited {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.branch_edited = false;
                }
                self.sync_branch_from_prompt(cx);
            }
            return;
        }
        if !dialog.branch_edited
            && text != dialog.synced_branch
            && let Some(dialog) = self.new_worktree.as_mut()
        {
            dialog.branch_edited = true;
        }
    }

    /// Closes every dropdown but `keep`, so one is open at a time.
    fn close_new_worktree_menus(dialog: &mut NewWorktree) {
        dialog.project_menu = None;
        dialog.agent_menu = None;
        dialog.model_menu = None;
        dialog.effort_menu = None;
        dialog.token_reduction_menu = None;
    }

    /// Opens or closes the project dropdown.
    fn toggle_new_worktree_project_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.project_menu.take().is_some() {
            return;
        }
        Self::close_new_worktree_menus(dialog);

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

    /// Opens or closes the configured-agent dropdown.
    fn toggle_agent_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.agent_menu.take().is_some() {
            return;
        }
        Self::close_new_worktree_menus(dialog);

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

    /// Opens or closes the model dropdown. Nothing to choose — no agent, or
    /// one ket cannot switch — and it stays shut.
    fn toggle_model_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.model_menu.take().is_some() || dialog.models.is_empty() {
            return;
        }
        Self::close_new_worktree_menus(dialog);
        let agent = dialog
            .agents
            .get(dialog.agent)
            .map_or_else(String::new, |name| agent_label(name));
        dialog.model_menu = Some(
            OpenMenu::new(
                "new-worktree-model".into(),
                None,
                form_width(),
                choice_items(
                    &dialog.models,
                    dialog.model.as_deref(),
                    &format!("What {agent} starts with"),
                ),
            )
            .selected(choice_index(&dialog.models, dialog.model.as_deref()))
            // Shifted back over the Agent column so the panel spans the form
            // rather than hanging off the dialog's right edge.
            .offset(-(column_width() + COLUMN_GAP), px(6.0)),
        );
    }

    /// Opens or closes the effort dropdown, as [`Self::toggle_model_menu`].
    fn toggle_effort_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.effort_menu.take().is_some() || dialog.efforts.is_empty() {
            return;
        }
        Self::close_new_worktree_menus(dialog);
        let agent = dialog
            .agents
            .get(dialog.agent)
            .map_or_else(String::new, |name| agent_label(name));
        dialog.effort_menu = Some(
            OpenMenu::new(
                "new-worktree-effort".into(),
                None,
                form_width(),
                choice_items(
                    &dialog.efforts,
                    dialog.effort.as_deref(),
                    &format!("What {agent} starts with"),
                ),
            )
            .selected(choice_index(&dialog.efforts, dialog.effort.as_deref()))
            .offset(-(column_width() + COLUMN_GAP) * 2.0, px(6.0)),
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
        Self::close_new_worktree_menus(dialog);

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
            .offset(px(0.0), px(6.0)),
        );
    }

    /// Lets whichever dropdown is open take a key before the dialog
    /// interprets it. Returns whether one did.
    fn new_worktree_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return false;
        };
        if let Some(chosen) = dropdown_key(&mut dialog.project_menu, event) {
            if let Some(project) = chosen {
                self.set_new_worktree_project(project);
            }
            return true;
        }
        if let Some(chosen) = dropdown_key(&mut dialog.agent_menu, event) {
            if let Some(agent) = chosen {
                Self::set_new_worktree_agent(dialog, agent);
            }
            return true;
        }
        if let Some(chosen) = dropdown_key(&mut dialog.model_menu, event) {
            if let Some(index) = chosen {
                dialog.model = index
                    .checked_sub(1)
                    .and_then(|at| dialog.models.get(at))
                    .map(|choice| choice.id.clone());
            }
            return true;
        }
        if let Some(chosen) = dropdown_key(&mut dialog.effort_menu, event) {
            if let Some(index) = chosen {
                dialog.effort = index
                    .checked_sub(1)
                    .and_then(|at| dialog.efforts.get(at))
                    .map(|choice| choice.id.clone());
            }
            return true;
        }
        if let Some(chosen) = dropdown_key(&mut dialog.token_reduction_menu, event) {
            if let Some(level) = chosen {
                dialog.token_reduction = level;
            }
            return true;
        }
        false
    }

    /// Makes the worktree, off the window's thread.
    ///
    /// With a prompt: the branch is named from it (or typed), the worktree
    /// made as a phone's work is, and the agent started on the prompt with
    /// the model and effort picked here — the dialog closes at once and a
    /// toast says how it went, as the quick prompt does. Without one: a
    /// worktree and a shell, which the dialog stays open to see refused.
    ///
    /// Creation fetches the remote first so the branch is cut from the main
    /// everyone else can see, and a fetch is a network round trip that may
    /// take seconds or time out. The dialog refuses a second submit while
    /// one runs; the bare result lands in [`Shell::finish_create_worktree`].
    pub(crate) fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_worktree.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let prompt = self.composer_text(PROMPT_EDITOR).trim().to_owned();
        let typed = dialog.name.read(cx).text();
        let edited = dialog.branch_edited;
        let branch = match submitted_branch(&prompt, &typed, edited) {
            Ok(branch) => branch,
            Err(why) => {
                self.refuse_new_worktree(why, cx);
                return;
            }
        };
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

        if prompt.is_empty() {
            // The model and effort picked are for the first session, which
            // the landing opens — carried there rather than lost.
            let launch =
                (dialog.model.is_some() || dialog.effort.is_some()).then(|| SessionLaunch {
                    model: dialog.model.clone(),
                    effort: dialog.effort.clone(),
                });
            if let Some(dialog) = self.new_worktree.as_mut() {
                dialog.error = None;
                dialog.busy = true;
            }
            // `Workspace` is opened inside the task rather than passed in,
            // as the merge does: it owns a store handle and a mutex of live
            // sessions.
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
                let _ = shell.update(cx, |shell, cx| {
                    shell.finish_create_worktree(result, launch, cx);
                });
            })
            .detach();
            cx.notify();
            return;
        }

        let Some(agent) = agent else {
            self.refuse_new_worktree("pick an agent to give the prompt to", cx);
            return;
        };
        let model = dialog.model.clone();
        let effort = dialog.effort.clone();
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.error = None;
            dialog.busy = true;
        }
        let root = project.clone();
        let named = cx.background_executor().spawn(async move {
            let workspace = Workspace::open()?;
            let exists = workspace
                .projects()?
                .into_iter()
                .find(|p| p.id == root)
                .is_some_and(|p| ket_core::git::Git::new(&p.root).branch_exists(&branch));
            branch_when_taken(branch, &prompt, exists, edited).map(|branch| (branch, prompt))
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let named = named.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Some(dialog) = shell.new_worktree.as_mut() {
                    dialog.busy = false;
                }
                match named {
                    Ok((branch, prompt)) => {
                        shell.close_new_worktree();
                        shell.toast(Tone::Info, format!("Starting {branch}…"), cx);
                        shell.start_work(
                            NewWork {
                                project,
                                branch,
                                prompt,
                                attachments: Vec::new(),
                                agent: Some(agent),
                                base: None,
                                economy: Some(token_reduction),
                                model,
                                effort,
                                origin: WorkOrigin::Dialog,
                            },
                            cx,
                        );
                    }
                    Err(e) => shell.refuse_new_worktree(e.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Keeps the dialog open with `why` under the fields and the branch
    /// selected, which is where a person fixes it — the usual next move is
    /// to replace the name, not to edit one character of it.
    fn refuse_new_worktree(&mut self, why: impl Into<SharedString>, cx: &mut Context<Self>) {
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.error = Some(why.into());
            let name = dialog.name.clone();
            name.update(cx, |input, _| {
                input.select_all();
                input.request_focus();
            });
        }
    }

    /// Puts the result of a bare [`Shell::create_worktree`] where it can be
    /// seen. `launch` is the model and effort picked for the first session,
    /// when any were.
    fn finish_create_worktree(
        &mut self,
        result: Result<(Worktree, PreparedBase), KetError>,
        launch: Option<SessionLaunch>,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.busy = false;
        }
        match result {
            Ok((created, prepared)) => {
                if let Some(notice) = prepared.notice() {
                    tracing::info!(worktree = %created.id, "{notice}");
                }
                self.close_new_worktree();
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
                // With a model or effort picked, the landing's own session
                // is held back and the agent opened with them instead.
                if let Some(selection) = self.position_of(&created.id) {
                    match (launch, created.agent.clone()) {
                        (Some(launch), Some(agent)) => {
                            self.suppress_auto_session = true;
                            self.select(selection, cx);
                            self.suppress_auto_session = false;
                            let pane = self.spaces.entry(created.id.clone()).or_default().focused;
                            self.open_agent_tab_with(pane, &agent, launch, cx);
                        }
                        _ => {
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
                }
            }
            Err(e) => self.refuse_new_worktree(e.to_string(), cx),
        }
        cx.notify();
    }

    /// Applies an Edit menu action to the prompt, while it has the keys.
    /// Returns whether it did: the branch field takes its own before the
    /// action reaches the shell (see `crate::input`), and a picker has
    /// nothing to edit.
    pub(crate) fn new_worktree_edit(
        &mut self,
        edit: NativeEdit,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dialog) = self.new_worktree.as_ref() else {
            return false;
        };
        let taken = dialog.name.read(cx).is_focused(window)
            || [
                &dialog.agent_focus,
                &dialog.model_focus,
                &dialog.effort_focus,
                &dialog.token_reduction_focus,
                &dialog.project_focus,
            ]
            .iter()
            .any(|handle| handle.is_focused(window));
        if taken {
            return false;
        }
        let chord = match edit {
            NativeEdit::Undo => "cmd-z",
            NativeEdit::Redo => "cmd-shift-z",
            NativeEdit::Cut => "cmd-x",
            NativeEdit::Copy => "cmd-c",
            NativeEdit::Paste => "cmd-v",
            NativeEdit::SelectAll => "cmd-a",
        };
        self.composer_key(
            PROMPT_EDITOR,
            &Keystroke::parse(chord).expect("valid edit chord"),
            cx,
        );
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.error = None;
        }
        self.sync_branch_from_prompt(cx);
        self.caret_wake(cx);
        cx.notify();
        true
    }

    /// Handles a key while the dialog is open. Returns whether it consumed one.
    ///
    /// Whichever control has the keyboard gets first refusal: the branch
    /// field takes text and its editing keys, the prompt takes everything
    /// but Tab and the chords, and what they decline — Escape, Tab, Enter on
    /// a picker — is what is left here. That ordering is the whole fix for
    /// the arrows editing the agent while you were typing: only the keys a
    /// field does not want ever reach a picker.
    pub(crate) fn new_worktree_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        self.new_worktree.as_ref()?;
        // Whichever menu is open is the topmost thing on screen, so it
        // answers before any field does.
        if self.new_worktree_menu_key(event) {
            return Some(Key::Taken);
        }
        let dialog = self.new_worktree.as_ref()?;
        let on_name = dialog.name.read(cx).is_focused(window);
        let name = dialog.name.clone();
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;

        if key.key == "escape" {
            self.close_new_worktree();
            return Some(Key::Taken);
        }
        if key.key == "enter" && command {
            self.create_worktree(cx);
            return Some(Key::Taken);
        }

        if on_name {
            // Text is never taken. It has to keep travelling until macOS's
            // input context sees it, or there are no dead keys and no input
            // methods — see [`crate::input`]. What it did to the field is
            // noticed on the next frame, in `new_worktree_view`.
            if is_text(key) {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = None;
                }
                return Some(Key::Text);
            }
            if name.update(cx, |input, cx| input.key(key, cx)) {
                // Editing is the answer to whatever the last refusal said.
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = None;
                }
                self.note_branch_edit(cx);
                return Some(Key::Taken);
            }
        }

        let on_agent = dialog_focused(self, window, |dialog| &dialog.agent_focus);
        let on_model = dialog_focused(self, window, |dialog| &dialog.model_focus);
        let on_effort = dialog_focused(self, window, |dialog| &dialog.effort_focus);
        let on_token_reduction =
            dialog_focused(self, window, |dialog| &dialog.token_reduction_focus);
        let on_project = dialog_focused(self, window, |dialog| &dialog.project_focus);
        // The prompt has the keys whenever no field or picker does. Not the
        // dialog's own handle: a click in the composer lands the focus on
        // the shell's handle, as the editor pane does everywhere, and that
        // is still the prompt.
        let on_prompt =
            !on_name && !on_agent && !on_model && !on_effort && !on_token_reduction && !on_project;
        match key.key.as_str() {
            "tab" => {
                if let Some(dialog) = self.new_worktree.as_ref() {
                    let prompt = dialog.focus.clone();
                    let name = dialog.name.clone();
                    let agent = dialog.agent_focus.clone();
                    let model = dialog.model_focus.clone();
                    let effort = dialog.effort_focus.clone();
                    let token_reduction = dialog.token_reduction_focus.clone();
                    let project = dialog.project_focus.clone();
                    // The project comes first on screen but last in the
                    // cycle: the dialog opens on the prompt, and Tab walks
                    // on from there and round.
                    if on_prompt {
                        // Landing on the name selects what is there, the way
                        // tabbing into a field does everywhere else.
                        name.update(cx, |input, _| input.select_all());
                        window.focus(name.read(cx).focus_handle());
                    } else if on_name {
                        window.focus(&agent);
                    } else if on_agent {
                        window.focus(&model);
                    } else if on_model {
                        window.focus(&effort);
                    } else if on_effort {
                        window.focus(&token_reduction);
                    } else if on_token_reduction && dialog.choose_project {
                        window.focus(&project);
                    } else {
                        window.focus(&prompt);
                    }
                }
            }
            // The prompt takes every other key: Enter is a new line there,
            // and the chord above is how it is sent.
            _ if on_prompt => {
                self.composer_key(PROMPT_EDITOR, key, cx);
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = None;
                }
                self.sync_branch_from_prompt(cx);
            }
            "enter" if on_project => self.toggle_new_worktree_project_menu(),
            "enter" if on_agent => self.toggle_agent_menu(),
            "enter" if on_model => self.toggle_model_menu(),
            "enter" if on_effort => self.toggle_effort_menu(),
            "enter" if on_token_reduction => self.toggle_token_reduction_menu(),
            "enter" => self.create_worktree(cx),
            // Opening first gives the shared menu ownership of subsequent
            // arrows, Enter and Escape.
            "up" | "down" | "space" if on_project => self.toggle_new_worktree_project_menu(),
            "up" | "down" | "space" if on_agent => self.toggle_agent_menu(),
            "up" | "down" | "space" if on_model => self.toggle_model_menu(),
            "up" | "down" | "space" if on_effort => self.toggle_effort_menu(),
            "up" | "down" | "space" if on_token_reduction => self.toggle_token_reduction_menu(),
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
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.new_worktree.as_mut()?;
        if dialog.wants_focus {
            dialog.wants_focus = false;
            window.focus(&dialog.focus);
        }
        // A paste from the Edit menu, or text the input method composed,
        // reaches the branch field without passing `new_worktree_key`.
        self.note_branch_edit(cx);
        let dialog = self.new_worktree.as_ref()?;
        // The prompt has the keyboard unless a field or picker has taken
        // it, and the well says so the way a focused field does.
        let writing = !dialog.name.read(cx).is_focused(window)
            && ![
                &dialog.agent_focus,
                &dialog.model_focus,
                &dialog.effort_focus,
                &dialog.token_reduction_focus,
                &dialog.project_focus,
            ]
            .iter()
            .any(|handle| handle.is_focused(window));
        self.set_composer_caret(PROMPT_EDITOR, writing);
        let prompt_text = self.composer_text(PROMPT_EDITOR);
        let editor = self.editor_pane(PROMPT_EDITOR, None, window, cx);
        let dialog = self.new_worktree.as_ref()?;
        let t = &self.theme;
        let blink = self.caret.visible;
        let chrome = self.chrome_family.clone();

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
            header("New worktree", t)
        } else {
            header_with(
                "New worktree in",
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

        // The prompt. Empty means a worktree and a shell, and the well says
        // so where the words would go.
        let empty = prompt_text.trim().is_empty();
        let mut prompt = textarea("new-worktree-prompt", editor, PROMPT_HEIGHT)
            .inset(FORM_INSET)
            .active(writing);
        if prompt_text.is_empty() {
            prompt = prompt.placeholder(
                "What should the agent do? Leave this empty for a worktree with just a shell.",
                PROMPT_TEXT,
            );
        }
        let prompt = prompt
            .render(t)
            .cursor_text()
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                if let Some(dialog) = this.new_worktree.as_ref() {
                    window.focus(&dialog.focus);
                }
                cx.notify();
            }));

        // The branch: from the prompt, said so after it, until it is typed
        // in — then the prompt's name is offered back as a reset.
        let stem = crate::phone_work::branch_stem(&prompt_text);
        let typed = dialog.name.read(cx).text();
        let origin: Option<AnyElement> = if dialog.branch_edited {
            (!stem.is_empty() && stem != typed).then(|| {
                button("new-worktree-branch-reset", stem.clone())
                    .link()
                    .small()
                    .leading(Icon::RotateCcw)
                    .render(t)
                    .font_family(chrome.clone())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.new_worktree.as_mut() {
                            dialog.branch_edited = false;
                            dialog.error = None;
                        }
                        this.sync_branch_from_prompt(cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
        } else {
            (!stem.is_empty() && stem == typed).then(|| {
                caption("from the prompt", t)
                    .font_family(chrome.clone())
                    .whitespace_nowrap()
                    .into_any_element()
            })
        };
        let mut style = Style::new(t, blink).leading(Icon::GitBranch);
        if let Some(origin) = origin {
            style = style.trailing(origin);
        }
        let name_row = text_field(
            &dialog.name,
            "new-worktree-name",
            dialog.error.is_some(),
            style,
            window,
            cx,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.notify()));

        let agent_name = dialog
            .agents
            .get(dialog.agent)
            .map(String::as_str)
            .unwrap_or("");
        let current: SharedString = if agent_name.is_empty() {
            "No agent (blank terminal)".into()
        } else {
            agent_label(agent_name).into()
        };
        // The command this project launches the chosen agent with, when it is
        // its own rather than the one in Settings: named in the trigger and
        // edged in the tint, so a worktree is never started on the wrong
        // account by someone who only read "Claude Code".
        let launches = (!agent_name.is_empty())
            .then(|| {
                self.projects
                    .iter()
                    .find(|project| project.id == dialog.project)?
                    .agent_overrides
                    .get(agent_name)
                    .map(crate::agent_override::short)
            })
            .flatten();
        let tint = crate::agent_override::hue(t);
        let agent_lit = dialog.agent_menu.is_some() || dialog.agent_focus.is_focused(window);
        let mut agent_row = select("new-worktree-agent", current).leading(mark(agent_name, t));
        if let Some(launches) = launches.clone() {
            agent_row = agent_row.trailing(
                div()
                    .flex_none()
                    .font_family(chrome.clone())
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
                        Self::set_new_worktree_agent(dialog, *agent);
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

        // Model and Effort: the agent's default until picked, and a dash
        // with nothing to pick — no agent, or one ket cannot switch.
        let model_menu = dialog.model_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, index, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.model = index
                            .checked_sub(1)
                            .and_then(|at| dialog.models.get(at))
                            .map(|choice| choice.id.clone());
                        dialog.model_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.model_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let model_select = dropdown(
            "new-worktree-model-select",
            select(
                "new-worktree-model",
                choice_label(&dialog.models, dialog.model.as_deref()),
            )
            .placeholder("—")
            .open(dialog.model_menu.is_some())
            .focused(dialog.model_focus.is_focused(window))
            .render(t),
            model_menu,
        )
        .track_focus(&dialog.model_focus)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window: &mut Window, cx| {
                if let Some(dialog) = this.new_worktree.as_ref() {
                    window.focus(&dialog.model_focus.clone());
                }
                this.toggle_model_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );
        let effort_menu = dialog.effort_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, index, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.effort = index
                            .checked_sub(1)
                            .and_then(|at| dialog.efforts.get(at))
                            .map(|choice| choice.id.clone());
                        dialog.effort_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.effort_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let effort_select = dropdown(
            "new-worktree-effort-select",
            select(
                "new-worktree-effort",
                choice_label(&dialog.efforts, dialog.effort.as_deref()),
            )
            .placeholder("—")
            .open(dialog.effort_menu.is_some())
            .focused(dialog.effort_focus.is_focused(window))
            .render(t),
            effort_menu,
        )
        .track_focus(&dialog.effort_focus)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window: &mut Window, cx| {
                if let Some(dialog) = this.new_worktree.as_ref() {
                    window.focus(&dialog.effort_focus.clone());
                }
                this.toggle_effort_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );

        // Economy, small in the footer's corner: a spending policy somebody
        // sets now and then, not a thing every worktree asks about. Off is
        // said in dim ink; a level is named, with what it changes after it.
        let level = dialog.token_reduction;
        let off = level == ket_core::worktree::default_token_reduction();
        let economy_label: SharedString = if off {
            "Economy off".into()
        } else {
            format!("Economy · {}", token_reduction_label(level)).into()
        };
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
        let economy_select = dropdown(
            "new-worktree-token-reduction-select",
            select("new-worktree-token-reduction", economy_label)
                .leading(token_reduction_ring(level, t))
                .inline()
                .open(dialog.token_reduction_menu.is_some())
                .focused(dialog.token_reduction_focus.is_focused(window))
                .render(t)
                .when(off, |el| el.text_color(paint(t.text.dim))),
            token_reduction_menu,
        )
        .track_focus(&dialog.token_reduction_focus)
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
        // What the level changes, as tags, and the share of output it is
        // assumed to trim. Nothing at Off, which changes nothing.
        let agent_for_policy = (!agent_name.is_empty()).then_some(agent_name);
        let changes = if off {
            Vec::new()
        } else {
            policy(ket_core::worktree::token_reduction(level), agent_for_policy)
        };
        let saving = (!off)
            .then(|| saving_figure(level))
            .flatten()
            .map(|text| caption(text, t).font_family(chrome.clone()));

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

        // Start with a prompt, Create without one. Disabled until there is
        // something to name the branch. The dialog still explains *why* if
        // the chord is pressed anyway — see `create_worktree` — so this
        // removes a dead click, not the message.
        let can_submit = !dialog.busy && (!empty || !typed.trim().is_empty());
        let create = button(
            "new-worktree-create",
            if empty { "Create worktree" } else { "Start" },
        )
        .primary()
        .enabled(can_submit)
        .hint("\u{2318}\u{23ce}")
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

        // Agent, Model and Effort side by side: each is one pick from a
        // short list, and a column each keeps the dialog short.
        let choices = div()
            .flex()
            .gap(COLUMN_GAP)
            .child(
                row("Agent", agent_select.into_any_element())
                    .w(column_width())
                    .flex_none(),
            )
            .child(
                row("Model", model_select.into_any_element())
                    .w(column_width())
                    .flex_none(),
            )
            .child(
                row("Effort", effort_select.into_any_element())
                    .w(column_width())
                    .flex_none(),
            );

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
                    .child(row("Prompt", prompt.into_any_element()))
                    .child(row("Branch", name_row.into_any_element()))
                    .child(choices)
                    .children(dialog.error.clone().map(|why| error(why, t)))
                    .child(
                        footer()
                            .child(economy_select)
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    .children(changes.into_iter().map(|words| tag(words, t)))
                                    .children(saving),
                            )
                            .child(cancel)
                            .child(create),
                    ),
            )
            .into_any_element(),
        )
    }
}

/// Whether the dialog's control picked by `handle` has the keyboard.
fn dialog_focused(
    shell: &Shell,
    window: &Window,
    handle: impl Fn(&NewWorktree) -> &FocusHandle,
) -> bool {
    shell
        .new_worktree
        .as_ref()
        .is_some_and(|dialog| handle(dialog).is_focused(window))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices(ids: &[(&str, &str)]) -> Vec<Choice> {
        ids.iter()
            .map(|(id, label)| Choice {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_branch_comes_from_the_prompt_until_it_is_typed() {
        assert_eq!(
            submitted_branch("Fix the login redirect after sign-in", "", false),
            Ok("fix-the-login-redirect-after".to_owned())
        );
        // Typed over: the typed name, made fit for git, prompt or no prompt.
        assert_eq!(
            submitted_branch("Fix the login redirect", "login redirect", true),
            Ok("login-redirect".to_owned())
        );
        assert_eq!(
            submitted_branch("", "spike sqlite", true),
            Ok("spike-sqlite".to_owned())
        );
    }

    #[test]
    fn a_submit_with_nothing_to_name_the_branch_says_why() {
        assert_eq!(
            submitted_branch("", "", false),
            Err("a worktree needs a prompt or a branch name")
        );
        assert_eq!(
            submitted_branch("...", "", false),
            Err("the prompt needs a word or two to name the branch from")
        );
        // Typed as nothing git would take: the same as nothing typed.
        assert_eq!(
            submitted_branch("", "?*~", true),
            Err("a worktree needs a prompt or a branch name")
        );
    }

    #[test]
    fn a_taken_name_from_the_prompt_gets_a_tag_and_a_typed_one_is_refused() {
        let free = branch_when_taken("fix-login".to_owned(), "fix login", false, false).unwrap();
        assert_eq!(free, "fix-login");

        let tagged = branch_when_taken("fix-login".to_owned(), "fix login", true, false).unwrap();
        assert!(tagged.starts_with("fix-login-"), "{tagged}");
        assert_eq!(tagged.len(), "fix-login-".len() + 4);

        let refused = branch_when_taken("fix-login".to_owned(), "fix login", true, true);
        assert!(matches!(refused, Err(KetError::Conflict(ref why)) if why.contains("fix-login")));
    }

    #[test]
    fn a_choice_menu_leads_with_the_default_and_ticks_the_current() {
        let models = choices(&[("opus", "Opus"), ("sonnet", "Sonnet")]);
        assert_eq!(choice_index(&models, None), 0);
        assert_eq!(choice_index(&models, Some("sonnet")), 2);
        assert_eq!(choice_index(&models, Some("gone")), 0);

        let rows = choice_items(&models, Some("opus"), "What Claude Code starts with");
        assert_eq!(rows.len(), 3);
        let checked: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter_map(|(at, row)| match row {
                MenuEntry::Item(item) if item.checked => Some(at),
                _ => None,
            })
            .collect();
        assert_eq!(checked, vec![1]);
    }

    #[test]
    fn a_trigger_reads_default_a_pick_or_nothing() {
        let models = choices(&[("opus", "Opus")]);
        assert_eq!(choice_label(&models, None), SharedString::from("Default"));
        assert_eq!(
            choice_label(&models, Some("opus")),
            SharedString::from("Opus")
        );
        assert_eq!(
            choice_label(&models, Some("gone")),
            SharedString::from("Default")
        );
        assert_eq!(choice_label(&[], Some("opus")), SharedString::default());
    }

    #[test]
    fn only_a_switchable_agent_offers_models_and_efforts() {
        let (models, efforts) = agent_choices("");
        assert!(models.is_empty() && efforts.is_empty());

        let (models, efforts) = agent_choices("claude");
        assert!(models.iter().any(|choice| choice.id == "fable"));
        assert_eq!(efforts.len(), 5);

        let (models, efforts) = agent_choices("no-such-agent");
        assert!(models.is_empty() && efforts.is_empty());
    }
}
