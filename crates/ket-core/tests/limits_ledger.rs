//! The ledger of sessions stopped at a limit: what a refusal, a retry, a
//! reset and time each do to it.

use ket_core::limits::{Hit, Ledger, Limit, Mark, Scope, Window, resets_at};
use ket_core::rate_limits::RateWindow;

const HOUR: u64 = 60 * 60 * 1000;
const DAY: u64 = 24 * HOUR;

fn mark(agent: &str, session: &str, since_ms: u64, resets_at_ms: Option<u64>) -> Mark {
    Mark {
        agent: agent.to_owned(),
        session: session.to_owned(),
        account: String::new(),
        pane: String::new(),
        limit: Limit {
            scope: Scope::Plan,
            window: Window::Session,
            since_ms,
            resets_at_ms,
            said: "No more for now".to_owned(),
        },
    }
}

fn window(name: &str, used_percent: f32, resets_at_ms: Option<u64>) -> RateWindow {
    RateWindow {
        name: name.to_owned(),
        used_percent,
        resets_at_ms,
        window_minutes: None,
    }
}

#[test]
fn a_refused_session_is_marked_and_found_by_any_spelling_of_its_agent() {
    let mut ledger = Ledger::default();
    assert!(ledger.hit(mark("Codex", "s1", 1_000, Some(5_000))));
    assert_eq!(ledger.marks().len(), 1);
    assert_eq!(ledger.marks()[0].agent, "codex");
    for agent in ["codex", "Codex", "codex:work"] {
        assert_eq!(ledger.of(agent, "s1").unwrap().since_ms, 1_000, "{agent}");
    }
    assert!(
        ledger.of("claude", "s1").is_none(),
        "another agent's session"
    );
    assert!(ledger.of("codex", "s2").is_none());
}

#[test]
fn a_retry_refused_again_is_the_same_stop() {
    let mut ledger = Ledger::default();
    ledger.hit(mark("claude", "s1", 1_000, Some(9_000)));
    assert!(
        !ledger.hit(mark("claude", "s1", 1_000, Some(9_000))),
        "the same news twice changes nothing"
    );

    // Refused again later, with no reset time this time and other words.
    let mut again = mark("claude", "s1", 4_000, None);
    again.limit.said = "Still no".to_owned();
    again.limit.window = Window::Weekly;
    assert!(ledger.hit(again));
    let limit = ledger.of("claude", "s1").unwrap();
    assert_eq!(limit.since_ms, 1_000, "when it first stopped");
    assert_eq!(limit.resets_at_ms, Some(9_000), "the time it had is kept");
    assert_eq!(limit.said, "Still no");
    assert_eq!(limit.window, Window::Weekly);
    assert_eq!(ledger.marks().len(), 1);
}

#[test]
fn a_session_refused_after_its_window_reset_is_stopped_anew() {
    let mut ledger = Ledger::default();
    ledger.hit(mark("claude", "s1", 1_000, Some(9_000)));
    assert!(ledger.hit(mark("claude", "s1", 12_000, None)));
    let limit = ledger.of("claude", "s1").unwrap();
    assert_eq!(limit.since_ms, 12_000);
    assert_eq!(
        limit.resets_at_ms, None,
        "the old window's time is not this one's"
    );
}

#[test]
fn a_session_that_gets_going_again_loses_its_mark() {
    let mut ledger = Ledger::default();
    ledger.hit(mark("codex", "s1", 1_000, None));
    ledger.hit(mark("codex", "s2", 1_000, None));
    assert!(ledger.clear("codex", "s1"));
    assert!(!ledger.clear("codex", "s1"), "it had none left");
    assert!(ledger.of("codex", "s1").is_none());
    assert!(ledger.of("codex", "s2").is_some());
}

#[test]
fn a_reset_time_learned_later_fills_in_and_never_overwrites() {
    let mut ledger = Ledger::default();
    ledger.hit(mark("codex", "s1", 1_000, None));
    assert!(ledger.learn_reset("codex", "s1", 7_000));
    assert!(!ledger.learn_reset("codex", "s1", 8_000));
    assert_eq!(ledger.of("codex", "s1").unwrap().resets_at_ms, Some(7_000));
    assert!(!ledger.learn_reset("codex", "nobody", 7_000));
}

#[test]
fn a_limit_is_over_once_its_reset_has_passed_and_never_when_none_was_named() {
    let named = mark("claude", "s1", 1_000, Some(5_000)).limit;
    assert!(!named.over(4_999));
    assert!(named.over(5_000));
    assert!(!mark("claude", "s1", 1_000, None).limit.over(u64::MAX));
}

#[test]
fn old_marks_go_a_day_after_their_reset_or_after_eight_days_with_none() {
    let mut ledger = Ledger::default();
    ledger.hit(mark("claude", "reset", 0, Some(HOUR)));
    ledger.hit(mark("codex", "unnamed", 0, None));
    assert!(!ledger.prune(HOUR + DAY - 1));
    assert!(ledger.prune(HOUR + DAY));
    assert!(ledger.of("claude", "reset").is_none());
    assert!(ledger.of("codex", "unnamed").is_some());
    assert!(!ledger.prune(8 * DAY - 1));
    assert!(ledger.prune(8 * DAY));
    assert!(ledger.marks().is_empty());
}

#[test]
fn the_ledger_survives_being_written_down() {
    let mut ledger = Ledger::default();
    ledger.hit(Mark {
        account: "/config/work".to_owned(),
        ..mark("claude", "s1", 1_000, Some(5_000))
    });
    ledger.hit(mark("codex", "s2", 2_000, None));
    let written = serde_json::to_string(&ledger).unwrap();
    assert_eq!(serde_json::from_str::<Ledger>(&written).unwrap(), ledger);
    // Flat and camel-cased, like everything else ket writes.
    assert!(written.contains(r#""sinceMs":1000"#), "{written}");
    assert!(written.contains(r#""resetsAtMs":5000"#), "{written}");
    // An empty or older file is an empty ledger.
    assert_eq!(
        serde_json::from_str::<Ledger>("{}").unwrap(),
        Ledger::default()
    );
}

#[test]
fn the_reset_is_the_named_windows_or_the_latest_among_the_full_ones() {
    let now = 1_000;
    let windows = [
        window("Session", 100.0, Some(5_000)),
        window("Weekly", 100.0, Some(90_000)),
        window("Opus weekly", 40.0, Some(70_000)),
    ];
    let hit = |scope, window| Hit { scope, window };

    assert_eq!(
        resets_at(hit(Scope::Plan, Window::Session), &windows, now),
        Some(5_000)
    );
    assert_eq!(
        resets_at(hit(Scope::Plan, Window::Weekly), &windows, now),
        Some(90_000)
    );
    // Not said which: both are spent, and work resumes with the later.
    assert_eq!(
        resets_at(hit(Scope::Plan, Window::Unknown), &windows, now),
        Some(90_000)
    );
    // A model's limit names no plan window: only a full one can be it.
    assert_eq!(
        resets_at(hit(Scope::Model, Window::Unknown), &windows[2..], now),
        None
    );
}

#[test]
fn an_old_reading_gives_no_reset_rather_than_a_wrong_one() {
    let hit = Hit {
        scope: Scope::Plan,
        window: Window::Unknown,
    };
    // Nothing full: this reading is from before the limit was reached.
    assert_eq!(
        resets_at(hit, &[window("Session", 62.0, Some(5_000))], 1_000),
        None
    );
    // Full, but its reset is already behind us, or was never given.
    assert_eq!(
        resets_at(hit, &[window("Session", 100.0, Some(5_000))], 6_000),
        None
    );
    assert_eq!(
        resets_at(hit, &[window("Session", 100.0, None)], 1_000),
        None
    );
    // The named window's time has passed; a full one still ahead stands in.
    let named = Hit {
        scope: Scope::Plan,
        window: Window::Session,
    };
    assert_eq!(
        resets_at(
            named,
            &[
                window("Session", 100.0, Some(500)),
                window("Weekly", 99.5, Some(90_000)),
            ],
            1_000
        ),
        Some(90_000)
    );
    assert_eq!(resets_at(hit, &[], 1_000), None);
}

#[test]
fn a_mark_follows_its_session_from_pane_to_pane() {
    let mut ledger = Ledger::default();
    ledger.hit(Mark {
        pane: "wt|codex|1".to_owned(),
        ..mark("codex", "s1", 1_000, None)
    });
    assert_eq!(ledger.in_pane("wt|codex|1").unwrap().since_ms, 1_000);
    assert!(ledger.in_pane("wt|codex|2").is_none());
    assert!(ledger.in_pane("").is_none(), "no pane is not a pane");

    assert!(!ledger.seen_in("codex", "s1", "wt|codex|1"), "where it was");
    assert!(ledger.seen_in("codex", "s1", "wt|codex|2"));
    assert!(ledger.in_pane("wt|codex|1").is_none());
    assert!(ledger.in_pane("wt|codex|2").is_some());
    assert!(!ledger.seen_in("codex", "nobody", "wt|codex|3"));

    // Refused again with no pane named: it is still where it was.
    ledger.hit(mark("codex", "s1", 2_000, None));
    assert!(ledger.in_pane("wt|codex|2").is_some());
}

#[test]
fn a_tab_that_moves_on_to_another_session_leaves_the_old_stop_behind() {
    let mut ledger = Ledger::default();
    ledger.hit(Mark {
        pane: "wt|codex|1".to_owned(),
        ..mark("codex", "s1", 1_000, None)
    });

    // `/new` in that tab: the new session was never refused.
    assert!(ledger.seen_in("codex", "s2", "wt|codex|1"));
    assert!(
        ledger.in_pane("wt|codex|1").is_none(),
        "asleep, it isn't stopped"
    );
    assert!(
        ledger.of("codex", "s1").is_some(),
        "the old session still is"
    );
    assert!(
        !ledger.seen_in("codex", "s2", "wt|codex|1"),
        "no news twice"
    );

    // Both refused: the tab is stopped at its own session's limit.
    ledger.hit(Mark {
        pane: "wt|codex|1".to_owned(),
        ..mark("codex", "s2", 5_000, None)
    });
    ledger.seen_in("codex", "s1", "wt|codex|2");
    assert_eq!(ledger.in_pane("wt|codex|1").unwrap().since_ms, 5_000);
    assert_eq!(ledger.in_pane("wt|codex|2").unwrap().since_ms, 1_000);
}

fn limit(scope: Scope, since_ms: u64, resets_at_ms: Option<u64>) -> Limit {
    Limit {
        scope,
        window: Window::Session,
        since_ms,
        resets_at_ms,
        said: String::new(),
    }
}

#[test]
fn a_stop_is_said_in_one_sentence_with_its_wait() {
    use ket_core::limits::stop_sentence;
    let minute = 60 * 1000;
    let plan = limit(Scope::Plan, 0, Some(2 * HOUR + 6 * minute));
    assert_eq!(
        stop_sentence("Codex", &plan, 0),
        "Codex has reached its usage limit and resets in 2 hours 6 minutes."
    );
    let model = limit(Scope::Model, 0, None);
    assert_eq!(
        stop_sentence("Claude", &model, 0),
        "Claude has reached its limit for this model."
    );
    // Past the reset there is no wait to name.
    assert_eq!(
        stop_sentence("Codex", &plan, 3 * HOUR),
        "Codex has reached its usage limit."
    );
}

#[test]
fn a_wait_is_its_two_largest_units() {
    use ket_core::limits::wait_words;
    let minute = 60 * 1000;
    assert_eq!(wait_words(1), "1 minute");
    assert_eq!(wait_words(45 * minute), "45 minutes");
    assert_eq!(wait_words(HOUR), "1 hour");
    assert_eq!(wait_words(2 * HOUR + 6 * minute), "2 hours 6 minutes");
    assert_eq!(wait_words(DAY + 3 * HOUR + 20 * minute), "1 day 3 hours");
    assert_eq!(wait_words(2 * DAY + 10 * minute), "2 days");
}

#[test]
fn the_bar_fills_from_the_stop_to_the_reset() {
    use ket_core::limits::waited;
    let stop = limit(Scope::Plan, HOUR, Some(3 * HOUR));
    assert_eq!(waited(&stop, HOUR), 0.0);
    assert!((waited(&stop, 2 * HOUR) - 0.5).abs() < 1e-6);
    assert_eq!(waited(&stop, 4 * HOUR), 1.0);
    assert_eq!(waited(&limit(Scope::Plan, HOUR, None), 2 * HOUR), 0.0);
}

#[test]
fn the_task_is_offered_to_another_harness_never_a_bare_local_model() {
    use ket_core::limits::hand_choices;
    let agents: Vec<String> = [
        "codex",
        "claude",
        "opencode",
        "opencode:ollama/qwen3-coder:30b",
        "ollama:qwen3:0.6b",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(
        hand_choices("codex", &agents),
        ["claude", "opencode", "opencode:ollama/qwen3-coder:30b"]
    );
    assert_eq!(hand_choices("opencode", &agents), ["codex", "claude"]);
}
