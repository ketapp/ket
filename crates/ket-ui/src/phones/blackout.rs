//! Settings → Devices → Blackout: the hours ket is shut off on every paired
//! device, and letting them back in.
//!
//! The setting is `ket_core::host::blackout`'s: the hours in the config,
//! and until when devices are let in in a file of the host's. The host
//! watches both, so every change here is written at once and taken up within
//! a second, with nothing to send it and nothing to wait for. A switch, a choice of days and where notifications go take effect as
//! they are thrown. The two times take effect on Return, or on ↑ and ↓ — not
//! as they are typed, because `1` on the way to `10 pm` is one in the
//! morning, and saving that would start a blackout.
//!
//! The form is the Automations schedule's, part for part: a label over each
//! control, a Repeat select, times typed into a field with a clock, the days
//! as a group, and the ruled line saying when it starts next.

use gpui::{
    AnyElement, Context, Entity, KeyDownEvent, MouseButton, SharedString, Window, div, prelude::*,
    px,
};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use ket_core::automation::{Clock, Dow};
use ket_core::host::blackout::{self, Blackout, Repeat, Standing};
use ket_core::theme::Theme;

use super::{PhonesPane, group_box, group_label};
use crate::Shell;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::preferences::Entry;
use crate::schedule_fields::{labelled, moment, summary_box};
use crate::ui::banner::banner;
use crate::ui::button::button;
use crate::ui::chip::caption;
use crate::ui::field::error;
use crate::ui::group::{button_group, segment};
use crate::ui::icon::{Icon, icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::row::sized_row;
use crate::ui::select::select;
use crate::ui::toast::Tone;
use crate::ui::toggle::switch;

/// How far ↑ and ↓ move a time, as they do an automation's.
const STEP_MINUTES: i32 = 15;

/// How wide the Repeat menu opens.
const REPEAT_MENU_W: gpui::Pixels = px(220.0);

/// How wide the Let them in menu opens.
const LET_IN_MENU_W: gpui::Pixels = px(232.0);

/// One of the two times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    From,
    Until,
}

/// What the Let them in menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LetIn {
    /// For this many minutes.
    Minutes(i64),
    /// Until the blackout on now ends.
    Rest,
    /// No more blackouts: the switch, thrown from here.
    TurnOff,
}

/// The group's state, kept on the Devices pane and dropped with it.
pub(super) struct BlackoutPane {
    /// The setting as it is on disk.
    saved: Blackout,
    /// The times as typed, which are the saved ones until Return.
    from: Entity<TextInput>,
    until: Entity<TextInput>,
    /// Why what was typed was not taken.
    error: Option<SharedString>,
    /// Whether Repeat was set to chosen days here: the days stay on show
    /// then, even while those chosen happen to be every day or the weekdays.
    choosing: bool,
    repeat_menu: Option<OpenMenu<Repeat>>,
    let_in_menu: Option<OpenMenu<LetIn>>,
}

impl BlackoutPane {
    fn new(saved: Blackout, cx: &mut Context<Shell>) -> Self {
        let from = TextInput::with_text("10:00 PM", &saved.from.label(), cx);
        let until = TextInput::with_text("7:00 AM", &saved.until.label(), cx);
        Self {
            choosing: saved.repeat() == Repeat::Days,
            saved,
            from,
            until,
            error: None,
            repeat_menu: None,
            let_in_menu: None,
        }
    }

    /// Closes its menus. Returns whether one was open.
    pub(super) fn close_menus(&mut self) -> bool {
        self.repeat_menu.take().is_some() | self.let_in_menu.take().is_some()
    }

    /// How the blackout stands now, by the saved setting.
    fn standing(&self) -> Standing {
        self.saved.standing(Timestamp::now(), &TimeZone::system())
    }

    /// The time shutting devices out now ends at, in words; `None` when
    /// they are not shut out. For a device's row.
    pub(super) fn shut_until(&self) -> Option<String> {
        self.standing()
            .shut_until()
            .map(|until| time_of(until, &TimeZone::system()))
    }

    fn input(&self, edge: Edge) -> &Entity<TextInput> {
        match edge {
            Edge::From => &self.from,
            Edge::Until => &self.until,
        }
    }

    /// The days a blackout starts on, Monday to Sunday.
    fn days(&self) -> [bool; 7] {
        Dow::ALL.map(|day| self.saved.days.is_empty() || self.saved.days.contains(&day))
    }
}

/// `9:00 AM`, on this Mac's clock.
fn time_of(at: Timestamp, tz: &TimeZone) -> String {
    let at = at.to_zoned(tz.clone());
    u8::try_from(at.hour())
        .ok()
        .zip(u8::try_from(at.minute()).ok())
        .and_then(|(hour, minute)| Clock::new(hour, minute))
        .map(Clock::label)
        .unwrap_or_default()
}

/// `9 hours`, `1 hour 30 minutes`, `45 minutes`.
fn length(minutes: u32) -> String {
    let unit = |count: u32, name: &str| match count {
        1 => format!("1 {name}"),
        count => format!("{count} {name}s"),
    };
    match (minutes / 60, minutes % 60) {
        (0, minutes) => unit(minutes, "minute"),
        (hours, 0) => unit(hours, "hour"),
        (hours, minutes) => format!("{} {}", unit(hours, "hour"), unit(minutes, "minute")),
    }
}

fn repeat_label(repeat: Repeat) -> &'static str {
    match repeat {
        Repeat::Daily => "Every day",
        Repeat::Weekdays => "Every weekday",
        Repeat::Days => "On chosen days",
    }
}

const REPEATS: [Repeat; 3] = [Repeat::Daily, Repeat::Weekdays, Repeat::Days];

impl Shell {
    fn blackout_pane(&mut self) -> Option<&mut BlackoutPane> {
        self.phones_pane()?.blackout.as_mut()
    }

    /// Call as the Devices section is shown: reads the setting, which
    /// another window may have changed, and makes the group's fields the
    /// first time.
    pub(super) fn blackout_shown(&mut self, cx: &mut Context<Self>) {
        if self.phones_pane().is_none() {
            return;
        }
        // The hours are the config's, read from disk again rather than
        // trusted to the copy this settings view opened with: another
        // window's Settings may have set them since, and this copy is what
        // the view's next save writes. Until when devices are let in is the
        // host's own file.
        let fresh = ket_core::config::Config::load()
            .ok()
            .map(|config| config.phones.blackout);
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        if let Some(fresh) = fresh {
            view.config.phones.blackout = fresh;
        }
        let mut saved = view.config.phones.blackout.clone();
        saved.let_in_until = match blackout::let_in() {
            Ok(seconds) => seconds,
            Err(why) => {
                self.toast_detail(
                    Tone::Error,
                    "Could not read whether devices are let in",
                    why.to_string(),
                    cx,
                );
                0
            }
        };
        if let Some(pane) = self.blackout_pane() {
            if pane.saved != saved {
                let (from, until) = (saved.from.label(), saved.until.label());
                pane.from.update(cx, |input, _| input.set_text(&from));
                pane.until.update(cx, |input, _| input.set_text(&until));
                pane.choosing = saved.repeat() == Repeat::Days;
                pane.saved = saved;
            }
            return;
        }
        let pane = BlackoutPane::new(saved, cx);
        self.watch_field(&pane.from, cx);
        self.watch_field(&pane.until, cx);
        if let Some(phones) = self.phones_pane() {
            phones.blackout = Some(pane);
        }
    }

    /// Changes the setting and writes it: the hours with the config the
    /// settings view holds, until when devices are let in to the host's
    /// file. The host takes both up from disk. What could not be written is
    /// not kept here either.
    fn change_blackout(&mut self, change: impl FnOnce(&mut Blackout), cx: &mut Context<Self>) {
        let Some(view) = self.preferences.as_mut() else {
            return;
        };
        let Some(pane) = view.phones.blackout.as_mut() else {
            return;
        };
        let mut next = pane.saved.clone();
        change(&mut next);
        if next == pane.saved {
            return;
        }
        let hours = |blackout: &Blackout| Blackout {
            let_in_until: 0,
            ..blackout.clone()
        };
        let mut failed = None;
        if hours(&next) != hours(&pane.saved) {
            // A config that could not be read is not written over.
            if !view.writable {
                return;
            }
            let was = std::mem::replace(&mut view.config.phones.blackout, hours(&next));
            match view.config.save() {
                Ok(()) => {
                    pane.saved = Blackout {
                        let_in_until: pane.saved.let_in_until,
                        ..next.clone()
                    };
                }
                Err(why) => {
                    view.config.phones.blackout = was;
                    failed = Some(why.to_string());
                }
            }
        }
        if failed.is_none() && next.let_in_until != pane.saved.let_in_until {
            match blackout::set_let_in(next.let_in_until) {
                Ok(()) => pane.saved.let_in_until = next.let_in_until,
                Err(why) => failed = Some(why.to_string()),
            }
        }
        if let Some(why) = failed {
            self.toast_detail(Tone::Error, "Could not save the blackout hours", why, cx);
        }
        cx.notify();
    }

    /// Takes the two times as typed, if both are times, and shows them the
    /// way every surface shows a time.
    fn commit_blackout_times(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        let typed = (
            Clock::parse(&pane.from.read(cx).text()),
            Clock::parse(&pane.until.read(cx).text()),
        );
        let (Some(from), Some(until)) = typed else {
            pane.error = Some("That isn\u{2019}t a time. Try 10:00 PM, 10pm or 22:00.".into());
            cx.notify();
            return;
        };
        pane.error = None;
        let (from_input, until_input) = (pane.from.clone(), pane.until.clone());
        from_input.update(cx, |input, _| input.set_text(&from.label()));
        until_input.update(cx, |input, _| input.set_text(&until.label()));
        self.change_blackout(
            |blackout| {
                blackout.from = from;
                blackout.until = until;
            },
            cx,
        );
        cx.notify();
    }

    /// ↑ or ↓ on a time: a quarter of an hour later or earlier, and taken.
    fn step_blackout_time(&mut self, edge: Edge, later: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        let input = pane.input(edge).clone();
        let saved = match edge {
            Edge::From => pane.saved.from,
            Edge::Until => pane.saved.until,
        };
        let by = if later { STEP_MINUTES } else { -STEP_MINUTES };
        let next = Clock::parse(&input.read(cx).text())
            .unwrap_or(saved)
            .step(by);
        input.update(cx, |input, _| input.set_text(&next.label()));
        self.commit_blackout_times(cx);
    }

    fn set_blackout_repeat(&mut self, repeat: Repeat, cx: &mut Context<Self>) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        pane.repeat_menu = None;
        pane.choosing = repeat == Repeat::Days;
        // Chosen days start from the days in force, to be taken away from.
        let days: Vec<Dow> = match repeat {
            Repeat::Daily => Vec::new(),
            Repeat::Weekdays => Dow::ALL
                .into_iter()
                .filter(|day| day.is_weekday())
                .collect(),
            Repeat::Days => Dow::ALL
                .into_iter()
                .zip(pane.days())
                .filter_map(|(day, on)| on.then_some(day))
                .collect(),
        };
        self.change_blackout(|blackout| blackout.days = days, cx);
        cx.notify();
    }

    /// Turns one day on or off. The last one stays: no days is no blackout,
    /// which is the switch's to say.
    fn toggle_blackout_day(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        let mut on = pane.days();
        let Some(day) = on.get_mut(at) else {
            return;
        };
        *day = !*day;
        let days: Vec<Dow> = Dow::ALL
            .into_iter()
            .zip(on)
            .filter_map(|(day, on)| on.then_some(day))
            .collect();
        if days.is_empty() {
            return;
        }
        self.change_blackout(|blackout| blackout.days = days, cx);
    }

    fn toggle_blackout_repeat_menu(&mut self) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        let open = pane.repeat_menu.is_some();
        pane.close_menus();
        if open {
            return;
        }
        let current = if pane.choosing {
            Repeat::Days
        } else {
            pane.saved.repeat()
        };
        let entries = REPEATS
            .into_iter()
            .map(|repeat| {
                let item = MenuItem::new(repeat, repeat_label(repeat));
                MenuEntry::Item(if repeat == current {
                    item.checked()
                } else {
                    item
                })
            })
            .collect();
        pane.repeat_menu = Some(
            OpenMenu::new("blackout-repeat".into(), None, REPEAT_MENU_W, entries)
                .selected(REPEATS.iter().position(|r| *r == current).unwrap_or(0))
                .offset(px(0.0), px(6.0)),
        );
    }

    fn toggle_let_in_menu(&mut self) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        let open = pane.let_in_menu.is_some();
        pane.close_menus();
        if open {
            return;
        }
        let entries = vec![
            MenuEntry::Heading("Let every device in".into()),
            MenuEntry::Item(MenuItem::new(LetIn::Minutes(15), "For 15 minutes")),
            MenuEntry::Item(MenuItem::new(LetIn::Minutes(60), "For an hour")),
            MenuEntry::Item(MenuItem::new(LetIn::Rest, "For the rest of this blackout")),
            MenuEntry::Separator,
            MenuEntry::Item(MenuItem::new(LetIn::TurnOff, "Turn blackout hours off")),
        ];
        pane.let_in_menu = Some(
            OpenMenu::new("blackout-let-in".into(), None, LET_IN_MENU_W, entries)
                .offset(px(0.0), px(6.0)),
        );
    }

    /// Lets every device in, as the menu's `choice` says for how long.
    fn let_devices_in(&mut self, choice: LetIn, cx: &mut Context<Self>) {
        let Some(pane) = self.blackout_pane() else {
            return;
        };
        pane.let_in_menu = None;
        let now = Timestamp::now();
        let until = match choice {
            LetIn::Minutes(minutes) => Some(now + SignedDuration::from_mins(minutes)),
            LetIn::Rest => pane.standing().now.map(|window| window.end),
            LetIn::TurnOff => None,
        };
        let seconds = until.and_then(|until| u64::try_from(until.as_second()).ok());
        match (choice, seconds) {
            (LetIn::TurnOff, _) => self.change_blackout(
                |blackout| {
                    blackout.enabled = false;
                    blackout.let_in_until = 0;
                },
                cx,
            ),
            (_, Some(seconds)) => {
                self.change_blackout(|blackout| blackout.let_in_until = seconds, cx);
            }
            // The blackout ended while the menu was open: there is nothing
            // left to be let in to, and the hours stay as they are.
            (_, None) => {}
        }
        cx.notify();
    }

    /// The group's keys, while the Devices pane shows it: an open menu's,
    /// then a time's — typing, ↑ and ↓, Return to take it, Tab to the other.
    /// Returns whether the key was the group's.
    pub(crate) fn blackout_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.blackout_pane() else {
            return false;
        };
        if let Some(menu) = pane.repeat_menu.as_mut() {
            match menu.key(event) {
                MenuKey::Ignored => {}
                MenuKey::Consumed => {
                    cx.notify();
                    return true;
                }
                MenuKey::Close => {
                    pane.repeat_menu = None;
                    cx.notify();
                    return true;
                }
                MenuKey::Run(repeat) => {
                    self.set_blackout_repeat(repeat, cx);
                    return true;
                }
            }
        } else if let Some(menu) = pane.let_in_menu.as_mut() {
            match menu.key(event) {
                MenuKey::Ignored => {}
                MenuKey::Consumed => {
                    cx.notify();
                    return true;
                }
                MenuKey::Close => {
                    pane.let_in_menu = None;
                    cx.notify();
                    return true;
                }
                MenuKey::Run(choice) => {
                    self.let_devices_in(choice, cx);
                    return true;
                }
            }
        }
        let Some(pane) = self.blackout_pane() else {
            return false;
        };
        let edge = if pane.from.read(cx).is_focused(window) {
            Edge::From
        } else if pane.until.read(cx).is_focused(window) {
            Edge::Until
        } else {
            return false;
        };
        let input = pane.input(edge).clone();
        let other = pane
            .input(match edge {
                Edge::From => Edge::Until,
                Edge::Until => Edge::From,
            })
            .clone();
        let key = &event.keystroke;
        match key.key.as_str() {
            // The view's: it lets go of the field, or closes.
            "escape" => return false,
            "up" | "down" => self.step_blackout_time(edge, key.key == "up", cx),
            "enter" => self.commit_blackout_times(cx),
            "tab" => {
                other.update(cx, |input, _| input.select_all());
                window.focus(other.read(cx).focus_handle());
            }
            // Typed characters reach the field through the input context.
            _ if is_text(key) => {}
            _ => {
                input.update(cx, |input, cx| input.key(key, cx));
            }
        }
        true
    }

    /// The Blackout group: what a blackout is, or that one is on and the way
    /// to let devices in; the switch; and, with it on, the hours.
    pub(super) fn blackout_settings(
        &self,
        pane: &PhonesPane,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entry> {
        let form = pane.blackout.as_ref()?;
        let tz = TimeZone::system();
        let standing = form.standing();
        let on = form.saved.enabled;
        let hold = form.saved.hold_notifications;

        let lead =
            if let Some(until) = standing.shut_until() {
                let notices = if hold {
                    "their notifications are held until then"
                } else {
                    "notifications still reach them"
                };
                banner(
                    "blackout-on",
                    Tone::Warning,
                    format!(
                        "A blackout is on until {}. ket is shut off on every paired device: they \
                     can\u{2019}t open chats or terminals, and {notices}. Your agents keep \
                     working. Only this Mac can let them back in.",
                        time_of(until, &tz)
                    ),
                    t,
                )
                .items_center()
                .child(self.let_in_control(form, t, cx))
                .into_any_element()
            } else if let (Some(now), Some(let_in)) = (standing.now, standing.let_in_until) {
                let words = if let_in >= now.end {
                    format!(
                        "Devices are let in for the rest of this blackout, which ends at {}.",
                        time_of(now.end, &tz)
                    )
                } else {
                    format!(
                        "Devices are let in until {}. The blackout shuts them out again then, \
                     until {}.",
                        time_of(let_in, &tz),
                        time_of(now.end, &tz)
                    )
                };
                banner("blackout-let-in-on", Tone::Info, words, t)
                    .items_center()
                    .child(button("blackout-shut", "Shut them out").render(t).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.change_blackout(|blackout| blackout.let_in_until = 0, cx);
                        }),
                    ))
                    .into_any_element()
            } else {
                let what = if hold {
                    "no chats, no terminals and no notifications"
                } else {
                    "no chats and no terminals"
                };
                banner(
                    "blackout-what",
                    Tone::Info,
                    format!(
                        "During blackout hours ket shuts off on every paired device: {what}. Your \
                     agents keep working on this Mac. Nothing on a device can end a blackout \
                     early. Only this Mac can let one back in."
                    ),
                    t,
                )
                .into_any_element()
            };

        let switch_row = sized_row("blackout-hours", false, px(40.0), t)
            .cursor_default()
            .child(icon(Icon::Moon, paint(t.text.dim)))
            .child(
                div()
                    .flex_none()
                    .font_family(self.font_family.clone())
                    .child("Blackout hours"),
            )
            .child(div().flex_1())
            .child(switch("blackout-switch", on, t).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.change_blackout(
                        |blackout| {
                            blackout.enabled = !on;
                            blackout.let_in_until = 0;
                        },
                        cx,
                    );
                },
            )));

        let body = group_box(t).child(switch_row).when(on, |el| {
            el.child(self.blackout_form(form, &standing, t, window, cx))
        });

        Some(Entry::setting(
            "Blackout hours shut off lock out devices phone bedtime downtime quiet night \
             limit let them in notifications hold",
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .pt(px(16.0))
                .child(group_label("Blackout", None, t))
                .child(lead)
                .child(body),
        ))
    }

    /// The Let them in button over its menu.
    fn let_in_control(&self, form: &BlackoutPane, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let menu = form.let_in_menu.as_ref().map(|menu| {
            menu.dropdown_view(
                t,
                cx,
                |this, choice, cx| {
                    this.let_devices_in(*choice, cx);
                },
                |this, cx| {
                    if let Some(pane) = this.blackout_pane() {
                        pane.close_menus();
                    }
                    cx.notify();
                },
            )
        });
        let chevron = if form.let_in_menu.is_some() {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        };
        dropdown(
            "blackout-let-in-select",
            button("blackout-let-in", "Let them in")
                .trailing(chevron)
                .render(t),
            menu,
        )
        .flex_none()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_let_in_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// The hours, under the switch: Repeat and the two times, the days when
    /// they are chosen, where notifications go, and when it starts next.
    fn blackout_form(
        &self,
        form: &BlackoutPane,
        standing: &Standing,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tz = TimeZone::system();
        let dim = paint(t.text.dim);
        let repeat = if form.choosing {
            Repeat::Days
        } else {
            form.saved.repeat()
        };

        let repeat_menu = form.repeat_menu.as_ref().map(|menu| {
            menu.dropdown_view(
                t,
                cx,
                |this, repeat, cx| {
                    this.set_blackout_repeat(*repeat, cx);
                },
                |this, cx| {
                    if let Some(pane) = this.blackout_pane() {
                        pane.close_menus();
                    }
                    cx.notify();
                },
            )
        });
        let repeat_select = dropdown(
            "blackout-repeat-select",
            select("blackout-repeat", repeat_label(repeat))
                .leading(icon(Icon::Repeat, dim))
                .open(form.repeat_menu.is_some())
                .render(t),
            repeat_menu,
        )
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_blackout_repeat_menu();
                cx.stop_propagation();
                cx.notify();
            }),
        );

        // What is typed and not yet taken: marked when it is not a time, and
        // said when it is one waiting on Return.
        let typed = (
            Clock::parse(&form.from.read(cx).text()),
            Clock::parse(&form.until.read(cx).text()),
        );
        let refused = form.error.is_some();
        let waiting = !refused
            && matches!(typed, (Some(from), Some(until))
                if (from, until) != (form.saved.from, form.saved.until));
        let time = |edge: Edge, id: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let invalid = refused
                && match edge {
                    Edge::From => typed.0.is_none(),
                    Edge::Until => typed.1.is_none(),
                };
            text_field(
                form.input(edge),
                id,
                invalid,
                Style::new(t, self.caret.visible)
                    .leading(Icon::Clock)
                    .trailing(caption("\u{2191}\u{2193}", t)),
                window,
                cx,
            )
            .on_click(cx.listener(|_, _, _, cx| cx.notify()))
            .into_any_element()
        };
        let from = time(Edge::From, "blackout-from", window, cx);
        let until = time(Edge::Until, "blackout-until", window, cx);

        let when_row = div()
            .flex()
            .gap(px(10.0))
            .child(
                labelled("Repeat", repeat_select.into_any_element(), t)
                    .flex_1()
                    .min_w_0(),
            )
            .child(labelled("From", from, t).flex_1().min_w_0())
            .child(labelled("Until", until, t).flex_1().min_w_0());

        let days = (repeat == Repeat::Days).then(|| {
            button_group("blackout-days")
                .fill()
                .children(Dow::ALL.into_iter().zip(form.days()).enumerate().map(
                    |(at, (day, on))| {
                        segment(("blackout-day", at), day.short())
                            .selected(on)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_blackout_day(at, cx);
                            }))
                    },
                ))
                .render(t)
        });

        let hold = form.saved.hold_notifications;
        let notices = button_group("blackout-notices")
            .child(
                segment("blackout-notices-hold", "Hold until it ends")
                    .selected(hold)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.change_blackout(|blackout| blackout.hold_notifications = true, cx);
                    })),
            )
            .child(
                segment("blackout-notices-send", "Keep sending")
                    .selected(!hold)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.change_blackout(|blackout| blackout.hold_notifications = false, cx);
                    })),
            )
            .render(t);

        let next = standing.now.or(standing.next);
        let (said, then) = match next {
            Some(window) => {
                let (start, end) = (
                    moment(&window.start.to_zoned(tz.clone())),
                    moment(&window.end.to_zoned(tz.clone())),
                );
                let said = if standing.now.is_some() {
                    format!("On now, until {end}.")
                } else {
                    format!("Starts {start} and ends {end}.")
                };
                // "every day", "every weekday", "on Mon, Wed and Fri".
                let days = match form.saved.repeat() {
                    Repeat::Days => format!("on {}", form.saved.repeat_label()),
                    _ => form.saved.repeat_label().to_lowercase(),
                };
                let then = format!("Then {days}, {} each.", length(form.saved.minutes()));
                (said, then)
            }
            None => ("No blackout is coming.".to_owned(), String::new()),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .px(px(10.0))
            .pt(px(8.0))
            .pb(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(when_row)
                    .children(form.error.clone().map(|why| error(why, t)))
                    .when(waiting, |el| {
                        el.child(caption("Press Return to set these hours.", t).mt(px(7.0)))
                    }),
            )
            .children(days)
            .child(labelled(
                "Notifications during a blackout",
                div().flex().child(notices).into_any_element(),
                t,
            ))
            .child(summary_box(said, then, t))
            .into_any_element()
    }
}
