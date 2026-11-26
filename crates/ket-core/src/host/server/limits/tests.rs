//! The worker, one tick at a time, against a host with no socket and
//! sessions whose files are whatever the test says they hold.

use std::cell::RefCell;
use std::sync::Arc;

use super::*;
use crate::limits::{Failure, Scope, Window};
use crate::terminal::{LocalTerminal, TerminalSize, TerminalSpec};

const T0: u64 = 1_800_000_000_000;
const HOUR: u64 = 60 * 60 * 1000;

/// What the agents' files hold, by session.
#[derive(Default)]
struct Fake {
    ended: RefCell<HashMap<String, Ended>>,
    quota: RefCell<HashMap<String, Vec<RateWindow>>>,
    written: RefCell<HashMap<String, u64>>,
    reads: RefCell<u32>,
    /// Something that happens while the next session file is read.
    meanwhile: RefCell<Option<Box<dyn FnOnce()>>>,
    /// The login published usage is read for, where it is not the session's.
    signed_in: RefCell<Option<String>>,
}

impl Fake {
    fn ends(&self, session: &str, failure: Failure, at_ms: Option<u64>, closed: bool) {
        self.ended.borrow_mut().insert(
            session.to_owned(),
            Ended {
                failure,
                at_ms,
                closed,
            },
        );
        *self
            .written
            .borrow_mut()
            .entry(session.to_owned())
            .or_default() += 100;
    }
}

impl Sessions for Fake {
    fn ended_on(&self, _: &str, session: &str) -> Option<Ended> {
        *self.reads.borrow_mut() += 1;
        if let Some(meanwhile) = self.meanwhile.borrow_mut().take() {
            meanwhile();
        }
        self.ended.borrow().get(session).cloned()
    }

    fn quota(&self, _: &str, session: &str) -> Vec<RateWindow> {
        self.quota
            .borrow()
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    fn written(&self, _: &str, session: &str) -> Option<u64> {
        self.written.borrow().get(session).copied()
    }

    fn account(&self, agent: &str, _: &str) -> String {
        if agent == "claude" {
            "work".to_owned()
        } else {
            String::new()
        }
    }

    fn usage_account(&self, agent: &str) -> String {
        self.signed_in
            .borrow()
            .clone()
            .unwrap_or_else(|| self.account(agent, ""))
    }
}

fn report(agent: &str, event: HookEvent, pane: &str, session: &str, at_ms: u64) -> HookReport {
    HookReport {
        agent: agent.to_owned(),
        event,
        worktree: Some("wt".to_owned()),
        cwd: None,
        pane: Some(pane.to_owned()),
        session: Some(session.to_owned()),
        tool_name: None,
        note: None,
        mutating_git: false,
        command: None,
        at_ms,
        subagent: None,
        subagent_type: None,
        subagent_description: None,
        subagent_model: None,
        tool_use_id: None,
        teammate: None,
        fresh_session: false,
        background: None,
        question: None,
        call: None,
        ruling: None,
        failure: None,
    }
}

/// Takes `report` in the way the hook listener's thread does.
fn hear(host: &Host, report: &HookReport) {
    let ended = {
        let mut status = lock(&host.status);
        status.report(report, |_| true);
        status.take_ended()
    };
    host.limits.heard(ended, &[]);
}

fn refused(agent: &str, pane: &str, session: &str, at_ms: u64, failure: Failure) -> HookReport {
    HookReport {
        failure: Some(Box::new(failure)),
        ..report(agent, HookEvent::StopFailure, pane, session, at_ms)
    }
}

fn session_limit() -> Failure {
    Failure::coded(
        "rate_limit",
        "You've hit your session limit · resets 10:40pm",
    )
}

fn usage_limit() -> Failure {
    Failure::coded("usage_limit_exceeded", "You’ve hit your usage limit.")
}

fn window(name: &str, used_percent: f32, resets_at_ms: u64) -> RateWindow {
    RateWindow {
        name: name.to_owned(),
        used_percent,
        resets_at_ms: Some(resets_at_ms),
        window_minutes: None,
    }
}

fn limit_of(host: &Host, pane: &str) -> Option<Limit> {
    lock(&host.status)
        .get(pane)
        .and_then(|pane| pane.limit.clone())
}

fn marks(host: &Host) -> Vec<Mark> {
    lock(&host.limits.ledger).marks().to_vec()
}

/// One tick that is not a slow one, and one that is.
fn tick(host: &Host, watch: &mut Watch, sessions: &Fake, now_ms: u64) {
    watch.ticks = 0;
    pass(host, watch, sessions, now_ms);
}

fn slow_tick(host: &Host, watch: &mut Watch, sessions: &Fake, now_ms: u64) {
    watch.ticks = SLOW_EVERY - 1;
    pass(host, watch, sessions, now_ms);
}

#[test]
fn a_claude_turn_refused_for_its_plan_marks_the_session_with_the_status_lines_reset() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    hear(
        &host,
        &report("claude", HookEvent::UserPrompt, "p1", "s1", T0),
    );
    // The status line Claude drew just before: both windows, one spent.
    host.limits.heard(
        Vec::new(),
        &[StatusLine {
            worktree: Some("wt".to_owned()),
            pane: Some("p1".to_owned()),
            agent: Some("claude".to_owned()),
            payload: serde_json::json!({"rate_limits": {
                "five_hour": {"utilization": 1.0, "resets_at": (T0 + 2 * HOUR) / 1000},
                "seven_day": {"utilization": 0.4, "resets_at": (T0 + 90 * HOUR) / 1000},
            }}),
        }],
    );
    hear(
        &host,
        &refused("claude", "p1", "s1", T0 + 500, session_limit()),
    );
    tick(&host, &mut watch, &files, T0 + 1_000);

    let limit = limit_of(&host, "p1").expect("the pane says it is stopped");
    assert_eq!((limit.scope, limit.window), (Scope::Plan, Window::Session));
    assert_eq!(limit.since_ms, T0 + 500);
    assert_eq!(limit.resets_at_ms, Some(T0 + 2 * HOUR));
    assert_eq!(limit.said, "You've hit your session limit · resets 10:40pm");
    assert_eq!(marks(&host)[0].account, "work");
    assert_eq!(
        *files.reads.borrow(),
        0,
        "its hook said why: no file is read"
    );
}

#[test]
fn a_retry_refused_again_is_the_same_stop_and_a_turn_that_ends_well_lifts_it() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    hear(&host, &refused("claude", "p1", "s1", T0, session_limit()));
    tick(&host, &mut watch, &files, T0 + 1_000);

    hear(
        &host,
        &report("claude", HookEvent::UserPrompt, "p1", "s1", T0 + 60_000),
    );
    tick(&host, &mut watch, &files, T0 + 60_500);
    assert!(
        limit_of(&host, "p1").is_some(),
        "asking again lifts nothing"
    );
    hear(
        &host,
        &refused("claude", "p1", "s1", T0 + 61_000, session_limit()),
    );
    tick(&host, &mut watch, &files, T0 + 62_000);
    assert_eq!(limit_of(&host, "p1").unwrap().since_ms, T0);

    // Stopped by hand: says nothing either way.
    hear(
        &host,
        &report("claude", HookEvent::UserPrompt, "p1", "s1", T0 + 70_000),
    );
    hear(
        &host,
        &report("claude", HookEvent::Interrupt, "p1", "s1", T0 + 71_000),
    );
    tick(&host, &mut watch, &files, T0 + 72_000);
    assert!(limit_of(&host, "p1").is_some());

    hear(
        &host,
        &report("claude", HookEvent::UserPrompt, "p1", "s1", T0 + 80_000),
    );
    hear(
        &host,
        &report("claude", HookEvent::Stop, "p1", "s1", T0 + 90_000),
    );
    tick(&host, &mut watch, &files, T0 + 91_000);
    assert_eq!(limit_of(&host, "p1"), None);
    assert!(marks(&host).is_empty());
}

#[test]
fn a_failure_that_is_not_a_limit_marks_nothing() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    let dropped = Failure::coded("server_error", "API Error: Connection lost mid-response.");
    hear(&host, &refused("claude", "p1", "s1", T0, dropped));
    tick(&host, &mut watch, &files, T0 + 1_000);
    assert_eq!(limit_of(&host, "p1"), None);
}

#[test]
fn a_session_running_tools_again_has_plainly_got_going() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    hear(&host, &refused("claude", "p1", "s1", T0, session_limit()));
    tick(&host, &mut watch, &files, T0 + 1_000);
    hear(
        &host,
        &report("claude", HookEvent::UserPrompt, "p1", "s1", T0 + 5_000),
    );
    hear(
        &host,
        &report("claude", HookEvent::PreTool, "p1", "s1", T0 + 6_000),
    );
    tick(&host, &mut watch, &files, T0 + 7_000);
    assert_eq!(limit_of(&host, "p1"), None);
}

#[test]
fn groks_bare_failure_is_read_from_its_session() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    files.ends(
        "g1",
        Failure::said("API error (status 402 Payment Required): usage balance exhausted"),
        None,
        true,
    );
    hear(
        &host,
        &report("grok", HookEvent::StopFailure, "p1", "g1", T0),
    );
    tick(&host, &mut watch, &files, T0 + 1_000);
    let limit = limit_of(&host, "p1").expect("marked");
    assert_eq!(limit.resets_at_ms, None, "Grok names no reset");
    assert!(limit.said.contains("balance exhausted"));
}

#[test]
fn a_codex_stop_is_checked_against_its_rollout_and_dated_from_its_quota() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    files.ends("c1", usage_limit(), Some(T0 + 900), true);
    files.quota.borrow_mut().insert(
        "c1".to_owned(),
        vec![
            window("Session", 100.0, T0 + 3 * HOUR),
            window("Weekly", 71.0, T0 + 100 * HOUR),
        ],
    );
    hear(
        &host,
        &report("codex", HookEvent::Stop, "p1", "c1", T0 + 1_000),
    );
    tick(&host, &mut watch, &files, T0 + 2_000);
    assert_eq!(
        limit_of(&host, "p1").unwrap().resets_at_ms,
        Some(T0 + 3 * HOUR)
    );

    // A later turn that stops cleanly, the old error still the last one the
    // rollout names: too long before this stop to be what it stopped on.
    hear(
        &host,
        &report("codex", HookEvent::Stop, "p1", "c1", T0 + HOUR),
    );
    tick(&host, &mut watch, &files, T0 + HOUR + 1_000);
    assert_eq!(limit_of(&host, "p1"), None);
}

#[test]
fn a_reset_time_said_later_is_filled_in_from_published_usage_while_it_is_fresh() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    files.ends("c1", usage_limit(), Some(T0), true);
    hear(&host, &report("codex", HookEvent::Stop, "p1", "c1", T0));
    tick(&host, &mut watch, &files, T0 + 1_000);
    assert_eq!(limit_of(&host, "p1").unwrap().resets_at_ms, None);

    let published = |fetched_at_ms: u64| crate::rate_limits::ProviderSnapshot {
        windows: vec![window("Session", 100.0, T0 + 4 * HOUR)],
        fetched_at_ms: Some(fetched_at_ms),
        ..crate::rate_limits::ProviderSnapshot::unsupported(Provider::Codex)
    };
    // A reading from before the limit could be about the window before it.
    *lock(&host.usage) = vec![published(T0 - HOUR)];
    slow_tick(&host, &mut watch, &files, T0 + 2_000);
    assert_eq!(limit_of(&host, "p1").unwrap().resets_at_ms, None);

    *lock(&host.usage) = vec![published(T0 + 1_500)];
    tick(&host, &mut watch, &files, T0 + 3_000);
    assert_eq!(
        limit_of(&host, "p1").unwrap().resets_at_ms,
        None,
        "looked for on the slow tick only"
    );
    slow_tick(&host, &mut watch, &files, T0 + 4_000);
    assert_eq!(
        limit_of(&host, "p1").unwrap().resets_at_ms,
        Some(T0 + 4 * HOUR)
    );
}

#[test]
fn another_logins_published_usage_never_dates_a_stop() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    *lock(&host.usage) = vec![crate::rate_limits::ProviderSnapshot {
        windows: vec![window("Session", 100.0, T0 + 4 * HOUR)],
        fetched_at_ms: Some(T0 + 500),
        ..crate::rate_limits::ProviderSnapshot::unsupported(Provider::Claude)
    }];
    *files.signed_in.borrow_mut() = Some("personal".to_owned());
    hear(&host, &refused("claude", "p1", "s1", T0, session_limit()));
    tick(&host, &mut watch, &files, T0 + 1_000);
    slow_tick(&host, &mut watch, &files, T0 + 2_000);
    assert_eq!(limit_of(&host, "p1").unwrap().resets_at_ms, None);

    // The same login: its reading says.
    *files.signed_in.borrow_mut() = None;
    slow_tick(&host, &mut watch, &files, T0 + 3_000);
    assert_eq!(
        limit_of(&host, "p1").unwrap().resets_at_ms,
        Some(T0 + 4 * HOUR)
    );
}

#[test]
fn a_stopped_session_is_still_stopped_in_the_tab_it_wakes_in() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    hear(&host, &refused("claude", "p1", "s1", T0, session_limit()));
    tick(&host, &mut watch, &files, T0 + 1_000);

    // Put to sleep: the pane is gone, and the ledger keeps the session.
    lock(&host.status).clear("p1");
    tick(&host, &mut watch, &files, T0 + 2_000);
    assert_eq!(marks(&host).len(), 1);

    // Woken under another key, resuming the same session.
    assert_eq!(host.limit_in("p1").unwrap().since_ms, T0, "its asleep tab");
    assert_eq!(host.limit_in("p0"), None);
    let mut woke = report("claude", HookEvent::SessionStart, "p2", "s1", T0 + HOUR);
    woke.fresh_session = true;
    hear(&host, &woke);
    tick(&host, &mut watch, &files, T0 + HOUR + 1_000);
    assert_eq!(limit_of(&host, "p2").unwrap().since_ms, T0);
    assert_eq!(host.limit_in("p1"), None, "it moved");
    assert!(host.limit_in("p2").is_some());
}

/// A Codex pane at work under a real terminal, so the host knows its key.
fn working_codex(host: &Arc<Host>, pane: &str, session: &str, at_ms: u64) {
    let terminal = LocalTerminal::open(&TerminalSpec {
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
    .expect("a terminal");
    host.file_keyed_for_tests(terminal, Some(pane.to_owned()));
    hear(
        host,
        &report("codex", HookEvent::UserPrompt, pane, session, at_ms),
    );
}

fn state_of(host: &Host, pane: &str) -> AgentState {
    lock(&host.status).get(pane).expect("the pane").state
}

#[test]
fn a_codex_turn_that_ran_out_without_a_word_is_ended_for_it() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    working_codex(&host, "wt|codex|1", "c1", T0);
    files.ends("c1", usage_limit(), Some(T0 + 4_000), true);

    // Not yet quiet for long enough, then quiet but the record too new.
    slow_tick(&host, &mut watch, &files, T0 + 4_500);
    assert_eq!(*files.reads.borrow(), 0);
    slow_tick(&host, &mut watch, &files, T0 + 6_000);
    assert_eq!(state_of(&host, "wt|codex|1"), AgentState::Thinking);

    slow_tick(&host, &mut watch, &files, T0 + 8_000);
    assert_eq!(state_of(&host, "wt|codex|1"), AgentState::Idle);
    assert!(lock(&host.status).get("wt|codex|1").unwrap().unread);
    // The end it was given is judged like any other, on the next tick.
    tick(&host, &mut watch, &files, T0 + 9_000);
    let limit = limit_of(&host, "wt|codex|1").expect("and it was a limit");
    assert_eq!(limit.since_ms, T0 + 8_000);
}

#[test]
fn a_prompt_heard_while_the_rollout_is_read_is_not_ended_for_the_turn_before() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    working_codex(&host, "wt|codex|1", "c1", T0);
    files.ends("c1", usage_limit(), Some(T0 + 4_000), true);
    let typed = host.clone();
    *files.meanwhile.borrow_mut() = Some(Box::new(move || {
        hear(
            &typed,
            &report(
                "codex",
                HookEvent::UserPrompt,
                "wt|codex|1",
                "c1",
                T0 + 7_500,
            ),
        );
    }));

    slow_tick(&host, &mut watch, &files, T0 + 8_000);
    assert_eq!(state_of(&host, "wt|codex|1"), AgentState::Thinking);
    tick(&host, &mut watch, &files, T0 + 9_000);
    assert_eq!(limit_of(&host, "wt|codex|1"), None);
}

#[test]
fn a_codex_pane_at_work_is_left_alone_unless_its_rollout_says_the_turn_is_over() {
    let host = Host::for_tests();
    let (mut watch, files) = (Watch::default(), Fake::default());
    working_codex(&host, "wt|codex|1", "c1", T0);

    // An error Codex may be retrying; then the end of the turn before this.
    files.ends(
        "c1",
        Failure::said("stream disconnected"),
        Some(T0 + 1_000),
        false,
    );
    slow_tick(&host, &mut watch, &files, T0 + 10_000);
    files.ends("c1", usage_limit(), Some(T0 - 60_000), true);
    slow_tick(&host, &mut watch, &files, T0 + 20_000);
    assert_eq!(state_of(&host, "wt|codex|1"), AgentState::Thinking);

    // Nothing written since: the rollout is not read again.
    let reads = *files.reads.borrow();
    slow_tick(&host, &mut watch, &files, T0 + 30_000);
    slow_tick(&host, &mut watch, &files, T0 + 40_000);
    assert_eq!(*files.reads.borrow(), reads);

    // And an agent with a failure event of its own is never second-guessed.
    hear(
        &host,
        &report("claude", HookEvent::UserPrompt, "p2", "s2", T0),
    );
    files.ends("s2", session_limit(), Some(T0 + 1_000), true);
    slow_tick(&host, &mut watch, &files, T0 + 50_000);
    assert_eq!(state_of(&host, "p2"), AgentState::Thinking);
}

#[test]
fn the_ledger_is_written_when_it_changes_and_read_back_whole() {
    let dir = std::env::temp_dir().join(format!("ket-limits-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("limits.json");
    let _ = std::fs::remove_file(&file);

    let host = Host::for_tests();
    let limits = Limits {
        file: Some(file.clone()),
        ..Limits::default()
    };
    limits.save();
    assert_eq!(
        serde_json::from_slice::<Ledger>(&std::fs::read(&file).unwrap()).unwrap(),
        Ledger::default()
    );

    let (mut watch, files) = (Watch::default(), Fake::default());
    hear(&host, &refused("claude", "p1", "s1", T0, session_limit()));
    tick(&host, &mut watch, &files, T0 + 1_000);
    *lock(&limits.ledger) = lock(&host.limits.ledger).clone();
    limits.save();
    let kept: Ledger = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(kept.of("claude", "s1").unwrap().since_ms, T0);
    let _ = std::fs::remove_dir_all(&dir);
}
