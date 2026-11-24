//! The Automations dialog: every automation, the chosen one's runs, and
//! switching one off, starting one now or deleting one.
//!
//! The list is down the left, soonest start first, then the paused and the
//! one-offs that have had their start, with New automation at its foot —
//! which opens the New worktree dialog on Schedule…. The chosen automation
//! fills the right: what it does and when, a strip of figures, and its runs
//! in a table, newest first; a run that made a worktree opens it. With no
//! automation yet the dialog is neither: `empty` draws it.
//!
//! What is kept, and the clock that starts them, are `ket_core::automation`
//! and the host's. This dialog reads the files they write and writes only
//! what a person changes here; the host notices within a second.

mod empty;
mod glance;
mod runs;
mod settings;

pub(crate) use glance::Glance;

pub(crate) use settings::editing;

use std::time::SystemTime;

use gpui::{
    AnyElement, Context, FocusHandle, FontWeight, KeyDownEvent, MouseButton, SharedString, Window,
    div, prelude::*, px, relative,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_core::automation::{Automation, Automations, Clock, Day, Runs, Task};
use ket_core::id::ProjectId;

use crate::Shell;
use crate::paint::paint;
use crate::ui::agent::mark;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{Motion, caption, image_badge, status_dot};
use crate::ui::dialog::{card, centered, close};
use crate::ui::group::{button_group, segment};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::row::block_row;
use crate::ui::toast::Tone;
use crate::ui::toggle::switch;

/// The dialog's width: the list, and room beside it for the runs' table.
const DIALOG_WIDTH: gpui::Pixels = px(1000.0);

/// Its height, short of the window's own.
const DIALOG_HEIGHT: gpui::Pixels = px(680.0);

/// The list's width.
const LIST_WIDTH: gpui::Pixels = px(288.0);

/// The chosen automation's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Runs,
    Settings,
}

/// The ⋯ menu's one pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum More {
    Delete,
}

/// The dialog's state, while it is open.
pub(crate) struct AutomationsDialog {
    all: Automations,
    runs: Runs,
    /// What the files' modified times were when last read; `None` before
    /// the first read.
    seen: Option<Option<SystemTime>>,
    runs_seen: Option<Option<SystemTime>>,
    /// The automation on the right, by id.
    selected: Option<String>,
    /// The list's project filter; `None` is every project's.
    project: Option<ProjectId>,
    more: Option<OpenMenu<More>>,
    /// Asking whether to delete the chosen automation.
    confirming_delete: bool,
    error: Option<SharedString>,
    focus: FocusHandle,
    wants_focus: bool,
    tab: Tab,
    /// The Settings tab's edit of the chosen automation.
    edit: Option<settings::Edit>,
}

/// Where an automation sits in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    ComingUp,
    Paused,
    Done,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::ComingUp => "Coming up",
            Group::Paused => "Paused",
            Group::Done => "Done",
        }
    }
}

/// The automations the list shows, in its order, each with its group and
/// next start.
fn shown<'a>(
    all: &'a Automations,
    project: Option<&ProjectId>,
    now: Timestamp,
    tz: &TimeZone,
) -> Vec<(Group, &'a Automation, Option<Timestamp>)> {
    let mut rows: Vec<(Group, &Automation, Option<Timestamp>)> = all
        .automations
        .iter()
        .filter(|automation| project.is_none_or(|id| &automation.project == id))
        .map(|automation| {
            let next = automation.next_start(now, tz);
            let group = if next.is_some() {
                Group::ComingUp
            } else if automation.finished() {
                Group::Done
            } else {
                Group::Paused
            };
            (group, automation, next)
        })
        .collect();
    rows.sort_by_key(|(group, automation, next)| (*group, *next, automation.created_ms));
    rows
}

/// `2:00 AM` on the Mac's clock.
fn time_of(at: Timestamp, tz: &TimeZone) -> String {
    let local = at.to_zoned(tz.clone());
    u8::try_from(local.hour())
        .ok()
        .zip(u8::try_from(local.minute()).ok())
        .and_then(|(hour, minute)| Clock::new(hour, minute))
        .map(Clock::label)
        .unwrap_or_default()
}

/// `Today`, `Tomorrow` or `Fri 20 Nov`.
fn day_of(at: Timestamp, tz: &TimeZone) -> String {
    let date = at.to_zoned(tz.clone()).date();
    let today = Timestamp::now().to_zoned(tz.clone()).date();
    if date == today {
        "Today".to_owned()
    } else if today.tomorrow().is_ok_and(|tomorrow| tomorrow == date) {
        "Tomorrow".to_owned()
    } else {
        Day::new(date).label()
    }
}

/// `Every day · Claude Code`, `Once · Command`.
fn detail(automation: &Automation) -> String {
    let what = match &automation.task {
        Task::Prompt { agent, .. } => agent
            .as_deref()
            .map_or_else(|| "Agent".to_owned(), crate::ui::agent::agent_label),
        Task::Command { .. } => "Command".to_owned(),
    };
    let when = automation.when.describe();
    let when = when.split(" at ").next().unwrap_or(&when).to_owned();
    format!("{when} \u{b7} {what}")
}

impl Shell {
    /// Opens the dialog, on `project`'s automations when one is named.
    pub(crate) fn open_automations(&mut self, project: Option<ProjectId>, cx: &mut Context<Self>) {
        self.popup = None;
        self.menu = None;
        let mut dialog = AutomationsDialog {
            all: Automations::default(),
            runs: Runs::default(),
            seen: None,
            runs_seen: None,
            selected: None,
            project,
            more: None,
            confirming_delete: false,
            error: None,
            focus: cx.focus_handle(),
            wants_focus: true,
            tab: Tab::Runs,
            edit: None,
        };
        reload(&mut dialog);
        let tz = TimeZone::system();
        dialog.selected = shown(&dialog.all, dialog.project.as_ref(), Timestamp::now(), &tz)
            .first()
            .map(|(_, automation, _)| automation.id.clone());
        self.automations = Some(dialog);
    }

    pub(crate) fn close_automations(&mut self) {
        self.end_automation_edit();
        self.automations = None;
    }

    /// Changes automation `id` in the file, as `change` does, and says why
    /// when it can't.
    fn change_automation(&mut self, id: &str, change: impl FnOnce(&mut Automation)) {
        let id = id.to_owned();
        let result = Automations::update(|all| {
            if let Some(automation) = all.get_mut(&id) {
                change(automation);
            }
            Ok(())
        });
        if let Some(dialog) = self.automations.as_mut() {
            match result {
                Ok(all) => {
                    // Read again next frame rather than taking the file's
                    // time for this copy's: the host may have written since.
                    dialog.all = all;
                    dialog.seen = None;
                    dialog.error = None;
                }
                Err(error) => dialog.error = Some(format!("Couldn\u{2019}t save: {error}").into()),
            }
        }
    }

    fn run_automation_now(&mut self, id: &str, cx: &mut Context<Self>) {
        self.change_automation(id, Automation::ask_to_run);
        let name = self
            .automations
            .as_ref()
            .and_then(|dialog| dialog.all.get(id))
            .map(|automation| automation.name.clone())
            .unwrap_or_default();
        self.toast(
            Tone::Info,
            format!("Starting \u{201c}{name}\u{201d}\u{2026}"),
            cx,
        );
    }

    fn delete_chosen_automation(&mut self) {
        let Some(dialog) = self.automations.as_mut() else {
            return;
        };
        dialog.confirming_delete = false;
        let Some(id) = dialog.selected.clone() else {
            return;
        };
        match Automations::delete(&id) {
            Ok(()) => {
                reload(dialog);
                let tz = TimeZone::system();
                dialog.selected =
                    shown(&dialog.all, dialog.project.as_ref(), Timestamp::now(), &tz)
                        .first()
                        .map(|(_, automation, _)| automation.id.clone());
            }
            Err(error) => dialog.error = Some(format!("Couldn\u{2019}t delete it: {error}").into()),
        }
    }

    /// New automation: the New worktree dialog, on Schedule…, in the
    /// project the list is on, else the chosen automation's, else the first.
    fn new_automation(&mut self, cx: &mut Context<Self>) {
        let project = self.automations.as_ref().and_then(|dialog| {
            dialog.project.clone().or_else(|| {
                let id = dialog.selected.as_deref()?;
                dialog
                    .all
                    .get(id)
                    .map(|automation| automation.project.clone())
            })
        });
        let index = project
            .and_then(|project| self.projects.iter().position(|node| node.id == project))
            .unwrap_or(0);
        if self.projects.is_empty() {
            return;
        }
        self.close_automations();
        self.open_new_worktree(index, false, cx);
        self.open_schedule(cx);
    }

    /// Moves the choice up or down the list.
    fn move_automation_choice(&mut self, by: isize) {
        let Some(dialog) = self.automations.as_mut() else {
            return;
        };
        let tz = TimeZone::system();
        let ids: Vec<String> = shown(&dialog.all, dialog.project.as_ref(), Timestamp::now(), &tz)
            .into_iter()
            .map(|(_, automation, _)| automation.id.clone())
            .collect();
        let at = dialog
            .selected
            .as_ref()
            .and_then(|id| ids.iter().position(|shown| shown == id));
        let next = match at {
            Some(at) => at
                .saturating_add_signed(by)
                .min(ids.len().saturating_sub(1)),
            None => 0,
        };
        dialog.selected = ids.get(next).cloned();
        dialog.confirming_delete = false;
    }

    fn toggle_automation_more(&mut self) {
        let Some(dialog) = self.automations.as_mut() else {
            return;
        };
        if dialog.more.take().is_some() {
            return;
        }
        dialog.more = Some(
            OpenMenu::new(
                "automations-more".into(),
                None,
                px(220.0),
                vec![MenuEntry::Item(
                    MenuItem::new(More::Delete, "Delete automation\u{2026}")
                        .icon(Icon::Trash)
                        .danger(),
                )],
            )
            .offset(px(-190.0), px(6.0)),
        );
    }

    /// The dialog's keys: Escape closes it, or stops asking to delete; ↑ and
    /// ↓ move through the list; ⌘⏎ starts the chosen one now. Every key is
    /// the dialog's while it is up.
    pub(crate) fn automations_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dialog) = self.automations.as_mut() else {
            return false;
        };
        if let Some(open) = dialog.more.as_mut() {
            match open.key(event) {
                MenuKey::Ignored => {}
                MenuKey::Consumed => return true,
                MenuKey::Close => {
                    dialog.more = None;
                    return true;
                }
                MenuKey::Run(More::Delete) => {
                    dialog.more = None;
                    dialog.confirming_delete = true;
                    return true;
                }
            }
        }
        if dialog.tab == Tab::Settings
            && !dialog.confirming_delete
            && self.automation_settings_key(event, window, cx).is_some()
        {
            return true;
        }
        let Some(dialog) = self.automations.as_ref() else {
            return false;
        };
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        let confirming = dialog.confirming_delete;
        match key.key.as_str() {
            "escape" if confirming => {
                if let Some(dialog) = self.automations.as_mut() {
                    dialog.confirming_delete = false;
                }
            }
            "enter" if confirming => self.delete_chosen_automation(),
            "escape" => self.close_automations(),
            "up" => self.move_automation_choice(-1),
            "down" => self.move_automation_choice(1),
            "enter" if command => {
                let id = self
                    .automations
                    .as_ref()
                    .and_then(|dialog| dialog.selected.clone());
                if let Some(id) = id {
                    self.run_automation_now(&id, cx);
                }
            }
            _ => {}
        }
        true
    }

    /// The dialog, or nothing when it is closed.
    pub(crate) fn automations_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.automations.as_mut()?;
        reload(dialog);
        if dialog.wants_focus {
            dialog.wants_focus = false;
            window.focus(&dialog.focus);
        }
        // The Settings tab edits the chosen automation: a different choice,
        // or the Runs tab, drops what was not saved.
        let stale = dialog.edit.as_ref().is_some_and(|edit| {
            dialog.tab != Tab::Settings || dialog.selected.as_ref() != Some(&edit.id)
        });
        let wants = dialog.tab == Tab::Settings && dialog.edit.is_none();
        let empty = dialog.all.automations.is_empty();
        if stale {
            self.end_automation_edit();
        }
        if stale || wants {
            self.begin_automation_edit(cx);
        }
        // With nothing scheduled there is no list and nothing to choose:
        // the dialog is the one pane that says so.
        let panes = if empty {
            vec![self.automations_empty(cx)]
        } else {
            vec![self.automations_list(cx)?, self.automation_pane(window, cx)]
        };
        let dialog = self.automations.as_ref()?;
        let t = &self.theme;
        Some(
            centered(
                card("automations", DIALOG_WIDTH, t)
                    .track_focus(&dialog.focus)
                    .p(px(0.0))
                    .gap(px(0.0))
                    .flex_row()
                    .max_w(relative(0.94))
                    .h(DIALOG_HEIGHT)
                    .max_h(relative(0.9))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if let Some(dialog) = this.automations.as_mut()
                                && dialog.more.take().is_some()
                            {
                                cx.notify();
                            }
                        }),
                    )
                    .children(panes),
            )
            .into_any_element(),
        )
    }

    /// The list down the left, with its filter and New automation.
    fn automations_list(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.automations.as_ref()?;
        let t = &self.theme;
        let tz = TimeZone::system();
        let now = Timestamp::now();
        let rows = shown(&dialog.all, dialog.project.as_ref(), now, &tz);
        let on = dialog
            .all
            .automations
            .iter()
            .filter(|automation| automation.enabled && !automation.finished())
            .count();
        let paused = dialog
            .all
            .automations
            .iter()
            .filter(|automation| !automation.enabled)
            .count();

        // A project filter, when more than one project has automations.
        let mut projects: Vec<ProjectId> = dialog
            .all
            .automations
            .iter()
            .map(|automation| automation.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        let filter = (projects.len() > 1).then(|| {
            let count = |project: Option<&ProjectId>| {
                dialog
                    .all
                    .automations
                    .iter()
                    .filter(|automation| project.is_none_or(|id| &automation.project == id))
                    .count()
            };
            let all = segment("automations-filter-all", "All")
                .count(count(None).to_string())
                .selected(dialog.project.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(dialog) = this.automations.as_mut() {
                        dialog.project = None;
                    }
                    cx.notify();
                }));
            let each = projects.iter().enumerate().map(|(at, project)| {
                let name = self
                    .projects
                    .iter()
                    .find(|node| &node.id == project)
                    .map_or_else(|| project.as_str().to_owned(), |node| node.name.to_string());
                let picked = project.clone();
                segment(("automations-filter", at), name)
                    .count(count(Some(project)).to_string())
                    .selected(dialog.project.as_ref() == Some(project))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(dialog) = this.automations.as_mut() {
                            dialog.project = Some(picked.clone());
                        }
                        cx.notify();
                    }))
            });
            button_group("automations-filter")
                .fill()
                .small()
                .child(all)
                .children(each)
                .render(t)
        });

        let mut list = div()
            .id("automations-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(2.0))
            .px(px(8.0))
            .overflow_y_scroll();
        let mut group = None;
        for (at, (row_group, automation, next)) in rows.iter().enumerate() {
            if group != Some(*row_group) {
                group = Some(*row_group);
                list = list.child(
                    caption(row_group.title(), t)
                        .px(px(8.0))
                        .pt(px(if at == 0 { 4.0 } else { 12.0 }))
                        .pb(px(4.0)),
                );
            }
            let selected = dialog.selected.as_deref() == Some(automation.id.as_str());
            let (time, day) = match next {
                Some(next) => (time_of(*next, &tz), day_of(*next, &tz)),
                None => (
                    "\u{2014}".to_owned(),
                    if automation.finished() {
                        "Ran"
                    } else {
                        "Paused"
                    }
                    .to_owned(),
                ),
            };
            let quiet = *row_group != Group::ComingUp;
            let ink = paint(if quiet { t.text.dim } else { t.text.primary });
            let last = dialog
                .runs
                .of(&automation.id)
                .next()
                .map(|run| runs::outcome_colour(&run.outcome, t));
            let id = automation.id.clone();
            list = list.child(
                block_row(("automation-row", at), selected, t)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .w(px(58.0))
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ink)
                                            .child(time),
                                    )
                                    .child(caption(day, t)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ink)
                                            .truncate()
                                            .child(automation.name.clone()),
                                    )
                                    .child(caption(detail(automation), t).truncate()),
                            )
                            .children(
                                last.map(|colour| status_dot(colour, Motion::Still, px(7.0))),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(dialog) = this.automations.as_mut() {
                            dialog.selected = Some(id.clone());
                            dialog.confirming_delete = false;
                        }
                        cx.notify();
                    })),
            );
        }
        if rows.is_empty() {
            list = list.child(
                caption(
                    "Nothing scheduled yet. New automation, or Start \u{25be} \u{2192} Schedule\u{2026} in the New worktree dialog, makes one.",
                    t,
                )
                .px(px(8.0))
                .pt(px(8.0))
                .line_height(relative(1.45)),
            );
        }

        let counts = match (on, paused) {
            (0, 0) => None,
            (on, 0) => Some(format!("{on} on")),
            (on, paused) => Some(format!("{on} on \u{b7} {paused} paused")),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .w(LIST_WIDTH)
                .flex_none()
                .h_full()
                .border_r_1()
                .border_color(paint(t.rule))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .px(px(16.0))
                        .pt(px(16.0))
                        .pb(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(crate::ui::TITLE)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(paint(t.text.primary))
                                        .child("Automations"),
                                )
                                .children(counts.map(|counts| caption(counts, t))),
                        )
                        .children(filter),
                )
                .child(list)
                .child(
                    div().flex().p(px(6.0)).child(
                        button("automations-new", "New automation")
                            .leading(Icon::Plus)
                            .small()
                            .render(t)
                            .flex_1()
                            .justify_center()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_automation(cx);
                                cx.notify();
                            })),
                    ),
                )
                .into_any_element(),
        )
    }

    /// The chosen automation: its heading and actions, its figures and its
    /// runs.
    fn automation_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let Some(dialog) = self.automations.as_ref() else {
            return div().into_any_element();
        };
        let closer = close("automations-close", t).on_click(cx.listener(|this, _, _, cx| {
            this.close_automations();
            cx.notify();
        }));
        let Some(automation) = dialog
            .selected
            .as_deref()
            .and_then(|id| dialog.all.get(id))
            .cloned()
        else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .p(px(16.0))
                .child(div().flex().child(div().flex_1()).child(closer))
                .into_any_element();
        };
        let node = self
            .projects
            .iter()
            .find(|node| node.id == automation.project);
        let badge = node.map(|node| {
            let initial: String = node
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(image_badge(node.image.clone(), node.color, initial, t))
                .child(caption(node.name.clone(), t))
        });
        let agent_mark = match &automation.task {
            Task::Prompt {
                agent: Some(agent), ..
            } => mark(agent, t).into_any_element(),
            _ => sized_icon(Icon::Terminal, px(16.0), paint(t.text.dim)).into_any_element(),
        };
        let id = automation.id.clone();
        let on = automation.enabled;
        let toggle = switch("automation-on", on, t).on_click(cx.listener({
            let id = id.clone();
            move |this, _, _, cx| {
                this.change_automation(&id, |automation| automation.set_enabled(!on));
                cx.notify();
            }
        }));
        let run_now = button("automation-run-now", "Run now")
            .leading(Icon::Play)
            .small()
            .render(t)
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| {
                    this.run_automation_now(&id, cx);
                    cx.notify();
                }
            }));
        let more_menu = dialog.more.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, _, cx| {
                    if let Some(dialog) = shell.automations.as_mut() {
                        dialog.more = None;
                        dialog.confirming_delete = true;
                    }
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(dialog) = shell.automations.as_mut() {
                        dialog.more = None;
                    }
                    cx.notify();
                },
            )
        });
        let more = dropdown(
            "automation-more",
            icon_button("automation-more-button", Icon::Ellipsis)
                .bare()
                .render(t),
            more_menu,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_automation_more();
                cx.stop_propagation();
                cx.notify();
            }),
        );

        let actions = if dialog.confirming_delete {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(caption("Delete it and its history? Its worktrees stay.", t))
                .child(
                    button("automation-delete", "Delete")
                        .danger()
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.delete_chosen_automation();
                            cx.notify();
                        })),
                )
                .child(
                    button("automation-keep", "Keep")
                        .ghost()
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(dialog) = this.automations.as_mut() {
                                dialog.confirming_delete = false;
                            }
                            cx.notify();
                        })),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(caption(if on { "On" } else { "Paused" }, t))
                        .child(toggle),
                )
                .child(run_now)
                .child(more)
        };

        let heading = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .h(px(32.0))
            .child(agent_mark)
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(paint(t.text.primary))
                    .truncate()
                    .child(automation.name.clone()),
            )
            .children(badge)
            .child(div().flex_1())
            .child(actions)
            .child(closer);

        let run_count = dialog.runs.of(&automation.id).count();
        let tab = dialog.tab;
        let tabs = button_group("automation-tabs")
            .child(
                segment("automation-tab-runs", "Runs")
                    .count(run_count.to_string())
                    .selected(tab == Tab::Runs)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.automations.as_mut() {
                            dialog.tab = Tab::Runs;
                        }
                        cx.notify();
                    })),
            )
            .child(
                segment("automation-tab-settings", "Settings")
                    .selected(tab == Tab::Settings)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.automations.as_mut() {
                            dialog.tab = Tab::Settings;
                        }
                        cx.notify();
                    })),
            )
            .render(t);
        let body = match tab {
            Tab::Runs => runs::runs_tab(self, &automation, cx),
            Tab::Settings => self
                .automation_settings_tab(window, cx)
                .unwrap_or_else(|| div().into_any_element()),
        };
        let dialog = self.automations.as_ref();
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(24.0))
            .pt(px(16.0))
            .pb(px(16.0))
            .gap(px(16.0))
            .child(heading)
            .child(div().flex().child(tabs))
            .children(
                dialog
                    .filter(|dialog| dialog.tab == Tab::Runs)
                    .and_then(|dialog| dialog.error.clone())
                    .map(|why| caption(why, t).text_color(paint(t.status.failed))),
            )
            .child(body)
            .into_any_element()
    }
}

/// Re-reads the automations and their runs when another process — the
/// host, the CLI, a phone through the host — has written them.
fn reload(dialog: &mut AutomationsDialog) {
    let modified = Automations::modified();
    if dialog.seen != Some(modified) {
        match Automations::load() {
            Ok(all) => {
                dialog.all = all;
                dialog.seen = Some(modified);
            }
            Err(error) => {
                dialog.error = Some(format!("Couldn\u{2019}t read them: {error}").into());
            }
        }
    }
    let Ok(root) = ket_core::paths::automations_dir() else {
        return;
    };
    let modified = Runs::modified_in(&root);
    if dialog.runs_seen != Some(modified)
        && let Ok(runs) = Runs::load_in(&root)
    {
        dialog.runs = runs;
        dialog.runs_seen = Some(modified);
    }
}

#[cfg(test)]
mod tests {
    use ket_core::automation::When;

    use super::*;

    fn made(name: &str, when: When, task: Task) -> Automation {
        let mut automation = Automation::new(&[], ProjectId::new("p"), name, task, when);
        automation.id = name.to_owned();
        automation
    }

    fn command() -> Task {
        Task::Command {
            line: "true".to_owned(),
        }
    }

    #[test]
    fn the_list_is_coming_up_soonest_first_then_paused_then_done() {
        let tz = TimeZone::get("America/Los_Angeles").unwrap();
        let now = Timestamp::now();
        let at = |hour| When::Daily {
            time: Clock::new(hour, 0).unwrap(),
        };
        let hourly = made("hourly", When::Hourly { minute: 59 }, command());
        let mut paused = made("paused", at(9), command());
        paused.enabled = false;
        let mut done = made(
            "done",
            When::Once {
                day: Day::new(now.to_zoned(tz.clone()).date()),
                time: Clock::new(0, 0).unwrap(),
            },
            command(),
        );
        done.handled(now);
        let all = Automations {
            automations: vec![done, paused, hourly],
        };
        let order: Vec<(Group, &str)> = shown(&all, None, now, &tz)
            .into_iter()
            .map(|(group, automation, _)| (group, automation.name.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                (Group::ComingUp, "hourly"),
                (Group::Paused, "paused"),
                (Group::Done, "done")
            ]
        );
        assert!(shown(&all, Some(&ProjectId::new("other")), now, &tz).is_empty());
    }

    #[test]
    fn a_rows_detail_is_how_often_and_what_runs() {
        let weekdays = When::Weekdays {
            time: Clock::new(9, 0).unwrap(),
        };
        assert_eq!(
            detail(&made("a", weekdays.clone(), command())),
            "Weekdays \u{b7} Command"
        );
        let prompt = Task::Prompt {
            text: "x".to_owned(),
            agent: Some("claude".to_owned()),
            model: None,
            effort: None,
        };
        assert!(detail(&made("b", weekdays, prompt)).starts_with("Weekdays \u{b7} "));
    }
}
