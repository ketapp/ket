//! The New worktree dialog's Schedule section: what the dialog becomes when
//! Start ▾ → Schedule… is chosen.
//!
//! The prompt, agent, model, effort and Economy above stay as they are; the
//! branch gives way to a name, and the Schedule section asks when and where
//! — the form in [`crate::schedule_fields`], shared with the Automations
//! dialog's Settings tab. Schedule saves it as an automation
//! (`ket_core::automation`) and the host starts it from then on; nothing is
//! created now.

use gpui::{
    AnyElement, Context, FocusHandle, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    Styled, Window, div, prelude::*, px,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_core::automation::{Automation, Automations, Task};

use super::{NewWorktree, PROMPT_EDITOR, dropdown_key, form_width};
use crate::Shell;
use crate::input::{Key, Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::schedule_fields::{ScheduleFields, labelled, moment};
use crate::ui::button::{Variant, button, split, split_more};
use crate::ui::chip::caption;
use crate::ui::icon::{Icon, icon};
use crate::ui::menu::{MenuEntry, MenuItem, OpenMenu, dropdown};
use crate::ui::toast::Tone;

/// How the dialog's main button starts the work: now, or on a schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartMode {
    Now,
    Schedule,
}

/// The Schedule section's state, while Schedule is chosen.
pub(crate) struct ScheduleForm {
    /// What lists call the automation; filled from the prompt until someone
    /// types here, after which it is theirs — as the branch is.
    name: gpui::Entity<TextInput>,
    name_edited: bool,
    synced_name: String,
    fields: ScheduleFields,
}

/// The form, wherever the dialog keeps it.
fn fields(shell: &mut Shell) -> Option<&mut ScheduleFields> {
    Some(&mut shell.new_worktree.as_mut()?.schedule.as_mut()?.fields)
}

fn fields_read(shell: &Shell) -> Option<&ScheduleFields> {
    Some(&shell.new_worktree.as_ref()?.schedule.as_ref()?.fields)
}

/// Closes the Schedule section's menus, and the Start ▾ menu: one dropdown
/// open at a time, across the whole dialog.
pub(super) fn close_menus(dialog: &mut NewWorktree) {
    dialog.start_menu = None;
    if let Some(form) = dialog.schedule.as_mut() {
        form.fields.close_menus();
    }
}

/// Points the Schedule section at the dialog's new project.
pub(super) fn retarget(dialog: &mut NewWorktree) {
    let project = dialog.project.clone();
    if let Some(form) = dialog.schedule.as_mut() {
        form.fields.retarget(&project);
    }
}

/// Whether the dialog runs no agent, so the prompt is a shell command.
fn is_command(dialog: &NewWorktree) -> bool {
    dialog.agents.get(dialog.agent).is_none_or(String::is_empty)
}

/// Whether one of the Schedule section's controls has the keyboard.
pub(super) fn focused(dialog: &NewWorktree, window: &Window, cx: &gpui::App) -> bool {
    dialog.schedule.as_ref().is_some_and(|form| {
        form.name.read(cx).is_focused(window) || form.fields.focused(window, cx)
    })
}

/// A name from the prompt: its first few words, the first letter capital —
/// `Audit the dependencies for new` from "audit the dependencies for new
/// security advisories…".
pub(crate) fn name_from(prompt: &str) -> String {
    let words: Vec<&str> = prompt
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .take(5)
        .collect();
    let mut name = words.join(" ");
    if let Some(first) = name.get(..1) {
        let upper = first.to_uppercase();
        name.replace_range(..1, &upper);
    }
    name
}

impl Shell {
    fn schedule_form_mut(&mut self) -> Option<&mut ScheduleForm> {
        self.new_worktree.as_mut()?.schedule.as_mut()
    }

    /// Start ▾ → Schedule…: the dialog becomes a new automation's.
    pub(crate) fn open_schedule(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.new_worktree.as_ref() else {
            return;
        };
        if dialog.schedule.is_some() {
            return;
        }
        let project = dialog.project.clone();
        let fields = ScheduleFields::new(&project, cx);
        let name = TextInput::new("What lists call it", cx);
        self.watch_field(&name, cx);
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        close_menus(dialog);
        dialog.error = None;
        dialog.schedule = Some(ScheduleForm {
            name,
            name_edited: false,
            synced_name: String::new(),
            fields,
        });
        self.sync_schedule_name(cx);
    }

    /// Start now instead: back to the dialog as it was.
    pub(super) fn close_schedule(&mut self) {
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.schedule = None;
            dialog.start_menu = None;
            dialog.error = None;
        }
    }

    /// Fills the name from the prompt, unless someone has typed one. Not
    /// from a command: its words can hold a secret, and a name goes into
    /// branches, the status bar and what phones are sent.
    pub(super) fn sync_schedule_name(&mut self, cx: &mut Context<Self>) {
        let command = self.new_worktree.as_ref().is_some_and(is_command);
        let prompt = if command {
            String::new()
        } else {
            self.composer_text(PROMPT_EDITOR)
        };
        let Some(form) = self.schedule_form_mut() else {
            return;
        };
        let typed = form.name.read(cx).text();
        if typed.is_empty() {
            form.name_edited = false;
        } else if typed != form.synced_name {
            form.name_edited = true;
        }
        if form.name_edited {
            return;
        }
        let name = name_from(&prompt);
        if typed != name {
            form.name.update(cx, |input, _| input.set_text(&name));
        }
        form.synced_name = name;
    }

    fn toggle_start_menu(&mut self) {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return;
        };
        if dialog.start_menu.take().is_some() {
            return;
        }
        let scheduling = dialog.schedule.is_some();
        close_menus(dialog);
        let chord = |keys: &str| ket_core::keybinding::Chord::parse(keys).ok();
        let mut now = MenuItem::new(StartMode::Now, "Start now")
            .subtitle("Opens the worktree and starts the agent")
            .icon(Icon::Play);
        let mut later = MenuItem::new(StartMode::Schedule, "Schedule\u{2026}")
            .subtitle("Run it later, or on a repeat")
            .icon(Icon::Clock);
        if let Some(chord) = chord("cmd+enter") {
            now = now.chord(chord);
        }
        if let Some(chord) = chord("alt+cmd+enter") {
            later = later.chord(chord);
        }
        let (now, later) = if scheduling {
            (now, later.checked())
        } else {
            (now.checked(), later)
        };
        dialog.start_menu = Some(
            OpenMenu::new(
                "new-worktree-start".into(),
                None,
                px(272.0),
                vec![MenuEntry::Item(now), MenuEntry::Item(later)],
            )
            .selected(usize::from(scheduling))
            .offset(px(-244.0), px(6.0)),
        );
    }

    fn pick_start(&mut self, mode: StartMode, cx: &mut Context<Self>) {
        if let Some(dialog) = self.new_worktree.as_mut() {
            dialog.start_menu = None;
        }
        match mode {
            StartMode::Now => self.close_schedule(),
            StartMode::Schedule => self.open_schedule(cx),
        }
    }

    /// Lets an open Schedule or Start menu take a key first. Returns whether
    /// one did.
    pub(super) fn schedule_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dialog) = self.new_worktree.as_mut() else {
            return false;
        };
        if let Some(chosen) = dropdown_key(&mut dialog.start_menu, event) {
            if let Some(mode) = chosen {
                self.pick_start(mode, cx);
            }
            return true;
        }
        self.schedule_fields_menu_key(fields, event, cx)
    }

    /// The Schedule section's keys: typing in the name, the form's own, and
    /// Tab through it all. `None` leaves the key to the rest of the dialog.
    pub(super) fn schedule_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let form = self.new_worktree.as_ref()?.schedule.as_ref()?;
        let key = &event.keystroke;
        let name = form.name.clone();
        let on_name = name.read(cx).is_focused(window);
        let in_fields = form.fields.focused(window, cx);
        let command = key.modifiers.platform || key.modifiers.control;

        if key.key == "tab" {
            let order = self.schedule_tab_order(cx)?;
            let at = order.iter().position(|handle| handle.is_focused(window));
            let next = at.map_or(0, |at| (at + 1) % order.len());
            if let Some(handle) = order.get(next) {
                window.focus(handle);
            }
            return Some(Key::Taken);
        }
        if on_name {
            if is_text(key) {
                if let Some(dialog) = self.new_worktree.as_mut() {
                    dialog.error = None;
                }
                return Some(Key::Text);
            }
            if key.key == "escape" || (key.key == "enter" && command) {
                return None;
            }
            // Enter in a field saves, as it does in the dialog's others.
            if key.key == "enter" {
                self.create_automation(cx);
                return Some(Key::Taken);
            }
            name.update(cx, |input, cx| input.key(key, cx));
            if let Some(dialog) = self.new_worktree.as_mut() {
                dialog.error = None;
            }
            self.sync_schedule_name(cx);
            return Some(Key::Taken);
        }
        let taken = self.schedule_fields_key(fields, form_width() / 2.0, event, window, cx);
        // The form leaves Enter in the time to the dialog: it saves.
        if taken.is_none() && in_fields && key.key == "enter" && !command {
            self.create_automation(cx);
            return Some(Key::Taken);
        }
        taken
    }

    /// Tab's order while Schedule is chosen: the prompt, the name, the
    /// agent's three, Economy, the project when it is a field, then the
    /// Schedule section top to bottom.
    fn schedule_tab_order(&self, cx: &gpui::App) -> Option<Vec<FocusHandle>> {
        let dialog = self.new_worktree.as_ref()?;
        let form = dialog.schedule.as_ref()?;
        let mut order = vec![
            dialog.focus.clone(),
            form.name.read(cx).focus_handle().clone(),
            dialog.agent_focus.clone(),
            dialog.model_focus.clone(),
            dialog.effort_focus.clone(),
            dialog.token_reduction_focus.clone(),
        ];
        if dialog.choose_project {
            order.push(dialog.project_focus.clone());
        }
        order.extend(form.fields.tab_order(cx));
        Some(order)
    }

    /// Saves the automation, says when it starts, and closes the dialog.
    pub(super) fn create_automation(&mut self, cx: &mut Context<Self>) {
        let prompt = self.composer_text(PROMPT_EDITOR).trim().to_owned();
        let Some(dialog) = self.new_worktree.as_ref() else {
            return;
        };
        let Some(form) = dialog.schedule.as_ref() else {
            return;
        };
        let refuse = |shell: &mut Self, why: String| {
            if let Some(dialog) = shell.new_worktree.as_mut() {
                dialog.error = Some(why.into());
            }
        };
        if prompt.is_empty() {
            refuse(
                self,
                "Write what it does first: each run starts with the prompt.".to_owned(),
            );
            return;
        }
        let typed_name = form.name.read(cx).text().trim().to_owned();
        let command = is_command(dialog);
        let name = match (
            typed_name.is_empty() || (command && !form.name_edited),
            command,
        ) {
            (false, _) => typed_name,
            (true, false) => name_from(&prompt),
            (true, true) => {
                refuse(
                    self,
                    "Name it: a command's own words aren't used, in case they hold a secret."
                        .to_owned(),
                );
                return;
            }
        };
        let when = match form.fields.when(cx).and_then(|when| {
            when.check_at(Timestamp::now(), &TimeZone::system())
                .map(|()| when)
        }) {
            Ok(when) => when,
            Err(why) => {
                refuse(self, why);
                return;
            }
        };
        let agent = dialog
            .agents
            .get(dialog.agent)
            .filter(|name| !name.is_empty())
            .cloned();
        // No agent: the prompt is a command, typed into a terminal.
        let task = match agent {
            Some(agent) => Task::Prompt {
                text: prompt,
                agent: Some(agent),
                model: dialog.model.clone(),
                effort: dialog.effort.clone(),
            },
            None => Task::Command { line: prompt },
        };
        let place = form.fields.place();
        let project = dialog.project.clone();
        let economy = dialog.token_reduction;
        let mut made = None;
        let saved = Automations::update(|all| {
            let mut automation = Automation::new(&all.automations, project, &name, task, when);
            automation.place = place;
            automation.economy = Some(economy);
            made = Some(automation.clone());
            all.automations.push(automation);
            Ok(())
        });
        match (saved, made) {
            (Ok(_), Some(automation)) => {
                let tz = TimeZone::system();
                let next = automation
                    .next_start(Timestamp::now(), &tz)
                    .map(|next| format!(" \u{b7} next {}", moment(&next.to_zoned(tz))))
                    .unwrap_or_default();
                self.close_new_worktree();
                self.toast(
                    Tone::Info,
                    format!("Scheduled \u{201c}{}\u{201d}{next}", automation.name),
                    cx,
                );
            }
            (Err(error), _) => refuse(self, format!("Couldn\u{2019}t save it: {error}")),
            (Ok(_), None) => {}
        }
    }

    /// The name field, in the branch's place.
    pub(super) fn schedule_name_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Each frame, so a switch to no agent takes the prompt's words back
        // out of an untouched name at once.
        self.sync_schedule_name(cx);
        let form = self.new_worktree.as_ref()?.schedule.as_ref()?;
        let t = &self.theme;
        let name = form.name.clone();
        let edited = form.name_edited;
        let said = match (edited, self.new_worktree.as_ref().is_some_and(is_command)) {
            (true, _) => "",
            (false, false) => "from the prompt",
            (false, true) => "type one for a command",
        };
        let style = Style::new(t, self.caret.visible).trailing(caption(said, t));
        let field = text_field(
            &name,
            "new-worktree-automation-name",
            false,
            style,
            window,
            cx,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.notify()));
        Some(labelled("Name", field.into_any_element(), t).into_any_element())
    }

    /// The Schedule section, under the agent's three.
    pub(super) fn schedule_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.new_worktree.as_ref()?.schedule.as_ref()?;
        let body = self.schedule_fields_view(fields, fields_read, form_width(), window, cx)?;
        let t = &self.theme;
        let heading = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(icon(Icon::Clock, paint(t.text.primary)))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(paint(t.text.primary))
                    .child("Schedule"),
            )
            .child(div().flex_1())
            .child(
                button("new-worktree-start-now-instead", "Start now instead")
                    .variant(Variant::Link)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_schedule();
                        cx.notify();
                    })),
            );
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .pt(px(14.0))
                .border_t_1()
                .border_color(paint(t.rule))
                .child(heading)
                .child(body)
                .into_any_element(),
        )
    }

    /// The dialog's main button and its ▾: Start, or Schedule once
    /// Schedule… is chosen, with the menu that switches between them.
    pub(super) fn start_split(
        &mut self,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let Some(dialog) = self.new_worktree.as_ref() else {
            return div().into_any_element();
        };
        let open = dialog.start_menu.is_some();
        let main = button("new-worktree-create", label)
            .primary()
            .enabled(enabled)
            .hint("\u{2318}\u{23ce}")
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.create_worktree(cx);
                cx.notify();
            }));
        let menu = dialog.start_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, mode, cx| {
                    shell.pick_start(*mode, cx);
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.new_worktree.as_mut() {
                        dialog.start_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let more = dropdown(
            "new-worktree-start-more",
            split_more("new-worktree-start-more-button", Variant::Primary, open, t),
            menu,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_start_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );
        split(main, more).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_prompts_first_words() {
        assert_eq!(
            name_from("audit the dependencies for new security advisories."),
            "Audit the dependencies for new"
        );
        assert_eq!(name_from("  \u{201c}Fix\u{201d} it!  "), "Fix it");
        assert_eq!(name_from("..."), "");
    }
}
