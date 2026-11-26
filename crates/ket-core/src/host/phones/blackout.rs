//! Blackout hours, enforced: see `crate::host::blackout` for the setting.
//!
//! The limit is hard because it is kept here, on the Mac, not on the phone.
//! While devices are shut out:
//!
//! - a phone's snapshot holds nothing but the blackout (`snapshot`);
//! - whatever it asks for is refused with `ErrorCode::Blackout` (`requests`),
//!   apart from where to send its notifications;
//! - every session open as the blackout begins is ended, and with it the
//!   terminals it was watching — the phone reconnects into the two above;
//! - notifications wait for the end, unless the setting says to keep
//!   sending them (`Phones::tick`).
//!
//! The setting is read again whenever the time of either file it lives in
//! changes, looked at once a tick: turning blackout hours off, or letting
//! devices in, takes effect within a second, with no message from the
//! window that did it.

use std::time::{Duration, Instant, SystemTime};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_remote::proto;

use super::{Event, Phones, lock};
use crate::host::blackout::{self, Blackout, WINDOWS_AHEAD};

/// The setting as the host last read it.
#[derive(Default)]
pub(super) struct Watch {
    plan: Blackout,
    /// Its files' times when it was read, which tell a change.
    stamp: (Option<SystemTime>, Option<SystemTime>),
    /// Whether devices were shut out at the last tick, to tell one beginning.
    shut: bool,
}

impl Watch {
    /// The setting on disk. One that cannot be read is no blackout hours,
    /// said in the log: there is nothing older to keep to.
    pub(super) fn load() -> Self {
        let stamp = blackout::stamp();
        let plan = blackout::load().unwrap_or_else(|error| {
            tracing::warn!(%error, "blackout: the setting is unreadable; starting without it");
            Blackout::default()
        });
        Self {
            plan,
            stamp,
            shut: false,
        }
    }

    /// Reads the setting again if either of its files has changed. One that
    /// has become unreadable changes nothing: a damaged file must not lift a
    /// blackout.
    fn refresh(&mut self) {
        let stamp = blackout::stamp();
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        match blackout::load() {
            Ok(plan) => self.plan = plan,
            Err(error) => {
                tracing::warn!(%error, "blackout: the setting is unreadable; keeping the last one");
            }
        }
    }
}

/// A blackout shutting devices out, as the tick needs it.
pub(super) struct Shut {
    /// How long until it ends.
    pub(super) left: Duration,
    /// Whether notifications wait for the end.
    pub(super) hold: bool,
}

fn ms(at: Timestamp) -> u64 {
    u64::try_from(at.as_millisecond()).unwrap_or(0)
}

impl Phones {
    /// Whether devices are shut out now, by the setting as last read.
    pub(super) fn shut(&self) -> bool {
        lock(&self.blackout)
            .plan
            .standing(Timestamp::now(), &TimeZone::system())
            .shut_until()
            .is_some()
    }

    /// Whether devices were shut out at the last tick: [`Self::shut`] without
    /// reading the clock, for what is asked once for every piece of a
    /// terminal's output. Set before a blackout's sessions are ended, so
    /// whatever is still queued for them finds it set.
    pub(super) fn was_shut(&self) -> bool {
        lock(&self.blackout).shut
    }

    /// The blackout hours as a phone is told them; `None` with none set.
    pub(super) fn blackout_message(&self) -> Option<proto::Blackout> {
        let watch = lock(&self.blackout);
        if !watch.plan.enabled {
            return None;
        }
        let (now, tz) = (Timestamp::now(), TimeZone::system());
        let standing = watch.plan.standing(now, &tz);
        Some(proto::Blackout {
            until_ms: standing.shut_until().map_or(0, ms),
            windows: watch
                .plan
                .windows(now, &tz, WINDOWS_AHEAD)
                .into_iter()
                .map(|window| proto::BlackoutWindow {
                    start_ms: ms(window.start),
                    end_ms: ms(window.end),
                })
                .collect(),
            repeat: watch.plan.repeat_label(),
            let_in_until_ms: standing.let_in_until.map_or(0, ms),
        })
    }

    /// Once a tick: takes up a changed setting, and as a blackout begins —
    /// its hour comes, it is turned on during its hours, or being let in
    /// runs out — ends every phone's session. Returns the blackout shutting
    /// devices out now, if one is.
    pub(super) fn watch_blackout(&self) -> Option<Shut> {
        let (shut, began) = {
            let mut watch = lock(&self.blackout);
            watch.refresh();
            let now = Timestamp::now();
            let shut = watch
                .plan
                .standing(now, &TimeZone::system())
                .shut_until()
                .map(|until| Shut {
                    left: Duration::from_millis(ms(until).saturating_sub(ms(now))),
                    hold: watch.plan.hold_notifications,
                });
            let began = shut.is_some() && !watch.shut;
            watch.shut = shut.is_some();
            (shut, began)
        };
        if began {
            // Not drained: each session takes itself out of `live` as it
            // ends, the way one the phone closed does.
            let live = lock(&self.live);
            for (_, feed) in live.values().flatten() {
                let _ = feed.send(Event::Closed);
            }
            tracing::info!(sessions = live.values().flatten().count(), "blackout began");
        }
        shut
    }

    /// When a notice held for a blackout is looked at again: its end.
    pub(super) fn held_until(shut: &Shut) -> Instant {
        Instant::now() + shut.left
    }
}
