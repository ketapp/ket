use super::*;

fn idle_sleep_pane() -> PaneAgentStatus {
    let mut activity = ket_core::activity::Tracker::new(["claude".to_owned()]);
    let report = serde_json::from_value(serde_json::json!({
        "agent": "claude",
        "event": "stop",
        "worktree": "wt",
        "pane": "wt|claude|a",
        "session": "saved-session",
        "mutatingGit": false,
        "atMs": 1000,
    }))
    .unwrap();
    activity.report(&report, |_| true);
    activity.pane_status("wt|claude|a").unwrap().clone()
}

#[test]
fn manual_sleep_allows_the_selected_idle_agent_but_automatic_sleep_protects_it() {
    let pane = idle_sleep_pane();
    let session = |trigger, focused| {
        sleep_session(trigger, focused, "claude", Some(&pane), Some(Signal::Quiet))
    };
    assert_eq!(session(SleepTrigger::Manual, true), Ok("saved-session"));
    assert_eq!(session(SleepTrigger::Manual, false), Ok("saved-session"));
    assert!(session(SleepTrigger::Automatic, true).is_err());
    assert_eq!(session(SleepTrigger::Automatic, false), Ok("saved-session"));
}

#[test]
fn manual_sleep_still_requires_a_matching_idle_agent_and_a_saved_session() {
    fn check(pane: Option<&PaneAgentStatus>, signal: Option<Signal>) -> Result<&str, String> {
        sleep_session(SleepTrigger::Manual, true, "claude", pane, signal)
    }

    let mut pane = idle_sleep_pane();
    assert!(check(None, Some(Signal::Quiet)).is_err());
    assert!(check(Some(&pane), None).is_err());
    // A quiet lead can still have a working or blocked subagent.
    for signal in [Signal::Working, Signal::Blocked] {
        assert!(check(Some(&pane), Some(signal)).is_err());
    }
    pane.agent = "codex".to_owned();
    assert!(check(Some(&pane), Some(Signal::Quiet)).is_err());
    pane.agent = "claude".to_owned();
    for state in [
        ket_core::event::AgentState::Thinking,
        ket_core::event::AgentState::AwaitingPermission,
    ] {
        pane.state = state;
        assert!(check(Some(&pane), Some(Signal::Quiet)).is_err());
    }
    pane.state = ket_core::event::AgentState::Idle;
    pane.session = None;
    assert!(check(Some(&pane), Some(Signal::Quiet)).is_err());
}

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

#[test]
fn a_url_is_found_from_any_column_inside_it() {
    let row = chars("see https://example.com/path for details");
    let span = url_span(&row, 4);
    assert_eq!(span, Some(4..28));
    assert_eq!(url_span(&row, 20), Some(4..28));
}

#[test]
fn trailing_prose_punctuation_is_not_part_of_the_url() {
    let row = chars("(https://example.com).");
    assert_eq!(url_span(&row, 5), Some(1..20));
    let uri: String = row[1..20].iter().collect();
    assert_eq!(uri, "https://example.com");
    // The wrapping punctuation itself is not part of the link.
    assert_eq!(url_span(&row, 0), None);
    assert_eq!(url_span(&row, 21), None);
}

#[test]
fn a_column_outside_any_url_finds_nothing() {
    let row = chars("see https://example.com for details");
    assert_eq!(url_span(&row, 0), None);
    assert_eq!(url_span(&row, 3), None);
    assert_eq!(url_span(&row, 30), None);
}

#[test]
fn a_bare_scheme_is_not_a_link() {
    let row = chars("https://");
    assert_eq!(url_span(&row, 2), None);
}
