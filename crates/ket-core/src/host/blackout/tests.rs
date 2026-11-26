//! Blackout hours in a real time zone: overnight, same-day, chosen days,
//! being let in, and a night the clocks change.

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::*;

fn la() -> TimeZone {
    TimeZone::get("America/Los_Angeles").unwrap()
}

/// `y-m-d h:mi` on Los Angeles' clock.
fn at(y: i16, m: i8, d: i8, h: i8, mi: i8) -> Timestamp {
    la().to_ambiguous_zoned(DateTime::new(y, m, d, h, mi, 0, 0).unwrap())
        .compatible()
        .unwrap()
        .timestamp()
}

/// Ten at night until seven in the morning, every day.
fn nightly() -> Blackout {
    Blackout {
        enabled: true,
        ..Blackout::default()
    }
}

fn seconds(at: Timestamp) -> u64 {
    u64::try_from(at.as_second()).unwrap()
}

#[test]
fn off_there_are_no_blackouts() {
    let off = Blackout::default();
    let midnight = at(2026, 11, 24, 0, 0);
    assert!(off.windows(midnight, &la(), 14).is_empty());
    assert_eq!(off.standing(midnight, &la()), Standing::default());
}

#[test]
fn an_overnight_blackout_runs_into_the_next_morning() {
    let blackout = nightly();
    // Tuesday 24 November 2026.
    let evening = blackout.standing(at(2026, 11, 24, 21, 59), &la());
    assert_eq!(evening.now, None);
    assert_eq!(evening.shut_until(), None);
    assert_eq!(
        evening.next,
        Some(Window {
            start: at(2026, 11, 24, 22, 0),
            end: at(2026, 11, 25, 7, 0),
        })
    );

    let starting = blackout.standing(at(2026, 11, 24, 22, 0), &la());
    assert_eq!(starting.shut_until(), Some(at(2026, 11, 25, 7, 0)));

    // After midnight it is still last night's.
    let small_hours = blackout.standing(at(2026, 11, 25, 3, 30), &la());
    assert_eq!(small_hours.shut_until(), Some(at(2026, 11, 25, 7, 0)));
    assert_eq!(
        small_hours.next.map(|window| window.start),
        Some(at(2026, 11, 25, 22, 0))
    );

    // It ends on the minute.
    let morning = blackout.standing(at(2026, 11, 25, 7, 0), &la());
    assert_eq!(morning.now, None);
}

#[test]
fn a_blackout_inside_one_day_ends_that_day() {
    let lunch = Blackout {
        enabled: true,
        from: clock(12, 0),
        until: clock(13, 30),
        ..Blackout::default()
    };
    assert_eq!(lunch.minutes(), 90);
    let standing = lunch.standing(at(2026, 11, 24, 12, 45), &la());
    assert_eq!(standing.shut_until(), Some(at(2026, 11, 24, 13, 30)));
    assert_eq!(lunch.standing(at(2026, 11, 24, 14, 0), &la()).now, None);
}

#[test]
fn the_same_time_twice_is_a_whole_day() {
    let all_day = Blackout {
        enabled: true,
        from: clock(9, 0),
        until: clock(9, 0),
        days: vec![Dow::Sat],
        ..Blackout::default()
    };
    assert_eq!(all_day.minutes(), 24 * 60);
    // Saturday 28 November 2026.
    assert_eq!(
        all_day
            .standing(at(2026, 11, 28, 20, 0), &la())
            .shut_until(),
        Some(at(2026, 11, 29, 9, 0))
    );
    assert_eq!(all_day.standing(at(2026, 11, 29, 10, 0), &la()).now, None);
}

#[test]
fn chosen_days_are_the_days_a_blackout_starts_on() {
    let weeknights = Blackout {
        enabled: true,
        days: vec![Dow::Mon, Dow::Tue, Dow::Wed, Dow::Thu, Dow::Fri],
        ..Blackout::default()
    };
    assert_eq!(weeknights.repeat(), Repeat::Weekdays);
    assert_eq!(weeknights.repeat_label(), "Every weekday");
    // Friday night's runs into Saturday morning.
    assert!(
        weeknights
            .standing(at(2026, 11, 28, 2, 0), &la())
            .shut_until()
            .is_some()
    );
    // Saturday night has none, and the next starts on Monday.
    let saturday = weeknights.standing(at(2026, 11, 28, 23, 0), &la());
    assert_eq!(saturday.now, None);
    assert_eq!(
        saturday.next.map(|window| window.start),
        Some(at(2026, 11, 30, 22, 0))
    );

    let some = Blackout {
        days: vec![Dow::Fri, Dow::Mon, Dow::Wed],
        ..nightly()
    };
    assert_eq!(some.repeat(), Repeat::Days);
    assert_eq!(some.repeat_label(), "Mon, Wed and Fri");
    assert_eq!(nightly().repeat_label(), "Every day");
    let every = Blackout {
        days: Dow::ALL.to_vec(),
        ..nightly()
    };
    assert_eq!(every.repeat(), Repeat::Daily);
}

#[test]
fn the_blackouts_ahead_start_with_the_one_in_force() {
    let windows = nightly().windows(at(2026, 11, 25, 3, 0), &la(), WINDOWS_AHEAD);
    assert_eq!(windows.len(), WINDOWS_AHEAD);
    assert_eq!(windows[0].start, at(2026, 11, 24, 22, 0));
    assert_eq!(windows[1].start, at(2026, 11, 25, 22, 0));
    assert!(windows.windows(2).all(|pair| pair[0].end <= pair[1].start));
}

#[test]
fn let_in_lifts_the_blackout_until_its_moment_and_no_longer() {
    let now = at(2026, 11, 24, 23, 0);
    let let_in = Blackout {
        let_in_until: seconds(now + SignedDuration::from_mins(15)),
        ..nightly()
    };
    let standing = let_in.standing(now, &la());
    assert!(standing.now.is_some());
    assert_eq!(standing.shut_until(), None);
    assert_eq!(
        standing.let_in_until,
        Some(now + SignedDuration::from_mins(15))
    );

    // Fifteen minutes on, it shuts again.
    let later = let_in.standing(now + SignedDuration::from_mins(15), &la());
    assert_eq!(later.let_in_until, None);
    assert_eq!(later.shut_until(), Some(at(2026, 11, 25, 7, 0)));

    // A moment set while no blackout is on means nothing.
    let afternoon = let_in.standing(at(2026, 11, 24, 15, 0), &la());
    assert_eq!(afternoon.let_in_until, None);
}

#[test]
fn the_hours_are_the_wall_clocks_through_a_clock_change() {
    // The clocks go forward on 14 March 2027: the night is an hour short.
    let spring = nightly()
        .standing(at(2027, 3, 14, 3, 30), &la())
        .now
        .unwrap();
    assert_eq!(spring.start, at(2027, 3, 13, 22, 0));
    assert_eq!(spring.end, at(2027, 3, 14, 7, 0));
    assert_eq!(
        spring.end.duration_since(spring.start),
        SignedDuration::from_hours(8)
    );
}

#[test]
fn the_setting_reads_back_as_written_and_tolerates_a_bare_file() {
    let blackout = Blackout {
        days: vec![Dow::Mon, Dow::Sun],
        hold_notifications: false,
        ..nightly()
    };
    let json = serde_json::to_string(&blackout).unwrap();
    assert_eq!(serde_json::from_str::<Blackout>(&json).unwrap(), blackout);
    assert!(json.contains(r#""from":"22:00""#), "{json}");
    assert_eq!(
        serde_json::from_str::<Blackout>("{}").unwrap(),
        Blackout::default()
    );
}
