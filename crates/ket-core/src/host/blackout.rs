//! Blackout hours: the hours ket is shut off on every paired device, set in
//! Settings → Devices and lifted only from this Mac.
//!
//! A hard limit a person sets on themselves: from ten at night until seven
//! in the morning, say, a paired phone can open nothing — no chats, no
//! terminals, and no way round it on the phone. The host enforces it (see
//! `phones::blackout`), so an app that ignores it still gets nothing; the
//! phone is told the hours so it can cover its screens on time, and with no
//! connection.
//!
//! The hours are `[phones.blackout]` in `config.toml`, with the rest of
//! Settings → Devices. One thing is not a setting: until when the desktop
//! has let devices in, which is a moment written as its button is pressed.
//! That is `blackout.json` beside `devices.json`, with what else the host
//! keeps about paired devices. The host notices a change to either by the
//! file's time, so the window sends it nothing, and a host that is not
//! running needs nothing.
//!
//! The hours are wall-clock times on the Mac's own clock, as an automation's
//! are (see `crate::automation::when`): ten at night stays ten at night
//! after the clocks change. Everything here but the file is a pure function
//! of the setting, a moment and a time zone.

use std::path::PathBuf;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use super::{host_error, identity};
use crate::Result;
use crate::automation::{Clock, Dow};

#[cfg(test)]
mod tests;

/// How many blackouts ahead a phone is told of: two weeks of nights, so one
/// that has not reached its Mac in that long still covers itself on time.
pub const WINDOWS_AHEAD: usize = 14;

/// How far ahead [`Blackout::windows`] looks for those. Chosen days can be a
/// week apart, so a count of days rather than of windows.
const DAYS_AHEAD: usize = 60;

/// The setting, and until when devices are let in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Blackout {
    /// Whether there are blackout hours at all.
    pub enabled: bool,
    /// When a blackout starts.
    pub from: Clock,
    /// When it ends: the same day when later than [`Self::from`], the next
    /// day otherwise. The same time is a whole day.
    pub until: Clock,
    /// The days a blackout starts on. Empty is every day.
    pub days: Vec<Dow>,
    /// Whether notifications wait for a blackout to end, rather than still
    /// being sent during one.
    pub hold_notifications: bool,
    /// Devices are let in until this moment, Unix seconds, blackout or not:
    /// the desktop's Let them in. Zero for never. Not part of the config:
    /// kept in `blackout.json` — see [`load`].
    #[serde(skip)]
    pub let_in_until: u64,
}

impl Default for Blackout {
    fn default() -> Self {
        Self {
            enabled: false,
            from: clock(22, 0),
            until: clock(7, 0),
            days: Vec::new(),
            hold_notifications: true,
            let_in_until: 0,
        }
    }
}

fn clock(hour: u8, minute: u8) -> Clock {
    Clock::new(hour, minute).unwrap_or_else(|| Clock::parse("0").expect("midnight is a time"))
}

/// Which days a blackout starts on, as Settings offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    /// Every day.
    Daily,
    /// Monday to Friday.
    Weekdays,
    /// The days chosen.
    Days,
}

/// One blackout: from `start` up to, not including, `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// When it starts.
    pub start: Timestamp,
    /// When it ends.
    pub end: Timestamp,
}

/// How things stand at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing {
    /// The blackout this moment falls in, whether or not devices have been
    /// let in during it.
    pub now: Option<Window>,
    /// Devices are let in until this moment, which is still to come, during
    /// [`Self::now`].
    pub let_in_until: Option<Timestamp>,
    /// The next blackout to start.
    pub next: Option<Window>,
}

impl Standing {
    /// When the blackout shutting devices out now ends; `None` when they are
    /// not shut out — no blackout, or one they have been let in during.
    pub fn shut_until(&self) -> Option<Timestamp> {
        match (self.now, self.let_in_until) {
            (Some(window), None) => Some(window.end),
            _ => None,
        }
    }
}

impl Blackout {
    /// Whether these are the hours a config that names none has: what
    /// keeps `[phones.blackout]` out of a config that never set any.
    pub fn is_unset(&self) -> bool {
        let hours = Self {
            let_in_until: 0,
            ..self.clone()
        };
        hours == Self::default()
    }

    /// Which of Settings' three answers [`Self::days`] is.
    pub fn repeat(&self) -> Repeat {
        let weekdays = self.days.len() == 5 && self.days.iter().all(|day| day.is_weekday());
        if self.days.is_empty() || Dow::ALL.iter().all(|day| self.days.contains(day)) {
            Repeat::Daily
        } else if weekdays {
            Repeat::Weekdays
        } else {
            Repeat::Days
        }
    }

    /// The days in words: `Every day`, `Every weekday`, `Mon, Wed and Fri`.
    pub fn repeat_label(&self) -> String {
        match self.repeat() {
            Repeat::Daily => "Every day".to_owned(),
            Repeat::Weekdays => "Every weekday".to_owned(),
            Repeat::Days => {
                let days: Vec<&str> = Dow::ALL
                    .into_iter()
                    .filter(|day| self.days.contains(day))
                    .map(Dow::short)
                    .collect();
                match days.split_last() {
                    Some((last, rest)) if !rest.is_empty() => {
                        format!("{} and {last}", rest.join(", "))
                    }
                    Some((only, _)) => (*only).to_owned(),
                    None => String::new(),
                }
            }
        }
    }

    /// How long one blackout lasts, in minutes: a whole day when it ends at
    /// the time it starts.
    pub fn minutes(&self) -> u32 {
        let of = |clock: Clock| u32::from(clock.hour()) * 60 + u32::from(clock.minute());
        let (from, until) = (of(self.from), of(self.until));
        if until > from {
            until - from
        } else {
            until + 24 * 60 - from
        }
    }

    /// The blackout that starts on `date`, if one does.
    fn window_on(&self, date: Date, tz: &TimeZone) -> Option<Window> {
        let starts = self.days.is_empty() || self.days.contains(&Dow::of(date.weekday()));
        if !starts {
            return None;
        }
        let ends_on = if self.until > self.from {
            date
        } else {
            date.tomorrow().ok()?
        };
        let (start, end) = (
            resolve(date, self.from, tz)?,
            resolve(ends_on, self.until, tz)?,
        );
        (end > start).then_some(Window { start, end })
    }

    /// The blackouts that have not ended by `now`, soonest first: the one in
    /// force, if any, then those to come, `count` at most. None when there
    /// are no blackout hours.
    pub fn windows(&self, now: Timestamp, tz: &TimeZone, count: usize) -> Vec<Window> {
        if !self.enabled || count == 0 {
            return Vec::new();
        }
        // From yesterday: last night's blackout may still be on.
        let Ok(mut date) = now.to_zoned(tz.clone()).date().yesterday() else {
            return Vec::new();
        };
        let mut windows = Vec::new();
        for _ in 0..DAYS_AHEAD {
            if let Some(window) = self.window_on(date, tz).filter(|window| window.end > now) {
                windows.push(window);
                if windows.len() == count {
                    break;
                }
            }
            let Ok(next) = date.tomorrow() else { break };
            date = next;
        }
        windows
    }

    /// How things stand at `now`.
    pub fn standing(&self, now: Timestamp, tz: &TimeZone) -> Standing {
        let windows = self.windows(now, tz, 2);
        let current = windows
            .first()
            .copied()
            .filter(|window| window.start <= now);
        let next = windows.into_iter().find(|window| window.start > now);
        let let_in_until = current.and_then(|_| {
            let until = Timestamp::from_second(i64::try_from(self.let_in_until).ok()?).ok()?;
            (until > now).then_some(until)
        });
        Standing {
            now: current,
            let_in_until,
            next,
        }
    }
}

/// `time` on `date`, on `tz`'s clock: as soon after as there is when the
/// clocks skip it, the first time when it happens twice.
fn resolve(date: Date, time: Clock, tz: &TimeZone) -> Option<Timestamp> {
    let local = DateTime::new(
        date.year(),
        date.month(),
        date.day(),
        i8::try_from(time.hour()).ok()?,
        i8::try_from(time.minute()).ok()?,
        0,
        0,
    )
    .ok()?;
    tz.to_ambiguous_zoned(local)
        .compatible()
        .ok()
        .map(|at| at.timestamp())
}

/// What `blackout.json` holds.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    let_in_until: u64,
}

/// Where the moment devices are let in until is kept.
fn path() -> Result<PathBuf> {
    Ok(identity::dir()?.join("blackout.json"))
}

/// The moment devices are let in until, Unix seconds; zero with no file.
pub fn let_in() -> Result<u64> {
    let bytes = match std::fs::read(path()?) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(host_error(error)),
    };
    let stored: Stored = serde_json::from_slice(&bytes).map_err(host_error)?;
    Ok(stored.let_in_until)
}

/// Lets devices in until `seconds`, Unix seconds, or with zero no longer:
/// written owner-only in place of the last file. The running host takes it
/// up within a second.
pub fn set_let_in(seconds: u64) -> Result<()> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(host_error)?;
    }
    let stored = Stored {
        let_in_until: seconds,
    };
    identity::write_private(
        &path,
        &serde_json::to_vec_pretty(&stored).map_err(host_error)?,
    )
}

/// The setting as it stands on disk: the config's hours, and until when
/// devices are let in. A `blackout.json` that cannot be read lets nobody in:
/// a damaged file must not cost the hours, which are the config's.
pub fn load() -> Result<Blackout> {
    let mut blackout = crate::config::Config::load()?.phones.blackout;
    blackout.let_in_until = let_in().unwrap_or_else(|error| {
        tracing::warn!(%error, "blackout: blackout.json is unreadable; nobody is let in");
        0
    });
    Ok(blackout)
}

/// The times of the two files [`load`] reads, which tell a change to
/// either; `None` for one that is not there.
pub fn stamp() -> (Option<SystemTime>, Option<SystemTime>) {
    let modified = |path: Result<PathBuf>| std::fs::metadata(path.ok()?).ok()?.modified().ok();
    (modified(crate::paths::config_file()), modified(path()))
}

/// Writes the whole of `blackout` to the two files it lives in, for a
/// caller with no config of its own in hand. Settings changes the config it
/// holds and saves that instead, so its other settings are not written over.
pub fn save(blackout: &Blackout) -> Result<()> {
    let mut config = crate::config::Config::load()?;
    config.phones.blackout = Blackout {
        let_in_until: 0,
        ..blackout.clone()
    };
    config.save()?;
    set_let_in(blackout.let_in_until)
}
