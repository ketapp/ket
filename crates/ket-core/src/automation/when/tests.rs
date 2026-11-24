//! The schedule maths in a real time zone, through both of its clock
//! changes.

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::*;

fn la() -> TimeZone {
    TimeZone::get("America/Los_Angeles").unwrap()
}

/// `y-m-d h:mi` on Los Angeles' clock; the first of the two when it happens
/// twice.
fn at(y: i16, m: i8, d: i8, h: i8, mi: i8) -> Timestamp {
    la().to_ambiguous_zoned(DateTime::new(y, m, d, h, mi, 0, 0).unwrap())
        .compatible()
        .unwrap()
        .timestamp()
}

fn clock(h: u8, m: u8) -> Clock {
    Clock::new(h, m).unwrap()
}

fn day(y: i16, m: i8, d: i8) -> Day {
    Day::new(Date::new(y, m, d).unwrap())
}

fn local(at: Timestamp) -> (i8, i8, i8, i8) {
    let z = at.to_zoned(la());
    (z.month(), z.day(), z.hour(), z.minute())
}

/// One made on Thursday 19 November 2026 at ten in the morning.
#[test]
fn a_typed_time_is_read_the_way_a_person_means_it() {
    assert_eq!(Clock::parse("9"), Some(clock(9, 0)));
    assert_eq!(Clock::parse("9am"), Some(clock(9, 0)));
    assert_eq!(Clock::parse("9:30 pm"), Some(clock(21, 30)));
    assert_eq!(Clock::parse("9.30PM"), Some(clock(21, 30)));
    assert_eq!(Clock::parse("21:30"), Some(clock(21, 30)));
    assert_eq!(Clock::parse("0930"), Some(clock(9, 30)));
    assert_eq!(Clock::parse("12am"), Some(clock(0, 0)));
    assert_eq!(Clock::parse("12 pm"), Some(clock(12, 0)));
    assert_eq!(Clock::parse("13pm"), None);
    assert_eq!(Clock::parse("25"), None);
    assert_eq!(Clock::parse("9:75"), None);
    assert_eq!(Clock::parse("soon"), None);
}

#[test]
fn a_time_is_shown_on_a_twelve_hour_clock() {
    assert_eq!(clock(0, 0).label(), "12:00 AM");
    assert_eq!(clock(9, 5).label(), "9:05 AM");
    assert_eq!(clock(12, 30).label(), "12:30 PM");
    assert_eq!(clock(21, 0).label(), "9:00 PM");
}

#[test]
fn a_schedule_says_itself_in_words() {
    let time = clock(18, 0);
    assert_eq!(
        When::Daily { time: clock(2, 0) }.describe(),
        "Every day at 2:00 AM"
    );
    assert_eq!(
        When::Weekdays { time: clock(9, 0) }.describe(),
        "Weekdays at 9:00 AM"
    );
    let days = vec![Dow::Fri, Dow::Mon, Dow::Wed, Dow::Mon];
    assert_eq!(
        When::Days { days, time }.describe(),
        "Mon, Wed and Fri at 6:00 PM"
    );
    let days = vec![Dow::Sun];
    assert_eq!(When::Days { days, time }.describe(), "Sun at 6:00 PM");
    assert_eq!(When::Hourly { minute: 5 }.describe(), "Every hour at :05");
    let once = When::Once {
        day: day(2026, 11, 20),
        time: clock(2, 0),
    };
    assert_eq!(once.describe(), "Once, Fri 20 Nov at 2:00 AM");
}

#[test]
fn each_kind_finds_its_next_start() {
    let tz = la();
    // Thursday 19 November, three in the afternoon.
    let now = at(2026, 11, 19, 15, 0);
    let daily = When::Daily { time: clock(2, 0) };
    assert_eq!(local(daily.next_after(now, &tz).unwrap()), (11, 20, 2, 0));
    let weekdays = When::Weekdays { time: clock(9, 0) };
    let friday_late = at(2026, 11, 20, 10, 0);
    assert_eq!(
        local(weekdays.next_after(friday_late, &tz).unwrap()),
        (11, 23, 9, 0)
    );
    let days = When::Days {
        days: vec![Dow::Mon, Dow::Wed, Dow::Fri],
        time: clock(18, 0),
    };
    assert_eq!(local(days.next_after(now, &tz).unwrap()), (11, 20, 18, 0));
    let hourly = When::Hourly { minute: 5 };
    assert_eq!(
        local(hourly.next_after(at(2026, 11, 19, 15, 10), &tz).unwrap()),
        (11, 19, 16, 5)
    );
    assert_eq!(
        local(hourly.next_after(at(2026, 11, 19, 15, 4), &tz).unwrap()),
        (11, 19, 15, 5)
    );
    assert_eq!(
        local(hourly.next_after(at(2026, 11, 19, 23, 30), &tz).unwrap()),
        (11, 20, 0, 5)
    );
    let once = When::Once {
        day: day(2026, 11, 20),
        time: clock(2, 0),
    };
    assert_eq!(local(once.next_after(now, &tz).unwrap()), (11, 20, 2, 0));
    assert_eq!(once.next_after(at(2026, 11, 21, 0, 0), &tz), None);
}

#[test]
fn a_start_is_strictly_after_the_moment_asked_about() {
    let tz = la();
    let daily = When::Daily { time: clock(2, 0) };
    let two = at(2026, 11, 20, 2, 0);
    assert_eq!(local(daily.next_after(two, &tz).unwrap()), (11, 21, 2, 0));
    assert_eq!(daily.latest_until(two, &tz), Some(two));
}

#[test]
fn a_time_the_clocks_skip_starts_as_soon_after_as_there_is() {
    let tz = la();
    // 14 March 2027: Los Angeles goes from 1:59:59 PST to 3:00 PDT.
    let daily = When::Daily { time: clock(2, 30) };
    let start = daily.next_after(at(2027, 3, 14, 0, 0), &tz).unwrap();
    assert_eq!(local(start), (3, 14, 3, 30));
    assert_eq!(local(daily.next_after(start, &tz).unwrap()), (3, 15, 2, 30));
}

#[test]
fn a_time_the_clocks_repeat_starts_once_the_first_time() {
    let tz = la();
    // 1 November 2026: 1:00 to 1:59 happens in PDT, then again in PST.
    let daily = When::Daily { time: clock(1, 30) };
    let first = daily.next_after(at(2026, 11, 1, 0, 0), &tz).unwrap();
    assert_eq!(first.to_zoned(tz.clone()).offset().seconds(), -7 * 3600);
    let next = daily.next_after(first, &tz).unwrap();
    assert_eq!(local(next), (11, 2, 1, 30));
}

#[test]
fn the_latest_start_is_found_looking_back() {
    let tz = la();
    let weekdays = When::Weekdays { time: clock(9, 0) };
    let monday_early = at(2026, 11, 23, 8, 0);
    assert_eq!(
        local(weekdays.latest_until(monday_early, &tz).unwrap()),
        (11, 20, 9, 0)
    );
    let hourly = When::Hourly { minute: 0 };
    assert_eq!(
        local(hourly.latest_until(at(2026, 11, 19, 15, 30), &tz).unwrap()),
        (11, 19, 15, 0)
    );
    let once = When::Once {
        day: day(2026, 11, 20),
        time: clock(2, 0),
    };
    assert_eq!(once.latest_until(at(2026, 11, 19, 15, 0), &tz), None);
}

#[test]
fn a_start_is_due_on_time_late_or_missed() {
    let tz = la();
    let daily = When::Daily { time: clock(2, 0) };
    let made = at(2026, 11, 19, 10, 0);
    let three_hours = SignedDuration::from_hours(3);
    let due = |now: Timestamp| daily.due(made, now, three_hours, &tz);
    let two = at(2026, 11, 20, 2, 0);
    assert_eq!(due(at(2026, 11, 19, 15, 0)), None, "nothing due yet");
    assert_eq!(due(at(2026, 11, 20, 2, 1)), Some(Due::OnTime(two)));
    assert_eq!(due(at(2026, 11, 20, 4, 0)), Some(Due::Late(two)));
    assert_eq!(due(at(2026, 11, 20, 6, 0)), Some(Due::Missed(two)));
    assert_eq!(
        daily.due(two, at(2026, 11, 20, 4, 0), three_hours, &tz),
        None,
        "dealt with"
    );
    assert_eq!(
        daily.due(made, at(2026, 11, 20, 2, 10), SignedDuration::ZERO, &tz),
        Some(Due::Missed(two)),
        "no catching up at all"
    );
}

#[test]
fn a_time_with_a_character_from_another_script_is_not_a_time() {
    assert_eq!(Clock::parse("9：5"), None, "a full-width colon");
    assert_eq!(Clock::parse("９"), None, "a full-width digit");
    assert_eq!(Clock::parse(""), None);
    assert_eq!(Clock::parse("pm"), None);
}

#[test]
fn the_latest_hourly_start_is_found_while_a_zone_falls_back_two_hours() {
    // Troll, 25 October 2026: 03:00 +02 becomes 01:00 +00, so 01:00 to 02:59
    // happen twice. At the second 01:05 the latest start is the first 02:05.
    let tz = TimeZone::get("Antarctica/Troll").unwrap();
    let hourly = When::Hourly { minute: 5 };
    let second_one_oh_five: Timestamp = "2026-10-25T01:05:00Z".parse().unwrap();
    let first_two_oh_five: Timestamp = "2026-10-25T00:05:00Z".parse().unwrap();
    assert_eq!(
        hourly.latest_until(second_one_oh_five, &tz),
        Some(first_two_oh_five)
    );
}

#[test]
fn a_typed_day_is_read_when_it_can_only_be_one() {
    assert_eq!(Dow::parse("mon"), Some(Dow::Mon));
    assert_eq!(Dow::parse("Monday"), Some(Dow::Mon));
    assert_eq!(Dow::parse("tues"), Some(Dow::Tue));
    assert_eq!(Dow::parse("th"), Some(Dow::Thu));
    assert_eq!(Dow::parse(" SU "), Some(Dow::Sun));
    assert_eq!(Dow::parse("t"), None, "Tuesday or Thursday");
    assert_eq!(Dow::parse("s"), None, "Saturday or Sunday");
    assert_eq!(Dow::parse("funday"), None);
}

#[test]
fn a_time_steps_to_the_next_quarter_hour_round_the_clock() {
    assert_eq!(clock(9, 0).step(15), clock(9, 15));
    assert_eq!(clock(9, 7).step(15), clock(9, 15));
    assert_eq!(clock(9, 7).step(-15), clock(9, 0));
    assert_eq!(clock(9, 0).step(-15), clock(8, 45));
    assert_eq!(clock(23, 50).step(15), clock(0, 0));
    assert_eq!(clock(0, 0).step(-15), clock(23, 45));
}

#[test]
fn a_one_off_in_the_past_cannot_be_saved() {
    let tz = la();
    let now = at(2026, 11, 20, 10, 0);
    let once = |h| When::Once {
        day: day(2026, 11, 20),
        time: clock(h, 0),
    };
    assert!(once(9).check_at(now, &tz).unwrap_err().contains("passed"));
    assert!(once(11).check_at(now, &tz).is_ok());
    assert!(
        When::Daily { time: clock(9, 0) }.check_at(now, &tz).is_ok(),
        "a repeat always has a next"
    );
}
