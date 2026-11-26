//! What a stop at a limit, and its reset, are worth saying, look by look.

use std::time::{Duration, Instant};

use ket_remote::proto::{self, Activity, session_limit};

use super::*;

const HOUR: u64 = 60 * 60 * 1000;

fn limit(since_ms: u64, resets_at_ms: u64) -> proto::SessionLimit {
    proto::SessionLimit {
        scope: session_limit::Scope::Plan as i32,
        window: session_limit::Window::Session as i32,
        since_ms,
        resets_at_ms,
        said: "You've hit your usage limit.".to_owned(),
    }
}

fn terminal(id: u64, agent: &str, limit: Option<proto::SessionLimit>) -> proto::TerminalSummary {
    proto::TerminalSummary {
        id,
        agent: agent.to_owned(),
        unread: true,
        limit,
        ..Default::default()
    }
}

fn worktree(id: &str, activity: Activity, agent: &str) -> proto::WorktreeSummary {
    proto::WorktreeSummary {
        id: id.to_owned(),
        name: id.to_owned(),
        activity: activity as i32,
        agent: agent.to_owned(),
        ..Default::default()
    }
}

fn host(worktrees: Vec<proto::WorktreeSummary>) -> proto::HostSnapshot {
    proto::HostSnapshot {
        projects: vec![proto::ProjectSummary {
            id: "p".to_owned(),
            name: "web".to_owned(),
            worktrees,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// `worktree` with one Codex tab, stopped at `limit` or not.
fn codex(
    id: &str,
    activity: Activity,
    limit: Option<proto::SessionLimit>,
) -> proto::WorktreeSummary {
    proto::WorktreeSummary {
        terminals: vec![terminal(7, "codex", limit)],
        ..worktree(id, activity, "codex")
    }
}

/// Looks with no delay, so what is news comes straight back.
fn look(notices: &mut Notices, snapshot: &proto::HostSnapshot) -> Vec<Notice> {
    notices.look(snapshot, Instant::now(), Duration::ZERO, &[])
}

/// How long news waits in the looks that give it time to stop being so.
const AFTER: Duration = Duration::from_secs(20);

#[test]
fn a_stop_at_a_limit_is_said_in_place_of_finished() {
    let now = crate::now_ms();
    let mut notices = Notices::default();
    look(
        &mut notices,
        &host(vec![codex("w1", Activity::Working, None)]),
    );

    // The turn ends and, a tick later, the ledger says why. Both are news
    // in one look here; the limit's notice is the one that goes.
    let stopped = host(vec![codex(
        "w1",
        Activity::Quiet,
        Some(limit(now, now + 2 * HOUR)),
    )]);
    let due = look(&mut notices, &stopped);
    assert_eq!(due.len(), 1, "one notice, not a finished and a limit");
    let notice = &due[0];
    assert_eq!(notice.kind, Kind::Limit);
    assert_eq!(notice.id, "w1/finished");
    assert_eq!(notice.title, "web \u{b7} w1");
    assert_eq!(
        notice.body,
        "Codex has reached its usage limit and resets in about 2 hours."
    );
    assert_eq!(notice.opens, Opens::Chat(7));

    // The same stop, looked at again, is not said again.
    assert!(look(&mut notices, &stopped).is_empty());
    assert!(look(&mut notices, &stopped).is_empty());
}

#[test]
fn a_stop_carried_on_from_before_its_notice_went_is_not_said() {
    let now = crate::now_ms();
    let mut notices = Notices::default();
    look(
        &mut notices,
        &host(vec![codex("w1", Activity::Working, None)]),
    );
    let stopped = host(vec![codex("w1", Activity::Quiet, Some(limit(now, 0)))]);
    let at = Instant::now();
    assert!(
        notices.look(&stopped, at, AFTER, &[]).is_empty(),
        "it waits"
    );
    // Switched to another model at the Mac: at work, the mark gone.
    let going = host(vec![codex("w1", Activity::Working, None)]);
    assert!(notices.look(&going, at + 2 * AFTER, AFTER, &[]).is_empty());
}

#[test]
fn a_limit_resetting_is_said_once_for_an_agent_while_nobody_has_carried_on() {
    let now = crate::now_ms();
    let mut notices = Notices::default();
    let stopped = |resets_at_ms| {
        host(vec![
            codex("w1", Activity::Quiet, Some(limit(now - HOUR, resets_at_ms))),
            codex("w2", Activity::Quiet, Some(limit(now - HOUR, resets_at_ms))),
        ])
    };
    // First seen already stopped: that is how things are, not news.
    look(&mut notices, &stopped(now + HOUR));
    assert!(look(&mut notices, &stopped(now + HOUR)).is_empty());

    // The reset passes.
    let reset = stopped(now - 1);
    let due = look(&mut notices, &reset);
    assert_eq!(due.len(), 1, "one for Codex, not one for each worktree");
    assert_eq!(due[0].kind, Kind::Reset);
    assert_eq!(due[0].id, "reset/codex");
    assert_eq!(
        due[0].body,
        "Codex\u{2019}s limit has reset. It can carry on."
    );
    assert!(look(&mut notices, &reset).is_empty(), "said once");
}

#[test]
fn a_reset_nobody_is_waiting_on_is_not_said() {
    let now = crate::now_ms();
    let mut notices = Notices::default();
    let stopped = |resets_at_ms| {
        host(vec![codex(
            "w1",
            Activity::Quiet,
            Some(limit(now - HOUR, resets_at_ms)),
        )])
    };
    look(&mut notices, &stopped(now + HOUR));
    let at = Instant::now();
    assert!(notices.look(&stopped(now - 1), at, AFTER, &[]).is_empty());
    // Carried on in another agent before the notice went: the mark is off.
    let going = host(vec![codex("w1", Activity::Working, None)]);
    assert!(notices.look(&going, at + 2 * AFTER, AFTER, &[]).is_empty());

    // And a limit with no reset named never resets.
    let mut notices = Notices::default();
    look(&mut notices, &stopped(0));
    assert!(look(&mut notices, &stopped(0)).is_empty());
}

#[test]
fn a_stop_in_a_tab_since_put_to_sleep_still_resets() {
    let now = crate::now_ms();
    let mut notices = Notices::default();
    let asleep = |resets_at_ms| {
        host(vec![proto::WorktreeSummary {
            asleep: vec![proto::AsleepTerminal {
                key: "w1|codex|1".to_owned(),
                agent: "codex".to_owned(),
                limit: Some(limit(now - HOUR, resets_at_ms)),
                ..Default::default()
            }],
            ..worktree("w1", Activity::Quiet, "")
        }])
    };
    look(&mut notices, &asleep(now + HOUR));
    let due = look(&mut notices, &asleep(now - 1));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].kind, Kind::Reset);
    assert_eq!(
        due[0].opens,
        Opens::Worktree,
        "no chat to open until it is woken"
    );
}

#[test]
fn a_models_limit_and_an_unnamed_reset_are_worded_as_what_they_are() {
    let now = 1_000_000;
    let model = proto::SessionLimit {
        scope: session_limit::Scope::Model as i32,
        ..limit(now, 0)
    };
    assert_eq!(
        limit_words("Claude", &model, now),
        "Claude has reached its limit for this model."
    );
    assert_eq!(
        limit_words("Codex", &limit(now, 0), now),
        "Codex has reached its usage limit."
    );
    // A reset already behind us is not one to come.
    assert_eq!(
        limit_words("Codex", &limit(now, now - 1), now),
        "Codex has reached its usage limit."
    );
}

#[test]
fn a_wait_is_said_to_the_nearest_unit_a_person_plans_by() {
    const MINUTE: u64 = 60_000;
    for (ms, said) in [
        (1, "1 minute"),
        (MINUTE, "1 minute"),
        (41 * MINUTE + 1, "42 minutes"),
        (89 * MINUTE, "89 minutes"),
        (90 * MINUTE, "2 hours"),
        (2 * HOUR + 20 * MINUTE, "2 hours"),
        (35 * HOUR, "35 hours"),
        (36 * HOUR, "2 days"),
        (5 * 24 * HOUR + 3 * HOUR, "5 days"),
    ] {
        assert_eq!(about(ms), said, "{ms}");
    }
}

#[test]
fn a_phone_from_before_limits_hears_a_stop_as_a_failure_and_no_reset() {
    use super::super::pushes::{Environment, Kinds, Registration};
    let wants = |kinds: Kinds, kind| {
        Registration {
            token: vec![7; 32],
            environment: Environment::Sandbox,
            key: [9; 32],
            kinds,
            registered_at: 0,
        }
        .wants(kind)
    };
    let finished = Kinds {
        finished: true,
        ..Kinds::default()
    };
    let limits = Kinds {
        limits: true,
        ..Kinds::default()
    };
    assert!(wants(finished, Kind::Limit));
    assert!(!wants(finished, Kind::Reset));
    assert!(wants(limits, Kind::Limit));
    assert!(wants(limits, Kind::Reset));
    assert!(!wants(Kinds::default(), Kind::Limit));
}
