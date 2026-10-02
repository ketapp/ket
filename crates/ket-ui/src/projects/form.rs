//! The form both project dialogs are made of.
//!
//! Adding a project and changing one ask the same questions — what it is
//! called, how its badge looks, which branch new worktrees start from, which
//! agent runs and with what command — so [`ProjectForm`] holds those drafts
//! once and draws them as parts each dialog places: the card, the identity
//! line and the labelled rows under it. What differs — the folder line, the
//! project's path, Remove — stays in [`super::add`] and [`super::settings`].
//!
//! The form holds *drafts* while they are being typed and hands them to core
//! in one write, so a colour that will not parse or a base that is not a
//! branch comes back as an error line rather than as half-applied settings.

use gpui::{
    AnyElement, App, Context, Div, Entity, KeyDownEvent, MouseButton, Pixels, Rgba, SharedString,
    Stateful, Window, div, prelude::*, px,
};
use ket_core::config::AgentLaunch;
use ket_core::id::ProjectId;
use ket_core::project::ProjectSettings;
use ket_core::theme::{Color, Theme};
use ket_core::workspace::Workspace;

use super::{PROJECT_COLORS, default_color};
use crate::Shell;
use crate::fonts::Prose;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::{alpha, paint};
use crate::ui::agent::{agent_label, mark, provider};
use crate::ui::button::icon_button;
use crate::ui::chip::{Motion, badge, caption, status_dot, swatch, tag};
use crate::ui::dialog::card;
use crate::ui::field::{error as field_error, label};
use crate::ui::group::{button_group, segment};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::popup::{Placement, anchored};
use crate::ui::select::select;
use crate::ui::toggle::checkbox;
use crate::ui::{CARD_PAD, FIELD_H};

/// Both dialogs' width.
pub(super) const CARD_WIDTH: Pixels = px(560.0);

/// The width of the label column beside each control.
const LABEL_WIDTH: Pixels = px(96.0);

/// The gap between the label column and its control.
const LABEL_GAP: Pixels = px(12.0);

/// How wide a control in the label column's rows is: the card's inside, less
/// the labels. The agent menu opens at this width, so it lines up with the
/// trigger it hangs from.
fn control_width() -> Pixels {
    CARD_WIDTH - CARD_PAD * 2.0 - LABEL_WIDTH - LABEL_GAP
}

/// The badge button beside the name: the badge, then the select's chevron.
const BADGE_TRIGGER_W: Pixels = px(74.0);

/// How wide the badge popover is: two rows of nine swatches and the gaps
/// between them, inside the popover's padding and border.
const PALETTE_WIDTH: Pixels = px(310.0);

/// The popover's padding.
const PALETTE_PAD: Pixels = px(12.0);

/// The gap between swatches.
const SWATCH_GAP: Pixels = px(8.0);

/// The hex field's width: `#rrggbb` in the mono, and the field's padding.
const HEX_W: Pixels = px(96.0);

/// Which text field in a project form has the keyboard.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    /// The display name.
    Name,
    /// The icon, in the badge popover.
    Icon,
    /// The badge colour as `#rrggbb`, in the badge popover.
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
    /// Draft icon, stored verbatim by core. Empty means none.
    pub(super) icon: Entity<TextInput>,
    /// Draft badge colour, as typed or as a swatch wrote it. Empty means the
    /// neutral default, which is its placeholder.
    ///
    /// The field rather than a separate `Option<String>`: a swatch writes its
    /// hex here, so there is one draft and the field always shows it.
    pub(super) hex: Entity<TextInput>,
    /// Whether the badge popover — icon and colour — is open.
    pub(super) palette_open: bool,
    /// Draft default base branch.
    pub(super) base: Entity<TextInput>,
    /// Draft preferred agent. `None` means whichever ket defaults to.
    pub(super) agent: Option<String>,
    /// The agent dropdown, while it is open. Row 0 is "Settings default";
    /// row `n` is `agents[n - 1]`.
    pub(super) agent_menu: Option<OpenMenu<usize>>,
    /// The agents the config knows, for the picker.
    pub(super) agents: Vec<String>,
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
    /// Why the last confirm was refused, if it was.
    pub(super) error: Option<SharedString>,
}

impl ProjectForm {
    /// A form holding `seed`, with the popover open when `focus` is in it.
    pub(super) fn new(workspace: &Workspace, seed: Seed<'_>, focus: Field, cx: &mut App) -> Self {
        let (agents, configured, commands) = agent_drafts(workspace, seed.settings, cx);
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
        Self {
            dir_name: seed.dir_name.to_owned().into(),
            name: TextInput::with_text(
                named.to_owned(),
                seed.settings.display_name.as_deref().unwrap_or_default(),
                cx,
            ),
            icon: TextInput::with_text(
                "An emoji or a letter",
                seed.settings.icon.as_deref().unwrap_or_default(),
                cx,
            ),
            hex: TextInput::with_text(PROJECT_COLORS[0].1, &hex, cx),
            palette_open: focus == Field::Icon || focus == Field::Hex,
            base: TextInput::with_text("main", seed.base, cx),
            agent: seed.agent,
            agent_menu: None,
            agents,
            configured,
            commands,
            hide_discovered: seed.settings.hide_discovered_worktrees,
            trust_automation: seed.trusted,
            backlog_in_repo: seed.backlog_in_repo,
            error: None,
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
            Field::Hex => self.hex.clone(),
            Field::Base => self.base.clone(),
            Field::Command(row) => self.commands[row].clone(),
        }
    }

    /// Every field the form has, shown or not.
    fn inputs(&self) -> Vec<Entity<TextInput>> {
        [&self.name, &self.icon, &self.hex, &self.base]
            .into_iter()
            .chain(&self.commands)
            .cloned()
            .collect()
    }

    /// The row in `agents` whose command is on show: the chosen agent's, or
    /// the first configured one's when the project takes the default — which
    /// is the agent "default" launches (see `Workspace::create_worktree`).
    fn command_row(&self) -> Option<usize> {
        match &self.agent {
            Some(name) => self.agents.iter().position(|agent| agent == name),
            None => (!self.agents.is_empty()).then_some(0),
        }
    }

    /// The badge colour the draft holds: the swatch or hex in the field, or
    /// the neutral default when it is empty. `None` when what is typed is not
    /// a colour yet, which the field shows as a refusal.
    fn colour(&self, cx: &App) -> Option<Color> {
        let text = self.hex.read(cx).text();
        let text = text.trim();
        if text.is_empty() {
            return Some(default_color());
        }
        Color::from_hex(text).ok()
    }

    /// The text field rows Tab walks, in order. The popover's two only while
    /// it is open, and only the command that is on show.
    fn order(&self) -> Vec<Field> {
        let mut order = vec![Field::Name];
        if self.palette_open {
            order.extend([Field::Icon, Field::Hex]);
        }
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
    /// the reason can name the field: core's message is about a config value,
    /// and this is a box in a popover that may not even be open — so the
    /// popover is opened to show it.
    pub(super) fn drafts(&mut self, cx: &App) -> Option<Drafts> {
        let Some(colour) = self.colour(cx) else {
            let typed = self.hex.read(cx).text();
            self.palette_open = true;
            self.error = Some(format!("{} is not a colour — use #rrggbb", typed.trim()).into());
            return None;
        };
        Some(Drafts {
            display_name: self.name.read(cx).text(),
            color: (colour != default_color()).then(|| colour.to_hex()),
            icon: self.icon.read(cx).text(),
            hide_discovered: self.hide_discovered,
            trust_automation: self.trust_automation,
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
    hide_discovered: bool,
    trust_automation: bool,
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
    /// config, and a refusal there comes back as the error.
    pub(super) fn write(self, workspace: &Workspace, id: &ProjectId) -> ket_core::Result<()> {
        // Carried through rather than rebuilt. The form does not edit
        // `build_dirs` — it is set in the config file — and a save that
        // quietly reset it would undo a per-project list the moment anyone
        // renamed the project.
        let existing = workspace.project_settings(id).unwrap_or_default();
        let build_dirs = existing.build_dirs;

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
        };
        workspace.set_project_settings(id, settings)?;
        workspace.set_default_base(id, &self.base)?;
        workspace.set_preferred_agent(id, self.agent.as_deref())?;
        workspace.set_automation_trusted(id, self.trust_automation)
    }
}

/// The form's agent rows for a project with `settings`: each configured
/// agent's name, its command line in Settings, and a field holding the
/// project's own command for it.
fn agent_drafts(
    workspace: &Workspace,
    settings: &ProjectSettings,
    cx: &mut App,
) -> (Vec<String>, Vec<SharedString>, Vec<Entity<TextInput>>) {
    let config = workspace.config();
    let agents = config
        .agents
        .iter()
        .map(|agent| agent.name.clone())
        .collect();
    let configured: Vec<SharedString> = config
        .agents
        .iter()
        .map(|agent| agent.launch_line(&[]).into())
        .collect();
    // The configured line is the placeholder, so an empty field reads as what
    // the project will run rather than as a blank.
    let commands = config
        .agents
        .iter()
        .zip(&configured)
        .map(|(agent, line)| {
            let own = settings
                .agents
                .get(&agent.name)
                .map(|launch| ket_core::shell::line(&launch.command, &launch.args))
                .unwrap_or_default();
            TextInput::with_text(line.clone(), &own, cx)
        })
        .collect();
    (agents, configured, commands)
}

/// A label beside its control, centred on a field's height whatever the
/// control is, so a checkbox row and a field row line up the same.
fn labelled(caption: &'static str, control: impl IntoElement, t: &Theme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(LABEL_GAP)
        .child(
            div()
                .w(LABEL_WIDTH)
                .h(FIELD_H)
                .flex_none()
                .flex()
                .items_center()
                .child(label(caption, t).mb(px(0.0))),
        )
        .child(control)
        .into_any_element()
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
        if self.form_agent_menu_key(event) {
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
            // The popover first, then the dialog: Escape backs out one layer
            // at a time.
            "escape" if form.palette_open => {
                form.palette_open = false;
                window.focus(form.name.read(cx).focus_handle());
                FormKey::Handled
            }
            "escape" => FormKey::Close,
            "enter" => FormKey::Submit,
            "tab" => {
                // Tab from nowhere lands on the first field.
                let next = match form.focused(window, cx) {
                    Some((field, _)) => form.step(field, event.keystroke.modifiers.shift),
                    None => Field::Name,
                };
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

    // ---- the agent menu -------------------------------------------------------

    /// Opens or closes the form's agent dropdown.
    fn toggle_form_agent_menu(&mut self, cx: &App) {
        let Some(form) = self.project_form_mut() else {
            return;
        };
        if form.agent_menu.take().is_some() {
            return;
        }
        form.palette_open = false;

        let Some(form) = self.project_form() else {
            return;
        };
        let menu = agent_menu(form, &self.theme, cx);
        if let Some(form) = self.project_form_mut() {
            form.agent_menu = Some(menu);
        }
    }

    /// Takes a pick from the agent dropdown: row 0 is the default.
    fn pick_form_agent(&mut self, row: usize) {
        if let Some(form) = self.project_form_mut() {
            form.agent = row
                .checked_sub(1)
                .and_then(|row| form.agents.get(row).cloned());
            form.agent_menu = None;
            form.error = None;
        }
    }

    /// Lets the agent dropdown take keys before the form interprets them.
    fn form_agent_menu_key(&mut self, event: &KeyDownEvent) -> bool {
        let Some(menu) = self
            .project_form_mut()
            .and_then(|form| form.agent_menu.as_mut())
        else {
            return false;
        };

        match menu.key(event) {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(form) = self.project_form_mut() {
                    form.agent_menu = None;
                }
                true
            }
            MenuKey::Run(row) => {
                self.pick_form_agent(row);
                true
            }
        }
    }

    // ---- views ----------------------------------------------------------------

    /// The dialog card a project form sits in.
    ///
    /// A press anywhere in it but the badge popover and its trigger puts the
    /// popover away.
    pub(super) fn form_card(&self, id: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        card(id, CARD_WIDTH, &self.theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                if let Some(form) = this.project_form_mut()
                    && form.palette_open
                {
                    form.palette_open = false;
                    // Keys typed next must not go to a field that is no longer
                    // drawn. A press on another field has already taken the
                    // keyboard by now.
                    let hidden = [&form.icon, &form.hex]
                        .into_iter()
                        .any(|input| input.read(cx).is_focused(window));
                    if hidden {
                        window.focus(form.name.read(cx).focus_handle());
                    }
                    cx.notify();
                }
            }),
        )
    }

    /// One of the form's text fields. A press on it selects what is in it.
    fn form_field(
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

    /// What the project looks like, on one line: its badge, which opens the
    /// icon and colour, beside its name.
    pub(super) fn form_identity(
        &self,
        form: &ProjectForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let colour = form.colour(cx);
        let shown = colour.unwrap_or_else(default_color);
        let initial = badge_initial(form, cx);

        let trigger = select("project-badge", "")
            .leading(badge(shown, initial.clone(), t))
            .open(form.palette_open)
            .render(t)
            .w(BADGE_TRIGGER_W)
            .flex_none()
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(form) = this.project_form_mut() {
                    form.palette_open = !form.palette_open;
                    form.agent_menu = None;
                    let hidden = !form.palette_open
                        && [&form.icon, &form.hex]
                            .into_iter()
                            .any(|input| input.read(cx).is_focused(window));
                    if hidden {
                        window.focus(form.name.read(cx).focus_handle());
                    }
                }
                cx.notify();
            }));
        let palette = form
            .palette_open
            .then(|| self.form_palette(form, colour, shown, initial, window, cx));
        let badge = anchored(
            "project-badge-popover",
            trigger.into_any_element(),
            palette,
            Placement::BelowStart,
            PALETTE_WIDTH,
            window,
            t,
        );
        let name = self
            .form_field(form, Field::Name, false, None, window, cx)
            .flex_1();

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(badge)
            .child(name)
            .into_any_element()
    }

    /// The badge popover: the icon, the swatches, and the colour as a name
    /// and a hex.
    fn form_palette(
        &self,
        form: &ProjectForm,
        colour: Option<Color>,
        shown: Color,
        initial: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let icon_field = self.form_field(form, Field::Icon, false, None, window, cx);
        let hex_field = self
            .form_field(form, Field::Hex, colour.is_none(), None, window, cx)
            .w(HEX_W)
            .flex_none()
            .font_family(self.font_family.clone());
        let named = match colour {
            Some(colour) => PROJECT_COLORS
                .iter()
                .find(|(_, hex)| Color::from_hex(hex).ok() == Some(colour))
                .map_or("Custom", |(name, _)| *name),
            None => "Not a colour",
        };
        let swatches = PROJECT_COLORS.chunks(9).enumerate().map(|(line, chunk)| {
            div()
                .flex()
                .gap(SWATCH_GAP)
                .children(chunk.iter().enumerate().map(|(at, (_, hex))| {
                    let index = line * 9 + at;
                    let each = Color::from_hex(hex).unwrap_or_else(|_| default_color());
                    swatch(("swatch", index), each, colour == Some(each), t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if let Some(form) = this.project_form_mut() {
                                // The default is the field's placeholder, so
                                // choosing it empties the field.
                                let text = if index == 0 { "" } else { *hex };
                                form.hex.update(cx, |input, _| input.set_text(text));
                                form.error = None;
                            }
                            cx.notify();
                        },
                    ))
                }))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(PALETTE_PAD)
            .prose()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label("Icon", t))
                    .child(icon_field)
                    .child(
                        div()
                            .pt(px(6.0))
                            .child(caption("Blank shows the name's first letter", t)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label("Colour", t))
                    .child(div().flex().flex_col().gap(SWATCH_GAP).children(swatches)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(badge(shown, initial, t))
                    .child(caption(named, t).flex_1().text_color(paint(t.text.primary)))
                    .child(hex_field),
            )
            .into_any_element()
    }

    /// The labelled rows under the identity line — base, agent, the agent's
    /// command, the two switches — and the refusal, if the last confirm was
    /// refused.
    pub(super) fn form_rows(
        &self,
        form: &ProjectForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = &self.theme;
        let base = self
            .form_field(form, Field::Base, false, Some(Icon::GitBranch), window, cx)
            .flex_1()
            .font_family(self.font_family.clone());

        let mut rows = vec![
            labelled("Base branch", base, t),
            labelled("Agent", self.form_agent(form, cx), t),
        ];
        rows.extend(
            self.form_command(form, window, cx)
                .map(|command| labelled("Command", command, t)),
        );
        rows.push(labelled(
            "Worktrees",
            div()
                .h(FIELD_H)
                .flex()
                .items_center()
                .child(self.form_hide_discovered(form, cx)),
            t,
        ));
        rows.push(labelled("Automation", self.form_trust(form, cx), t));
        rows.extend(
            form.backlog_in_repo
                .map(|in_repo| labelled("Backlog", self.form_backlog(in_repo, cx), t)),
        );
        rows.extend(
            form.error
                .clone()
                .map(|why| field_error(why, t).into_any_element()),
        );
        rows
    }

    /// The agent dropdown. A dropdown rather than a button per agent so the
    /// list can grow.
    fn form_agent(&self, form: &ProjectForm, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let shown = form
            .command_row()
            .and_then(|row| form.agents.get(row))
            .map(String::as_str);
        let mut trigger = select(
            "project-agent",
            shown.map_or_else(|| "No agents configured".to_owned(), agent_label),
        )
        .leading(mark(shown.unwrap_or(""), t))
        .open(form.agent_menu.is_some());
        if form.agent.is_none() {
            trigger = trigger.trailing(tag("Settings default", t));
        }
        let panel = form.agent_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, row, cx| {
                    shell.pick_form_agent(*row);
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(form) = shell.project_form_mut() {
                        form.agent_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        dropdown("project-agent-select", trigger.render(t), panel)
            .flex_1()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.toggle_form_agent_menu(cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The chosen agent's command, what it replaces, and any other agent this
    /// project already runs differently — those have no field on show, and a
    /// command nobody can see is a session on the wrong account.
    ///
    /// Only the chosen agent's command is on show: the command is a fact about
    /// that agent, and a field per configured agent grows the dialog with
    /// every one added. `None` when no agent is configured.
    fn form_command(
        &self,
        form: &ProjectForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = &self.theme;
        let mono = self.font_family.clone();
        let tint = crate::agent_override::hue(t);
        let row = form.command_row()?;
        let input = &form.commands[row];
        let set = !input.read(cx).text().trim().is_empty();
        let typing = input.read(cx).is_focused(window);

        let end: AnyElement = if set {
            icon_button("project-command-reset", Icon::RotateCcw)
                .bare()
                .dense()
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.project_form() {
                        form.commands[row].update(cx, |input, _| input.clear());
                    }
                    cx.stop_propagation();
                    cx.notify();
                }))
                .into_any_element()
        } else {
            tag("from Settings", t).into_any_element()
        };
        // A command in force wears the override's tint until the field is
        // being typed in — see `crate::agent_override`.
        let field = self
            .form_field(form, Field::Command(row), false, None, window, cx)
            .flex_1()
            .font_family(mono.clone())
            .when(set && !typing, |el| el.border_color(alpha(tint, 0.6)))
            .child(end);

        let agent = agent_label(&form.agents[row]);
        let project = match form.dir_name.as_ref() {
            "" => "this project",
            name => name,
        };
        let note = if set {
            line()
                .flex_wrap()
                .child(status_dot(tint, Motion::Still, px(6.0)))
                .child(caption("Replaces", t))
                .child(
                    caption(form.configured[row].clone(), t)
                        .font_family(mono.clone())
                        .line_through(),
                )
                .child(caption(
                    format!("for every {agent} session in {project}"),
                    t,
                ))
        } else {
            line().child(caption(
                "Blank runs the one in Settings. Type a command to change it here.",
                t,
            ))
        };
        let others = form
            .agents
            .iter()
            .zip(&form.commands)
            .enumerate()
            .filter(|(other, _)| *other != row)
            .filter_map(|(_, (name, input))| {
                let text = input.read(cx).text();
                let word = text.split_whitespace().next()?.to_owned();
                Some(
                    line()
                        .child(mark(name, t))
                        .child(caption(format!("{} runs", agent_label(name)), t))
                        .child(caption(word, t).font_family(mono.clone()).text_color(tint))
                        .child(caption("here too", t)),
                )
            });

        Some(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(7.0))
                .child(div().flex().child(field))
                .child(note)
                .children(others)
                .into_any_element(),
        )
    }

    /// The discovered-worktrees switch.
    fn form_hide_discovered(&self, form: &ProjectForm, cx: &mut Context<Self>) -> AnyElement {
        checkbox(
            "hide-discovered",
            form.hide_discovered,
            "Hide worktrees ket did not create",
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(form) = this.project_form_mut() {
                form.hide_discovered = !form.hide_discovered;
            }
            cx.notify();
        }))
        .into_any_element()
    }

    /// Where the backlog is kept: privately, or in the repository for
    /// everyone who has it — and what that means, under the choice.
    fn form_backlog(&self, in_repo: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let choice = button_group("project-backlog")
            .small()
            .child(
                segment("project-backlog-private", "Private")
                    .leading(Icon::Lock, paint(t.text.dim))
                    .selected(!in_repo)
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        if let Some(form) = this.project_form_mut() {
                            form.backlog_in_repo = Some(false);
                        }
                        cx.notify();
                    })),
            )
            .child(
                segment("project-backlog-repo", "In the repository")
                    .leading(Icon::BookMarked, paint(t.status.running))
                    .selected(in_repo)
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        if let Some(form) = this.project_form_mut() {
                            form.backlog_in_repo = Some(true);
                        }
                        cx.notify();
                    })),
            )
            .render(t);
        let note = if in_repo {
            "Notes and their files live in .ket/backlog/, one file per note. Commit \
             them, and everyone with the repository shares one backlog."
        } else {
            "Notes stay on this Mac, in ket's own data."
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .flex_1()
            .min_h(FIELD_H)
            .justify_center()
            .child(div().flex().child(choice))
            .child(caption(note, t))
            .into_any_element()
    }

    /// The switch for the repository's own automation.
    fn form_trust(&self, form: &ProjectForm, cx: &mut Context<Self>) -> AnyElement {
        checkbox(
            "trust-automation",
            form.trust_automation,
            "Run hooks and post-command from .ket.toml",
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(form) = this.project_form_mut() {
                form.trust_automation = !form.trust_automation;
            }
            cx.notify();
        }))
        .into_any_element()
    }
}

/// The agent dropdown's rows: "Settings default", then each agent with what
/// it would launch here — the project's own command in the override's tint,
/// else the one in Settings. With many agents that is the difference between
/// picking one, and picking one then checking what it runs.
fn agent_menu(form: &ProjectForm, t: &Theme, cx: &App) -> OpenMenu<usize> {
    let tint = crate::agent_override::hue(t);
    let launches = |row: usize| -> (SharedString, Option<Rgba>) {
        let own = form.commands[row].read(cx).text();
        match own.split_whitespace().next() {
            Some(word) => (word.to_owned().into(), Some(tint)),
            None => (
                form.configured[row]
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
                    .into(),
                None,
            ),
        }
    };
    let current = form
        .agent
        .as_ref()
        .and_then(|name| form.agents.iter().position(|agent| agent == name))
        .map_or(0, |row| row + 1);
    let checked = |item: MenuItem<usize>, row: usize| {
        MenuEntry::Item(if current == row { item.checked() } else { item })
    };

    let mut entries = Vec::with_capacity(form.agents.len() + 2);
    if let Some(first) = form.agents.first() {
        let (which, colour) = provider(first, t);
        let item = MenuItem::new(0, "Settings default")
            .tinted_icon(which, colour)
            .detail(agent_label(first), None);
        entries.push(checked(item, 0));
        entries.push(MenuEntry::Separator);
    }
    for (row, name) in form.agents.iter().enumerate() {
        let (which, colour) = provider(name, t);
        let (command, ink) = launches(row);
        let item = MenuItem::new(row + 1, agent_label(name))
            .tinted_icon(which, colour)
            .detail(command, ink);
        entries.push(checked(item, row + 1));
    }
    OpenMenu::new(
        "project-agent".into(),
        Some("Choose an agent…".into()),
        control_width(),
        entries,
    )
    .selected(current)
    .offset(px(0.0), px(6.0))
}

/// The badge's mark: the icon, else the first letter of the name, else of
/// the folder's name.
fn badge_initial(form: &ProjectForm, cx: &App) -> SharedString {
    let icon = form.icon.read(cx).text();
    if !icon.trim().is_empty() {
        return icon.trim().to_owned().into();
    }
    let name = form.name.read(cx).text();
    let name = match name.trim() {
        "" => form.dir_name.as_ref(),
        typed => typed,
    };
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
        .into()
}

/// One line of the notes under the command: captions and marks in a row.
fn line() -> Div {
    div().flex().items_center().gap(px(5.0))
}
