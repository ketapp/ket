//! When an automation starts: once, on a day and at a time, or again and
//! again — every day, every weekday, on chosen days, or every hour.
//!
//! Times are wall-clock times in the Mac's own time zone, never instants: an
//! automation set for nine in the morning starts at nine after the clocks
//! change, and after the Mac has travelled. Turning a wall-clock time into an
//! instant is jiff's job. A time the clocks skip, going forward, starts as
//! soon after it as there is; one that happens twice, going back, starts the
//! first time.
//!
//! Everything here is a pure function of a [`When`], a moment and a time
//! zone, so the host's clock and the tests ask the same questions.

use std::fmt;

use jiff::civil::{Date, DateTime, Weekday};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// How late a start can be and still count as on time: the host looks at the
/// clock every second, but a busy or briefly suspended process can miss a
/// few.
pub const ON_TIME: SignedDuration = SignedDuration::from_mins(2);

/// A wall-clock time, to the minute. Kept in JSON as `"HH:MM"`, 24-hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Clock {
    hour: u8,
    minute: u8,
}

impl Clock {
    /// `hour` in 0–23 and `minute` in 0–59, or `None`.
    pub fn new(hour: u8, minute: u8) -> Option<Self> {
        (hour < 24 && minute < 60).then_some(Self { hour, minute })
    }

    /// 0–23.
    pub fn hour(self) -> u8 {
        self.hour
    }

    /// 0–59.
    pub fn minute(self) -> u8 {
        self.minute
    }

    /// What a person typed: `9`, `9am`, `9:30`, `9:30 pm`, `21:30`, `0930`.
    /// A bare hour under 12 is the morning's. `None` when it isn't a time.
    pub fn parse(typed: &str) -> Option<Self> {
        let text = typed.trim().to_ascii_lowercase().replace(['.', ' '], "");
        let (digits, half) = if let Some(rest) = text.strip_suffix("am") {
            (rest, Some(false))
        } else if let Some(rest) = text.strip_suffix("pm") {
            (rest, Some(true))
        } else {
            (text.as_str(), None)
        };
        // Only ASCII digits and one colon, so the splits below fall on
        // character boundaries: a full-width colon is not a time.
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == ':') {
            return None;
        }
        let (hour, minute) = match digits.split_once(':') {
            Some((hour, minute)) => (hour.parse::<u8>().ok()?, minute.parse::<u8>().ok()?),
            None if digits.len() > 2 => {
                let (hour, minute) = digits.split_at(digits.len() - 2);
                (hour.parse::<u8>().ok()?, minute.parse::<u8>().ok()?)
            }
            None => (digits.parse::<u8>().ok()?, 0),
        };
        let hour = match half {
            Some(_) if !(1..=12).contains(&hour) => return None,
            Some(false) => hour % 12,
            Some(true) => hour % 12 + 12,
            None => hour,
        };
        Self::new(hour, minute)
    }

    /// `minutes` later, or earlier when negative, round the clock — the time
    /// field's ↑ and ↓. Lands on a multiple of `minutes`, so a ↑ from 9:07
    /// by 15 is 9:15, not 9:22.
    pub fn step(self, minutes: i32) -> Self {
        let day = 24 * 60;
        let now = i32::from(self.hour) * 60 + i32::from(self.minute);
        let size = minutes.abs().max(1);
        let floor = now - now.rem_euclid(size);
        let moved = if minutes > 0 {
            floor + size
        } else if now == floor {
            floor - size
        } else {
            floor
        };
        let moved = moved.rem_euclid(day);
        Self {
            hour: u8::try_from(moved / 60).unwrap_or(0),
            minute: u8::try_from(moved % 60).unwrap_or(0),
        }
    }

    /// `9:00 AM`, `12:30 PM`: how every surface shows a time.
    pub fn label(self) -> String {
        let (hour, half) = match self.hour {
            0 => (12, "AM"),
            1..=11 => (self.hour, "AM"),
            12 => (12, "PM"),
            later => (later - 12, "PM"),
        };
        format!("{hour}:{:02} {half}", self.minute)
    }
}

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

impl From<Clock> for String {
    fn from(clock: Clock) -> Self {
        clock.to_string()
    }
}

impl TryFrom<String> for Clock {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.split_once(':')
            .and_then(|(hour, minute)| Clock::new(hour.parse().ok()?, minute.parse().ok()?))
            .ok_or_else(|| format!("not a time: {text:?}, which should read like 09:30"))
    }
}

/// A calendar day. Kept in JSON as `"YYYY-MM-DD"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Day(Date);

impl Day {
    /// The day `date` is.
    pub fn new(date: Date) -> Self {
        Self(date)
    }

    /// The calendar date.
    pub fn date(self) -> Date {
        self.0
    }

    /// `Fri 20 Nov`, as every surface shows a day.
    pub fn label(self) -> String {
        self.0.strftime("%a %-d %b").to_string()
    }
}

impl From<Day> for String {
    fn from(day: Day) -> Self {
        day.0.to_string()
    }
}

impl TryFrom<String> for Day {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse::<Date>()
            .map(Self)
            .map_err(|e| format!("not a day: {text:?} ({e})"))
    }
}

/// A day of the week, kept in JSON as `"mon"` … `"sun"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dow {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Dow {
    /// Monday to Sunday.
    pub const ALL: [Dow; 7] = [
        Dow::Mon,
        Dow::Tue,
        Dow::Wed,
        Dow::Thu,
        Dow::Fri,
        Dow::Sat,
        Dow::Sun,
    ];

    /// The day `weekday` is.
    pub fn of(weekday: Weekday) -> Self {
        match weekday {
            Weekday::Monday => Dow::Mon,
            Weekday::Tuesday => Dow::Tue,
            Weekday::Wednesday => Dow::Wed,
            Weekday::Thursday => Dow::Thu,
            Weekday::Friday => Dow::Fri,
            Weekday::Saturday => Dow::Sat,
            Weekday::Sunday => Dow::Sun,
        }
    }

    /// `Mon` … `Sun`.
    pub fn short(self) -> &'static str {
        match self {
            Dow::Mon => "Mon",
            Dow::Tue => "Tue",
            Dow::Wed => "Wed",
            Dow::Thu => "Thu",
            Dow::Fri => "Fri",
            Dow::Sat => "Sat",
            Dow::Sun => "Sun",
        }
    }

    /// What a person typed: `mon`, `Monday`, `tues`, `th`… — two letters
    /// at least, and the start of one day's name only. `None` otherwise.
    pub fn parse(typed: &str) -> Option<Self> {
        const NAMES: [&str; 7] = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ];
        let typed = typed.trim().to_ascii_lowercase();
        let mut found = Dow::ALL
            .into_iter()
            .zip(NAMES)
            .filter(|(_, name)| typed.len() >= 2 && name.starts_with(&typed));
        let (day, _) = found.next()?;
        found.next().is_none().then_some(day)
    }

    /// Monday to Friday.
    pub fn is_weekday(self) -> bool {
        !matches!(self, Dow::Sat | Dow::Sun)
    }
}

/// When an automation starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "repeat", rename_all = "snake_case")]
pub enum When {
    /// Once, on `day` at `time`.
    Once {
        /// The day.
        day: Day,
        /// The time on it.
        time: Clock,
    },
    /// Every day at `time`.
    Daily {
        /// The time.
        time: Clock,
    },
    /// Monday to Friday at `time`.
    Weekdays {
        /// The time.
        time: Clock,
    },
    /// On each of `days` at `time`.
    Days {
        /// The days, in any order. Never empty — see [`When::check`].
        days: Vec<Dow>,
        /// The time.
        time: Clock,
    },
    /// Every hour, `minute` minutes past.
    Hourly {
        /// 0–59.
        minute: u8,
    },
}

/// Whether a start that came due is taken, and how late it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Due at this moment, and the clock is within [`ON_TIME`] of it.
    OnTime(Timestamp),
    /// Due at this moment, which passed while the Mac was asleep or ket was
    /// not running, and is still within the automation's catch-up window.
    Late(Timestamp),
    /// Due at this moment, and too long ago to start now.
    Missed(Timestamp),
}

impl Due {
    /// The moment it was due.
    pub fn at(self) -> Timestamp {
        match self {
            Due::OnTime(at) | Due::Late(at) | Due::Missed(at) => at,
        }
    }
}

impl When {
    /// Whether this starts once rather than repeating.
    pub fn is_once(&self) -> bool {
        matches!(self, When::Once { .. })
    }

    /// Why this can never start, or `Ok`.
    pub fn check(&self) -> Result<(), String> {
        match self {
            When::Days { days, .. } if days.is_empty() => {
                Err("pick at least one day for it to start on".to_owned())
            }
            When::Hourly { minute } if *minute > 59 => {
                Err(format!("{minute} isn't a minute past the hour"))
            }
            _ => Ok(()),
        }
    }

    /// Why this can't be saved at `now`: never starts at all, or a one-off
    /// whose moment has passed — saved, it would never start, nor be missed.
    pub fn check_at(&self, now: Timestamp, tz: &TimeZone) -> Result<(), String> {
        self.check()?;
        if self.is_once() && self.next_after(now, tz).is_none() {
            return Err("That time has passed: pick a later one.".to_owned());
        }
        Ok(())
    }

    /// How late a missed start may still be taken when nobody has said: half
    /// an hour for an hourly one, twelve hours for a one-off, three hours for
    /// the rest.
    pub fn default_catch_up(&self) -> SignedDuration {
        match self {
            When::Hourly { .. } => SignedDuration::from_mins(30),
            When::Once { .. } => SignedDuration::from_hours(12),
            _ => SignedDuration::from_hours(3),
        }
    }

    /// `Every day at 2:00 AM`, `Weekdays at 9:00 AM`, `Mon, Wed and Fri at
    /// 6:00 PM`, `Every hour at :05`, `Once, Fri 20 Nov at 2:00 AM`.
    pub fn describe(&self) -> String {
        match self {
            When::Once { day, time } => format!("Once, {} at {}", day.label(), time.label()),
            When::Daily { time } => format!("Every day at {}", time.label()),
            When::Weekdays { time } => format!("Weekdays at {}", time.label()),
            When::Days { days, time } => {
                let mut days = days.clone();
                days.sort();
                days.dedup();
                let names: Vec<&str> = days.iter().map(|day| day.short()).collect();
                let listed = match names.split_last() {
                    Some((last, [])) => (*last).to_owned(),
                    Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
                    None => "No day".to_owned(),
                };
                format!("{listed} at {}", time.label())
            }
            When::Hourly { minute } => format!("Every hour at :{minute:02}"),
        }
    }

    /// The first start strictly after `after`, or `None` for a one-off whose
    /// moment has passed.
    pub fn next_after(&self, after: Timestamp, tz: &TimeZone) -> Option<Timestamp> {
        let now = after.to_zoned(tz.clone()).datetime();
        match self {
            When::Once { day, time } => resolve(day.date(), *time, tz).filter(|at| *at > after),
            When::Hourly { minute } => {
                let mut hour = start_of_hour(now)?;
                // A day and a half of hours covers any day the clocks change.
                for _ in 0..36 {
                    let at = resolve_at(hour, *minute, tz);
                    if let Some(at) = at.filter(|at| *at > after) {
                        return Some(at);
                    }
                    hour = next_hour(hour)?;
                }
                None
            }
            _ => {
                let time = self.time()?;
                let mut date = now.date();
                for _ in 0..9 {
                    if self.falls_on(date)
                        && let Some(at) = resolve(date, time, tz).filter(|at| *at > after)
                    {
                        return Some(at);
                    }
                    date = date.tomorrow().ok()?;
                }
                None
            }
        }
    }

    /// The latest start at or before `now`, or `None` when there hasn't been
    /// one: a one-off still to come.
    pub fn latest_until(&self, now: Timestamp, tz: &TimeZone) -> Option<Timestamp> {
        let local = now.to_zoned(tz.clone()).datetime();
        match self {
            When::Once { day, time } => resolve(day.date(), *time, tz).filter(|at| *at <= now),
            When::Hourly { minute } => {
                // From a few hours ahead on the wall clock: while the clocks
                // go back, a later wall-clock hour's first pass can already
                // be behind us — two hours of it, where a zone falls back
                // two.
                let mut hour = start_of_hour(local)?;
                for _ in 0..3 {
                    hour = next_hour(hour)?;
                }
                for _ in 0..40 {
                    let at = resolve_at(hour, *minute, tz);
                    if let Some(at) = at.filter(|at| *at <= now) {
                        return Some(at);
                    }
                    hour = previous_hour(hour)?;
                }
                None
            }
            _ => {
                let time = self.time()?;
                let mut date = local.date();
                for _ in 0..9 {
                    if self.falls_on(date)
                        && let Some(at) = resolve(date, time, tz).filter(|at| *at <= now)
                    {
                        return Some(at);
                    }
                    date = date.yesterday().ok()?;
                }
                None
            }
        }
    }

    /// Whether a start has come due since `since`, and if so whether to take
    /// it: on time, late but within `catch_up`, or missed.
    ///
    /// Only the latest start counts. A Mac asleep through three nightly
    /// starts runs once on waking, not three times, and the two before it
    /// are not reported: what was skipped is the work of one run, done once.
    pub fn due(
        &self,
        since: Timestamp,
        now: Timestamp,
        catch_up: SignedDuration,
        tz: &TimeZone,
    ) -> Option<Due> {
        let at = self.latest_until(now, tz).filter(|at| *at > since)?;
        let late = now.duration_since(at);
        Some(if late <= ON_TIME {
            Due::OnTime(at)
        } else if late <= catch_up {
            Due::Late(at)
        } else {
            Due::Missed(at)
        })
    }

    /// The wall-clock time a daily kind starts at; `None` for the others.
    fn time(&self) -> Option<Clock> {
        match self {
            When::Daily { time } | When::Weekdays { time } | When::Days { time, .. } => Some(*time),
            When::Once { .. } | When::Hourly { .. } => None,
        }
    }

    /// Whether a daily kind starts on `date`.
    fn falls_on(&self, date: Date) -> bool {
        let day = Dow::of(date.weekday());
        match self {
            When::Daily { .. } => true,
            When::Weekdays { .. } => day.is_weekday(),
            When::Days { days, .. } => days.contains(&day),
            When::Once { .. } | When::Hourly { .. } => false,
        }
    }
}

/// `time` on `date` in `tz`: forward past a gap, the first of a fold.
fn resolve(date: Date, time: Clock, tz: &TimeZone) -> Option<Timestamp> {
    let local = DateTime::new(
        date.year(),
        date.month(),
        date.day(),
        i8::try_from(time.hour).ok()?,
        i8::try_from(time.minute).ok()?,
        0,
        0,
    )
    .ok()?;
    tz.to_ambiguous_zoned(local)
        .compatible()
        .ok()
        .map(|at| at.timestamp())
}

/// `minute` past the hour that starts at `hour`.
fn resolve_at(hour: DateTime, minute: u8, tz: &TimeZone) -> Option<Timestamp> {
    let time = Clock::new(u8::try_from(hour.hour()).ok()?, minute)?;
    resolve(hour.date(), time, tz)
}

fn start_of_hour(at: DateTime) -> Option<DateTime> {
    DateTime::new(at.year(), at.month(), at.day(), at.hour(), 0, 0, 0).ok()
}

fn next_hour(hour: DateTime) -> Option<DateTime> {
    if hour.hour() == 23 {
        let date = hour.date().tomorrow().ok()?;
        DateTime::new(date.year(), date.month(), date.day(), 0, 0, 0, 0).ok()
    } else {
        DateTime::new(
            hour.year(),
            hour.month(),
            hour.day(),
            hour.hour() + 1,
            0,
            0,
            0,
        )
        .ok()
    }
}

fn previous_hour(hour: DateTime) -> Option<DateTime> {
    if hour.hour() == 0 {
        let date = hour.date().yesterday().ok()?;
        DateTime::new(date.year(), date.month(), date.day(), 23, 0, 0, 0).ok()
    } else {
        DateTime::new(
            hour.year(),
            hour.month(),
            hour.day(),
            hour.hour() - 1,
            0,
            0,
            0,
        )
        .ok()
    }
}
