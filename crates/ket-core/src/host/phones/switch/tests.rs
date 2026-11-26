//! A phone's preview of a hand-over, and what it reads to make one.

use std::sync::Arc;

use ket_remote::proto::{self, envelope::Payload};

use super::*;
use crate::agent_hooks::{HookEvent, HookReport};
use crate::terminal::{LocalTerminal, TerminalSize, TerminalSpec};

fn cat() -> LocalTerminal {
    LocalTerminal::open(&TerminalSpec {
        command: "/bin/cat".to_owned(),
        args: Vec::new(),
        cwd: std::env::temp_dir(),
        env: std::collections::BTreeMap::new(),
        env_remove: Vec::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
        adopt_only: false,
    })
    .expect("a terminal")
}

fn started(pane: &str, session: &str) -> HookReport {
    HookReport {
        agent: "codex".to_owned(),
        event: HookEvent::SessionStart,
        worktree: Some("wt".to_owned()),
        cwd: None,
        pane: Some(pane.to_owned()),
        session: Some(session.to_owned()),
        tool_name: None,
        note: None,
        mutating_git: false,
        command: None,
        at_ms: crate::now_ms(),
        subagent: None,
        subagent_type: None,
        subagent_description: None,
        subagent_model: None,
        tool_use_id: None,
        teammate: None,
        fresh_session: true,
        background: None,
        question: None,
        call: None,
        ruling: None,
        failure: None,
    }
}

fn message(reply: Option<proto::Envelope>) -> (proto::ErrorCode, String) {
    match reply.and_then(|reply| reply.payload) {
        Some(Payload::Error(error)) => (
            proto::ErrorCode::try_from(error.code).unwrap_or(proto::ErrorCode::Unspecified),
            error.message,
        ),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn a_preview_names_only_an_agent_the_mac_can_start() {
    let host = Host::for_tests();
    let id = host.file_keyed_for_tests(cat(), Some("wt|codex|1".to_owned()));
    let ask = proto::PreviewSwitch {
        terminal_id: id,
        agent: "claude".to_owned(),
        depth: proto::SwitchDepth::Recent as i32,
    };
    assert_eq!(
        message(preview_from_phone(&host, 7, &ask)),
        (
            proto::ErrorCode::NotFound,
            "that agent isn't one ket on your Mac can start".to_owned()
        )
    );
}

#[test]
fn the_session_handed_over_is_the_one_the_pane_reports_and_whether_a_limit_stopped_it() {
    let host: Arc<Host> = Host::for_tests();
    assert_eq!(session_of(&host, 99), Err("that terminal has closed"));

    let id = host.file_keyed_for_tests(cat(), Some("wt|codex|1".to_owned()));
    assert_eq!(
        session_of(&host, id),
        Err("no agent has reported from this terminal yet")
    );

    assert!(host.report_for_tests(&started("wt|codex|1", "s1")));
    assert_eq!(
        session_of(&host, id),
        Ok(("codex".to_owned(), "s1".to_owned(), false))
    );
}

#[test]
fn a_tab_that_moved_on_since_its_brief_was_read_is_not_switched() {
    let host: Arc<Host> = Host::for_tests();
    let id = host.file_keyed_for_tests(cat(), Some("wt|codex|1".to_owned()));
    assert!(host.report_for_tests(&started("wt|codex|1", "s1")));
    assert!(!moved_on(&host, id, "s1"));
    assert!(!moved_on(&host, id, ""), "a phone that named none");

    // Resumed into another session while the phone read the brief.
    assert!(host.report_for_tests(&started("wt|codex|1", "s2")));
    assert!(moved_on(&host, id, "s1"));
}

#[test]
fn a_depth_the_phone_did_not_name_is_recent() {
    assert_eq!(depth_of(proto::SwitchDepth::Task as i32), Depth::Task);
    assert_eq!(depth_of(proto::SwitchDepth::Full as i32), Depth::Full);
    assert_eq!(depth_of(proto::SwitchDepth::Recent as i32), Depth::Recent);
    assert_eq!(
        depth_of(proto::SwitchDepth::Unspecified as i32),
        Depth::Recent
    );
    assert_eq!(depth_of(42), Depth::Recent);
}
