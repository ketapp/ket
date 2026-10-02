//! Agent sessions, driven end to end.
//!
//! The agent here is `sh`. That is deliberate: a test that needs Claude, Codex
//! or OpenCode installed and authenticated is a test that does not run, and the
//! things worth pinning — the session lifecycle, cancellation, timeouts, the
//! refusal to start in an unprovisioned worktree — are all transport-level
//! behaviour that a shell script exercises exactly as well as a model does.
//!
//! The real agents are verified by handshake in `ket-core::config`, and by
//! actually running them, which is not something a test suite should do on
//! every `cargo test`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ket_core::config::{AgentSpec, AgentTimeouts, Config, Transport};
use ket_core::event::{Envelope, Event, SessionOutcome};
use ket_core::store::Store;
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, init_repo};

/// A "agent" that reads the prompt and answers, then exits.
fn echo_agent() -> AgentSpec {
    AgentSpec {
        name: "echo".to_owned(),
        transport: Transport::Pty,
        command: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "read line; printf 'answering: %s\\n' \"$line\"".to_owned(),
        ],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        launch: None,
    }
}

/// An agent that never says anything and never exits.
fn silent_agent() -> AgentSpec {
    AgentSpec {
        name: "silent".to_owned(),
        transport: Transport::Pty,
        command: "sh".to_owned(),
        args: vec!["-c".to_owned(), "sleep 600".to_owned()],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        launch: None,
    }
}

fn workspace_with(sandbox: &Sandbox, agents: Vec<AgentSpec>) -> Workspace {
    let mut config = Config {
        agents,
        ..Config::default()
    };
    // Nothing to provision in these fixtures, so provisioning is a no-op that
    // still marks the worktree ready — which is what agents require.
    config.provision.directories.clear();
    config.provision.files.clear();

    Workspace::with_config(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        config,
    )
}

/// A workspace with one project and one provisioned worktree.
fn ready(sandbox: &Sandbox, agents: Vec<AgentSpec>) -> (Workspace, ket_core::worktree::Worktree) {
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace_with(sandbox, agents);
    let project = ws.add_project(&repo).expect("add project");
    let (worktree, _) = ws
        .create_and_provision(&project.id, "work", None, None)
        .expect("create and provision");

    (ws, worktree)
}

/// Drains everything published so far.
fn drain(events: &mut tokio::sync::broadcast::Receiver<Envelope>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        out.push(envelope.event);
    }
    out
}

#[tokio::test]
async fn a_session_runs_and_reports_what_the_agent_said() {
    let sandbox = Sandbox::new("agent-run");
    let (ws, worktree) = ready(&sandbox, vec![echo_agent()]);

    let mut events = ws.bus().subscribe();
    let outcome = ws
        .run_agent(&worktree.id, Some("echo"), "hello there")
        .await
        .expect("run");

    assert_eq!(outcome, SessionOutcome::Completed);

    let events = drain(&mut events);
    let output: String = events
        .iter()
        .filter_map(|e| match e {
            Event::AgentOutput { chunk, .. } => Some(chunk.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        // The prompt now carries ket's own preamble ahead of it (see
        // `agent::first_prompt`), so the echoed line is no longer exactly
        // the prompt — just checking the prompt is still in there.
        output.contains("answering:") && output.contains("hello there"),
        "the agent's answer never reached the event stream: {output:?}"
    );
}

#[tokio::test]
async fn the_event_stream_has_the_same_shape_whatever_the_transport() {
    // The uniformity requirement, as far as a test can pin it: a session begins,
    // moves through states, produces output, and ends — and a consumer never has
    // to know which transport produced any of it. ACP adds detail inside this
    // shape; it does not change the shape.
    let sandbox = Sandbox::new("agent-shape");
    let (ws, worktree) = ready(&sandbox, vec![echo_agent()]);

    let mut events = ws.bus().subscribe();
    ws.run_agent(&worktree.id, Some("echo"), "hello")
        .await
        .expect("run");

    let events = drain(&mut events);

    assert!(
        matches!(events.first(), Some(Event::AgentSessionStarted { .. })),
        "first event was {:?}",
        events.first()
    );
    assert!(
        matches!(
            events.last(),
            Some(Event::AgentSessionEnded {
                outcome: SessionOutcome::Completed,
                ..
            })
        ),
        "last event was {:?}",
        events.last()
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::AgentStateChanged { .. })),
        "a session that never changed state is not a session"
    );
}

#[tokio::test]
async fn an_unprovisioned_worktree_will_not_run_an_agent() {
    // The entire justification for Epic 2b. An agent dropped into a bare
    // checkout fails on its first command and bills you for working out why.
    let sandbox = Sandbox::new("agent-unprovisioned");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace_with(&sandbox, vec![echo_agent()]);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "bare", None, None)
        .expect("create worktree");

    let message = ws
        .run_agent(&worktree.id, Some("echo"), "hello")
        .await
        .expect_err("expected a refusal")
        .to_string();

    assert!(
        message.contains("provision"),
        "unhelpful message: {message}"
    );
}

#[tokio::test]
async fn an_unknown_agent_is_refused_by_name() {
    let sandbox = Sandbox::new("agent-unknown");
    let (ws, worktree) = ready(&sandbox, vec![echo_agent()]);

    let message = ws
        .run_agent(&worktree.id, Some("nonexistent"), "hello")
        .await
        .expect_err("expected a refusal")
        .to_string();

    assert!(message.contains("nonexistent"), "{message}");
}

#[tokio::test]
async fn a_session_can_always_be_cancelled() {
    // Invariant 4, at the only level that matters: a real process, running, with
    // no intention of stopping.
    let sandbox = Sandbox::new("agent-cancel");
    let (ws, worktree) = ready(&sandbox, vec![silent_agent()]);

    let ws = std::sync::Arc::new(ws);
    let running = tokio::spawn({
        let ws = std::sync::Arc::clone(&ws);
        let id = worktree.id.clone();
        async move { ws.run_agent(&id, Some("silent"), "sit there").await }
    });

    // Wait for the session to actually exist before cancelling it, so this
    // tests cancellation rather than a race against startup.
    let started = Instant::now();
    while ws.cancel_all() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the session never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let outcome = tokio::time::timeout(Duration::from_secs(20), running)
        .await
        .expect("cancellation should not hang")
        .expect("task")
        .expect("run");

    assert_eq!(outcome, SessionOutcome::Cancelled);
}

#[tokio::test]
async fn cancel_session_stops_only_the_named_session() {
    let sandbox = Sandbox::new("agent-cancel-by-id");
    let (ws, worktree) = ready(&sandbox, vec![silent_agent()]);

    let ws = std::sync::Arc::new(ws);
    let mut events = ws.bus().subscribe();
    let running = tokio::spawn({
        let ws = std::sync::Arc::clone(&ws);
        let id = worktree.id.clone();
        async move { ws.run_agent(&id, Some("silent"), "sit there").await }
    });

    // Wait for the session to exist, and learn its id the same way a client
    // would: off the event it announces itself with.
    let started = Instant::now();
    let session_id = loop {
        if let Some(id) = drain(&mut events).into_iter().find_map(|e| match e {
            Event::AgentSessionStarted { session_id, .. } => Some(session_id),
            _ => None,
        }) {
            break id;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the session never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    // A stale or made-up id answers false rather than cancelling anything.
    let bogus = ket_core::id::SessionId::new("not-a-real-session");
    assert!(!ws.cancel_session(&bogus));

    assert!(ws.cancel_session(&session_id));

    let outcome = tokio::time::timeout(Duration::from_secs(20), running)
        .await
        .expect("cancellation should not hang")
        .expect("task")
        .expect("run");

    assert_eq!(outcome, SessionOutcome::Cancelled);

    // The session is gone once its run has returned, so cancelling it again
    // reports that there was nothing left to cancel.
    assert!(!ws.cancel_session(&session_id));
}

#[tokio::test]
async fn answer_permission_on_an_unknown_session_reports_no_live_request() {
    // The true path — a live request actually being settled — is exercised at
    // the `SessionContext` level in `src/agent.rs`; reaching it through
    // `Workspace` needs a transport that raises permission requests (ACP),
    // which the `sh`-based fixtures here deliberately do not.
    let sandbox = Sandbox::new("agent-answer-permission-unknown");
    let (ws, _worktree) = ready(&sandbox, vec![echo_agent()]);

    let bogus = ket_core::id::SessionId::new("not-a-real-session");
    assert!(!ws.answer_permission(&bogus, "req-1", true));
}

#[tokio::test]
async fn a_silent_agent_is_eventually_given_up_on() {
    // A pty cannot say which state it is in, so the bound is on silence. Without
    // one, an agent that wedges holds a worktree forever.
    let sandbox = Sandbox::new("agent-timeout");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let mut config = Config {
        agents: vec![silent_agent()],
        ..Config::default()
    };
    config.provision.directories.clear();
    config.provision.files.clear();
    config.agent.timeouts = AgentTimeouts {
        executing_tool_secs: 1,
        ..AgentTimeouts::default()
    };

    let ws = Workspace::with_config(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        config,
    );
    let project = ws.add_project(&repo).expect("add project");
    let (worktree, _) = ws
        .create_and_provision(&project.id, "work", None, None)
        .expect("provision");

    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        ws.run_agent(&worktree.id, Some("silent"), "sit there"),
    )
    .await
    .expect("the timeout must fire on its own")
    .expect("run");

    assert!(
        !matches!(outcome, SessionOutcome::Completed),
        "a wedged agent must not look like a successful one: {outcome:?}"
    );
}

#[tokio::test]
async fn sessions_are_listed_and_can_be_reaped() {
    let sandbox = Sandbox::new("agent-ps");
    let (ws, worktree) = ready(&sandbox, vec![echo_agent()]);

    ws.run_agent(&worktree.id, Some("echo"), "hello")
        .await
        .expect("run");

    let sessions = ws.sessions(None).expect("sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].agent, "echo");
    assert!(sessions[0].status.is_ended());
    assert_eq!(sessions[0].worktree_id, worktree.id);

    assert_eq!(ws.reap_sessions().expect("reap"), 1);
    assert!(ws.sessions(None).expect("sessions").is_empty());
}

#[tokio::test]
async fn a_session_whose_driver_died_is_not_reported_as_running() {
    // A force-quit leaves a record behind claiming to be live. Nothing would
    // ever finish it, and `ket ps` would grow a permanent lie.
    use ket_core::agent::{HEARTBEAT_STALE_AFTER, Session, SessionStatus};
    use ket_core::event::AgentState;
    use ket_core::id::SessionId;
    use ket_core::store::State;

    let sandbox = Sandbox::new("agent-orphan");
    let (ws, worktree) = ready(&sandbox, vec![echo_agent()]);

    let store = Store::at(sandbox.path("state.json"));
    let mut state: State = store.load().expect("load");
    state.sessions.push(Session {
        id: SessionId::new("orphan"),
        project_id: worktree.project_id.clone(),
        worktree_id: worktree.id.clone(),
        agent: "echo".to_owned(),
        status: SessionStatus::Live(AgentState::Thinking),
        driver_pid: 999_999,
        started_at_ms: 0,
        // Last heard from well past the staleness threshold.
        heartbeat_ms: 0,
    });
    store.save(&state).expect("save");

    let listed = ws.sessions(None).expect("sessions");
    let orphan = listed
        .iter()
        .find(|s| s.id.as_str() == "orphan")
        .expect("orphan listed");

    assert!(
        orphan.status.is_ended(),
        "an abandoned session must not still claim to be running: {:?}",
        orphan.status
    );
    assert!(HEARTBEAT_STALE_AFTER.as_millis() > 0);

    assert_eq!(ws.reap_sessions().expect("reap"), 1);
}
