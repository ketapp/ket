//! Automations themselves — when one is due, what its runs are called — and
//! the files they and their runs are kept in.

use jiff::civil::DateTime;
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

fn ms(at: Timestamp) -> u64 {
    u64::try_from(at.as_millisecond()).unwrap()
}

fn clock(h: u8, m: u8) -> Clock {
    Clock::new(h, m).unwrap()
}

fn day(y: i16, m: i8, d: i8) -> Day {
    Day::new(jiff::civil::Date::new(y, m, d).unwrap())
}

/// One made on Thursday 19 November 2026 at ten in the morning.
fn nightly() -> Automation {
    let mut automation = Automation::new(
        &[],
        crate::id::ProjectId::new("ket"),
        "Audit dependencies",
        Task::Prompt {
            text: "Audit the dependencies.".to_owned(),
            agent: Some("claude".to_owned()),
            model: None,
            effort: None,
        },
        When::Daily { time: clock(2, 0) },
    );
    automation.created_ms = ms(at(2026, 11, 19, 10, 0));
    automation
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ket-automation-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_file_reads_like_the_schedule() {
    let mut automation = nightly();
    automation.when = When::Days {
        days: vec![Dow::Mon, Dow::Wed],
        time: clock(9, 30),
    };
    let json = serde_json::to_value(&automation).unwrap();
    assert_eq!(json["when"]["repeat"], "days");
    assert_eq!(json["when"]["days"], serde_json::json!(["mon", "wed"]));
    assert_eq!(json["when"]["time"], "09:30");
    assert_eq!(json["task"]["kind"], "prompt");
    assert_eq!(json["place"]["kind"], "new_worktree");
    let back: Automation = serde_json::from_value(json).unwrap();
    assert_eq!(back, automation);

    let once: When =
        serde_json::from_str(r#"{"repeat":"once","day":"2026-11-23","time":"16:00"}"#).unwrap();
    assert_eq!(once.describe(), "Once, Mon 23 Nov at 4:00 PM");
    assert!(serde_json::from_str::<When>(r#"{"repeat":"daily","time":"25:00"}"#).is_err());
}

#[test]
fn an_old_file_without_the_newer_fields_still_loads() {
    let json = r#"{
        "id": "abc", "name": "x", "project": "ket",
        "task": {"kind": "command", "line": "npm run e2e"},
        "when": {"repeat": "hourly", "minute": 0},
        "created_ms": 1, "updated_ms": 1
    }"#;
    let automation: Automation = serde_json::from_str(json).unwrap();
    assert!(automation.enabled);
    assert!(automation.skip_while_busy);
    assert_eq!(automation.place, Place::NewWorktree { base: None });
}

#[test]
fn a_start_is_taken_on_time_late_or_not_at_all() {
    let tz = la();
    let automation = nightly();
    let due = |now: Timestamp| automation.due(now, &tz);
    assert_eq!(due(at(2026, 11, 19, 15, 0)), None, "nothing due yet");
    let two = at(2026, 11, 20, 2, 0);
    assert_eq!(
        due(two.checked_add(SignedDuration::from_secs(30)).unwrap()),
        Some(Due::OnTime(two))
    );
    assert_eq!(
        due(at(2026, 11, 20, 4, 0)),
        Some(Due::Late(two)),
        "within three hours"
    );
    assert_eq!(due(at(2026, 11, 20, 6, 0)), Some(Due::Missed(two)));

    let mut never_late = nightly();
    never_late.catch_up_minutes = Some(0);
    assert_eq!(
        never_late.due(at(2026, 11, 20, 2, 10), &tz),
        Some(Due::Missed(two))
    );
}

#[test]
fn a_start_dealt_with_or_from_before_it_existed_is_not_due() {
    let tz = la();
    let mut automation = nightly();
    let two = at(2026, 11, 20, 2, 0);
    automation.handled(two);
    assert_eq!(automation.due(at(2026, 11, 20, 4, 0), &tz), None);
    assert_eq!(
        automation.due(at(2026, 11, 21, 2, 1), &tz),
        Some(Due::OnTime(at(2026, 11, 21, 2, 0)))
    );

    let mut made_late = nightly();
    made_late.created_ms = ms(at(2026, 11, 20, 3, 0));
    assert_eq!(made_late.due(at(2026, 11, 20, 4, 0), &tz), None);
}

#[test]
fn only_the_latest_of_several_missed_starts_counts() {
    let tz = la();
    let automation = nightly();
    // Asleep from Thursday night to Sunday at three in the morning.
    let sunday = at(2026, 11, 22, 3, 0);
    assert_eq!(
        automation.due(sunday, &tz),
        Some(Due::Late(at(2026, 11, 22, 2, 0)))
    );
}

#[test]
fn a_paused_or_finished_automation_does_not_start() {
    let tz = la();
    let mut paused = nightly();
    paused.enabled = false;
    assert_eq!(paused.due(at(2026, 11, 20, 2, 0), &tz), None);
    assert_eq!(paused.next_start(at(2026, 11, 19, 15, 0), &tz), None);

    let mut once = nightly();
    once.when = When::Once {
        day: day(2026, 11, 20),
        time: clock(2, 0),
    };
    assert!(once.next_start(at(2026, 11, 19, 15, 0), &tz).is_some());
    once.handled(at(2026, 11, 20, 2, 0));
    assert!(once.finished());
    assert_eq!(once.next_start(at(2026, 11, 19, 15, 0), &tz), None);
}

#[test]
fn each_run_gets_a_branch_of_its_own() {
    let tz = la();
    let mut automation = nightly();
    let two = at(2026, 11, 20, 2, 0);
    assert_eq!(automation.branch_for(two, &tz), "audit-dependencies-1120");
    automation.when = When::Hourly { minute: 5 };
    assert_eq!(
        automation.branch_for(at(2026, 11, 20, 15, 5), &tz),
        "audit-dependencies-1120-1505"
    );
    automation.when = When::Once {
        day: day(2026, 11, 20),
        time: clock(2, 0),
    };
    assert_eq!(automation.branch_for(two, &tz), "audit-dependencies");
    automation.name = "!!!".to_owned();
    automation.when = When::Daily { time: clock(2, 0) };
    assert_eq!(automation.branch_for(two, &tz), "automation-1120");
}

#[test]
fn upcoming_is_soonest_first_without_the_paused() {
    let tz = la();
    let mut all = Automations::default();
    let nightly = nightly();
    let mut morning = Automation::new(
        std::slice::from_ref(&nightly),
        crate::id::ProjectId::new("ket"),
        "Triage",
        Task::Command {
            line: "true".to_owned(),
        },
        When::Weekdays { time: clock(9, 0) },
    );
    morning.created_ms = nightly.created_ms;
    let mut paused = morning.clone();
    paused.id = "paused".to_owned();
    paused.enabled = false;
    all.automations = vec![morning, nightly, paused];
    let upcoming = all.upcoming(at(2026, 11, 19, 15, 0), &tz);
    let names: Vec<&str> = upcoming.iter().map(|(a, _)| a.name.as_str()).collect();
    assert_eq!(names, ["Audit dependencies", "Triage"]);
}

#[test]
fn the_file_keeps_what_is_saved_and_refuses_a_schedule_that_never_starts() {
    let root = temp_root("file");
    assert_eq!(Automations::load_in(&root).unwrap(), Automations::default());
    assert!(Automations::modified_in(&root).is_none());

    let automation = nightly();
    let id = automation.id.clone();
    Automations::update_in(&root, |all| {
        all.automations.push(automation.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(
        Automations::load_in(&root).unwrap().get(&id),
        Some(&automation)
    );
    assert!(Automations::modified_in(&root).is_some());

    let refused = Automations::update_in(&root, |all| {
        all.get_mut(&id).unwrap().when = When::Days {
            days: Vec::new(),
            time: clock(9, 0),
        };
        Ok(())
    });
    assert!(refused.is_err());
    assert_eq!(
        Automations::load_in(&root).unwrap().get(&id),
        Some(&automation)
    );

    std::fs::write(root.join("automations.json"), "{ not json").unwrap();
    assert!(Automations::load_in(&root).is_err());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn deleting_an_automation_forgets_its_runs_and_no_others() {
    let root = temp_root("delete");
    let automation = nightly();
    let id = automation.id.clone();
    Automations::update_in(&root, |all| {
        all.automations.push(automation);
        Ok(())
    })
    .unwrap();
    let run = |of: &str| Run {
        automation: of.to_owned(),
        due_ms: 1,
        at_ms: crate::now_ms(),
        outcome: Outcome::Done,
        worktree: None,
        branch: None,
        ended_ms: None,
        tokens: None,
        command: None,
    };
    Runs::update_in(&root, |runs| {
        runs.record(run(&id));
        runs.record(run("other"));
    })
    .unwrap();
    Automations::delete_in(&root, &id).unwrap();
    assert!(Automations::load_in(&root).unwrap().get(&id).is_none());
    let runs = Runs::load_in(&root).unwrap();
    assert_eq!(runs.of(&id).count(), 0);
    assert_eq!(runs.of("other").count(), 1);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_one_off_that_has_run_does_not_run_again_in_another_time_zone() {
    let new_york = TimeZone::get("America/New_York").unwrap();
    let mut once = nightly();
    once.when = When::Once {
        day: day(2026, 11, 20),
        time: clock(9, 0),
    };
    let nine_in_new_york = DateTime::new(2026, 11, 20, 9, 0, 0, 0)
        .unwrap()
        .to_zoned(new_york)
        .unwrap()
        .timestamp();
    once.handled(nine_in_new_york);
    // The Mac flies west: nine o'clock comes round again in Los Angeles.
    assert_eq!(once.due(at(2026, 11, 20, 9, 30), &la()), None);
}

#[test]
fn a_start_missed_while_paused_is_not_taken_on_resuming() {
    let tz = la();
    let mut automation = nightly();
    automation.set_enabled(false);
    automation.set_enabled(true);
    assert!(automation.resumed_ms.is_some());
    // Paused through Friday's 2:00 start, back on at 3:00.
    automation.resumed_ms = Some(ms(at(2026, 11, 20, 3, 0)));
    assert_eq!(automation.due(at(2026, 11, 20, 4, 0), &tz), None);
    let saturday = at(2026, 11, 21, 2, 0);
    assert_eq!(
        automation.due(at(2026, 11, 21, 2, 1), &tz),
        Some(Due::OnTime(saturday))
    );
}

#[test]
fn a_branch_already_taken_gets_a_number() {
    let tz = la();
    let automation = nightly();
    let two = at(2026, 11, 20, 2, 0);
    let taken = ["audit-dependencies-1120", "audit-dependencies-1120-2"];
    let branch = automation.unused_branch(two, &tz, |name| taken.contains(&name));
    assert_eq!(branch, "audit-dependencies-1120-3");
    assert_eq!(
        automation.unused_branch(two, &tz, |_| false),
        "audit-dependencies-1120"
    );
}

#[test]
fn the_files_are_for_their_owner_alone() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = temp_root("private");
    Automations::update_in(&root, |all| {
        all.automations.push(nightly());
        Ok(())
    })
    .unwrap();
    Runs::update_in(&root, |_| {}).unwrap();
    for name in ["automations.json", "runs.json"] {
        let mode = std::fs::metadata(root.join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn changes_from_two_processes_at_once_are_both_kept() {
    let root = temp_root("lock");
    let writers: Vec<_> = (0..2)
        .map(|writer| {
            let root = root.clone();
            std::thread::spawn(move || {
                for n in 0..25 {
                    Automations::update_in(&root, |all| {
                        let mut automation = nightly();
                        automation.id = format!("{writer}-{n}");
                        all.automations.push(automation);
                        Ok(())
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(Automations::load_in(&root).unwrap().automations.len(), 50);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_delete_that_cannot_clear_the_history_leaves_the_automation() {
    let root = temp_root("delete-refused");
    let automation = nightly();
    let id = automation.id.clone();
    Automations::update_in(&root, |all| {
        all.automations.push(automation);
        Ok(())
    })
    .unwrap();
    std::fs::write(root.join("runs.json"), "{ not json").unwrap();
    assert!(Automations::delete_in(&root, &id).is_err());
    assert!(Automations::load_in(&root).unwrap().get(&id).is_some());
    std::fs::remove_dir_all(&root).ok();
}
