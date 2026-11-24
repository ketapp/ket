//! What the window shows of automations while their dialog is shut: the
//! status bar's chip with the next start, and a toast when a start did not
//! happen — missed while the Mac slept, or refused.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};

use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div, prelude::*, px};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_core::automation::{Automations, Outcome, Runs};

use super::{day_of, time_of};
use crate::Shell;
use crate::paint::paint;
use crate::ui::chip::readout;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::toast::Tone;

/// How often the next start is worked out again: it moves as time passes,
/// not only when the file changes.
const RECKON_EVERY: Duration = Duration::from_secs(10);

/// The window's glance at automations.
#[derive(Default)]
pub(crate) struct Glance {
    seen: Option<Option<SystemTime>>,
    runs_seen: Option<Option<SystemTime>>,
    all: Automations,
    reckoned: Option<Instant>,
    /// The soonest start: the automation's name, and when.
    next: Option<(String, Timestamp)>,
    /// The starts already told about, by automation and due time.
    told: HashSet<(String, u64)>,
    /// Whether the runs have been read once: what was already there then is
    /// old news, never a toast.
    primed: bool,
}

impl Shell {
    /// Re-reads what changed and works out the next start; says when a start
    /// was missed or refused. On the window's tick.
    pub(crate) fn refresh_automation_glance(&mut self, cx: &mut Context<Self>) {
        let glance = &mut self.automation_glance;
        let modified = Automations::modified();
        let mut changed = false;
        if glance.seen != Some(modified) {
            if let Ok(all) = Automations::load() {
                glance.all = all;
                changed = true;
            }
            glance.seen = Some(modified);
        }
        if changed
            || glance
                .reckoned
                .is_none_or(|at| at.elapsed() >= RECKON_EVERY)
        {
            glance.reckoned = Some(Instant::now());
            let tz = TimeZone::system();
            glance.next = glance
                .all
                .upcoming(Timestamp::now(), &tz)
                .first()
                .map(|(automation, at)| (automation.name.clone(), *at));
        }
        let Ok(root) = ket_core::paths::automations_dir() else {
            return;
        };
        let modified = Runs::modified_in(&root);
        if glance.runs_seen == Some(modified) {
            return;
        }
        glance.runs_seen = Some(modified);
        let Ok(runs) = Runs::load_in(&root) else {
            return;
        };
        let mut news = Vec::new();
        for run in &runs.runs {
            let fresh = glance.told.insert((run.automation.clone(), run.due_ms));
            if !fresh || !glance.primed {
                continue;
            }
            let why = match &run.outcome {
                Outcome::Missed { why } | Outcome::CouldNotStart { why } => why,
                _ => continue,
            };
            let name = glance
                .all
                .get(&run.automation)
                .map_or_else(String::new, |automation| automation.name.clone());
            news.push(format!("\u{201c}{name}\u{201d} didn\u{2019}t start: {why}"));
        }
        glance.primed = true;
        for said in news {
            self.toast(Tone::Error, said, cx);
        }
    }

    /// The status bar's chip — the clock, the next start's time and its
    /// name — while anything is scheduled. A click opens the Automations
    /// dialog.
    pub(crate) fn automation_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (name, at) = self.automation_glance.next.as_ref()?;
        let t = &self.theme.on_backdrop();
        let tz = TimeZone::system();
        let day = day_of(*at, &tz);
        let when = if day == "Today" {
            time_of(*at, &tz)
        } else {
            format!("{day} {}", time_of(*at, &tz))
        };
        Some(
            div()
                .id("automation-chip")
                .cursor_pointer()
                .child(
                    readout(t)
                        .child(sized_icon(Icon::Clock, px(12.0), paint(t.text.primary)))
                        .child(when)
                        .child(div().text_color(paint(t.text.dim)).child(name.clone())),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open_automations(None, cx);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }
}
