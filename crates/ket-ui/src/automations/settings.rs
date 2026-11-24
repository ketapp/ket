//! The chosen automation's Settings tab: its name and what it does, when it
//! starts and where it runs — the same form as the New worktree dialog's
//! Schedule section — how late it may start after a sleep, and whether it
//! skips while the last run works. Save writes it; Discard puts back what is
//! saved; Delete takes it and its history.

use gpui::{
    AnyElement, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, ParentElement, Styled,
    Window, div, prelude::*, px,
};
use ket_core::automation::{Automation, Task};

use crate::Shell;
use crate::input::{Key, Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::schedule_fields::{ScheduleFields, labelled};
use crate::ui::button::button;
use crate::ui::chip::caption;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::section::setting_row;
use crate::ui::select::select;
use crate::ui::textarea::{FORM_INSET, textarea};
use crate::ui::toggle::switch;

/// The composer the prompt is edited in.
const PROMPT_EDITOR: &str = "automations:prompt";

/// The prompt's well.
const PROMPT_HEIGHT: gpui::Pixels = px(96.0);

/// How wide the tab's fields run.
const FORM_WIDTH: gpui::Pixels = px(640.0);

/// The catch-up choices: minutes late a start may still be taken, `None` for
/// the repeat's own default, `Some(0)` for never.
const CATCH_UP: [(Option<u32>, &str); 5] = [
    (None, "As its repeat suggests"),
    (Some(30), "Start within 30 minutes"),
    (Some(180), "Start within 3 hours"),
    (Some(720), "Start within 12 hours"),
    (Some(0), "Never start late"),
];

/// The tab's state: the chosen automation as it is being edited.
pub(crate) struct Edit {
    /// Which automation.
    pub(super) id: String,
    name: Entity<TextInput>,
    prompt_focus: FocusHandle,
    fields: ScheduleFields,
    catch_up: Option<u32>,
    catch_up_focus: FocusHandle,
    catch_up_menu: Option<OpenMenu<Option<u32>>>,
    skip_while_busy: bool,
}

fn fields(shell: &mut Shell) -> Option<&mut ScheduleFields> {
    Some(&mut shell.automations.as_mut()?.edit.as_mut()?.fields)
}

fn fields_read(shell: &Shell) -> Option<&ScheduleFields> {
    Some(&shell.automations.as_ref()?.edit.as_ref()?.fields)
}

impl Edit {
    /// Whether one of its fields has the keyboard.
    pub(super) fn focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.name.read(cx).is_focused(window)
            || self.prompt_focus.is_focused(window)
            || self.catch_up_focus.is_focused(window)
            || self.fields.focused(window, cx)
    }
}

impl Shell {
    /// Starts editing the chosen automation, unless it already is.
    pub(super) fn begin_automation_edit(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.automations.as_ref() else {
            return;
        };
        let Some(id) = dialog.selected.clone() else {
            return;
        };
        if dialog.edit.as_ref().is_some_and(|edit| edit.id == id) {
            return;
        }
        let Some(automation) = dialog.all.get(&id).cloned() else {
            return;
        };
        let text = match &automation.task {
            Task::Prompt { text, .. } => text.clone(),
            Task::Command { line } => line.clone(),
        };
        self.open_composer(PROMPT_EDITOR, &text);
        let name = TextInput::new("What lists call it", cx);
        name.update(cx, |input, _| input.set_text(&automation.name));
        self.watch_field(&name, cx);
        let fields = ScheduleFields::of(&automation, cx);
        let edit = Edit {
            id,
            name,
            prompt_focus: cx.focus_handle(),
            fields,
            catch_up: automation.catch_up_minutes,
            catch_up_focus: cx.focus_handle(),
            catch_up_menu: None,
            skip_while_busy: automation.skip_while_busy,
        };
        if let Some(dialog) = self.automations.as_mut() {
            dialog.edit = Some(edit);
        }
    }

    /// Stops editing, keeping nothing unsaved.
    pub(super) fn end_automation_edit(&mut self) {
        if let Some(dialog) = self.automations.as_mut() {
            dialog.edit = None;
        }
        self.close_composer(PROMPT_EDITOR);
    }

    /// The automation as the tab now has it, or why it can't be.
    fn edited_automation(&self, cx: &gpui::App) -> Option<Result<Automation, String>> {
        let dialog = self.automations.as_ref()?;
        let edit = dialog.edit.as_ref()?;
        let mut automation = dialog.all.get(&edit.id)?.clone();
        let text = self.composer_text(PROMPT_EDITOR).trim().to_owned();
        if text.is_empty() {
            return Some(Err(
                "Say what it does: each run starts with this.".to_owned()
            ));
        }
        let name = edit.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            return Some(Err("Give it a name.".to_owned()));
        }
        let when = match edit.fields.when(cx) {
            Ok(when) => when,
            Err(why) => return Some(Err(why)),
        };
        automation.name = name;
        match &mut automation.task {
            Task::Prompt { text: saved, .. } => *saved = text,
            Task::Command { line } => *line = text,
        }
        automation.when = when;
        automation.place = edit.fields.place();
        automation.catch_up_minutes = edit.catch_up;
        automation.skip_while_busy = edit.skip_while_busy;
        Some(Ok(automation))
    }

    /// Whether the tab differs from what is saved.
    fn automation_edit_dirty(&self, cx: &gpui::App) -> bool {
        let Some(dialog) = self.automations.as_ref() else {
            return false;
        };
        let Some(edit) = dialog.edit.as_ref() else {
            return false;
        };
        let saved = dialog.all.get(&edit.id);
        match self.edited_automation(cx) {
            Some(Ok(edited)) => saved.is_some_and(|saved| {
                let mut saved = saved.clone();
                saved.updated_ms = edited.updated_ms;
                saved != edited
            }),
            Some(Err(_)) => true,
            None => false,
        }
    }

    fn save_automation_edit(&mut self, cx: &mut Context<Self>) {
        let Some(result) = self.edited_automation(cx) else {
            return;
        };
        let edited = match result {
            Ok(edited) => edited,
            Err(why) => {
                if let Some(dialog) = self.automations.as_mut() {
                    dialog.error = Some(why.into());
                }
                return;
            }
        };
        let id = edited.id.clone();
        let when_changed = self
            .automations
            .as_ref()
            .and_then(|dialog| dialog.all.get(&id))
            .is_some_and(|saved| saved.when != edited.when);
        // A new schedule has to start: a one-off moved to a time already
        // gone would never run.
        if when_changed
            && let Err(why) = edited
                .when
                .check_at(jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
        {
            if let Some(dialog) = self.automations.as_mut() {
                dialog.error = Some(why.into());
            }
            return;
        }
        let rearm = when_changed && edited.when.is_once();
        self.change_automation(&id, move |automation| {
            // A one-off given a new moment starts again: its last start is
            // history, not a reason never to.
            let handled = if rearm { None } else { automation.handled_ms };
            let run_now = automation.run_now_ms;
            *automation = edited;
            automation.handled_ms = handled;
            automation.run_now_ms = run_now;
            automation.updated_ms = ket_core::now_ms();
            // A new schedule starts from now: nothing it would have had
            // before is taken late.
            if when_changed {
                automation.resumed_ms = Some(ket_core::now_ms());
            }
        });
        self.end_automation_edit();
        self.begin_automation_edit(cx);
    }

    fn discard_automation_edit(&mut self, cx: &mut Context<Self>) {
        self.end_automation_edit();
        self.begin_automation_edit(cx);
    }

    fn toggle_catch_up_menu(&mut self) {
        let Some(edit) = self
            .automations
            .as_mut()
            .and_then(|dialog| dialog.edit.as_mut())
        else {
            return;
        };
        if edit.catch_up_menu.take().is_some() {
            return;
        }
        edit.fields.close_menus();
        let current = edit.catch_up;
        let entries = CATCH_UP
            .into_iter()
            .map(|(minutes, label)| {
                let item = MenuItem::new(minutes, label);
                MenuEntry::Item(if minutes == current {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        edit.catch_up_menu = Some(
            OpenMenu::new(
                "automation-catch-up".into(),
                None,
                FORM_WIDTH / 2.0,
                entries,
            )
            .selected(
                CATCH_UP
                    .iter()
                    .position(|(minutes, _)| *minutes == current)
                    .unwrap_or(0),
            )
            .offset(px(0.0), px(6.0)),
        );
    }

    /// The tab's keys. `None` leaves the key to the dialog: nothing of the
    /// tab's has the keyboard, or it is Escape.
    pub(super) fn automation_settings_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let edit = self.automations.as_mut()?.edit.as_mut()?;
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        if let Some(open) = edit.catch_up_menu.as_mut() {
            match open.key(event) {
                MenuKey::Ignored => {}
                MenuKey::Consumed => return Some(Key::Taken),
                MenuKey::Close => {
                    edit.catch_up_menu = None;
                    return Some(Key::Taken);
                }
                MenuKey::Run(minutes) => {
                    edit.catch_up = minutes;
                    edit.catch_up_menu = None;
                    return Some(Key::Taken);
                }
            }
        }
        if self.schedule_fields_menu_key(fields, event, cx) {
            return Some(Key::Taken);
        }
        if key.key == "s" && command {
            self.save_automation_edit(cx);
            return Some(Key::Taken);
        }
        let edit = self.automations.as_ref()?.edit.as_ref()?;
        if key.key == "escape" || !edit.focused(window, cx) {
            return None;
        }
        let name = edit.name.clone();
        if key.key == "tab" {
            let mut order = vec![
                name.read(cx).focus_handle().clone(),
                edit.prompt_focus.clone(),
            ];
            order.extend(edit.fields.tab_order(cx));
            order.push(edit.catch_up_focus.clone());
            let at = order.iter().position(|handle| handle.is_focused(window));
            let next = at.map_or(0, |at| (at + 1) % order.len());
            if let Some(handle) = order.get(next) {
                window.focus(handle);
            }
            return Some(Key::Taken);
        }
        if name.read(cx).is_focused(window) {
            if is_text(key) {
                return Some(Key::Text);
            }
            name.update(cx, |input, cx| input.key(key, cx));
            return Some(Key::Taken);
        }
        if edit.prompt_focus.is_focused(window) {
            if is_text(key) && !command {
                return Some(Key::Text);
            }
            self.composer_key(PROMPT_EDITOR, key, cx);
            return Some(Key::Taken);
        }
        if edit.catch_up_focus.is_focused(window) {
            if matches!(key.key.as_str(), "enter" | "space" | "up" | "down") {
                self.toggle_catch_up_menu();
            }
            return Some(Key::Taken);
        }
        self.schedule_fields_key(fields, FORM_WIDTH / 2.0, event, window, cx)
            .or(Some(Key::Taken))
    }

    /// The Settings tab, for the automation being edited.
    pub(super) fn automation_settings_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let writing = self
            .automations
            .as_ref()?
            .edit
            .as_ref()?
            .prompt_focus
            .is_focused(window);
        self.set_composer_caret(PROMPT_EDITOR, writing);
        let editor = self.editor_pane(PROMPT_EDITOR, None, window, cx);
        let schedule = self.schedule_fields_view(fields, fields_read, FORM_WIDTH, window, cx)?;
        let dirty = self.automation_edit_dirty(cx);
        let dialog = self.automations.as_ref()?;
        let edit = dialog.edit.as_ref()?;
        let automation = dialog.all.get(&edit.id)?;
        let t = &self.theme;

        let heading = |words: &'static str| {
            div()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(paint(t.text.primary))
                .child(words)
        };
        let section = |first: bool| {
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .py(px(16.0))
                .when(!first, |el| el.border_t_1().border_color(paint(t.rule)))
        };

        let name = text_field(
            &edit.name,
            "automation-name",
            false,
            Style::new(t, self.caret.visible),
            window,
            cx,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.notify()));
        let prompt_focus = edit.prompt_focus.clone();
        let prompt = textarea("automation-prompt", editor, PROMPT_HEIGHT)
            .inset(FORM_INSET)
            .active(writing)
            .render(t)
            .cursor_text()
            .track_focus(&edit.prompt_focus)
            .capture_any_mouse_down(cx.listener(move |_, _, window, cx| {
                window.focus(&prompt_focus);
                cx.notify();
            }));
        let runs_with = match &automation.task {
            Task::Prompt {
                agent,
                model,
                effort,
                ..
            } => {
                let agent = agent.as_deref().map_or_else(
                    || "The project's agent".to_owned(),
                    crate::ui::agent::agent_label,
                );
                let mut said = format!("Runs with {agent}");
                for part in [model, effort].into_iter().flatten() {
                    said.push_str(&format!(" \u{b7} {part}"));
                }
                said
            }
            Task::Command { .. } => "Runs as a command, typed into a terminal".to_owned(),
        };
        let what = section(true)
            .child(heading("What it does"))
            .child(labelled("Name", name.into_any_element(), t))
            .child(labelled(
                if matches!(automation.task, Task::Command { .. }) {
                    "Command"
                } else {
                    "Prompt"
                },
                prompt.into_any_element(),
                t,
            ))
            .child(caption(runs_with, t));

        let when = section(false)
            .child(heading("When and where"))
            .child(schedule);

        let catch_label = CATCH_UP
            .iter()
            .find(|(minutes, _)| *minutes == edit.catch_up)
            .map_or("As its repeat suggests", |(_, label)| label);
        let catch_menu = edit.catch_up_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, minutes, cx| {
                    if let Some(edit) = shell.automations.as_mut().and_then(|d| d.edit.as_mut()) {
                        edit.catch_up = *minutes;
                        edit.catch_up_menu = None;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(edit) = shell.automations.as_mut().and_then(|d| d.edit.as_mut()) {
                        edit.catch_up_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let catch_up = dropdown(
            "automation-catch-up-select",
            select("automation-catch-up", catch_label)
                .open(catch_menu.is_some())
                .focused(edit.catch_up_focus.is_focused(window))
                .render(t),
            catch_menu,
        )
        .track_focus(&edit.catch_up_focus)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window: &mut Window, cx| {
                if let Some(edit) = this.automations.as_ref().and_then(|d| d.edit.as_ref()) {
                    window.focus(&edit.catch_up_focus.clone());
                }
                this.toggle_catch_up_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );
        let skip = edit.skip_while_busy;
        let rules = section(false)
            .child(heading("Rules"))
            .child(
                labelled("If the Mac was asleep", catch_up.into_any_element(), t)
                    .w(FORM_WIDTH / 2.0),
            )
            .child(
                setting_row(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(paint(t.text.primary))
                                .child("Skip a run while the last one is still working"),
                        )
                        .child(caption(
                            "Otherwise two runs work side by side, and spend twice.",
                            t,
                        )),
                )
                .child(switch("automation-skip-busy", skip, t).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(edit) = this.automations.as_mut().and_then(|d| d.edit.as_mut())
                        {
                            edit.skip_while_busy = !skip;
                        }
                        cx.notify();
                    }),
                )),
            );

        let said = match (dirty, &dialog.error) {
            (_, Some(error)) => caption(error.clone(), t).text_color(paint(t.status.failed)),
            (true, None) => caption("Unsaved changes", t),
            (false, None) => caption("No changes", t),
        };
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .pt(px(12.0))
            .border_t_1()
            .border_color(paint(t.rule))
            .child(
                button("automation-delete-footer", "Delete automation")
                    .danger()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.automations.as_mut() {
                            dialog.confirming_delete = true;
                        }
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(said)
            .child(
                button("automation-discard", "Discard")
                    .enabled(dirty)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.discard_automation_edit(cx);
                        cx.notify();
                    })),
            )
            .child(
                button("automation-save", "Save")
                    .primary()
                    .enabled(dirty)
                    .hint("\u{2318}S")
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_automation_edit(cx);
                        cx.notify();
                    })),
            );

        Some(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("automation-settings")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(what)
                        .child(when)
                        .child(rules),
                )
                .child(footer)
                .into_any_element(),
        )
    }
}

/// Whether the dialog's Settings tab is saving text into a field: the shell
/// keeps the keyboard there while it is.
pub(crate) fn editing(shell: &Shell) -> bool {
    shell
        .automations
        .as_ref()
        .is_some_and(|dialog| dialog.edit.is_some())
}
