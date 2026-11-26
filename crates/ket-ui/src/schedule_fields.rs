//! When an automation starts and where it runs, as a form: Repeat, then Date
//! and Time for a one-off or Time for a repeat, days for chosen days; Runs in
//! and From; and a last line saying when it starts next.
//!
//! One form for both places that ask: the New worktree dialog's Schedule
//! section and the Automations dialog's Settings tab. Each keeps a
//! [`ScheduleFields`] in its own state and hands the functions here an
//! [`Access`] that finds it — so the menus, keys and drawing live once.

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, Window, div, prelude::*, px,
};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
use ket_core::automation::{Automation, Clock, Day, Dow, Place, When};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::input::{Key, Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::ui::chip::caption;
use crate::ui::field::label;
use crate::ui::group::{button_group, segment};
use crate::ui::icon::{Icon, icon, sized_icon};
use crate::ui::menu::{DropdownPanel, MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;

/// How far ↑ and ↓ move the time.
const STEP_MINUTES: i32 = 15;

/// How many days ahead the Date menu offers.
const DAYS_AHEAD: i8 = 14;

/// Finds the form in whichever dialog holds it.
pub(crate) type Access = fn(&mut Shell) -> Option<&mut ScheduleFields>;

/// The same, to read.
pub(crate) type Read = fn(&Shell) -> Option<&ScheduleFields>;

/// How often an automation repeats, as the Repeat menu offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Repeat {
    Once,
    Daily,
    Weekdays,
    Days,
    Hourly,
}

impl Repeat {
    const ALL: [Repeat; 5] = [
        Repeat::Once,
        Repeat::Daily,
        Repeat::Weekdays,
        Repeat::Days,
        Repeat::Hourly,
    ];

    fn label(self) -> &'static str {
        match self {
            Repeat::Once => "Doesn\u{2019}t repeat",
            Repeat::Daily => "Every day",
            Repeat::Weekdays => "Every weekday",
            Repeat::Days => "On chosen days",
            Repeat::Hourly => "Every hour",
        }
    }
}

/// The form's state.
pub(crate) struct ScheduleFields {
    repeat: Repeat,
    repeat_focus: FocusHandle,
    repeat_menu: Option<OpenMenu<Repeat>>,
    /// A one-off's day.
    day: Date,
    day_focus: FocusHandle,
    day_menu: Option<OpenMenu<Date>>,
    /// The time, as typed: `9:00 AM`, or `:05` past the hour.
    time: Entity<TextInput>,
    /// Monday to Sunday, for chosen days.
    days: [bool; 7],
    /// The worktree each run opens a tab in; `None` makes a new one.
    place: Option<WorktreeId>,
    places: Vec<(WorktreeId, String)>,
    place_focus: FocusHandle,
    place_menu: Option<OpenMenu<Option<WorktreeId>>>,
    /// The branch a new worktree starts from.
    base: String,
    bases: Vec<String>,
    base_focus: FocusHandle,
    base_menu: Option<OpenMenu<String>>,
}

impl ScheduleFields {
    /// A fresh form for `project`: every weekday at nine, in a new worktree
    /// from the project's own base.
    pub(crate) fn new(project: &ProjectId, cx: &mut Context<Shell>) -> Self {
        let (places, bases, base) = choices(project);
        let time = TextInput::new("9:00 AM", cx);
        time.update(cx, |input, _| input.set_text("9:00 AM"));
        cx.observe(&time, |_, _, cx| cx.notify()).detach();
        let today = Timestamp::now().to_zoned(TimeZone::system()).date();
        Self {
            repeat: Repeat::Weekdays,
            repeat_focus: cx.focus_handle(),
            repeat_menu: None,
            day: today.tomorrow().unwrap_or(today),
            day_focus: cx.focus_handle(),
            day_menu: None,
            time,
            days: [true, false, true, false, true, false, false],
            place: None,
            places,
            place_focus: cx.focus_handle(),
            place_menu: None,
            base,
            bases,
            base_focus: cx.focus_handle(),
            base_menu: None,
        }
    }

    /// The form as `automation` already is.
    pub(crate) fn of(automation: &Automation, cx: &mut Context<Shell>) -> Self {
        let mut fields = Self::new(&automation.project, cx);
        let (repeat, time) = match &automation.when {
            When::Once { day, time } => {
                fields.day = day.date();
                (Repeat::Once, time.label())
            }
            When::Daily { time } => (Repeat::Daily, time.label()),
            When::Weekdays { time } => (Repeat::Weekdays, time.label()),
            When::Days { days, time } => {
                fields.days = Dow::ALL.map(|day| days.contains(&day));
                (Repeat::Days, time.label())
            }
            When::Hourly { minute } => (Repeat::Hourly, format!(":{minute:02}")),
        };
        fields.repeat = repeat;
        fields.time.update(cx, |input, _| input.set_text(&time));
        match &automation.place {
            Place::NewWorktree { base } => {
                if let Some(base) = base {
                    fields.base.clone_from(base);
                }
            }
            Place::Worktree { id } => fields.place = Some(id.clone()),
        }
        fields
    }

    /// The schedule the form makes, or why it makes none.
    pub(crate) fn when(&self, cx: &App) -> Result<When, String> {
        when_of(self.repeat, self.day, self.days, &self.time.read(cx).text())
    }

    /// Where each run works.
    pub(crate) fn place(&self) -> Place {
        match &self.place {
            Some(id) => Place::Worktree { id: id.clone() },
            None => Place::NewWorktree {
                base: Some(self.base.clone()),
            },
        }
    }

    /// The same form for another project: its worktrees and branches, from
    /// its own base, in a new worktree — a worktree of the last project is
    /// never where this one runs.
    pub(crate) fn retarget(&mut self, project: &ProjectId) {
        let (places, bases, base) = choices(project);
        self.places = places;
        self.bases = bases;
        self.base = base;
        self.place = None;
        self.close_menus();
    }

    /// Closes its menus: one dropdown open at a time.
    pub(crate) fn close_menus(&mut self) {
        self.repeat_menu = None;
        self.day_menu = None;
        self.place_menu = None;
        self.base_menu = None;
    }

    /// Whether one of its controls has the keyboard.
    pub(crate) fn focused(&self, window: &Window, cx: &App) -> bool {
        self.time.read(cx).is_focused(window)
            || [
                &self.repeat_focus,
                &self.day_focus,
                &self.place_focus,
                &self.base_focus,
            ]
            .iter()
            .any(|handle| handle.is_focused(window))
    }

    /// Its controls in Tab's order, top to bottom.
    pub(crate) fn tab_order(&self, cx: &App) -> Vec<FocusHandle> {
        let mut order = vec![self.repeat_focus.clone()];
        if self.repeat == Repeat::Once {
            order.push(self.day_focus.clone());
        }
        order.push(self.time.read(cx).focus_handle().clone());
        order.push(self.place_focus.clone());
        if self.place.is_none() {
            order.push(self.base_focus.clone());
        }
        order
    }
}

/// `project`'s worktrees, for Runs in; its branches, for From; and its own
/// base.
fn choices(project: &ProjectId) -> (Vec<(WorktreeId, String)>, Vec<String>, String) {
    Workspace::open()
        .ok()
        .and_then(|workspace| {
            let project = workspace
                .projects()
                .ok()?
                .into_iter()
                .find(|p| &p.id == project)?;
            let places = workspace
                .worktrees(Some(&project.id))
                .unwrap_or_default()
                .into_iter()
                .map(|worktree| {
                    let label = worktree
                        .name
                        .clone()
                        .unwrap_or_else(|| worktree.branch.clone());
                    (worktree.id, label)
                })
                .collect();
            let bases = ket_core::git::Git::new(&project.root)
                .local_branches()
                .unwrap_or_default();
            Some((places, bases, project.default_base))
        })
        .unwrap_or_else(|| (Vec::new(), Vec::new(), "main".to_owned()))
}

/// The schedule the choices make, or why they make none.
pub(crate) fn when_of(
    repeat: Repeat,
    day: Date,
    days: [bool; 7],
    typed: &str,
) -> Result<When, String> {
    if repeat == Repeat::Hourly {
        let minute = typed
            .trim()
            .trim_start_matches(':')
            .parse::<u8>()
            .ok()
            .filter(|minute| *minute < 60)
            .or_else(|| Clock::parse(typed).map(Clock::minute))
            .ok_or_else(|| {
                format!("\u{201c}{typed}\u{201d} isn\u{2019}t minutes past the hour; try :05")
            })?;
        return Ok(When::Hourly { minute });
    }
    let time = Clock::parse(typed)
        .ok_or_else(|| format!("\u{201c}{typed}\u{201d} isn\u{2019}t a time; try 9:00 AM"))?;
    let when = match repeat {
        Repeat::Once => When::Once {
            day: Day::new(day),
            time,
        },
        Repeat::Daily | Repeat::Hourly => When::Daily { time },
        Repeat::Weekdays => When::Weekdays { time },
        Repeat::Days => When::Days {
            days: Dow::ALL
                .into_iter()
                .zip(days)
                .filter_map(|(day, on)| on.then_some(day))
                .collect(),
            time,
        },
    };
    when.check()?;
    Ok(when)
}

/// `Fri 20 Nov, 9:00 AM`.
pub(crate) fn moment(at: &Zoned) -> String {
    let time = u8::try_from(at.hour())
        .ok()
        .zip(u8::try_from(at.minute()).ok())
        .and_then(|(hour, minute)| Clock::new(hour, minute))
        .map(Clock::label)
        .unwrap_or_default();
    format!("{}, {time}", Day::new(at.date()).label())
}

/// The form's last two lines: when it starts next, and what then.
fn summary(when: &When, tz: &TimeZone) -> (String, String) {
    let Some(next) = when.next_after(Timestamp::now(), tz) else {
        return (
            "That time has passed.".to_owned(),
            "Pick a later day or time.".to_owned(),
        );
    };
    let next = moment(&next.to_zoned(tz.clone()));
    if when.is_once() {
        return (
            format!("Starts once, {next}."),
            "If the Mac is asleep then, it starts when it wakes, up to 12 hours late.".to_owned(),
        );
    }
    (
        format!("Next start: {next}."),
        format!(
            "Then {}. While a run is still working, the next one is skipped.",
            lowercase_first(&when.describe())
        ),
    )
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// A label over its control, as every dialog's fields are.
pub(crate) fn labelled(title: &'static str, control: AnyElement, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .child(label(title, t))
        .child(control)
}

impl Shell {
    fn toggle_repeat_menu_in(&mut self, access: Access, width: gpui::Pixels) {
        let Some(fields) = access(self) else {
            return;
        };
        let open = fields.repeat_menu.is_some();
        fields.close_menus();
        if open {
            return;
        }
        let current = fields.repeat;
        let entries = Repeat::ALL
            .into_iter()
            .map(|repeat| {
                let item = MenuItem::new(repeat, repeat.label());
                MenuEntry::Item(if repeat == current {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        fields.repeat_menu = Some(
            OpenMenu::new("schedule-repeat".into(), None, width, entries)
                .selected(Repeat::ALL.iter().position(|r| *r == current).unwrap_or(0))
                .offset(px(0.0), px(6.0)),
        );
    }

    fn set_repeat_in(&mut self, access: Access, repeat: Repeat, cx: &mut Context<Self>) {
        let Some(fields) = access(self) else {
            return;
        };
        let was = fields.repeat;
        fields.repeat = repeat;
        fields.repeat_menu = None;
        // Moving into or out of hourly, the field means something else.
        let text = match (was == Repeat::Hourly, repeat == Repeat::Hourly) {
            (false, true) => Some(":00"),
            (true, false) => Some("9:00 AM"),
            _ => None,
        };
        if let Some(text) = text {
            fields.time.update(cx, |input, _| input.set_text(text));
        }
    }

    fn toggle_day_menu_in(&mut self, access: Access, width: gpui::Pixels) {
        let Some(fields) = access(self) else {
            return;
        };
        let open = fields.day_menu.is_some();
        fields.close_menus();
        if open {
            return;
        }
        let current = fields.day;
        let today = Timestamp::now().to_zoned(TimeZone::system()).date();
        let days: Vec<Date> = std::iter::successors(Some(today), |day| day.tomorrow().ok())
            .take(usize::try_from(DAYS_AHEAD).unwrap_or(14))
            .collect();
        let entries = days
            .iter()
            .enumerate()
            .map(|(n, day)| {
                let label = match n {
                    0 => format!("Today \u{b7} {}", Day::new(*day).label()),
                    1 => format!("Tomorrow \u{b7} {}", Day::new(*day).label()),
                    _ => Day::new(*day).label(),
                };
                let item = MenuItem::new(*day, label);
                MenuEntry::Item(if *day == current {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        let selected = days.iter().position(|day| *day == current).unwrap_or(0);
        fields.day_menu = Some(
            OpenMenu::new("schedule-day".into(), None, width, entries)
                .selected(selected)
                .offset(px(0.0), px(6.0)),
        );
    }

    fn toggle_place_menu_in(&mut self, access: Access, width: gpui::Pixels) {
        let Some(fields) = access(self) else {
            return;
        };
        let open = fields.place_menu.is_some();
        fields.close_menus();
        if open {
            return;
        }
        let current = fields.place.clone();
        let fresh = MenuItem::new(None, "A new worktree each run").icon(Icon::Plus);
        let mut entries = vec![MenuEntry::Item(if current.is_none() {
            fresh.checked()
        } else {
            fresh
        })];
        entries.extend(fields.places.iter().map(|(id, label)| {
            let item = MenuItem::new(Some(id.clone()), label.clone()).icon(Icon::GitBranch);
            MenuEntry::Item(if current.as_ref() == Some(id) {
                item.checked()
            } else {
                item
            })
        }));
        fields.place_menu = Some(
            OpenMenu::new(
                "schedule-place".into(),
                Some("Find a worktree\u{2026}".into()),
                width,
                entries,
            )
            .offset(px(0.0), px(6.0)),
        );
    }

    fn toggle_base_menu_in(&mut self, access: Access, width: gpui::Pixels) {
        let Some(fields) = access(self) else {
            return;
        };
        let open = fields.base_menu.is_some();
        fields.close_menus();
        if open || fields.bases.is_empty() {
            return;
        }
        let current = fields.base.clone();
        let selected = fields
            .bases
            .iter()
            .position(|base| *base == current)
            .unwrap_or(0);
        let entries = fields
            .bases
            .iter()
            .map(|base| {
                let item = MenuItem::new(base.clone(), base.clone()).icon(Icon::GitBranch);
                MenuEntry::Item(if *base == current {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        fields.base_menu = Some(
            OpenMenu::new(
                "schedule-base".into(),
                Some("Find a branch\u{2026}".into()),
                width,
                entries,
            )
            .selected(selected)
            .offset(px(0.0), px(6.0)),
        );
    }

    /// Lets the form's open menu take a key first. Returns whether one did.
    pub(crate) fn schedule_fields_menu_key(
        &mut self,
        access: Access,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(fields) = access(self) else {
            return false;
        };
        if let Some(chosen) = menu_key(&mut fields.repeat_menu, event) {
            if let Some(repeat) = chosen {
                self.set_repeat_in(access, repeat, cx);
            }
            return true;
        }
        if let Some(chosen) = menu_key(&mut fields.day_menu, event) {
            if let Some(day) = chosen {
                fields.day = day;
            }
            return true;
        }
        if let Some(chosen) = menu_key(&mut fields.place_menu, event) {
            if let Some(place) = chosen {
                fields.place = place;
            }
            return true;
        }
        if let Some(chosen) = menu_key(&mut fields.base_menu, event) {
            if let Some(base) = chosen {
                fields.base = base;
            }
            return true;
        }
        false
    }

    /// The form's keys: typing in the time and ↑ ↓ on it, Enter, Space or
    /// an arrow on a picker. `None` when the form doesn't have the keyboard,
    /// or when the key is the dialog's — Escape, Tab, ⌘⏎, and Enter in the
    /// time, which saves.
    pub(crate) fn schedule_fields_key(
        &mut self,
        access: Access,
        width: gpui::Pixels,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Key> {
        let fields = access(self)?;
        let key = &event.keystroke;
        let time = fields.time.clone();
        let hourly = fields.repeat == Repeat::Hourly;
        let on_time = time.read(cx).is_focused(window);
        let on_repeat = fields.repeat_focus.is_focused(window);
        let on_day = fields.day_focus.is_focused(window);
        let on_place = fields.place_focus.is_focused(window);
        let on_base = fields.base_focus.is_focused(window);
        if !(on_time || on_repeat || on_day || on_place || on_base) {
            return None;
        }
        let command = key.modifiers.platform || key.modifiers.control;
        if key.key == "escape" || key.key == "tab" || (key.key == "enter" && command) {
            return None;
        }
        if on_time {
            if matches!(key.key.as_str(), "up" | "down") {
                let typed = time.read(cx).text();
                let by = if key.key == "up" {
                    STEP_MINUTES
                } else {
                    -STEP_MINUTES
                };
                let next = if hourly {
                    let minute = typed
                        .trim()
                        .trim_start_matches(':')
                        .parse::<i32>()
                        .unwrap_or(0);
                    let stepped = minute - minute.rem_euclid(5) + by.signum() * 5;
                    format!(":{:02}", stepped.rem_euclid(60))
                } else {
                    Clock::parse(&typed)
                        .or_else(|| Clock::new(9, 0))
                        .map(|clock| clock.step(by).label())
                        .unwrap_or(typed)
                };
                time.update(cx, |input, _| input.set_text(&next));
                return Some(Key::Taken);
            }
            if key.key == "enter" {
                return None;
            }
            if is_text(key) {
                return Some(Key::Text);
            }
            time.update(cx, |input, cx| input.key(key, cx));
            return Some(Key::Taken);
        }
        if matches!(key.key.as_str(), "enter" | "space" | "up" | "down") {
            if on_repeat {
                self.toggle_repeat_menu_in(access, width);
            } else if on_day {
                self.toggle_day_menu_in(access, width);
            } else if on_place {
                self.toggle_place_menu_in(access, width);
            } else if on_base {
                self.toggle_base_menu_in(access, width);
            }
        }
        // Anything else on a picker goes no further: the dialog would take
        // it for its prompt's.
        Some(Key::Taken)
    }

    /// Draws the form `form_width` wide: the When row, the days, the Where
    /// row and the summary.
    pub(crate) fn schedule_fields_view(
        &mut self,
        access: Access,
        read: Read,
        form_width: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let fields = read(self)?;
        let t = &self.theme;
        let dim = paint(t.text.dim);
        let gap = px(10.0);
        let third = (form_width - gap * 2.0) / 3.0;
        let half = (form_width - gap) / 2.0;
        let menu_width = half;

        let repeat_menu = fields.repeat_menu.as_ref().map(|menu| {
            menu.dropdown_view(
                t,
                cx,
                move |shell, repeat, cx| {
                    shell.set_repeat_in(access, *repeat, cx);
                    cx.notify();
                },
                move |shell, cx| {
                    if let Some(fields) = access(shell) {
                        fields.repeat_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let repeat = picker(
            "schedule-repeat",
            fields.repeat.label().into(),
            Some(icon(Icon::Repeat, dim).into_any_element()),
            &fields.repeat_focus,
            repeat_menu,
            t,
            window,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window: &mut Window, cx| {
                if let Some(fields) = access(this) {
                    window.focus(&fields.repeat_focus.clone());
                }
                this.toggle_repeat_menu_in(access, menu_width);
                cx.stop_propagation();
                cx.notify();
            }),
        );

        let day_menu = fields.day_menu.as_ref().map(|menu| {
            menu.dropdown_view(
                t,
                cx,
                move |shell, day, cx| {
                    if let Some(fields) = access(shell) {
                        fields.day = *day;
                        fields.day_menu = None;
                    }
                    cx.notify();
                },
                move |shell, cx| {
                    if let Some(fields) = access(shell) {
                        fields.day_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let day = picker(
            "schedule-day",
            Day::new(fields.day).label().into(),
            Some(icon(Icon::Calendar, dim).into_any_element()),
            &fields.day_focus,
            day_menu,
            t,
            window,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window: &mut Window, cx| {
                if let Some(fields) = access(this) {
                    window.focus(&fields.day_focus.clone());
                }
                this.toggle_day_menu_in(access, menu_width);
                cx.stop_propagation();
                cx.notify();
            }),
        );

        let hourly = fields.repeat == Repeat::Hourly;
        let time_input = fields.time.clone();
        let time = text_field(
            &time_input,
            "schedule-time",
            false,
            Style::new(t, self.caret.visible)
                .leading(Icon::Clock)
                .trailing(caption("\u{2191}\u{2193}", t)),
            window,
            cx,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.notify()));
        let time = labelled(
            if hourly { "At minute" } else { "Time" },
            time.into_any_element(),
            t,
        );

        let when_row = if fields.repeat == Repeat::Once {
            div()
                .flex()
                .gap(gap)
                .child(
                    labelled("Repeat", repeat.into_any_element(), t)
                        .w(third)
                        .flex_none(),
                )
                .child(
                    labelled("Date", day.into_any_element(), t)
                        .w(third)
                        .flex_none(),
                )
                .child(time.w(third).flex_none())
        } else {
            div()
                .flex()
                .gap(gap)
                .child(
                    labelled("Repeat", repeat.into_any_element(), t)
                        .w(half)
                        .flex_none(),
                )
                .child(time.w(half).flex_none())
        };

        let days = (fields.repeat == Repeat::Days).then(|| {
            button_group("schedule-days")
                .fill()
                .children(Dow::ALL.into_iter().enumerate().map(|(at, day)| {
                    let on = fields.days.get(at).copied().unwrap_or(false);
                    segment(("schedule-day-of-week", at), day.short())
                        .selected(on)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(fields) = access(this)
                                && let Some(day) = fields.days.get_mut(at)
                            {
                                *day = !*day;
                            }
                            cx.notify();
                        }))
                }))
                .render(t)
        });

        let place_label: SharedString = match &fields.place {
            None => "A new worktree each run".into(),
            Some(id) => fields
                .places
                .iter()
                .find(|(place, _)| place == id)
                .map_or_else(|| id.as_str().to_owned(), |(_, label)| label.clone())
                .into(),
        };
        let place_menu = fields.place_menu.as_ref().map(|menu| {
            menu.dropdown_view(
                t,
                cx,
                move |shell, place, cx| {
                    if let Some(fields) = access(shell) {
                        fields.place.clone_from(place);
                        fields.place_menu = None;
                    }
                    cx.notify();
                },
                move |shell, cx| {
                    if let Some(fields) = access(shell) {
                        fields.place_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        let place = picker(
            "schedule-place",
            place_label,
            None,
            &fields.place_focus,
            place_menu,
            t,
            window,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window: &mut Window, cx| {
                if let Some(fields) = access(this) {
                    window.focus(&fields.place_focus.clone());
                }
                this.toggle_place_menu_in(access, menu_width);
                cx.stop_propagation();
                cx.notify();
            }),
        );
        let base = fields.place.is_none().then(|| {
            let base_menu = fields.base_menu.as_ref().map(|menu| {
                menu.dropdown_view(
                    t,
                    cx,
                    move |shell, base, cx| {
                        if let Some(fields) = access(shell) {
                            fields.base.clone_from(base);
                            fields.base_menu = None;
                        }
                        cx.notify();
                    },
                    move |shell, cx| {
                        if let Some(fields) = access(shell) {
                            fields.base_menu = None;
                        }
                        cx.notify();
                    },
                )
            });
            picker(
                "schedule-base",
                fields.base.clone().into(),
                Some(icon(Icon::GitBranch, dim).into_any_element()),
                &fields.base_focus,
                base_menu,
                t,
                window,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window: &mut Window, cx| {
                    if let Some(fields) = access(this) {
                        window.focus(&fields.base_focus.clone());
                    }
                    this.toggle_base_menu_in(access, menu_width);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
        });
        let where_row = div()
            .flex()
            .gap(gap)
            .child(
                labelled("Runs in", place.into_any_element(), t)
                    .w(if base.is_some() { half } else { form_width })
                    .flex_none(),
            )
            .children(base.map(|base| {
                labelled("From", base.into_any_element(), t)
                    .w(half)
                    .flex_none()
            }));

        let (said, then) = match fields.when(cx) {
            Ok(when) => summary(&when, &TimeZone::system()),
            Err(why) => (why, String::new()),
        };
        let summary_box = summary_box(said, then, t);

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(when_row)
                .children(days)
                .child(where_row)
                .child(summary_box)
                .into_any_element(),
        )
    }
}

/// A form's last lines, in a ruled box under an arrow: what the settings
/// above it come to — when it starts next — and, dimmer, what then. Shared
/// with Settings → Devices → Blackout, whose hours end the same way.
pub(crate) fn summary_box(said: String, then: String, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .gap(px(10.0))
        .items_start()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(crate::ui::RADIUS_MD)
        .border_1()
        .border_color(paint(t.border))
        .child(sized_icon(
            Icon::ArrowRight,
            px(16.0),
            paint(t.text.primary),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(paint(t.text.primary))
                        .child(said),
                )
                .when(!then.is_empty(), |el| {
                    el.child(caption(then, t).text_size(px(12.0)))
                }),
        )
}

/// A select over its menu, keeping the keyboard on `focus`.
fn picker(
    id: &'static str,
    value: SharedString,
    leading: Option<AnyElement>,
    focus: &FocusHandle,
    menu: Option<DropdownPanel>,
    t: &Theme,
    window: &Window,
) -> gpui::Stateful<gpui::Div> {
    let mut trigger = select(id, value)
        .open(menu.is_some())
        .focused(focus.is_focused(window));
    if let Some(leading) = leading {
        trigger = trigger.leading(leading);
    }
    dropdown(
        SharedString::from(format!("{id}-select")),
        trigger.render(t),
        menu,
    )
    .track_focus(focus)
    .cursor_pointer()
}

/// A menu's answer to a key, as the dialogs' own `dropdown_key`: `None`
/// when it is closed or let the key pass.
fn menu_key<T: Clone + 'static>(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_make_a_schedule_or_say_why_not() {
        let day = Date::new(2026, 11, 20).unwrap();
        let none = [false; 7];
        let describe =
            |repeat, typed: &str| when_of(repeat, day, none, typed).map(|w| w.describe());
        assert_eq!(
            describe(Repeat::Daily, "2am").unwrap(),
            "Every day at 2:00 AM"
        );
        assert_eq!(
            describe(Repeat::Weekdays, "9:00 AM").unwrap(),
            "Weekdays at 9:00 AM"
        );
        assert_eq!(
            describe(Repeat::Once, "9am").unwrap(),
            "Once, Fri 20 Nov at 9:00 AM"
        );
        assert_eq!(
            describe(Repeat::Hourly, ":05").unwrap(),
            "Every hour at :05"
        );
        assert!(
            describe(Repeat::Daily, "soon")
                .unwrap_err()
                .contains("isn\u{2019}t a time")
        );
        assert!(describe(Repeat::Days, "9am").is_err(), "no day chosen");
        let mon_fri = [true, false, false, false, true, false, false];
        assert_eq!(
            when_of(Repeat::Days, day, mon_fri, "6pm")
                .unwrap()
                .describe(),
            "Mon and Fri at 6:00 PM"
        );
    }
}
