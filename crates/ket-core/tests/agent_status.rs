//! `StatusStore` — the one place an agent's own hook reports are turned into
//! a pane's status: state transitions, the cancel latch, subagent routing,
//! and pane placement.

use std::path::PathBuf;

use ket_core::agent_hooks::{HookEvent, HookReport, PermissionQuestion};
use ket_core::agent_status::{AgentStatusSnapshot, CancelSource, StatusStore};
use ket_core::event::AgentState;

fn report(event: HookEvent, pane: &str, worktree: &str, session: &str, at_ms: u64) -> HookReport {
    HookReport {
        agent: "claude".to_owned(),
        event,
        worktree: Some(worktree.to_owned()),
        cwd: None,
        pane: (!pane.is_empty()).then(|| pane.to_owned()),
        session: (!session.is_empty()).then(|| session.to_owned()),
        tool_name: None,
        note: None,
        mutating_git: false,
        at_ms,
        subagent: None,
        subagent_type: None,
        subagent_description: None,
        subagent_model: None,
        tool_use_id: None,
        call: None,
        teammate: None,
        fresh_session: false,
        background: None,
        question: None,
    }
}

fn known_any(_: &str) -> bool {
    true
}

// ---- basic report handling ---------------------------------------------------

#[test]
fn a_report_with_no_worktree_is_dropped() {
    let mut store = StatusStore::new(1);
    let mut r = report(HookEvent::UserPrompt, "p1", "w1", "", 100);
    r.worktree = None;
    assert!(!store.report(&r, known_any));
    assert!(store.get("p1").is_none());
}

#[test]
fn a_fresh_pane_is_established_and_readable() {
    let mut store = StatusStore::new(1);
    let changed = store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 100),
        known_any,
    );
    assert!(changed);

    let pane = store.get("p1").expect("pane recorded");
    assert_eq!(pane.worktree, "w1");
    assert_eq!(pane.agent, "claude");
    assert_eq!(pane.state, AgentState::Thinking);
    assert_eq!(pane.session.as_deref(), Some("s1"));
    assert_eq!(store.entries().count(), 1);
}

#[test]
fn an_event_with_no_state_on_an_unknown_pane_changes_nothing() {
    let mut store = StatusStore::new(1);
    // `Compact` maps to no state at all.
    let changed = store.report(
        &report(HookEvent::Compact, "p1", "w1", "s1", 100),
        known_any,
    );
    assert!(!changed);
    assert!(store.get("p1").is_none());
}

#[test]
fn event_state_mapping_moves_the_pane_through_its_lifecycle() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::SessionStart, "p1", "w1", "s1", 0),
        known_any,
    );
    assert_eq!(store.get("p1").unwrap().state, AgentState::Starting);

    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 1),
        known_any,
    );
    assert_eq!(store.get("p1").unwrap().state, AgentState::Thinking);

    store.report(&report(HookEvent::PreTool, "p1", "w1", "s1", 2), known_any);
    assert_eq!(store.get("p1").unwrap().state, AgentState::ExecutingTool);

    store.report(
        &report(HookEvent::PostTool, "p1", "w1", "s1", 2 + 10_000),
        known_any,
    );
    assert_eq!(store.get("p1").unwrap().state, AgentState::Thinking);

    store.report(
        &report(HookEvent::Stop, "p1", "w1", "s1", 3 + 10_000),
        known_any,
    );
    assert_eq!(store.get("p1").unwrap().state, AgentState::Idle);
}

#[test]
fn a_permission_request_is_awaiting_and_a_denial_returns_to_thinking() {
    let mut store = StatusStore::new(1);
    let mut r = report(HookEvent::PermissionRequest, "p1", "w1", "s1", 0);
    r.question = Some(Box::new(PermissionQuestion {
        tool: "Bash".to_owned(),
        subject: Some("rm -rf /tmp/x".to_owned()),
        id: 0,
        prompt: None,
    }));
    store.report(&r, known_any);
    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::AwaitingPermission);
    assert!(pane.question.is_some());

    store.report(
        &report(HookEvent::PermissionDenied, "p1", "w1", "s1", 1),
        known_any,
    );
    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::Thinking);
    assert!(pane.question.is_none());
}

#[test]
fn a_notification_restating_the_wait_keeps_the_earlier_question() {
    let mut store = StatusStore::new(1);
    let mut r = report(HookEvent::PermissionRequest, "p1", "w1", "s1", 0);
    r.question = Some(Box::new(PermissionQuestion {
        tool: "Bash".to_owned(),
        subject: Some("rm -rf /tmp/x".to_owned()),
        id: 0,
        prompt: None,
    }));
    store.report(&r, known_any);
    let first_question = store.get("p1").unwrap().question.clone().unwrap();

    // A `Notification` restating the same wait carries no question of its own.
    store.report(
        &report(HookEvent::Notification, "p1", "w1", "s1", 1),
        known_any,
    );
    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::AwaitingPermission);
    assert_eq!(
        pane.question.as_ref().unwrap().subject,
        first_question.subject
    );
}

#[test]
fn two_permission_requests_get_different_question_ids() {
    let mut store = StatusStore::new(1);
    let ask = |subject: &str, at_ms: u64| {
        let mut r = report(HookEvent::PermissionRequest, "p1", "w1", "s1", at_ms);
        r.question = Some(Box::new(PermissionQuestion {
            tool: "Bash".to_owned(),
            subject: Some(subject.to_owned()),
            id: 0,
            prompt: None,
        }));
        r
    };

    store.report(&ask("first", 0), known_any);
    let first_id = store.get("p1").unwrap().question.as_ref().unwrap().id;

    store.report(
        &report(HookEvent::PermissionDenied, "p1", "w1", "s1", 1),
        known_any,
    );
    store.report(&ask("second", 2), known_any);
    let second_id = store.get("p1").unwrap().question.as_ref().unwrap().id;

    assert_ne!(first_id, second_id);
}

// ---- the tool call's minimum visible time -------------------------------------

#[test]
fn a_tool_call_shows_as_executing_until_its_minimum_visible_time_has_passed() {
    let mut store = StatusStore::new(1);
    store.report(&report(HookEvent::PreTool, "p1", "w1", "s1", 0), known_any);
    // Returns to `Thinking` quickly — faster than a redraw could show it.
    store.report(
        &report(HookEvent::PostTool, "p1", "w1", "s1", 10),
        known_any,
    );

    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::Thinking);

    // Just under the minimum, measured from when the tool call started
    // (at_ms 0): still shown as the tool running.
    let (shown, _, _, _) = pane.shown(ket_core::activity::MIN_TOOL_VISIBLE_MS - 1);
    assert_eq!(shown, AgentState::ExecutingTool);

    // At or past the minimum: shown as what was actually reported.
    let (shown, _, _, _) = pane.shown(ket_core::activity::MIN_TOOL_VISIBLE_MS);
    assert_eq!(shown, AgentState::Thinking);
}

// ---- cancellation --------------------------------------------------------------

#[test]
fn an_interrupt_event_cancels_the_turn_with_the_provider_as_source() {
    let mut store = StatusStore::new(1);
    store.report(&report(HookEvent::PreTool, "p1", "w1", "s1", 0), known_any);
    store.report(
        &report(HookEvent::Interrupt, "p1", "w1", "s1", 1),
        known_any,
    );

    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::Idle);
    assert_eq!(pane.cancel.unwrap().source, CancelSource::Provider);
}

#[test]
fn a_report_that_only_restates_the_turn_is_held_against_a_cancel() {
    let mut store = StatusStore::new(1);
    store.report(&report(HookEvent::PreTool, "p1", "w1", "s1", 0), known_any);
    store.report(
        &report(HookEvent::Interrupt, "p1", "w1", "s1", 1),
        known_any,
    );

    // A late `PostTool` from the cancelled turn changes nothing.
    let changed = store.report(&report(HookEvent::PostTool, "p1", "w1", "s1", 2), known_any);
    assert!(!changed);
    assert_eq!(store.get("p1").unwrap().state, AgentState::Idle);
    assert!(store.get("p1").unwrap().cancel.is_some());
}

#[test]
fn a_new_prompt_releases_a_held_cancel() {
    let mut store = StatusStore::new(1);
    store.report(&report(HookEvent::PreTool, "p1", "w1", "s1", 0), known_any);
    store.report(
        &report(HookEvent::Interrupt, "p1", "w1", "s1", 1),
        known_any,
    );

    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 2),
        known_any,
    );
    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::Thinking);
    assert!(pane.cancel.is_none());
}

#[test]
fn cancel_only_applies_to_a_working_lead() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::SessionStart, "p1", "w1", "s1", 0),
        known_any,
    );
    assert_eq!(store.get("p1").unwrap().state, AgentState::Starting);

    // `Starting` is not "working", so a Ctrl-C does nothing.
    assert!(!store.cancel("p1", CancelSource::CtrlC, 1));

    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 1),
        known_any,
    );
    assert!(store.cancel("p1", CancelSource::CtrlC, 2));
    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::Idle);
    assert_eq!(pane.cancel.unwrap().source, CancelSource::CtrlC);
}

#[test]
fn cancel_on_an_unknown_pane_is_false() {
    let mut store = StatusStore::new(1);
    assert!(!store.cancel("nowhere", CancelSource::Phone, 0));
}

#[test]
fn escape_stops_a_working_lead_without_latching_a_verdict() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );

    assert!(store.escape("p1", 1));
    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::Idle);
    assert!(pane.cancel.is_none(), "escape is a guess, not a verdict");

    // Unlike a real cancel, a later report is not held against it.
    let changed = store.report(&report(HookEvent::PostTool, "p1", "w1", "s1", 2), known_any);
    assert!(changed);
}

#[test]
fn escape_on_an_idle_pane_does_nothing() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::SessionStart, "p1", "w1", "s1", 0),
        known_any,
    );
    store.report(&report(HookEvent::Stop, "p1", "w1", "s1", 1), known_any);
    assert!(!store.escape("p1", 2));
}

// ---- pane lifecycle --------------------------------------------------------

#[test]
fn clear_forgets_a_pane_and_its_bound_sessions() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );
    assert!(store.get("p1").is_some());

    assert!(store.clear("p1"));
    assert!(store.get("p1").is_none());

    // The session bound to the cleared pane no longer resolves a pane-less
    // report to it.
    let mut r = report(HookEvent::PostTool, "", "w1", "s1", 1);
    r.pane = None;
    let changed = store.report(&r, known_any);
    assert!(changed, "an unplaced pane is still created");
    // But it is the worktree-keyed placeholder, not the old pane.
    assert!(store.get("worktree:w1").is_some());
}

#[test]
fn clear_on_an_absent_pane_is_false() {
    let mut store = StatusStore::new(1);
    assert!(!store.clear("nowhere"));
}

#[test]
fn a_report_with_no_pane_is_filed_under_its_worktree() {
    let mut store = StatusStore::new(1);
    let mut r = report(HookEvent::UserPrompt, "", "w1", "s1", 0);
    r.pane = None;
    store.report(&r, known_any);

    assert!(store.get("worktree:w1").is_some());
}

#[test]
fn a_named_pane_ket_does_not_run_is_left_unfiled() {
    let mut store = StatusStore::new(1);
    let r = report(HookEvent::UserPrompt, "not-mine", "w1", "s1", 0);
    let changed = store.report(&r, |pane| pane == "known-pane");
    assert!(!changed);
    assert!(store.get("not-mine").is_none());
}

#[test]
fn a_session_is_bound_to_its_pane_and_a_later_pane_less_report_resolves_there() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );

    let mut r = report(HookEvent::PostTool, "", "w1", "s1", 1);
    r.pane = None;
    let changed = store.report(&r, known_any);
    assert!(changed);
    assert_eq!(store.get("p1").unwrap().state, AgentState::Thinking);
    // It did not also create an unplaced entry.
    assert!(store.get("worktree:w1").is_none());
}

#[test]
fn bind_session_wins_before_the_first_report() {
    let mut store = StatusStore::new(1);
    store.bind_session("s1", "p1");

    let mut r = report(HookEvent::UserPrompt, "", "w1", "s1", 0);
    r.pane = None;
    store.report(&r, known_any);

    assert!(store.get("p1").is_some());
}

// ---- superseded sessions -----------------------------------------------------

#[test]
fn a_report_from_an_older_session_is_dropped_unless_it_starts_a_new_one() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );

    // A stray report naming the old session after the pane moved on... but
    // the pane has not moved on yet, so this still applies normally. Now
    // actually move the pane to a new session.
    store.report(
        &report(HookEvent::SessionStart, "p1", "w1", "s2", 1),
        known_any,
    );
    assert_eq!(store.get("p1").unwrap().session.as_deref(), Some("s2"));

    // A late report from the old session is dropped...
    let changed = store.report(&report(HookEvent::PostTool, "p1", "w1", "s1", 2), known_any);
    assert!(!changed);

    // ...but a `SessionStart` for yet another session always takes over.
    let changed = store.report(
        &report(HookEvent::SessionStart, "p1", "w1", "s3", 3),
        known_any,
    );
    assert!(changed);
    assert_eq!(store.get("p1").unwrap().session.as_deref(), Some("s3"));
}

// ---- subagents -----------------------------------------------------------------

#[test]
fn a_subagent_report_is_filed_under_its_lead_and_readable_as_a_subagent() {
    let mut store = StatusStore::new(1);
    let mut r = report(HookEvent::SubagentStart, "p1", "w1", "s1", 0);
    r.subagent = Some("child-1".to_owned());
    r.subagent_type = Some("Explore".to_owned());
    store.report(&r, known_any);

    let pane = store.get("p1").unwrap();
    assert_eq!(
        pane.state,
        AgentState::ExecutingTool,
        "a lead created purely by a subagent report is treated as working"
    );
    assert_eq!(pane.subagents().len(), 1);
}

#[test]
fn a_notification_is_dropped_while_a_subagent_holds_the_prompt() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );

    let mut child = report(HookEvent::PermissionRequest, "p1", "w1", "s1", 1);
    child.subagent = Some("child-1".to_owned());
    store.report(&child, known_any);

    // The lead's own notification about the same wait says nothing new.
    let changed = store.report(
        &report(HookEvent::Notification, "p1", "w1", "s1", 2),
        known_any,
    );
    assert!(!changed);
}

#[test]
fn a_subagent_report_after_the_lead_has_gone_quiet_past_the_linger_resets_it_to_working() {
    let mut store = StatusStore::new(1);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );

    let far_later = ket_core::activity::REPORT_LINGERS_MS + 1;
    let mut child = report(HookEvent::SubagentStart, "p1", "w1", "s1", far_later);
    child.subagent = Some("child-1".to_owned());
    store.report(&child, known_any);

    let pane = store.get("p1").unwrap();
    assert_eq!(pane.state, AgentState::ExecutingTool);
    assert!(pane.cancel.is_none());
}

// ---- routing (Codex's worktree repair) -----------------------------------------

#[test]
fn route_corrects_a_codex_reports_worktree_by_its_cwd() {
    let mut store = StatusStore::new(1);
    let cwd = PathBuf::from("/tmp/some/worktree/path/that/does/not/need/to/exist");
    store.set_worktrees([("correct-id".to_owned(), cwd.clone())]);

    let mut r = report(HookEvent::UserPrompt, "p1", "wrong-id", "s1", 0);
    r.agent = "codex".to_owned();
    r.cwd = Some(Box::new(cwd));

    store.route(&mut r);
    assert_eq!(r.worktree.as_deref(), Some("correct-id"));
}

#[test]
fn route_leaves_a_non_codex_report_alone() {
    let mut store = StatusStore::new(1);
    let cwd = PathBuf::from("/tmp/some/worktree/path");
    store.set_worktrees([("correct-id".to_owned(), cwd.clone())]);

    let mut r = report(HookEvent::UserPrompt, "p1", "wrong-id", "s1", 0);
    r.agent = "claude".to_owned();
    r.cwd = Some(Box::new(cwd));

    store.route(&mut r);
    assert_eq!(r.worktree.as_deref(), Some("wrong-id"));
}

#[test]
fn route_leaves_a_codex_report_alone_when_no_cwd_matches() {
    let mut store = StatusStore::new(1);
    store.set_worktrees([("known".to_owned(), PathBuf::from("/tmp/known"))]);

    let mut r = report(HookEvent::UserPrompt, "p1", "unresolved", "s1", 0);
    r.agent = "codex".to_owned();
    r.cwd = Some(Box::new(PathBuf::from("/tmp/somewhere/else")));

    store.route(&mut r);
    assert_eq!(r.worktree.as_deref(), Some("unresolved"));
}

// ---- snapshots -------------------------------------------------------------

#[test]
fn a_snapshot_reflects_the_store() {
    let mut store = StatusStore::new(7);
    store.report(
        &report(HookEvent::UserPrompt, "p1", "w1", "s1", 0),
        known_any,
    );

    let snapshot = store.snapshot();
    assert_eq!(snapshot.epoch, 7);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.panes.len(), 1);
}

#[test]
fn a_snapshot_from_a_new_epoch_always_supersedes() {
    let older = AgentStatusSnapshot {
        epoch: 1,
        revision: 100,
        panes: Vec::new(),
    };
    let newer_epoch = AgentStatusSnapshot {
        epoch: 2,
        revision: 0,
        panes: Vec::new(),
    };
    assert!(newer_epoch.supersedes(Some(&older)));
}

#[test]
fn within_an_epoch_only_a_later_revision_supersedes() {
    let base = AgentStatusSnapshot {
        epoch: 1,
        revision: 5,
        panes: Vec::new(),
    };
    let later = AgentStatusSnapshot {
        epoch: 1,
        revision: 6,
        panes: Vec::new(),
    };
    let same = AgentStatusSnapshot {
        epoch: 1,
        revision: 5,
        panes: Vec::new(),
    };
    let earlier = AgentStatusSnapshot {
        epoch: 1,
        revision: 4,
        panes: Vec::new(),
    };

    assert!(later.supersedes(Some(&base)));
    assert!(!same.supersedes(Some(&base)));
    assert!(!earlier.supersedes(Some(&base)));
}

#[test]
fn any_snapshot_supersedes_nothing_held_yet() {
    let snapshot = AgentStatusSnapshot {
        epoch: 1,
        revision: 0,
        panes: Vec::new(),
    };
    assert!(snapshot.supersedes(None));
}
