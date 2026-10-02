//! The event bus.
//!
//! Everything observable that happens inside ket becomes an [`Event`], published
//! through an [`EventBus`] and consumed by whatever is watching — `ket events
//! --follow` today, the web shell's WebSocket in Epic 5.
//!
//! Two decisions here are load-bearing, and are made now rather than later:
//!
//! 1. **Serde-serializable, camelCase on the wire.** These envelopes become the
//!    IPC payload, and the eventual consumer is TypeScript. Picking the casing
//!    now avoids a rename sweep once a frontend exists.
//! 2. **Monotonic sequence numbers.** A dropped WebSocket has to resume from the
//!    last envelope it saw rather than losing the session (Epic 6), and that is
//!    only possible if envelopes are numbered from the start.
//!
//! [`Event`] is deliberately *not* `#[non_exhaustive]`. Everything that consumes
//! it lives in this workspace, so exhaustive matching is a feature: adding a
//! variant should produce a compile error at every site that must handle it.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::id::{ProjectId, SessionId, WorktreeId};

/// Envelopes retained for subscribers that fall behind.
///
/// A slow subscriber past this bound receives `RecvError::Lagged` and is told how
/// many it missed, rather than the producer blocking. Terminal output can be
/// bursty, so the buffer is generous.
pub const DEFAULT_CAPACITY: usize = 1024;

/// A published event plus the metadata every consumer needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    /// Monotonic within one bus, starting at 1. Used to resume a dropped stream.
    ///
    /// Per *process*, not global. Two `ket` commands running at once each start
    /// at 1, so sequence numbers in the shared journal repeat and are not an
    /// ordering across processes — use [`Envelope::at_ms`], or the order lines
    /// were appended, for that. Within the M1 shell, which owns one bus, it is
    /// exactly the resume token it looks like.
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    ///
    /// A plain integer rather than a formatted timestamp: unambiguous on the
    /// wire, and `new Date(atMs)` on the TypeScript side.
    pub at_ms: u64,
    /// The event itself, flattened so the wire shape is one object.
    #[serde(flatten)]
    pub event: Event,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    /// A project was registered with ket.
    ProjectRegistered {
        /// The project.
        project_id: ProjectId,
        /// Display name, usually the repository directory name.
        name: String,
    },

    /// A project was deregistered. The repository itself is untouched.
    ProjectRemoved {
        /// The project.
        project_id: ProjectId,
    },

    /// A project's preferences changed — its name, colour, icon, default
    /// base or preferred agent.
    ///
    /// Carries only the id. Unlike open worktrees, settings are read once per
    /// sidebar rebuild rather than on every event, so a second client is
    /// better served by re-reading them than by a copy in the journal.
    ProjectSettingsChanged {
        /// The project.
        project_id: ProjectId,
    },

    /// A worktree was created for a project.
    WorktreeCreated {
        /// The worktree.
        worktree_id: WorktreeId,
        /// Its owning project.
        project_id: ProjectId,
        /// Branch checked out in the worktree.
        branch: String,
    },

    /// A worktree was removed.
    WorktreeRemoved {
        /// The worktree.
        worktree_id: WorktreeId,
    },

    /// A project's durable state changed — which worktrees are open, or which
    /// one has focus.
    ///
    /// Carries the new state rather than just the id, so a second client can
    /// render the change without a round trip. The layout blob is deliberately
    /// not included: it is opaque to core, potentially large, and a client that
    /// cares can read it.
    ProjectStateChanged {
        /// The project.
        project_id: ProjectId,
        /// Worktrees now open, in the order they were opened.
        open: Vec<WorktreeId>,
        /// The worktree with focus, if any.
        active: Option<WorktreeId>,
    },

    /// Provisioning began for a worktree.
    WorktreeProvisionStarted {
        /// The worktree.
        worktree_id: WorktreeId,
    },

    /// A provisioning step completed.
    ///
    /// `step` is a short human-readable description such as
    /// `"cloning node_modules"` or `"copied .env"`. It names files but **never**
    /// contains their contents — provisioning copies secrets, and this string
    /// reaches the UI and the logs.
    WorktreeProvisionProgress {
        /// The worktree.
        worktree_id: WorktreeId,
        /// What just happened.
        step: String,
    },

    /// Provisioning finished successfully.
    WorktreeProvisionFinished {
        /// The worktree.
        worktree_id: WorktreeId,
        /// Wall time taken.
        duration_ms: u64,
        /// Whether any directory fell back to a full copy instead of
        /// copy-on-write — the signal that provisioning is about to get slow.
        degraded: bool,
    },

    /// Provisioning failed. The worktree is not usable by an agent.
    WorktreeProvisionFailed {
        /// The worktree.
        worktree_id: WorktreeId,
        /// What went wrong.
        why: String,
    },

    /// An agent run started against a worktree.
    AgentSessionStarted {
        /// The session.
        session_id: SessionId,
        /// The worktree it is confined to.
        worktree_id: WorktreeId,
        /// Configured agent name, e.g. `"claude"`.
        agent: String,
    },

    /// An agent reported how much context it is using.
    ///
    /// Published per turn, carrying the latest values rather than a delta — a
    /// consumer that missed one is still correct after the next.
    AgentUsage {
        /// The session.
        session_id: SessionId,
        /// The latest reading.
        usage: SessionUsage,
    },

    /// An agent session changed state.
    AgentStateChanged {
        /// The session.
        session_id: SessionId,
        /// State being left.
        from: AgentState,
        /// State being entered.
        to: AgentState,
    },

    /// An agent produced output.
    ///
    /// Normalized across transports: an ACP agent and a PTY agent both arrive
    /// here, so downstream consumers never branch on transport.
    AgentOutput {
        /// The session.
        session_id: SessionId,
        /// A chunk of output. Not guaranteed to end on a line boundary.
        chunk: String,
    },

    /// An agent asked to use a tool and configuration did not settle it.
    ///
    /// The session is now in [`AgentState::AwaitingPermission`] and will stay
    /// there until a client answers or the session is cancelled. `tool` is the
    /// ACP *tool kind* — `read`, `edit`, `execute` and so on — which is the same
    /// vocabulary whichever agent is running, so a policy written once applies
    /// to all of them.
    AgentPermissionRequested {
        /// The session.
        session_id: SessionId,
        /// Identifies this request when answering it.
        request_id: String,
        /// The kind of tool being requested.
        tool: String,
        /// The agent's own description of what it wants to do.
        title: String,
    },

    /// A permission request was settled.
    AgentPermissionResolved {
        /// The session.
        session_id: SessionId,
        /// Which request.
        request_id: String,
        /// Whether the agent may proceed.
        allowed: bool,
        /// Whether configuration decided it rather than a person.
        ///
        /// Worth distinguishing: a run where every decision came from policy is
        /// reproducible, and one where a person clicked through them is not.
        by_policy: bool,
    },

    /// An agent session finished, for any reason.
    AgentSessionEnded {
        /// The session.
        session_id: SessionId,
        /// How it ended.
        outcome: SessionOutcome,
    },
}

/// Lifecycle of an agent session.
///
/// `Starting → Authenticating → Idle → Thinking → ExecutingTool → Idle`, with
/// [`AgentState::AwaitingPermission`] reachable from either working state.
///
/// Every non-terminal state has an exit — invariant 4. There is no state here
/// that a timeout or a cancel cannot leave, and the sensible timeout differs
/// enough between them that they are tracked separately rather than merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentState {
    /// Process spawned, handshake not yet complete.
    Starting,
    /// Completing an authentication flow before the session is usable.
    ///
    /// Not every agent needs this, but some do — assuming an agent is ready to
    /// prompt the moment it spawns is how a session hangs with no explanation.
    Authenticating,
    /// Ready, waiting for a prompt.
    Idle,
    /// Waiting on the model.
    Thinking,
    /// Running a tool the model asked for — a build, a test suite, a search.
    ///
    /// Deliberately distinct from [`AgentState::Thinking`]: "waiting on the
    /// model" and "running your test suite" have wildly different expected
    /// durations, and collapsing them makes a four-minute test run
    /// indistinguishable from a stalled request.
    ExecutingTool,
    /// Blocked on a permission decision from the user.
    AwaitingPermission,
}

/// How an agent session ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionOutcome {
    /// Finished normally.
    Completed,
    /// Cancelled by the user.
    Cancelled,
    /// Failed.
    Failed {
        /// What went wrong.
        why: String,
    },
}

/// An amount of money, as an agent reported it.
///
/// Stored in millionths of a currency unit rather than as the `f64` the protocol
/// sends. Money in binary floating point is a bug waiting for a total, and
/// [`Envelope`] derives `Eq`, which `f64` cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    /// Millionths of one unit of `currency`.
    pub micros: i64,
    /// ISO 4217 code, exactly as the agent gave it.
    ///
    /// Not parsed into an enum: an agent reporting a currency ket has never heard
    /// of should still be displayable, and inventing a closed set here would turn
    /// that into an error for no gain.
    pub currency: String,
}

impl Money {
    /// Builds an amount from the protocol's floating-point form.
    pub fn from_amount(amount: f64, currency: impl Into<String>) -> Self {
        Self {
            micros: (amount * 1_000_000.0).round() as i64,
            currency: currency.into(),
        }
    }
}

/// What one agent session has consumed so far.
///
/// **Context pressure, not account quota.** This is how full the model's context
/// window is on this turn, which is what quietly ruins long runs. How much of a
/// subscription is left is a different measurement from a different source — see
/// the provider rate-limit work — and conflating them would put two unrelated
/// numbers under one label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    /// Tokens currently in context.
    pub used: u64,
    /// The context window, in tokens.
    pub size: u64,
    /// Cumulative session cost, where the agent reports one.
    pub cost: Option<Money>,
}

impl SessionUsage {
    /// How full the context is, 0.0 to 1.0, or `None` if the window is unknown.
    ///
    /// `None` rather than zero for a zero-sized window: a progress bar drawn at 0%
    /// says "plenty of room", which is the opposite of "we do not know".
    pub fn fraction(&self) -> Option<f64> {
        (self.size > 0).then(|| self.used as f64 / self.size as f64)
    }
}

/// A broadcast bus carrying everything that happens inside ket.
#[derive(Debug)]
pub struct EventBus {
    tx: broadcast::Sender<Envelope>,
    seq: AtomicU64,
    journal: Option<crate::journal::Journal>,
}

impl EventBus {
    /// Creates a bus retaining `capacity` envelopes for subscribers that fall behind.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self {
            tx,
            seq: AtomicU64::new(0),
            journal: None,
        }
    }

    /// Creates a bus that also records everything it publishes to `journal`.
    ///
    /// The in-memory channel reaches only this process. The journal is how a
    /// separate `ket events --follow` sees a `ket run` happening in another
    /// terminal — see [`crate::journal`].
    pub fn with_journal(capacity: usize, journal: crate::journal::Journal) -> Self {
        Self {
            journal: Some(journal),
            ..Self::new(capacity)
        }
    }

    /// Publishes `event`, returning the sequence number assigned to it.
    ///
    /// Publishing with no subscribers is normal, not an error: the CLI runs with
    /// nothing attached most of the time. Sequence numbers still advance, so a
    /// subscriber attaching later can tell it missed something.
    pub fn publish(&self, event: Event) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let envelope = Envelope {
            seq,
            at_ms: crate::now_ms(),
            event,
        };

        // Journal before broadcasting, so an event is on disk before anything
        // can act on it and publish a consequence of it.
        if let Some(journal) = &self.journal {
            journal.append(&envelope);
        }

        // `send` fails only when every receiver has been dropped.
        if self.tx.send(envelope).is_err() {
            tracing::trace!(seq, "event published with no subscribers");
        }

        seq
    }

    /// Subscribes to every envelope published from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }

    /// Number of live subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Event {
        Event::WorktreeCreated {
            worktree_id: WorktreeId::new("wt-1"),
            project_id: ProjectId::new("proj-a"),
            branch: "729-fix".to_owned(),
        }
    }

    #[test]
    fn sequence_numbers_start_at_one_and_increment() {
        let bus = EventBus::default();
        assert_eq!(bus.publish(sample()), 1);
        assert_eq!(bus.publish(sample()), 2);
        assert_eq!(bus.publish(sample()), 3);
    }

    #[test]
    fn publishing_without_subscribers_still_advances_the_sequence() {
        // A subscriber attaching later must be able to tell that it missed
        // events, which only works if the counter advanced while nobody watched.
        let bus = EventBus::default();
        assert_eq!(bus.subscriber_count(), 0);
        bus.publish(sample());
        bus.publish(sample());

        let mut rx = bus.subscribe();
        assert_eq!(bus.publish(sample()), 3);
        assert_eq!(rx.try_recv().unwrap().seq, 3);
    }

    #[tokio::test]
    async fn subscribers_receive_published_envelopes() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        bus.publish(sample());
        let envelope = rx.recv().await.unwrap();

        assert_eq!(envelope.seq, 1);
        assert_eq!(envelope.event, sample());
        assert!(envelope.at_ms > 0);
    }

    #[test]
    fn a_lagging_subscriber_is_told_rather_than_blocking_the_producer() {
        let bus = EventBus::new(2);
        let mut rx = bus.subscribe();

        for _ in 0..5 {
            bus.publish(sample());
        }

        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }

    #[test]
    fn wire_format_is_a_flat_camel_case_object() {
        // This is the contract the TypeScript client will be written against.
        // If it changes, that client breaks silently — so pin it here.
        let envelope = Envelope {
            seq: 7,
            at_ms: 1_725_000_000_000,
            event: sample(),
        };

        let json: serde_json::Value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "seq": 7,
                "atMs": 1_725_000_000_000_u64,
                "type": "worktreeCreated",
                "worktreeId": "wt-1",
                "projectId": "proj-a",
                "branch": "729-fix",
            })
        );
    }

    #[test]
    fn agent_states_are_camel_case_on_the_wire() {
        // Pinned deliberately: the client renders these directly, and the
        // `executingTool` / `thinking` split is a distinction the UI depends on
        // to tell a long build apart from a stalled model request.
        let cases = [
            (AgentState::Starting, r#""starting""#),
            (AgentState::Authenticating, r#""authenticating""#),
            (AgentState::Idle, r#""idle""#),
            (AgentState::Thinking, r#""thinking""#),
            (AgentState::ExecutingTool, r#""executingTool""#),
            (AgentState::AwaitingPermission, r#""awaitingPermission""#),
        ];

        for (state, expected) in cases {
            assert_eq!(serde_json::to_string(&state).unwrap(), expected);
        }
    }

    #[test]
    fn envelopes_round_trip_through_json() {
        let envelope = Envelope {
            seq: 1,
            at_ms: 42,
            event: Event::AgentSessionEnded {
                session_id: SessionId::new("s-1"),
                outcome: SessionOutcome::Failed {
                    why: "adapter not installed".to_owned(),
                },
            },
        };

        let text = serde_json::to_string(&envelope).unwrap();
        assert_eq!(serde_json::from_str::<Envelope>(&text).unwrap(), envelope);
    }
}
