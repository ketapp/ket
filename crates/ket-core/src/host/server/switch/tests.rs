//! A phone's switch, asked of the window showing the tab.

use super::super::tests::handoffs::{next_json_reply, opened};
use super::super::tests::window;
use super::super::{Reply, Request, handle};
use super::Switch;

fn to_claude(beside: bool) -> Switch {
    Switch {
        agent: "claude".to_owned(),
        model: Some("fable".to_owned()),
        effort: None,
        text: "Codex was working on a task in this worktree…".to_owned(),
        beside,
        session: Some("019a-codex".to_owned()),
    }
}

#[test]
fn a_switch_is_asked_of_the_window_showing_the_tab_and_resolves_when_the_new_agent_opens() {
    let host = super::super::Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|codex|out");
    let switching = {
        let host = host.clone();
        std::thread::spawn(move || host.switch_agent("wt|codex|out", to_claude(false)))
    };
    let req = match next_json_reply(&mut reader) {
        Reply::SwitchAgent {
            req,
            key,
            agent,
            model,
            effort,
            text,
            beside,
            session,
        } => {
            assert_eq!((key.as_str(), agent.as_str()), ("wt|codex|out", "claude"));
            assert_eq!(session.as_deref(), Some("019a-codex"));
            assert_eq!((model.as_deref(), effort), (Some("fable"), None));
            assert!(text.starts_with("Codex was working"));
            assert!(!beside);
            req
        }
        other => panic!("expected a switch, got {other:?}"),
    };
    // In its place: the old tab's terminal may close at any moment.
    assert!(host.replacing("wt|codex|out"));
    assert!(!host.replacing("wt|codex|other"));
    assert!(handle(
        &host,
        1,
        &out,
        Request::SwitchResult {
            req,
            key: Some("wt|claude|new".to_owned()),
            error: None,
        }
    ));
    let new = opened(&host, &out, &mut reader, 2, "wt|claude|new");
    assert_eq!(switching.join().unwrap(), Ok(new));
    assert!(!host.replacing("wt|codex|out"), "answered");
}

#[test]
fn a_switch_the_window_refuses_says_why() {
    let host = super::super::Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|codex|out");
    let switching = {
        let host = host.clone();
        std::thread::spawn(move || host.switch_agent("wt|codex|out", to_claude(true)))
    };
    let Reply::SwitchAgent { req, beside, .. } = next_json_reply(&mut reader) else {
        panic!("expected a switch");
    };
    assert!(beside);
    assert!(!host.replacing("wt|codex|out"), "beside, it stays");
    assert!(handle(
        &host,
        1,
        &out,
        Request::SwitchResult {
            req,
            key: None,
            error: Some("it hadn't started: its tab is at a shell prompt".to_owned()),
        }
    ));
    assert_eq!(
        switching.join().unwrap(),
        Err("it hadn't started: its tab is at a shell prompt".to_owned())
    );
    // An answer nobody waits for any more changes nothing.
    host.finish_switch(req, Some("wt|claude|late".to_owned()), None);
}

#[test]
fn a_switch_for_a_tab_no_window_shows_is_refused_at_once() {
    let host = super::super::Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|codex|shown");
    assert_eq!(
        host.switch_agent("wt|codex|elsewhere", to_claude(false)),
        Err("that tab isn't open in a ket window on your Mac".to_owned())
    );
}

#[test]
fn a_tab_is_switched_once_at_a_time_even_after_the_host_stops_waiting() {
    let host = super::super::Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|codex|out");
    let first = {
        let host = host.clone();
        std::thread::spawn(move || host.switch_agent("wt|codex|out", to_claude(false)))
    };
    let Reply::SwitchAgent { req, .. } = next_json_reply(&mut reader) else {
        panic!("expected a switch");
    };
    // The host gives up on the window; the window may still finish it.
    let gave_up = first.join().unwrap().unwrap_err();
    assert!(
        gave_up.contains("still starting the new agent"),
        "{gave_up}"
    );
    assert_eq!(
        host.switch_agent("wt|codex|out", to_claude(true)),
        Err(gave_up)
    );
    // Once the window answers, the tab can be switched again.
    host.finish_switch(req, Some("wt|claude|late".to_owned()), None);
    let again = {
        let host = host.clone();
        std::thread::spawn(move || host.switch_agent("wt|codex|out", to_claude(true)))
    };
    assert!(matches!(
        next_json_reply(&mut reader),
        Reply::SwitchAgent { .. }
    ));
    let _ = again.join();
}
