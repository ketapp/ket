//! Agent sessions — the state machine every transport normalises onto.
//!
//! Three agents are in rotation from day one and none of them is privileged, so
//! the uniformity requirement here is load-bearing: if comparing three agents on
//! one task means three code paths, the comparison view in Epic 9 never works.
//! Everything below is transport-agnostic. ACP and PTY both drive *this*, and
//! consumers downstream never learn which one produced an event.
//!
//! The lifecycle is:
//!
//! ```text
//! Starting → Authenticating → Idle → Thinking → ExecutingTool → Idle
//!                                        ↕            ↕
//!                                   AwaitingPermission
//! ```
//!
//! with every state able to end. That last part is invariant 4 and is checked by
//! a test rather than trusted: a session you cannot get out of is the first place
//! "every state is escapable" gets broken.

pub mod acp;
pub mod pty;

/// Said once, before the person's own first message of a session — the one
/// thing any agent should know about the tool it is running inside,
/// regardless of which CLI or model it is.
///
/// Kept to a single line pointing at `ket --help` rather than a list of
/// commands: the help text is the actual documentation and stays correct as
/// commands come and go, where a preamble that named them would drift the
/// first time one changed.
const PREAMBLE: &str =
    "This project is managed with ket — run `ket --help` to see what you can do from here.";

/// A session's first prompt, with [`PREAMBLE`] ahead of it.
///
/// Joined with a space rather than a newline: a PTY transport is one line in,
/// one answer out (see `agent::pty::run`), and a prompt with a newline in it
/// is two lines to whatever is reading stdin — the second of which, the
/// person's actual message, would never arrive. ACP has no such constraint,
/// but there is no reason for the two transports to say this differently.
///
/// The one place this is said, so every agent — ACP or PTY, local or hosted —
/// is told the same way: what ket wants an agent to know does not depend on
/// how ket happens to talk to it.
pub fn first_prompt(prompt: &str) -> String {
    format!("{PREAMBLE} {prompt}")
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::{AgentTimeouts, PermissionConfig, PermissionPolicy};
use crate::event::{AgentState, Event, EventBus, SessionOutcome, SessionUsage};
use crate::id::{ProjectId, SessionId, WorktreeId};
use crate::{KetError, Result};

/// Where a session is in its lifecycle.
///
/// Live states and terminal outcomes are deliberately different types rather
/// than one flat enum. A terminal state carries *why* it ended, which no live
/// state has, and flattening the two makes "cancelled" and "failed" look like
/// places a session might come back from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", content = "value")]
pub enum SessionStatus {
    /// Running, in the given state.
    Live(AgentState),
    /// Finished, for the given reason.
    Ended(SessionOutcome),
}

impl SessionStatus {
    /// Whether the session has finished.
    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }

    /// The live state, or `None` once the session has ended.
    pub fn live(&self) -> Option<AgentState> {
        match self {
            Self::Live(state) => Some(*state),
            Self::Ended(_) => None,
        }
    }
}

/// One run of an agent against one worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Stable identifier.
    pub id: SessionId,
    /// The project the worktree belongs to.
    ///
    /// Denormalised from the worktree on purpose: `ket ps` lists sessions across
    /// every project, and that listing should not have to resolve a worktree to
    /// find out which project each row belongs to.
    pub project_id: ProjectId,
    /// The worktree this session is confined to.
    pub worktree_id: WorktreeId,
    /// Configured agent name, e.g. `"claude"`.
    pub agent: String,
    /// Where it is in its lifecycle.
    pub status: SessionStatus,
    /// Process id of the ket process driving this session.
    ///
    /// Sessions do not survive the process that started them, so a record whose
    /// driver is gone is a record of something already dead — see
    /// [`Session::is_abandoned`].
    pub driver_pid: u32,
    /// When the session started.
    pub started_at_ms: u64,
    /// When the driving process last confirmed this record.
    ///
    /// A heartbeat rather than a liveness syscall: it also catches a driver that
    /// is technically alive and wedged, which is the failure a `kill(pid, 0)`
    /// check would happily report as healthy.
    pub heartbeat_ms: u64,
}

/// How long a session record may go unrefreshed before its driver is presumed dead.
///
/// Generous relative to [`HEARTBEAT_INTERVAL`], so an ordinarily busy process is
/// never mistaken for a crashed one.
pub const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(30);

/// How often a driving process refreshes its sessions' heartbeats.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

impl Session {
    /// Whether the process driving this session appears to be gone.
    ///
    /// A force-quit leaves records behind, and they must not accumulate as
    /// permanently "running" sessions that nothing will ever finish.
    pub fn is_abandoned(&self, now_ms: u64) -> bool {
        !self.status.is_ended()
            && now_ms.saturating_sub(self.heartbeat_ms)
                > u64::try_from(HEARTBEAT_STALE_AFTER.as_millis()).unwrap_or(u64::MAX)
    }
}

/// Whether moving from `from` to `to` is a legal transition.
///
/// Enforced rather than documented. A transport that reports nonsense — an ACP
/// adapter emitting a tool call after the turn ended, say — should produce a
/// visible complaint, not a session whose state silently means nothing.
pub fn is_legal(from: AgentState, to: AgentState) -> bool {
    use AgentState::{Authenticating, AwaitingPermission, ExecutingTool, Idle, Starting, Thinking};

    match from {
        Starting => matches!(to, Authenticating | Idle),
        Authenticating => matches!(to, Idle),
        Idle => matches!(to, Thinking),
        // A turn can go back and forth between waiting on the model and running
        // what the model asked for, any number of times, before settling.
        Thinking => matches!(to, ExecutingTool | AwaitingPermission | Idle),
        ExecutingTool => matches!(to, Thinking | AwaitingPermission | Idle),
        // A permission decision returns to whichever half of the turn asked.
        AwaitingPermission => matches!(to, Thinking | ExecutingTool | Idle),
    }
}

/// How long a state may last before the session is considered stuck.
///
/// `None` means "as long as it likes", and is reserved for states whose duration
/// is a human's business rather than a machine's: a session waiting for its next
/// prompt, or for someone to answer a permission request, is not stalled.
///
/// The [`AgentState::ExecutingTool`] and [`AgentState::Thinking`] split exists
/// precisely for this. A four-minute test suite and a four-minute stalled model
/// request are indistinguishable if you only track "busy", and any single
/// timeout covering both is either uselessly long or kills real builds.
pub fn timeout_for(state: AgentState, timeouts: &AgentTimeouts) -> Option<Duration> {
    match state {
        AgentState::Starting => Some(Duration::from_secs(timeouts.starting_secs)),
        AgentState::Authenticating => Some(Duration::from_secs(timeouts.authenticating_secs)),
        AgentState::Thinking => Some(Duration::from_secs(timeouts.thinking_secs)),
        AgentState::ExecutingTool => Some(Duration::from_secs(timeouts.executing_tool_secs)),
        AgentState::Idle | AgentState::AwaitingPermission => None,
    }
}

/// Every state a session can be in while running.
///
/// Listed once here so the exhaustiveness tests below cannot drift from the enum
/// as variants are added.
pub const ALL_STATES: [AgentState; 6] = [
    AgentState::Starting,
    AgentState::Authenticating,
    AgentState::Idle,
    AgentState::Thinking,
    AgentState::ExecutingTool,
    AgentState::AwaitingPermission,
];

/// Everything a transport needs in order to run one session.
///
/// Both transports are handed one of these and drive it the same way. That is
/// the uniformity requirement in practice: the ACP path and the PTY path differ
/// in how much they can *observe*, never in what they produce.
#[derive(Debug)]
pub struct SessionContext {
    /// The session being run.
    pub id: SessionId,
    /// The worktree the agent is confined to. Its process runs here.
    pub worktree: PathBuf,
    /// Where events go.
    pub bus: Arc<EventBus>,
    /// Per-state bounds.
    pub timeouts: AgentTimeouts,
    /// Tool permission policy.
    pub permissions: PermissionConfig,
    /// Current live state. Behind a lock because the transports move between
    /// threads — a PTY reader thread and an async driver, in particular.
    state: Mutex<AgentState>,
    /// When the current state was entered. The watchdog measures against this.
    state_since: Mutex<std::time::Instant>,
    /// Stop signal, handed out by clone to whatever needs to wait on it.
    cancel: CancelSignal,
    /// Permission requests waiting on a client's answer.
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
    /// The most recent usage reading, or `None` from a transport that cannot
    /// report one.
    ///
    /// Latest value rather than a history. Usage arrives every turn, so keeping
    /// each reading would be an unbounded buffer growing with session length —
    /// the exact shape the memory budget exists to refuse. Anything that wants
    /// the history already has it: every reading is published to the bus and
    /// lands in the event journal.
    usage: Mutex<Option<SessionUsage>>,
}

impl SessionContext {
    /// Starts a session in [`AgentState::Starting`].
    pub fn new(
        id: SessionId,
        worktree: impl Into<PathBuf>,
        bus: Arc<EventBus>,
        timeouts: AgentTimeouts,
        permissions: PermissionConfig,
        cancel: CancelSignal,
    ) -> Self {
        Self {
            id,
            worktree: worktree.into(),
            bus,
            timeouts,
            permissions,
            state: Mutex::new(AgentState::Starting),
            state_since: Mutex::new(std::time::Instant::now()),
            cancel,
            pending: Mutex::new(HashMap::new()),
            usage: Mutex::new(None),
        }
    }

    /// A clone of the session's stop signal.
    pub fn cancel_signal(&self) -> CancelSignal {
        self.cancel.clone()
    }

    /// The current live state.
    pub fn state(&self) -> AgentState {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Moves to `to`, announcing it.
    ///
    /// An illegal transition is refused and logged rather than applied. A
    /// transport reporting nonsense — a tool call arriving after the turn ended,
    /// say — should leave a visible complaint and a state that still means
    /// something, not quietly redefine the lifecycle.
    ///
    /// Transitioning to the state you are already in is a no-op, not an error:
    /// an agent running three tools in a row reports `ExecutingTool` three
    /// times, and that is not a lifecycle change.
    pub fn transition(&self, to: AgentState) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let from = *state;

        if from == to {
            return;
        }

        if !is_legal(from, to) {
            tracing::warn!(
                session = %self.id,
                ?from,
                ?to,
                "transport reported an illegal state transition; ignoring it"
            );
            return;
        }

        *state = to;
        drop(state);

        *self.state_since.lock().unwrap_or_else(|e| e.into_inner()) = std::time::Instant::now();

        self.bus.publish(Event::AgentStateChanged {
            session_id: self.id.clone(),
            from,
            to,
        });
    }

    /// How long the current state may last, if it is bounded at all.
    pub fn current_timeout(&self) -> Option<Duration> {
        timeout_for(self.state(), &self.timeouts)
    }

    /// How long the session has been in its current state.
    pub fn time_in_state(&self) -> Duration {
        self.state_since
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }

    /// Whether the session has outstayed its current state's bound.
    pub fn is_stuck(&self) -> bool {
        self.current_timeout()
            .is_some_and(|limit| self.time_in_state() > limit)
    }

    /// Publishes a chunk of agent output.
    pub fn output(&self, chunk: impl Into<String>) {
        self.bus.publish(Event::AgentOutput {
            session_id: self.id.clone(),
            chunk: chunk.into(),
        });
    }

    /// Records what the agent reported using, and publishes it.
    ///
    /// Only ACP carries this. A PTY session never calls it, which is why the
    /// reading is an `Option` — a transport that cannot measure must read as
    /// *unknown*, never as zero. A status bar showing "0 tokens" beside a working
    /// agent is worse than one showing nothing at all.
    pub fn record_usage(&self, usage: SessionUsage) {
        if let Ok(mut latest) = self.usage.lock() {
            *latest = Some(usage.clone());
        }

        self.bus.publish(Event::AgentUsage {
            session_id: self.id.clone(),
            usage,
        });
    }

    /// The latest usage reading, or `None` if this transport never reports one.
    pub fn usage(&self) -> Option<SessionUsage> {
        self.usage.lock().ok().and_then(|latest| latest.clone())
    }

    /// Decides a permission request from configuration alone.
    ///
    /// Returns `None` when the policy is [`PermissionPolicy::Prompt`], meaning a
    /// person has to answer and the session moves to
    /// [`AgentState::AwaitingPermission`].
    pub fn decide(&self, tool: &str) -> Option<bool> {
        match self.permissions.for_tool(tool) {
            PermissionPolicy::Allow => Some(true),
            PermissionPolicy::Deny => Some(false),
            PermissionPolicy::Prompt => None,
        }
    }

    /// Settles one permission request, asking a client only if policy will not.
    ///
    /// `tool` is the ACP tool kind, so a policy is written once and means the
    /// same thing for every agent.
    ///
    /// [`AgentState::AwaitingPermission`] has no timeout on purpose — a person
    /// deciding is not a stall — so the escape from it is cancellation, which
    /// this waits on alongside the answer. Without that arm, an unanswered
    /// request would be a state nothing could leave, which is invariant 4
    /// broken in the most literal way.
    pub async fn request_permission(&self, request_id: &str, tool: &str, title: &str) -> bool {
        if let Some(allowed) = self.decide(tool) {
            self.bus.publish(Event::AgentPermissionResolved {
                session_id: self.id.clone(),
                request_id: request_id.to_owned(),
                allowed,
                by_policy: true,
            });
            return allowed;
        }

        let resume_to = self.state();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id.to_owned(), tx);

        self.transition(AgentState::AwaitingPermission);
        self.bus.publish(Event::AgentPermissionRequested {
            session_id: self.id.clone(),
            request_id: request_id.to_owned(),
            tool: tool.to_owned(),
            title: title.to_owned(),
        });

        let mut cancel = self.cancel.clone();
        let allowed = tokio::select! {
            answer = rx => answer.unwrap_or(false),
            // Cancelled, or the answering client went away. Refusing is the
            // safe reading of both: a tool that never ran can be run again.
            () = cancel.cancelled() => false,
        };

        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id);

        self.transition(resume_to);
        self.bus.publish(Event::AgentPermissionResolved {
            session_id: self.id.clone(),
            request_id: request_id.to_owned(),
            allowed,
            by_policy: false,
        });

        allowed
    }

    /// Answers a pending permission request.
    ///
    /// Returns whether there was one to answer, so a client can tell a stale
    /// answer — one for a request already resolved or cancelled — from a live one.
    pub fn answer_permission(&self, request_id: &str, allowed: bool) -> bool {
        let responder = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id);

        match responder {
            Some(tx) => tx.send(allowed).is_ok(),
            None => false,
        }
    }
}

/// The stop half of a session's cancel signal.
///
/// Cloneable and cheap, so a CLI's Ctrl-C handler, a UI button, and a shutdown
/// path can all hold one. Cancelling twice is harmless.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    tx: tokio::sync::watch::Sender<bool>,
}

/// The listening half, held by whichever transport is running the session.
#[derive(Debug, Clone)]
pub struct CancelSignal {
    rx: tokio::sync::watch::Receiver<bool>,
}

/// Creates a linked cancel handle and signal.
pub fn cancellation() -> (CancelHandle, CancelSignal) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (CancelHandle { tx }, CancelSignal { rx })
}

impl CancelHandle {
    /// Asks the session to stop. Returns immediately.
    pub fn cancel(&self) {
        // A failed send means the session already ended, which is not an error:
        // cancelling something that has finished is the normal race.
        let _ = self.tx.send(true);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }
}

impl CancelSignal {
    /// Resolves once cancellation is requested.
    ///
    /// Safe to hold in a `select!` arm: it resolves immediately if cancellation
    /// already happened, so a late listener cannot miss the signal and wait
    /// forever — which would be exactly the unescapable state invariant 4 bans.
    pub async fn cancelled(&mut self) {
        if *self.rx.borrow() {
            return;
        }

        // The sender being dropped means nothing can ever cancel this session.
        // Treat that as "never", not as cancellation.
        while self.rx.changed().await.is_ok() {
            if *self.rx.borrow() {
                return;
            }
        }

        std::future::pending::<()>().await
    }
}

/// Fails a session that has outstayed its state's bound.
pub fn timed_out(id: &SessionId, state: AgentState, limit: Duration) -> KetError {
    KetError::Agent {
        agent: id.to_string(),
        why: format!("stuck in {state:?} for more than {}s", limit.as_secs()),
    }
}

/// Convenience alias for a transport's result.
pub type SessionResult = Result<SessionOutcome>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_happy_path_is_legal() {
        let path = [
            AgentState::Starting,
            AgentState::Authenticating,
            AgentState::Idle,
            AgentState::Thinking,
            AgentState::ExecutingTool,
            AgentState::Idle,
        ];

        for pair in path.windows(2) {
            assert!(
                is_legal(pair[0], pair[1]),
                "{:?} → {:?} should be legal",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn permission_is_reachable_from_both_working_states() {
        assert!(is_legal(
            AgentState::Thinking,
            AgentState::AwaitingPermission
        ));
        assert!(is_legal(
            AgentState::ExecutingTool,
            AgentState::AwaitingPermission
        ));
    }

    #[test]
    fn a_permission_decision_returns_to_the_turn() {
        assert!(is_legal(
            AgentState::AwaitingPermission,
            AgentState::Thinking
        ));
        assert!(is_legal(
            AgentState::AwaitingPermission,
            AgentState::ExecutingTool
        ));
    }

    #[test]
    fn a_session_cannot_go_backwards_into_startup() {
        // Re-authenticating mid-turn, or restarting, is a new session. Allowing
        // it here would make "has this session started yet" unanswerable.
        for state in ALL_STATES {
            assert!(
                !is_legal(state, AgentState::Starting),
                "{state:?} → Starting should be illegal"
            );
        }
        assert!(!is_legal(AgentState::Thinking, AgentState::Authenticating));
        assert!(!is_legal(AgentState::Idle, AgentState::ExecutingTool));
    }

    #[test]
    fn no_state_is_a_dead_end() {
        // Invariant 4. A live state with no legal successor would be a session
        // that can only be escaped by killing the process.
        for state in ALL_STATES {
            assert!(
                ALL_STATES.iter().any(|&next| is_legal(state, next)),
                "{state:?} has no legal successor"
            );
        }
    }

    #[test]
    fn only_states_that_wait_on_a_person_are_untimed() {
        // Anything else with no bound is a place a session can hang forever,
        // which is the same invariant from the other direction.
        let timeouts = AgentTimeouts::default();

        for state in ALL_STATES {
            let untimed = timeout_for(state, &timeouts).is_none();
            let waits_on_a_person =
                matches!(state, AgentState::Idle | AgentState::AwaitingPermission);
            assert_eq!(
                untimed, waits_on_a_person,
                "{state:?} has the wrong timeout policy"
            );
        }
    }

    #[test]
    fn running_a_tool_is_allowed_far_longer_than_waiting_on_the_model() {
        // The whole reason the two states are separate. If this ever inverts, a
        // real test suite gets killed as though it were a stalled request.
        let timeouts = AgentTimeouts::default();
        let thinking = timeout_for(AgentState::Thinking, &timeouts).unwrap();
        let tool = timeout_for(AgentState::ExecutingTool, &timeouts).unwrap();

        assert!(
            tool > thinking,
            "tool {tool:?} should outlast thinking {thinking:?}"
        );
    }

    #[test]
    fn an_unrefreshed_session_is_treated_as_abandoned() {
        // A force-quit leaves records behind. They must not pile up as sessions
        // that claim to be running and never finish.
        let session = session_at(1_000);
        let stale = 1_000 + HEARTBEAT_STALE_AFTER.as_millis() as u64 + 1;

        assert!(!session.is_abandoned(1_000));
        assert!(session.is_abandoned(stale));
    }

    #[test]
    fn a_finished_session_is_never_abandoned() {
        // It is not running, so nothing is expected to be refreshing it.
        let mut session = session_at(1_000);
        session.status = SessionStatus::Ended(SessionOutcome::Completed);

        assert!(!session.is_abandoned(u64::MAX));
    }

    #[test]
    fn the_heartbeat_interval_leaves_room_for_a_busy_process() {
        assert!(
            HEARTBEAT_STALE_AFTER >= HEARTBEAT_INTERVAL * 3,
            "a single slow tick would declare a healthy session dead"
        );
    }

    #[test]
    fn status_is_tagged_on_the_wire() {
        // The client renders this directly, so pin the shape.
        assert_eq!(
            serde_json::to_value(SessionStatus::Live(AgentState::ExecutingTool)).unwrap(),
            serde_json::json!({ "status": "live", "value": "executingTool" })
        );
        assert_eq!(
            serde_json::to_value(SessionStatus::Ended(SessionOutcome::Cancelled)).unwrap(),
            serde_json::json!({ "status": "ended", "value": { "kind": "cancelled" } })
        );
    }

    fn session_at(heartbeat_ms: u64) -> Session {
        Session {
            id: SessionId::new("s-1"),
            project_id: ProjectId::new("proj"),
            worktree_id: WorktreeId::new("wt"),
            agent: "claude".to_owned(),
            status: SessionStatus::Live(AgentState::Thinking),
            driver_pid: std::process::id(),
            started_at_ms: 0,
            heartbeat_ms,
        }
    }
}
