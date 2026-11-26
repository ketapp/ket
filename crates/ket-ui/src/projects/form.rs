//! The form both project dialogs are made of.
//!
//! Adding a project and changing one ask the same questions — what it is
//! called, how its badge looks, which branch new worktrees start from, which
//! agent runs and with what command — so [`ProjectForm`] holds those drafts
//! once and draws them as parts both dialogs place in the same two columns
//! (see [`super::settings`]): the badge's icon and colour in
//! [`super::badge`], the agent and its command in [`super::agent`], the
//! GitHub account in [`super::github`]. What differs — the folder line, the
//! project's path, the backlog — stays in [`super::add`] and
//! [`super::settings`].
//!
//! The form holds *drafts* while they are being typed and hands them to core
//! in one write, so a colour that will not parse or a base that is not a
//! branch comes back as an error line rather than as half-applied settings.

use std::path::Path;
use std::sync::Arc;

use gpui::{
    App, Context, Div, Entity, KeyDownEvent, MouseButton, Pixels, SharedString, Stateful, Window,
    div, prelude::*, px,
};
use ket_core::config::AgentLaunch;
use ket_core::id::ProjectId;
use ket_core::project::ProjectSettings;
use ket_core::theme::Color;
use ket_core::workspace::Workspace;

use super::agent::agent_drafts;
use super::badge::matching_glyphs;
use super::settings::{CONTROL_W, Tip};
use super::{PROJECT_COLORS, default_color};
use crate::Shell;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::ui::dialog::card;
use crate::ui::glyph::{self, Glyph};
use crate::ui::icon::Icon;
use crate::ui::menu::OpenMenu;

/// Which text field in a project form has the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// The display name.
    Name,
    /// The icon as typed, in the icon panel.
    Icon,
    /// What the icon panel's glyphs are filtered by.
    IconFilter,
    /// The badge colour as `#rrggbb`.
    Hex,
    /// The default base branch.
    Base,
    /// One agent's command for this project, by its row in
    /// [`ProjectForm::commands`].
    Command(usize),
}

/// What a project form opens holding.
pub(super) struct Seed<'a> {
    /// The folder's name. Empty while a new project has no folder yet.
    pub(super) dir_name: &'a str,
    /// The project's settings, or the defaults for one being added.
    pub(super) settings: &'a ProjectSettings,
    /// The default base branch. Empty while there is no repository to ask.
    pub(super) base: &'a str,
    /// The preferred agent. `None` means whichever ket defaults to.
    pub(super) agent: Option<String>,
    /// Whether repository-owned automation may run.
    pub(super) trusted: bool,
    /// Whether the backlog is kept in the repository. `None` for a project
    /// being added, which starts with a private one and asks nothing.
    pub(super) backlog_in_repo: Option<bool>,
    /// Whether the backlog follows the repository's GitHub issues. `None`
    /// for a project being added, which asks nothing about it either.
    pub(super) github_issues: Option<bool>,
    /// The GitHub account picked, `None` inside for the default. `None`
    /// for a project being added.
    pub(super) github_account: Option<Option<String>>,
}

/// The badge picture a project form holds until it is confirmed.
///
/// A file picked is only a path until Save, which is when core copies it in
/// ([`Workspace::import_project_image`]): Cancel leaves nothing behind, and a
/// file refused is refused at the pick, by [`ket_core::project_image::check`],
/// so it never becomes the draft at all.
#[derive(Clone)]
pub(super) enum ImageDraft {
    /// Whatever the project has now, if anything.
    Keep,
    /// A file picked and checked, not copied in yet.
    Picked(Arc<Path>),
    /// Taken off: the badge goes back to its icon.
    Cleared,
}

/// Drafts of every field a project dialog edits, until it is confirmed.
pub(crate) struct ProjectForm {
    /// The directory's name: what the project is called with no display name,
    /// and what the badge's initial comes from. Empty while a new project has
    /// no folder yet.
    pub(super) dir_name: SharedString,
    /// Draft display name. Empty means "use the directory name".
    ///
    /// Entities because a text field is one — see [`crate::input`] for why the
    /// input method needs them to be.
    pub(super) name: Entity<TextInput>,
    /// Draft icon as typed, stored verbatim by core. Empty means none — or
    /// the glyph, when one is picked.
    pub(super) icon: Entity<TextInput>,
    /// What the icon panel's glyphs are narrowed to, by name. Emptied each
    /// time the panel opens: it is a way to find a glyph, not a draft.
    pub(super) icon_filter: Entity<TextInput>,
    /// Draft glyph, picked from the icon panel's grid ([`crate::ui::glyph`]).
    ///
    /// Beside the typed icon rather than written into it: the field would
    /// otherwise show `glyph:leaf`, which is the store's spelling, not
    /// something to read. Text typed in the field wins — typing a letter is
    /// the newer choice — and picking a glyph empties the field.
    pub(super) glyph: Option<Glyph>,
    /// Draft badge colour, as typed or as a swatch wrote it. Empty means the
    /// neutral default, which is its placeholder.
    ///
    /// The field rather than a separate `Option<String>`: a swatch writes its
    /// hex here, so there is one draft and the field always shows it.
    pub(super) hex: Entity<TextInput>,
    /// Draft badge picture. A new project's starts as [`ImageDraft::Keep`],
    /// which for it is none.
    pub(super) image: ImageDraft,
    /// Where the picture the project has is kept, for drawing
    /// [`ImageDraft::Keep`]. `None` with no picture, or none on disk.
    pub(super) kept_image: Option<Arc<Path>>,
    /// Whether the icon panel is open.
    pub(super) palette_open: bool,
    /// Draft default base branch.
    pub(super) base: Entity<TextInput>,
    /// Draft preferred agent. `None` means whichever ket defaults to.
    pub(super) agent: Option<String>,
    /// The agent dropdown, while it is open. Row 0 is "Settings default";
    /// row `n` is `agents[n - 1]`, then `local_agents` after them.
    pub(super) agent_menu: Option<OpenMenu<usize>>,
    /// The agents the config knows, for the picker.
    pub(super) agents: Vec<String>,
    /// The local agents this Mac can run now — see `crate::local_models`.
    /// Picked like any other, but with no command of the project's own:
    /// ket builds a local agent's launch itself.
    pub(super) local_agents: Vec<String>,
    /// Each agent's command line in Settings, in the order of `agents`: what
    /// a blank command runs, and what a set one replaces.
    pub(super) configured: Vec<SharedString>,
    /// Draft command for each agent in this project, in the order of
    /// `agents`. Empty means the one in Settings, which is the placeholder.
    pub(super) commands: Vec<Entity<TextInput>>,
    /// Draft of the discovered-worktrees switch.
    pub(super) hide_discovered: bool,
    /// Whether repository-owned hooks and post-provision commands may run.
    pub(super) trust_automation: bool,
    /// Where the backlog is to be kept: in the repository or privately.
    /// `None` while the form asks nothing about it — see [`Seed`].
    pub(super) backlog_in_repo: Option<bool>,
    /// Draft of the GitHub issues switch. `None` while the form asks nothing
    /// about it.
    pub(super) github_issues: Option<bool>,
    /// Draft of the GitHub account: `None` inside for the default, and
    /// `None` while the form asks nothing about it.
    pub(super) github_account: Option<Option<String>>,
    /// The account dropdown, while it is open.
    pub(super) github_menu: Option<OpenMenu<usize>>,
    /// The accounts that dropdown lists, in its order after "Default".
    pub(super) github_logins: Vec<String>,
    /// Why the last confirm was refused, if it was.
    pub(super) error: Option<SharedString>,
    /// The ⓘ, or the image's remove button, under the pointer: whose
    /// tooltip is up.
    pub(super) tip: Option<Tip>,
}

impl ProjectForm {
    /// A form holding `seed`, with the icon panel open when `focus` is in it.
    pub(super) fn new(workspace: &Workspace, seed: Seed<'_>, focus: Field, cx: &mut App) -> Self {
        let (agents, configured, commands) = agent_drafts(workspace, seed.settings, cx);
        let local_agents =
            ket_core::agents::Catalogue::detect(workspace.config(), &ket_core::agents::ShellProber)
                .runnable()
                .filter(|entry| ket_core::local_models::model_tag(&entry.name).is_some())
                .map(|entry| entry.name.clone())
                .collect();
        // The default colour is the placeholder rather than the text, so a
        // project nobody coloured keeps no colour until someone picks one.
        let hex = seed
            .settings
            .color
            .clone()
            .filter(|hex| !hex.eq_ignore_ascii_case(PROJECT_COLORS[0].1))
            .unwrap_or_default();
        let named = match seed.dir_name {
            "" => "The folder's name",
            name => name,
        };
        let stored_icon = seed.settings.icon.as_deref().unwrap_or_default();
        let glyph = glyph::named(stored_icon);
        Self {
            dir_name: seed.dir_name.to_owned().into(),
            name: TextInput::with_text(
                named.to_owned(),
                seed.settings.display_name.as_deref().unwrap_or_default(),
                cx,
            ),
            icon: TextInput::with_text(
                "Or type a letter or an emoji",
                if glyph.is_some() { "" } else { stored_icon },
                cx,
            ),
            icon_filter: TextInput::new("Filter icons", cx),
            glyph,
            hex: TextInput::with_text(PROJECT_COLORS[0].1, &hex, cx),
            image: ImageDraft::Keep,
            kept_image: None,
            palette_open: in_panel(focus),
            base: TextInput::with_text("main", seed.base, cx),
            agent: seed.agent,
            agent_menu: None,
            agents,
            local_agents,
            configured,
            commands,
            hide_discovered: seed.settings.hide_discovered_worktrees,
            trust_automation: seed.trusted,
            backlog_in_repo: seed.backlog_in_repo,
            github_issues: seed.github_issues,
            github_account: seed.github_account,
            github_menu: None,
            github_logins: Vec::new(),
            error: None,
            tip: None,
        }
    }

    /// Which field has the keyboard, if any, and the field itself.
    ///
    /// Read back off the handles rather than remembered, so a field that was
    /// blurred by a press elsewhere is not still believed to be focused.
    fn focused(&self, window: &Window, cx: &App) -> Option<(Field, Entity<TextInput>)> {
        self.order().into_iter().find_map(|field| {
            let input = self.input(field);
            input.read(cx).is_focused(window).then_some((field, input))
        })
    }

    fn input(&self, field: Field) -> Entity<TextInput> {
        match field {
            Field::Name => self.name.clone(),
            Field::Icon => self.icon.clone(),
            Field::IconFilter => self.icon_filter.clone(),
            Field::Hex => self.hex.clone(),
            Field::Base => self.base.clone(),
            Field::Command(row) => self.commands[row].clone(),
        }
    }

    /// Every field the form has, shown or not.
    fn inputs(&self) -> Vec<Entity<TextInput>> {
        [
            &self.name,
            &self.icon,
            &self.icon_filter,
            &self.hex,
            &self.base,
        ]
        .into_iter()
        .chain(&self.commands)
        .cloned()
        .collect()
    }

    /// `field` when it is on show, else the field that stands in for it: the
    /// command on show for any agent's command, and the name for anything
    /// else that is not drawn. The keyboard is never handed to a field the
    /// dialog does not show.
    pub(super) fn visible(&self, field: Field) -> Field {
        match field {
            Field::Command(_) => self.command_row().map_or(Field::Name, Field::Command),
            other if self.order().contains(&other) => other,
            _ => Field::Name,
        }
    }

    /// The fields that go when the icon panel closes.
    fn panel_inputs(&self) -> [&Entity<TextInput>; 2] {
        [&self.icon_filter, &self.icon]
    }

    /// Opens the icon panel, and closes either menu: one of them open at a
    /// time. The icon panel opens with its filter
    /// empty and holding the keyboard, so typing narrows the glyphs at once.
    pub(super) fn open_panel(&mut self, cx: &mut App) {
        self.palette_open = true;
        self.agent_menu = None;
        self.github_menu = None;
        self.icon_filter.update(cx, |input, _| {
            input.clear();
            input.request_focus();
        });
    }

    /// Closes the icon panel, if it is open.
    pub(super) fn close_panel(&mut self, window: &mut Window, cx: &App) {
        if !self.palette_open {
            return;
        }
        self.palette_open = false;
        // Keys typed next must not go to a field that is no longer drawn. A
        // press on another field has already taken the keyboard by now.
        let hidden = self
            .panel_inputs()
            .into_iter()
            .any(|input| input.read(cx).is_focused(window));
        if hidden {
            window.focus(self.name.read(cx).focus_handle());
        }
    }

    /// Takes a glyph from the grid, `None` being the letters cell. Picking
    /// one empties the typed icon, which would otherwise outrank it. The icon
    /// panel is a dropdown, so it closes on the pick.
    pub(super) fn pick_glyph(&mut self, glyph: Option<Glyph>, window: &mut Window, cx: &mut App) {
        self.glyph = glyph;
        self.icon.update(cx, |input, _| input.set_text(""));
        self.error = None;
        self.close_panel(window, cx);
    }

    /// How wide the agent and account menus open: as wide as the trigger
    /// they hang from.
    pub(super) fn menu_width(&self) -> Pixels {
        CONTROL_W
    }

    /// The row in `agents` whose command is on show: the chosen agent's, or
    /// the first configured one's when the project takes the default — which
    /// is the agent "default" launches (see `Workspace::create_worktree`).
    pub(super) fn command_row(&self) -> Option<usize> {
        match &self.agent {
            Some(name) => self.agents.iter().position(|agent| agent == name),
            None => (!self.agents.is_empty()).then_some(0),
        }
    }

    /// The badge colour the draft holds: the swatch or hex in the field, or
    /// the neutral default when it is empty. `None` when what is typed is not
    /// a colour yet, which the field shows as a refusal.
    pub(super) fn colour(&self, cx: &App) -> Option<Color> {
        let text = self.hex.read(cx).text();
        let text = text.trim();
        if text.is_empty() {
            return Some(default_color());
        }
        Color::from_hex(text).ok()
    }

    /// The picture the badge wears as drafted: the one picked, else the one
    /// the project has, unless it was taken off.
    pub(super) fn image_shown(&self) -> Option<Arc<Path>> {
        match &self.image {
            ImageDraft::Keep => self.kept_image.clone(),
            ImageDraft::Picked(path) => Some(path.clone()),
            ImageDraft::Cleared => None,
        }
    }

    /// The icon as core will store it: what is typed, else the glyph's
    /// stored form, else nothing.
    fn icon_draft(&self, cx: &App) -> String {
        let typed = self.icon.read(cx).text();
        if !typed.trim().is_empty() {
            return typed;
        }
        self.glyph.map(Glyph::stored).unwrap_or_default()
    }

    /// The text fields Tab walks, in reading order: the icon panel's only
    /// while it is open, and only the command that is on show.
    fn order(&self) -> Vec<Field> {
        let mut order = vec![Field::Name];
        if self.palette_open {
            order.extend([Field::IconFilter, Field::Icon]);
        }
        order.push(Field::Hex);
        order.push(Field::Base);
        order.extend(self.command_row().map(Field::Command));
        order
    }

    /// The field after `field`, or before it when `back`, wrapping.
    fn step(&self, field: Field, back: bool) -> Field {
        let order = self.order();
        let at = order.iter().position(|f| *f == field).unwrap_or(0);
        let next = if back { at + order.len() - 1 } else { at + 1 };
        order[next % order.len()]
    }

    /// The drafts as core takes them.
    ///
    /// A colour that does not parse is refused here rather than by core so
    /// the reason can name the field: core's message is about a config value.
    /// The box is on show, unless the icon panel is hanging over it: the
    /// panel goes, and the box takes the keyboard.
    pub(super) fn drafts(&mut self, cx: &mut App) -> Option<Drafts> {
        let Some(colour) = self.colour(cx) else {
            let typed = self.hex.read(cx).text();
            self.palette_open = false;
            self.hex.update(cx, |input, _| input.request_focus());
            self.error = Some(format!("{} is not a color — use #rrggbb", typed.trim()).into());
            return None;
        };
        Some(Drafts {
            display_name: self.name.read(cx).text(),
            color: (colour != default_color()).then(|| colour.to_hex()),
            icon: self.icon_draft(cx),
            image: self.image.clone(),
            hide_discovered: self.hide_discovered,
            trust_automation: self.trust_automation,
            github_issues: self.github_issues,
            github_account: self.github_account.clone(),
            base: self.base.read(cx).text(),
            agent: self.agent.clone(),
            // The whole line goes in as the command: it is shell text, typed
            // at a prompt as it stands, so splitting it into words here would
            // only be a chance to quote it differently from how it was written.
            commands: self
                .agents
                .iter()
                .zip(&self.commands)
                .map(|(name, input)| (name.clone(), input.read(cx).text()))
                .collect(),
        })
    }
}

/// What a project form hands core, read off its fields.
pub(super) struct Drafts {
    display_name: String,
    color: Option<String>,
    icon: String,
    image: ImageDraft,
    hide_discovered: bool,
    trust_automation: bool,
    /// `None` when the form did not ask, which keeps what is stored.
    github_issues: Option<bool>,
    /// The same.
    github_account: Option<Option<String>>,
    base: String,
    agent: Option<String>,
    /// Each configured agent's command line, as typed.
    commands: Vec<(String, String)>,
}

impl Drafts {
    /// Writes every draft to the project `id`.
    ///
    /// Presentation is written first because it can only fail on the store
    /// itself; the base and agent are checked against the repository and the
    /// config, and a refusal there comes back as the error. A picture picked
    /// is copied in before any of it, so a file that has gone since it was
    /// picked is refused with nothing written.
    pub(super) fn write(self, workspace: &Workspace, id: &ProjectId) -> ket_core::Result<()> {
        // Carried through rather than rebuilt. The form does not edit
        // `build_dirs` — it is set in the config file — and a save that
        // quietly reset it would undo a per-project list the moment anyone
        // renamed the project.
        let existing = workspace.project_settings(id).unwrap_or_default();
        let build_dirs = existing.build_dirs;
        let image = match self.image {
            ImageDraft::Keep => existing.image,
            ImageDraft::Picked(path) => Some(workspace.import_project_image(id, &path)?),
            ImageDraft::Cleared => None,
        };

        // An override for an agent the config no longer lists is kept, not
        // dropped: it has no row here to be cleared from, and the agent coming
        // back should find the project the way it was left.
        let mut agents = existing.agents;
        for (name, line) in self.commands {
            // The configured line typed out again is no override, and storing
            // it as one would mark the project for nothing.
            let configured = workspace
                .config()
                .agent(&name)
                .map(|spec| spec.launch_line(&[]));
            if configured.as_deref() == Some(line.trim()) {
                agents.remove(&name);
                continue;
            }
            // Blank is cleared by `normalised`, on the way into the store.
            agents.insert(
                name,
                AgentLaunch {
                    command: line,
                    args: Vec::new(),
                },
            );
        }

        let settings = ProjectSettings {
            display_name: Some(self.display_name),
            color: self.color,
            icon: Some(self.icon),
            hide_discovered_worktrees: self.hide_discovered,
            build_dirs,
            agents,
            backlog_in_repo: existing.backlog_in_repo,
            github_issues: self.github_issues.unwrap_or(existing.github_issues),
            github_account: self.github_account.unwrap_or(existing.github_account),
            pinned: existing.pinned,
            hidden: existing.hidden,
            image,
        };
        workspace.set_project_settings(id, settings)?;
        workspace.set_default_base(id, &self.base)?;
        workspace.set_preferred_agent(id, self.agent.as_deref())?;
        workspace.set_automation_trusted(id, self.trust_automation)
    }
}

/// Whether `field` is drawn in the icon panel, so a form asked to open
/// there opens it.
fn in_panel(field: Field) -> bool {
    matches!(field, Field::Icon | Field::IconFilter)
}

/// What a key meant to a project form, past what the form did with it.
pub(super) enum FormKey {
    /// Taken by a field, the agent menu or the form itself.
    Handled,
    /// Escape, with nothing left in the form to back out of.
    Close,
    /// Return: the dialog's confirm.
    Submit,
}

impl Shell {
    /// The form of whichever project dialog is open. Only one ever is: they
    /// are modals — see `Shell::modal_open`.
    pub(super) fn project_form(&self) -> Option<&ProjectForm> {
        self.settings
            .as_ref()
            .map(|dialog| &dialog.form)
            .or_else(|| self.adding_project.as_ref().map(|dialog| &dialog.form))
    }

    /// [`Shell::project_form`], to change.
    pub(super) fn project_form_mut(&mut self) -> Option<&mut ProjectForm> {
        match (&mut self.settings, &mut self.adding_project) {
            (Some(dialog), _) => Some(&mut dialog.form),
            (None, Some(dialog)) => Some(&mut dialog.form),
            (None, None) => None,
        }
    }

    /// Gives a form that is about to go up the keyboard at `focus`, and
    /// redraws it as its fields are typed in.
    pub(super) fn watch_form(&mut self, form: &ProjectForm, focus: Field, cx: &mut Context<Self>) {
        // Asked for as it is built and granted on the first frame — see
        // `TextInput::request_focus`. Asked once rather than asserted every
        // frame, which is what made the old field impossible to leave.
        form.input(focus)
            .update(cx, |input, _| input.request_focus());
        for input in form.inputs() {
            self.watch_field(&input, cx);
        }
        self.popup = None;
    }

    // ---- keys -----------------------------------------------------------------

    /// Handles a keystroke in the open project form.
    ///
    /// The agent menu first, then the focused field, which owns the caret,
    /// the selection and every editing chord; what both decline is the
    /// form's. Escape and Return come back for the dialog to act on.
    pub(super) fn project_form_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FormKey {
        if self.form_agent_menu_key(event) || self.form_github_menu_key(event, cx) {
            return FormKey::Handled;
        }

        let input = self
            .project_form()
            .and_then(|form| form.focused(window, cx))
            .map(|(_, input)| input);
        if let Some(input) = input {
            // Text is never taken here. It has to keep travelling until
            // macOS's input context sees it — see [`crate::input`].
            let typed = is_text(&event.keystroke)
                || input.update(cx, |input, cx| input.key(&event.keystroke, cx));
            if typed {
                if let Some(form) = self.project_form_mut() {
                    form.error = None;
                }
                return FormKey::Handled;
            }
        }

        let Some(form) = self.project_form_mut() else {
            return FormKey::Handled;
        };
        match event.keystroke.key.as_str() {
            // The icon panel first, then the dialog: Escape backs out one layer
            // at a time.
            "escape" if form.palette_open => {
                form.palette_open = false;
                window.focus(form.name.read(cx).focus_handle());
                FormKey::Handled
            }
            "escape" => FormKey::Close,
            // In the icon panel's filter with something typed, Return takes
            // the first glyph left, as a menu's filter takes its first row.
            // Anywhere else, and with nothing to take, it is the dialog's.
            "enter" => {
                let filtering = form.palette_open && form.icon_filter.read(cx).is_focused(window);
                let query = form.icon_filter.read(cx).text();
                let first = filtering
                    .then(|| query.trim())
                    .filter(|query| !query.is_empty())
                    .and_then(|query| matching_glyphs(query).next());
                match first {
                    Some(glyph) => {
                        form.pick_glyph(Some(glyph), window, cx);
                        FormKey::Handled
                    }
                    None => FormKey::Submit,
                }
            }
            "tab" => {
                // Tab from nowhere lands on the first field.
                let next = match form.focused(window, cx) {
                    Some((field, _)) => form.step(field, event.keystroke.modifiers.shift),
                    None => Field::Name,
                };
                // The icon panel hangs over the colour, hex field and all, so
                // Tab leaving it puts it away rather than typing under it.
                if !in_panel(next) {
                    form.palette_open = false;
                }
                // Tabbing into a field selects it, so the next keystroke
                // replaces the value rather than extending it. That is what
                // every other text field does.
                let input = form.input(next);
                input.update(cx, |input, _| input.select_all());
                window.focus(input.read(cx).focus_handle());
                FormKey::Handled
            }
            // Swallowed, like every other key a modal is given: a chord typed
            // over a dialog must not also reach the shell. Keymap actions are
            // dispatched before any `on_key_down`, so this does not take Cmd-Q
            // away.
            _ => FormKey::Handled,
        }
    }

    // ---- views ----------------------------------------------------------------

    /// The dialog card a project form sits in, `width` wide.
    ///
    /// A press anywhere in it but an open panel and its trigger puts the
    /// panel away — see [`Shell::put_form_panels_away`].
    pub(super) fn form_card(
        &self,
        id: &'static str,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        card(id, width, &self.theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                if this.put_form_panels_away(window, cx) {
                    cx.notify();
                }
            }),
        )
    }

    /// Puts away what the open form has open, for a press that landed outside
    /// it. Returns whether anything closed.
    ///
    /// That is the icon panel and both menus.
    pub(super) fn put_form_panels_away(&mut self, window: &mut Window, cx: &App) -> bool {
        let Some(form) = self.project_form_mut() else {
            return false;
        };
        let open = form.palette_open;
        form.close_panel(window, cx);
        let agent = form.agent_menu.take().is_some();
        let account = form.github_menu.take().is_some();
        open || agent || account
    }

    /// One of the form's text fields. A press on it selects what is in it.
    pub(super) fn form_field(
        &self,
        form: &ProjectForm,
        which: Field,
        invalid: bool,
        leading: Option<Icon>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut style = Style::new(&self.theme, self.caret.visible);
        if let Some(which) = leading {
            style = style.leading(which);
        }
        let id = match which {
            Field::Name => "field-name".into(),
            Field::Icon => "field-icon".into(),
            Field::IconFilter => "field-icon-filter".into(),
            Field::Hex => "field-hex".into(),
            Field::Base => "field-base".into(),
            Field::Command(row) => gpui::ElementId::from(("field-command", row)),
        };
        text_field(&form.input(which), id, invalid, style, window, cx).on_click(cx.listener(
            move |this, _, _, cx| {
                if let Some(form) = this.project_form() {
                    form.input(which).update(cx, |input, _| input.select_all());
                }
                cx.notify();
            },
        ))
    }
}

/// One line of notes: captions and marks in a row.
pub(super) fn line() -> Div {
    div().flex().items_center().gap(px(5.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_panel_names_the_fields_it_holds() {
        assert!(!in_panel(Field::Hex));
        assert!(in_panel(Field::IconFilter));
        assert!(in_panel(Field::Icon));
        assert!(!in_panel(Field::Name));
    }
}
