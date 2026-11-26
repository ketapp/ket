//! A project form's agent: which one its worktrees start, and the command
//! it is launched with in this project.
//!
//! Both dialogs draw these from [`ProjectForm`]'s drafts, in their Agent
//! section.

use gpui::{
    AnyElement, App, Context, Div, Entity, KeyDownEvent, MouseButton, Rgba, SharedString, Stateful,
    Window, prelude::*, px,
};
use ket_core::project::ProjectSettings;
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;

use super::form::{Field, ProjectForm, line};
use crate::Shell;
use crate::input::TextInput;
use crate::paint::alpha;
use crate::ui::agent::{agent_label, mark, provider};
use crate::ui::button::icon_button;
use crate::ui::chip::{Motion, caption, status_dot};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;

impl Shell {
    // ---- the agent menu -------------------------------------------------------

    /// Opens or closes the form's agent dropdown.
    fn toggle_form_agent_menu(&mut self, window: &mut Window, cx: &App) {
        let Some(form) = self.project_form_mut() else {
            return;
        };
        if form.agent_menu.take().is_some() {
            return;
        }
        form.close_panel(window, cx);
        form.github_menu = None;

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
            form.agent = row.checked_sub(1).and_then(|row| {
                form.agents
                    .iter()
                    .chain(&form.local_agents)
                    .nth(row)
                    .cloned()
            });
            form.agent_menu = None;
            form.error = None;
        }
    }

    /// Lets the agent dropdown take keys before the form interprets them.
    pub(super) fn form_agent_menu_key(&mut self, event: &KeyDownEvent) -> bool {
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

    /// The agent dropdown. A dropdown rather than a button per agent so the
    /// list can grow. Size it at the call site. It shows the agent's name
    /// alone, whether the project picked it or takes Settings' default.
    pub(super) fn form_agent(&self, form: &ProjectForm, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = &self.theme;
        let shown = form.agent.as_deref().or_else(|| {
            form.command_row()
                .and_then(|row| form.agents.get(row))
                .map(String::as_str)
        });
        let trigger = select(
            "project-agent",
            shown.map_or_else(|| "No agents configured".to_owned(), agent_label),
        )
        .leading(mark(shown.unwrap_or(""), t))
        .open(form.agent_menu.is_some());
        let panel = form.agent_menu.as_ref().map(|menu| {
            menu.dropdown_view(
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
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.toggle_form_agent_menu(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// The field for the command of the agent in `row` of
    /// [`ProjectForm::agents`]. Size it at the call site.
    ///
    /// A command in force wears the override's tint until the field is being
    /// typed in — see `crate::agent_override` — and carries the button that
    /// clears it. A blank one leaves its placeholder to say what runs.
    pub(super) fn command_field(
        &self,
        form: &ProjectForm,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = &self.theme;
        let tint = crate::agent_override::hue(t);
        let input = &form.commands[row];
        let set = !input.read(cx).text().trim().is_empty();
        let typing = input.read(cx).is_focused(window);

        let end: Option<AnyElement> = if set {
            Some(
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
                    .into_any_element(),
            )
        } else {
            None
        };
        self.form_field(form, Field::Command(row), false, None, window, cx)
            .font_family(self.font_family.clone())
            .when(set && !typing, |el| el.border_color(alpha(tint, 0.6)))
            .children(end)
    }

    /// The lines under the command: what a command set here replaces, and
    /// each other agent this project runs with a command of its own. What a
    /// blank command does is the command's tooltip's to say.
    pub(super) fn command_notes(
        &self,
        form: &ProjectForm,
        row: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = &self.theme;
        let mono = self.font_family.clone();
        let tint = crate::agent_override::hue(t);
        let set = !form.commands[row].read(cx).text().trim().is_empty();
        let agent = agent_label(&form.agents[row]);
        let project = match form.dir_name.as_ref() {
            "" => "this project",
            name => name,
        };
        let note = if set {
            Some(
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
                    )),
            )
        } else {
            None
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
        note.into_iter()
            .chain(others)
            .map(IntoElement::into_any_element)
            .collect()
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
        .and_then(|name| {
            form.agents
                .iter()
                .chain(&form.local_agents)
                .position(|agent| agent == name)
        })
        .map_or(0, |row| row + 1);
    let checked = |item: MenuItem<usize>, row: usize| {
        MenuEntry::Item(if current == row { item.checked() } else { item })
    };

    let mut entries = Vec::with_capacity(form.agents.len() + form.local_agents.len() + 3);
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
    // Local agents last, set apart: what they run is the model, on this Mac.
    if !form.local_agents.is_empty() {
        entries.push(MenuEntry::Separator);
    }
    for (offset, name) in form.local_agents.iter().enumerate() {
        let row = form.agents.len() + offset + 1;
        let (which, colour) = provider(name, t);
        let item = MenuItem::new(row, agent_label(name))
            .tinted_icon(which, colour)
            .detail("on this Mac", None);
        entries.push(checked(item, row));
    }
    OpenMenu::new(
        "project-agent".into(),
        Some("Choose an agent…".into()),
        form.menu_width(),
        entries,
    )
    .selected(current)
    .offset(px(0.0), px(6.0))
}

/// The form's agent rows for a project with `settings`: each configured
/// agent's name, its command line in Settings, and a field holding the
/// project's own command for it.
pub(super) fn agent_drafts(
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
