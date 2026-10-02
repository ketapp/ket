//! What each worktree is doing right now.
//!
//! ket learns that an agent is working in two quite different ways, and neither
//! one alone is enough:
//!
//! 1. **Sessions it drives.** `ket run` spawns an agent through
//!    [`crate::agent`], which already models the whole lifecycle and persists a
//!    [`Session`] record. That path knows everything — which agent, which
//!    state, whether it is blocked on a permission.
//! 2. **Agents a person starts themselves.** Someone opens a terminal in a
//!    worktree and types `claude`. ket spawned the shell, not the agent, so no
//!    session record exists and the whole machinery above sees nothing. This is
//!    the common case today, and it is why the sidebar sat there showing
//!    nothing while an agent worked.
//!
//! This module is the one place those two are reconciled, so that everything
//! downstream — the sidebar now, an agent dashboard later — asks one question
//! and gets one answer per worktree. Consumers never learn which source an
//! answer came from, the same way [`crate::agent`] never lets them learn
//! whether a session is driven over ACP or a pty.
//!
//! **Observation is not control.** An [`Activity::Observed`] agent cannot be
//! cancelled, cannot be answered when it asks a permission, and will not say
//! what it is thinking. Keeping that distinct from [`Activity::Session`] in
//! the type is deliberate — a dashboard that offers a Cancel button on a row
//! ket cannot cancel is worse than one that does not.
//!
//! **Nothing here asks an agent anything.** There was a third source once: ask
//! `claude` itself which sessions are working, on the sidebar's own five-second
//! tick. It cost a process spawn — about 170ms, on the thread that draws the
//! window and reads the keyboard — to refine a state this module is explicitly
//! allowed to leave unrefined, and it answered a question the push feed had
//! already answered better. Reports come *to* ket, they are not fetched: see
//! [`crate::agent_hooks`], which installs into the agent's own global config
//! and so covers an agent a person started by hand just as much as one ket
//! launched.
//!
//! What is left in [`Activity::Observed`] is the residue — an agent with no
//! hooks, or one in a directory ket cannot place — and for that, "a process is
//! running" is the whole of the evidence and therefore the whole of the claim.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::{Session, SessionStatus};
use crate::agent_hooks::HookReport;
use crate::agent_status::{
    AgentStatusSnapshot, CancelSource, PaneAgentStatus, StatusStore, settled,
};
use crate::event::{AgentState, SessionOutcome};
use crate::id::{SessionId, WorktreeId};
use crate::sessions::DiscoveredSession;
use crate::subagents::{Subagent, SubagentState};

/// How long a hook report is believed before the other sources take over.
///
/// Hooks fire on every prompt and every tool call, so a working agent renews
/// this constantly and an idle one has sent a `Stop` saying so. The bound is
/// for the case neither covers: an agent killed outright, which sends nothing
/// and would otherwise leave the row claiming to be thinking forever.
pub const REPORT_LINGERS_MS: u64 = 10 * 60 * 1000;

/// How long a tool call's note stays on screen before a report that it has
/// gone back to thinking is allowed to retire it.
///
/// A hook fires on the way into a tool and again on the way out, and for
/// anything that finishes faster than a poll — a `git commit` on a small
/// repo, a one-line `Read` — both reports arrive between one redraw and the
/// next. Holding the newer one for less than that redraw interval does
/// nothing: the first time anyone reads the tracker after the hold starts is
/// already past it, so the row still jumps straight from "thinking" to
/// "thinking" with "merging" never having existed to a person watching it.
/// The hold has to outlast the slowest thing polling it — currently the
/// sidebar's own tick, `ket_ui::status_bar::POLL` (5s) — with enough margin
/// that a report landing right before a redraw still survives to the one
/// after it. Applied on read by
/// [`PaneAgentStatus::shown`](crate::agent_status::PaneAgentStatus::shown);
/// a shorter value here compiles and does nothing.
pub const MIN_TOOL_VISIBLE_MS: u64 = 7_000;

/// How long a fresh session may claim to be starting before it is shown as
/// ready instead.
///
/// Claude Code has a hook for "a turn began" and one for "a turn ended", but
/// none for "the process is up and sitting at its own prompt" — the moment
/// that actually deserves this word. Left alone, a session that has not yet
/// been given a first prompt would say "starting" forever: `SessionStart` is
/// the last report it will ever send before someone types something. Claude
/// Code's own startup, past that hook returning, is a few seconds at most, so
/// once this much time has passed without a further report, the row is
/// almost certainly idle rather than still booting.
pub const STARTING_READY_MS: u64 = 15 * 1000;

/// How often the agents' own stores are worth re-reading.
///
/// Slower than the tick everything else here runs on, and deliberately: a
/// scan walks a directory per agent per worktree, and what it is looking for
/// is a conversation on disk that nothing is running. That does not change
/// between one second and the next, and paying a filesystem sweep every tick
/// to find out is the kind of poll that shows up in a profile.
pub const RECORDS_EVERY_MS: u64 = 30 * 1000;

/// How long a finished session stays worth showing.
///
/// A session that failed is news for a while and then it is history. Without a
/// bound, a worktree whose agent failed once wears a red light forever and the
/// light stops meaning anything.
pub const ENDED_LINGERS_MS: u64 = 5 * 60 * 1000;

/// What a worktree's row should be lit with.
///
/// [`Activity::is_working`] answers "is a process alive", which is not the
/// same question and was never the right one for a status light: an agent
/// sitting at its own prompt is a live process making no progress, and a row
/// that goes green for it is green almost always — which is how the light
/// stopped meaning anything. This answers "what should the eye be told",
/// which is the question the sidebar and the tab strip actually have.
///
/// Deliberately smaller than [`Activity`]: the row also folds in two facts
/// core does not own — whether the checkout is dirty and whether it is
/// missing — so this covers only what an activity itself knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Nothing worth lighting: no agent, one waiting at its prompt, or a
    /// session that ended uneventfully.
    Quiet,

    /// Something is running that is not known to be an agent making progress —
    /// a build in the shell, a session still starting up, or an agent ket only
    /// watched start and cannot question. Worth saying, not worth a colour or
    /// motion: both are claims about progress that this state has no evidence
    /// for.
    Running,

    /// An agent is working: waiting on the model, or running a tool it asked
    /// for. The one state that earns the running colour.
    Working,

    /// Git is part-way through rewriting the worktree — a merge, rebase,
    /// cherry-pick or revert either running now or left unresolved.
    ///
    /// Its own state rather than a flavour of [`Signal::Working`], because it
    /// is a fact about the *checkout* and not about an agent: it survives the
    /// agent that started it, and a tree stopped mid-rebase on a conflict is
    /// not making progress at all. Rows that wear this were green before it
    /// existed, which is the one colour that says the opposite of what a
    /// half-applied rebase means.
    Merging,

    /// Stopped on a permission prompt. The only state that is asking for
    /// something, and the loudest for that reason.
    Blocked,

    /// Ended badly, recently enough to still be news.
    Failed,
}

/// What is happening in one worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Activity {
    /// Nothing running that ket can see.
    Idle,

    /// An agent session ket started and drives.
    Session {
        /// Which session, so a consumer can act on it.
        session_id: SessionId,
        /// The agent's configured name.
        agent: String,
        /// Where it is in its lifecycle.
        state: AgentState,
        /// What the agent said about the state it is in, in its own words.
        ///
        /// The tool it is running, or the line it stopped on — the state says
        /// which, so one field carries both. Only ever set from a source that
        /// hears it from the agent itself; a session ket merely drives reports
        /// its state and nothing finer. `None` falls back to the state's own
        /// phrase, which is what every row said before this existed.
        note: Option<String>,
        /// Whether the tool it is running is `git` rewriting the worktree.
        ///
        /// Only ever true while the state is
        /// [`AgentState::ExecutingTool`], and only for a session heard from
        /// through its hooks: a session ket merely drives is watched rather
        /// than heard, and knows a tool is running without knowing which.
        ///
        /// Defaulted on the way in, so an activity serialised before this
        /// field existed still reads.
        #[serde(default)]
        mutating_git: bool,
    },

    /// An agent someone started in a terminal, that has never reported.
    ///
    /// ket did not start it and cannot drive it — see the module docs — and
    /// this is all it knows: a process is alive and its name matches an agent.
    /// Not whether it is thinking, not whether it is waiting on somebody.
    ///
    /// Rare, and worth keeping rare rather than refining: an agent launched
    /// into any ket pane inherits the hook environment, so it reports for
    /// itself and arrives as [`Activity::Session`] instead. What lands here is
    /// an agent whose hooks ket never installed, or one whose worktree it
    /// could not place — and for those, "something is running" is the whole of
    /// the evidence and therefore the whole of the claim.
    Observed {
        /// The command, which matched a known agent.
        agent: String,
    },

    /// Something that is not an agent is running in the worktree's terminal —
    /// a build, a test run, a long `git` operation.
    ///
    /// Worth surfacing for the same reason the agent states are: a worktree
    /// that is busy looks identical to an idle one otherwise.
    Busy {
        /// What to say about it — a recognised git verb ("committing",
        /// "merging") when the command is one, else the foreground command's
        /// name.
        command: String,
        /// Whether this is git changing the worktree's history: a commit,
        /// merge, rebase, cherry-pick, revert, reset, push, pull, checkout,
        /// or stash. Unlike an anonymous build or test run, this is progress
        /// on the worktree itself and earns the same colour a working agent
        /// gets.
        mutating: bool,
    },

    /// A session an agent recorded in its own store, with nothing running now.
    ///
    /// This is what a worktree looks like after ket has been closed and
    /// reopened: the process is gone, but the conversation is on disk and can
    /// be resumed. See [`crate::sessions`].
    Recorded {
        /// The agent that recorded it.
        agent: String,
        /// Its own identifier, which its resume flag expects.
        session: String,
        /// What the agent calls it, when it names sessions at all.
        title: Option<String>,
        /// When it was last written to.
        updated_at_ms: u64,
    },

    /// A session that finished recently enough to still be worth showing.
    Ended {
        /// The agent that was running.
        agent: String,
        /// How it went.
        outcome: SessionOutcome,
    },
}

impl Activity {
    /// What the row should say about this, as opposed to what is technically
    /// running — see [`Signal`].
    ///
    /// The answers that withhold a colour are the point of the whole type. An
    /// [`Activity::Observed`] agent is a process ket did not start and cannot
    /// question: it knows the program is alive and nothing else, so it reports
    /// only that much — claiming it is *working* is a claim it has no evidence
    /// for. And a [`Session`](Activity::Session) parked on
    /// [`AgentState::Idle`] has told ket, in so many words, that it is waiting
    /// for a person. Neither one gets a colour.
    pub fn signal(&self) -> Signal {
        match self {
            Activity::Idle | Activity::Recorded { .. } => Signal::Quiet,
            // Git rewriting the worktree's history is its own state: the
            // thing making progress is the checkout, not an agent, and it is
            // worth telling apart from a turn at a glance. Any other busy
            // command — a build, a test run, a read-only `git status` — makes
            // no claim ket has evidence for, so it stays uncoloured like an
            // agent it could only observe.
            Activity::Busy { mutating: true, .. } => Signal::Merging,
            Activity::Observed { .. } | Activity::Busy { .. } => Signal::Running,
            // An agent's own `git rebase` earns the same state a rebase in
            // the terminal does. The tool call is indistinguishable from any
            // other from the outside — Claude sends "read the dialog
            // conflict" as its description either way — so this rides on the
            // command the hook reports rather than on anything visible, which
            // is exactly why the row used to sit on the running colour
            // through an entire conflicted rebase.
            Activity::Session {
                state: AgentState::ExecutingTool,
                mutating_git: true,
                ..
            } => Signal::Merging,
            Activity::Session { state, .. } => match state {
                AgentState::Idle => Signal::Quiet,
                AgentState::Starting | AgentState::Authenticating => Signal::Running,
                AgentState::Thinking | AgentState::ExecutingTool => Signal::Working,
                AgentState::AwaitingPermission => Signal::Blocked,
            },
            Activity::Ended { outcome, .. } => match outcome {
                SessionOutcome::Failed { .. } => Signal::Failed,
                SessionOutcome::Cancelled | SessionOutcome::Completed => Signal::Quiet,
            },
        }
    }

    /// Whether something is running right now.
    pub fn is_working(&self) -> bool {
        matches!(
            self,
            Activity::Session { .. } | Activity::Observed { .. } | Activity::Busy { .. }
        )
    }

    /// Whether it is stopped waiting for a person.
    ///
    /// The one state that should pull the eye: an agent blocked on a permission
    /// prompt makes no progress until someone answers it, and looks exactly
    /// like a thinking one until you open the pane.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            Activity::Session {
                state: AgentState::AwaitingPermission,
                ..
            }
        )
    }

    /// Whether it ended badly.
    pub fn failed(&self) -> bool {
        matches!(
            self,
            Activity::Ended {
                outcome: SessionOutcome::Failed { .. },
                ..
            }
        )
    }

    /// Whether ket can act on this — cancel it, answer it.
    ///
    /// False for anything observed: see the module docs.
    pub fn is_controllable(&self) -> bool {
        matches!(self, Activity::Session { .. })
    }

    /// The agent's name, where there is one.
    pub fn agent(&self) -> Option<&str> {
        match self {
            Activity::Session { agent, .. }
            | Activity::Observed { agent, .. }
            | Activity::Recorded { agent, .. }
            | Activity::Ended { agent, .. } => Some(agent),
            Activity::Idle | Activity::Busy { .. } => None,
        }
    }

    /// The phrase that goes *beside* the agent's own mark.
    ///
    /// [`Self::label`] names the agent because it is read on its own; a row
    /// that draws the provider's logo has already said which agent it is, and
    /// repeating the word next to the mark is the sort of doubling that makes
    /// a sidebar look unconsidered. So this is the same phrase with the name
    /// taken out — and, for the states that have no agent, the whole of it.
    pub fn detail(&self) -> String {
        match self {
            Activity::Idle => String::new(),
            Activity::Session { state, note, .. } => doing(*state, note.as_deref()),
            // Running, and that is the whole of what is known about it. An
            // agent that has reported says what it is doing and arrives as
            // `Session`; one that has not is a live process and no more, so
            // the word matches `signal()`'s uncoloured `Signal::Running`
            // rather than claiming a turn nobody has evidence of.
            Activity::Observed { .. } => "running".to_owned(),
            Activity::Busy { command, .. } => command.clone(),
            Activity::Recorded { title, .. } => match title {
                Some(title) => title.clone(),
                None => "session ready".to_owned(),
            },
            Activity::Ended { outcome, .. } => match outcome {
                SessionOutcome::Failed { .. } => "failed".to_owned(),
                SessionOutcome::Cancelled => "cancelled".to_owned(),
                SessionOutcome::Completed => "done".to_owned(),
            },
        }
    }

    /// One short phrase for a row that has no room for a sentence.
    pub fn label(&self) -> String {
        match self {
            Activity::Idle => String::new(),
            Activity::Session {
                agent, state, note, ..
            } => {
                format!("{agent} · {}", doing(*state, note.as_deref()))
            }
            // The agent's name alone: `label` is read where there is no mark
            // beside it, and "claude · working" for a session ket cannot
            // question claims more than it knows.
            Activity::Observed { agent, .. } => agent.clone(),
            Activity::Busy { command, .. } => command.clone(),
            Activity::Recorded { agent, title, .. } => match title {
                Some(title) => format!("{agent} · {title}"),
                None => agent.clone(),
            },
            Activity::Ended { agent, outcome } => match outcome {
                SessionOutcome::Failed { .. } => format!("{agent} · failed"),
                SessionOutcome::Cancelled => format!("{agent} · cancelled"),
                SessionOutcome::Completed => format!("{agent} · done"),
            },
        }
    }
}

/// What a session is doing, preferring the agent's own words for it.
///
/// The state's phrase is the floor, not the answer. "running a tool" is true
/// of a two-minute test suite and of reading one file, and "ready" is what a
/// row said for the whole hour after an agent finished — both of them true,
/// both of them the least interesting thing that could be said. Once the
/// agent has spoken for itself, saying that back is strictly better. The
/// states in between answer for themselves: there is nothing finer to know
/// about *thinking*.
fn doing(state: AgentState, note: Option<&str>) -> String {
    match (state, note) {
        (AgentState::ExecutingTool | AgentState::Idle, Some(note)) => note.to_owned(),
        _ => describe(state).to_owned(),
    }
}

/// A live state in the words a person reading a sidebar would use.
fn describe(state: AgentState) -> &'static str {
    match state {
        AgentState::Starting => "starting",
        AgentState::Authenticating => "signing in",
        AgentState::Idle => "ready",
        AgentState::Thinking => "thinking",
        AgentState::ExecutingTool => "running a tool",
        AgentState::AwaitingPermission => "waiting on you",
    }
}

/// What a worktree's terminal has in its foreground.
///
/// Fed in by whoever owns the terminals — the shell — because core has no
/// handle on them and no business polling one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Foreground {
    /// The shell itself is at a prompt; nothing is running.
    Shell,
    /// Some other program has the terminal.
    Running(String),
}

/// Every worktree's current activity, merged from every source.
///
/// A projection, not a store. What agents said about themselves lives in a
/// [`StatusStore`] — this process's own, for the terminals it runs, and a
/// replica of the ket host's, for the terminals the host runs — and this
/// combines those with what it can see for itself: each terminal's
/// foreground, the sessions ket drives, and what the agents have on disk.
/// [`Tracker::activity`] applies the linger rule on read so a caller cannot
/// see a stale state that should have expired.
#[derive(Debug, Default)]
pub struct Tracker {
    /// The last foreground reading per worktree.
    observed: HashMap<WorktreeId, Foreground>,
    /// The most recent session per worktree, driven by ket.
    sessions: HashMap<WorktreeId, Session>,
    /// Agent names to recognise in a terminal's foreground.
    known_agents: Vec<String>,
    /// What the agents in this process's own terminals said about
    /// themselves.
    ///
    /// The highest authority there is: every other source infers, this one is
    /// the agent reporting on itself.
    local: StatusStore,
    /// The host's store, as it last sent it — the same authority, for the
    /// terminals the host runs. Kept through a lost connection, so rows do
    /// not go dark while it comes back; its evidence ages as it would have.
    hosted: Option<AgentStatusSnapshot>,
    /// The newest session each worktree's agents have on disk.
    ///
    /// The third source, and the only one that survives ket being closed: a
    /// process dies with the app, a store does not.
    recorded: HashMap<WorktreeId, DiscoveredSession>,
    /// When the stores were last read, for [`Self::wants_records`].
    recorded_at_ms: Option<u64>,
}

impl Tracker {
    /// A tracker that recognises `agents` when it sees them in a terminal.
    ///
    /// The names come from [`crate::agents::Catalogue`] rather than a list here:
    /// an agent ket can launch is an agent ket should recognise, and two lists
    /// would drift.
    pub fn new(agents: impl IntoIterator<Item = String>) -> Self {
        Self {
            known_agents: agents.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Whether the agents' stores are due another read — see
    /// [`RECORDS_EVERY_MS`].
    ///
    /// The policy lives here rather than in the caller so that every consumer
    /// of a tracker gets the same cadence without having to know there is one.
    pub fn wants_records(&self, now_ms: u64) -> bool {
        self.recorded_at_ms
            .is_none_or(|at| now_ms.saturating_sub(at) >= RECORDS_EVERY_MS)
    }

    /// Replaces what every worktree has on disk, and marks the stores read.
    ///
    /// The bulk form of [`Self::record`], on [`Self::sync_sessions`]'s terms:
    /// a worktree absent from the answer has nothing recorded for it, and a
    /// stale entry left behind is a row offering to resume a conversation
    /// that has been deleted.
    pub fn sync_records(
        &mut self,
        now_ms: u64,
        records: impl IntoIterator<Item = (WorktreeId, DiscoveredSession)>,
    ) {
        self.recorded.clear();
        self.recorded.extend(records);
        self.recorded_at_ms = Some(now_ms);
    }

    /// Replaces the worktrees a Codex report's working directory can be
    /// placed in — see [`StatusStore::route`].
    pub fn set_worktrees(&mut self, worktrees: impl IntoIterator<Item = (String, PathBuf)>) {
        self.local.set_worktrees(worktrees);
    }

    /// Corrects the worktree on a report this tracker is not the owner of,
    /// for the readers of it that are not about status — usage, rollouts.
    pub fn route(&self, report: &mut HookReport) {
        self.local.route(report);
    }

    /// Takes one report from an agent in one of this process's own
    /// terminals. `known` says whether a pane key names one of them. Returns
    /// whether anything changed.
    pub fn report(&mut self, report: &HookReport, known: impl Fn(&str) -> bool) -> bool {
        self.local.report(report, known)
    }

    /// A cancel verdict typed into one of this process's own terminals — see
    /// [`StatusStore::cancel`].
    pub fn cancel(&mut self, pane: &str, source: CancelSource, now_ms: u64) -> bool {
        self.local.cancel(pane, source, now_ms)
    }

    /// Escape typed into one of this process's own terminals — see
    /// [`StatusStore::escape`].
    pub fn escape(&mut self, pane: &str, now_ms: u64) -> bool {
        self.local.escape(pane, now_ms)
    }

    /// Forgets one of this process's own terminals, which has closed or
    /// exited.
    pub fn clear_pane(&mut self, pane: &str) -> bool {
        self.local.clear(pane)
    }

    /// Takes the host's latest snapshot, if it is newer than the one held —
    /// see [`AgentStatusSnapshot::supersedes`]. Returns whether it was.
    pub fn accept_snapshot(&mut self, snapshot: AgentStatusSnapshot) -> bool {
        if !snapshot.supersedes(self.hosted.as_ref()) {
            return false;
        }
        self.hosted = Some(snapshot);
        true
    }

    /// Records the newest session an agent has on disk for `worktree`.
    ///
    /// `None` clears it, for a worktree whose store has nothing in it.
    pub fn record(&mut self, worktree: &WorktreeId, session: Option<DiscoveredSession>) {
        match session {
            Some(session) => {
                self.recorded.insert(worktree.clone(), session);
            }
            None => {
                self.recorded.remove(worktree);
            }
        }
    }

    /// Records what a worktree's terminal has in its foreground.
    ///
    /// `None` means the worktree has no terminal at all, which is different
    /// from having one sitting at a prompt.
    pub fn observe(&mut self, worktree: &WorktreeId, foreground: Option<Foreground>) {
        match foreground {
            Some(state) => {
                self.observed.insert(worktree.clone(), state);
            }
            None => {
                self.observed.remove(worktree);
            }
        }
    }

    /// Replaces what is known about ket-driven sessions.
    ///
    /// Takes the whole list rather than deltas: the store is the truth, this is
    /// a projection of it, and reconciling deltas against a file another process
    /// also writes is how the two drift apart.
    pub fn sync_sessions(&mut self, sessions: impl IntoIterator<Item = Session>) {
        self.sessions.clear();
        for session in sessions {
            // Newest wins: a worktree run several times over should report what
            // it is doing now, not the first thing it ever did.
            let keep = self
                .sessions
                .get(&session.worktree_id)
                .is_none_or(|existing| session.started_at_ms >= existing.started_at_ms);
            if keep {
                self.sessions.insert(session.worktree_id.clone(), session);
            }
        }
    }

    /// Every pane any owner has told this tracker about. A pane is in one
    /// store or the other, never both: its agent reports to whichever
    /// process runs its terminal.
    fn panes(&self) -> impl Iterator<Item = &PaneAgentStatus> {
        self.local.entries().chain(
            self.hosted
                .iter()
                .flat_map(|snapshot| snapshot.panes.iter()),
        )
    }

    /// Whether a pane's report is still the best thing known about it.
    ///
    /// Two ways to stand, and the second is the one that matters.
    ///
    /// A report is believed on its own for [`REPORT_LINGERS_MS`]. But that
    /// bound was only ever there to cover an agent *killed outright*, which
    /// sends no `Stop` and would otherwise leave the row claiming to be
    /// thinking forever. That is a question about whether a process is alive,
    /// and a wall clock is a poor way to ask it: an agent sitting at its own
    /// prompt is alive, has already said it is idle, and is doing exactly what
    /// it last reported — yet ten minutes on, the clock alone throws that away
    /// and the row falls back to guessing about a process ket can see.
    ///
    /// So the foreground answers it instead, which costs nothing: it is the
    /// same reading the tracker already takes every tick. While the worktree
    /// still has that agent in it, the report has not gone stale. When the
    /// agent is gone, so is the evidence, and the linger expires as it always
    /// did.
    ///
    /// **Only for a state a live process confirms.** An agent that last said
    /// it was idle and is still running is still idle — nothing but a new
    /// prompt can change that, and a new prompt sends a report. But an agent
    /// that last said it was *working* may have been interrupted, and Claude
    /// Code sends nothing at all for that, so a cancelled turn's last word is
    /// whatever it said on the way in. Renewing that on liveness alone would
    /// pin a row to "thinking" for as long as the agent stayed open. Those
    /// keep the wall clock, and fall back to it when it runs out.
    fn stands(&self, pane: &PaneAgentStatus, now_ms: u64) -> bool {
        // A subagent at work is the lead at work: a foreground one holds the
        // lead inside a single tool call for as long as it runs.
        if now_ms.saturating_sub(pane.heard_at_ms()) < REPORT_LINGERS_MS {
            return true;
        }
        if !settled(pane.state) {
            return false;
        }

        // The *same* agent, not merely any: someone who quits `claude` and
        // starts `codex` in that pane has replaced the thing the report was
        // about, and renewing it would caption the new agent with the old
        // one's last words.
        let worktree = WorktreeId::new(pane.worktree.clone());
        let Some(Foreground::Running(command)) = self.observed.get(&worktree) else {
            return false;
        };
        self.match_agent(command)
            .is_some_and(|agent| agent.eq_ignore_ascii_case(&pane.agent))
    }

    /// What one pane's agent is doing, and the signal it earns with its
    /// subagents rolled in.
    fn project(&self, pane: &PaneAgentStatus, now_ms: u64) -> (Activity, Signal) {
        let worktree = WorktreeId::new(pane.worktree.clone());
        let (reported, note, mutating_git, at_ms) = pane.shown(now_ms);
        let state = if reported == AgentState::Starting
            && self.has_started(&worktree, now_ms.saturating_sub(at_ms))
        {
            AgentState::Idle
        } else {
            reported
        };
        let activity = Activity::Session {
            session_id: SessionId::new(format!("hook:{worktree}")),
            agent: pane.agent.clone(),
            state,
            note,
            mutating_git,
        };
        let signal = roll_up(activity.signal(), &pane.subagents());
        (activity, signal)
    }

    /// The standing panes in `worktree`.
    fn panes_in<'a>(
        &'a self,
        worktree: &'a WorktreeId,
        now_ms: u64,
    ) -> impl Iterator<Item = &'a PaneAgentStatus> + 'a {
        self.panes()
            .filter(move |pane| pane.worktree == worktree.as_str() && self.stands(pane, now_ms))
    }

    /// The pane that speaks for `worktree`: the loudest by the sidebar's own
    /// rank, and the one heard from last between equals — its subagents
    /// included, since their work is the lead's.
    fn leading(&self, worktree: &WorktreeId, now_ms: u64) -> Option<(Activity, Signal)> {
        self.panes_in(worktree, now_ms)
            .map(|pane| {
                let (activity, signal) = self.project(pane, now_ms);
                (rank(signal), pane.heard_at_ms(), activity, signal)
            })
            .max_by_key(|(rank, at_ms, ..)| (*rank, *at_ms))
            .map(|(_, _, activity, signal)| (activity, signal))
    }

    /// What `worktree` is doing, as of the last time it was told.
    ///
    /// `now_ms` decides whether a finished session has lingered long enough to
    /// stop being news — passed in rather than read from the clock so the rule
    /// is the caller's to control.
    pub fn activity(&self, worktree: &WorktreeId, now_ms: u64) -> Activity {
        // An agent reporting on itself beats anything inferred about it.
        match self.leading(worktree, now_ms) {
            Some((activity, _)) => activity,
            None => self.inferred(worktree, now_ms),
        }
    }

    /// What one pane's agent is doing and the signal it earns, while its own
    /// report stands. `None` for a pane no agent has reported from.
    pub fn pane(&self, pane: &str, now_ms: u64) -> Option<(Activity, Signal)> {
        self.local
            .get(pane)
            .or_else(|| {
                self.hosted
                    .iter()
                    .flat_map(|snapshot| snapshot.panes.iter())
                    .find(|entry| entry.pane == pane)
            })
            .filter(|entry| self.stands(entry, now_ms))
            .map(|entry| self.project(entry, now_ms))
    }

    /// What `worktree` is doing by every source except the agents' own
    /// reports: the sessions ket drives, the terminals' foreground, and what
    /// is on disk.
    pub fn inferred(&self, worktree: &WorktreeId, now_ms: u64) -> Activity {
        // A driven session knows strictly more than an observation does, so it
        // wins whenever there is one to have.
        if let Some(session) = self.sessions.get(worktree) {
            match &session.status {
                SessionStatus::Live(state) => {
                    return Activity::Session {
                        session_id: session.id.clone(),
                        agent: session.agent.clone(),
                        state: *state,
                        // A driven session is watched, not heard from: the
                        // lifecycle says it is running a tool and never which.
                        note: None,
                        mutating_git: false,
                    };
                }
                SessionStatus::Ended(outcome) => {
                    let ended_at = session.heartbeat_ms.max(session.started_at_ms);
                    if now_ms.saturating_sub(ended_at) < ENDED_LINGERS_MS {
                        return Activity::Ended {
                            agent: session.agent.clone(),
                            outcome: outcome.clone(),
                        };
                    }
                }
            }
        }

        match self.observed.get(worktree) {
            Some(Foreground::Running(command)) => match self.match_agent(command) {
                Some(agent) => Activity::Observed { agent },
                None => match git_progress(command) {
                    Some((command, mutating)) => Activity::Busy { command, mutating },
                    None => Activity::Busy {
                        command: short_name(command),
                        mutating: false,
                    },
                },
            },
            // Nothing is running, so fall back to what is on disk. This is the
            // reopened-app case: no process, no session record ket wrote, but
            // the agent's own store still has the conversation.
            Some(Foreground::Shell) | None => match self.recorded.get(worktree) {
                Some(found) => Activity::Recorded {
                    agent: found.agent.clone(),
                    session: found.id.clone(),
                    title: found.title.clone(),
                    updated_at_ms: found.updated_at_ms,
                },
                None => Activity::Idle,
            },
        }
    }

    /// The subagents `worktree`'s agents have running, oldest first.
    ///
    /// Only those whose lead's own report still stands: a subagent is only as
    /// believable as the evidence for the agent that started it.
    pub fn subagents(&self, worktree: &WorktreeId, now_ms: u64) -> Vec<Subagent> {
        let mut rows: Vec<Subagent> = self
            .panes_in(worktree, now_ms)
            .flat_map(PaneAgentStatus::subagents)
            .collect();
        rows.sort_by(|a, b| (a.started_at_ms, &a.id).cmp(&(b.started_at_ms, &b.id)));
        rows
    }

    /// What `worktree`'s row should be lit with, subagents included: the
    /// loudest of its panes, each rolled up with its own subagents — see
    /// [`roll_up`].
    pub fn signal(&self, worktree: &WorktreeId, now_ms: u64) -> Signal {
        match self.leading(worktree, now_ms) {
            Some((_, signal)) => signal,
            None => self.inferred(worktree, now_ms).signal(),
        }
    }

    /// Every worktree with something to report, for a consumer that wants the
    /// whole picture rather than one row — the dashboard this exists for.
    pub fn working(&self, now_ms: u64) -> Vec<(WorktreeId, Activity)> {
        let reported: Vec<WorktreeId> = self
            .panes()
            .map(|pane| WorktreeId::new(pane.worktree.clone()))
            .collect();
        let mut out: Vec<(WorktreeId, Activity)> = self
            .observed
            .keys()
            .chain(self.sessions.keys())
            .chain(self.recorded.keys())
            .chain(reported.iter())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|id| (id.clone(), self.activity(id, now_ms)))
            .filter(|(_, activity)| !matches!(activity, Activity::Idle))
            .collect();
        out.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        out
    }

    /// Whether a foreground command line names one of the agents ket knows.
    ///
    /// The whole command line is scanned, not just the executable, because an
    /// agent installed through npm runs as `node …/claude-code/cli.js` — the
    /// executable is `node` and only the arguments still say `claude`. Tokens
    /// are split on the separators a path and an argument list use, so `claude`
    /// matches `/usr/local/bin/claude`, `claude-code` and `@anthropic-ai/claude-code`
    /// but not a directory that merely contains the letters.
    /// Whether a session that reported itself as starting has finished.
    ///
    /// `SessionStart` is the only thing Claude Code says before somebody types,
    /// and it means "a session exists" rather than "the process is still coming
    /// up" — by the time the hook runs, the agent is there. So the honest
    /// answer is not a stopwatch: if the agent's own command is what ket can
    /// see in that worktree's terminal, it has started, and the row should say
    /// so at once rather than after a delay chosen to be probably long enough.
    ///
    /// The timer stays as the fallback, for the worktrees where there is
    /// nothing to see — an agent in a terminal ket does not own, or one whose
    /// process name does not resemble its configured command.
    fn has_started(&self, worktree: &WorktreeId, elapsed: u64) -> bool {
        if let Some(Foreground::Running(command)) = self.observed.get(worktree)
            && self.match_agent(command).is_some()
        {
            return true;
        }
        elapsed >= STARTING_READY_MS
    }

    fn match_agent(&self, command: &str) -> Option<String> {
        let tokens: Vec<String> = command
            .split(|c: char| c == '/' || c == '\\' || c.is_whitespace() || c == '@')
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();

        self.known_agents.iter().find_map(|agent| {
            let wanted = agent.to_ascii_lowercase();
            let hit = tokens.iter().any(|token| {
                let stem = token.split('.').next().unwrap_or(token);
                stem == wanted || stem.starts_with(&format!("{wanted}-"))
            });
            hit.then(|| agent.clone())
        })
    }
}

/// A lead's own signal, raised by what its subagents are doing.
///
/// A subagent waiting on a person is the lead waiting on one — the prompt is
/// in the lead's pane. A subagent at work keeps a lead that has finished its
/// turn reading as working, as long as the background work it left running
/// goes on.
pub fn roll_up(lead: Signal, subagents: &[Subagent]) -> Signal {
    if subagents.iter().any(|s| s.state == SubagentState::Blocked) {
        return Signal::Blocked;
    }
    match lead {
        Signal::Quiet | Signal::Running
            if subagents.iter().any(|s| s.state == SubagentState::Working) =>
        {
            Signal::Working
        }
        other => other,
    }
}

/// A command line reduced to something that fits a row.
///
/// The executable's leaf and nothing else: `/opt/homebrew/bin/cargo test --all`
/// is `cargo`, which is the part a person scanning a sidebar is reading for.
fn short_name(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .unwrap_or(command)
        .rsplit('/')
        .next()
        .unwrap_or(command)
        .to_owned()
}

/// Global `git` flags that take a value, so that value is never mistaken for
/// the subcommand — `git -C ../other-worktree commit` is committing, not
/// running some unknown verb named after a path.
const GIT_FLAGS_WITH_ARG: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

/// Where a signal stands in the sidebar's order of what to show first — the
/// same order `ket_ui`'s project list sorts by.
fn rank(signal: Signal) -> u8 {
    match signal {
        Signal::Blocked => 5,
        Signal::Working => 4,
        Signal::Merging => 3,
        Signal::Running => 2,
        Signal::Failed => 1,
        Signal::Quiet => 0,
    }
}

/// What a live `git` invocation in the worktree's terminal is doing, read off
/// the same command line [`short_name`] would otherwise cut down to just
/// `git` — the subcommand is the only part of it a person cares about.
///
/// `None` for anything that is not `git`, and for a `git` subcommand this
/// does not recognise: better to fall back to the plain command name than
/// guess at a verb that turns out to be a typo or a plumbing command nobody
/// asked for. The bool says whether it counts as changing the worktree's
/// history, which is what [`Activity::Busy`]'s `mutating` field carries.
pub(crate) fn git_progress(command: &str) -> Option<(String, bool)> {
    let mut tokens = command.split_whitespace();
    let exe = tokens.next()?;
    if exe.rsplit(['/', '\\']).next().unwrap_or(exe) != "git" {
        return None;
    }

    let verb = loop {
        let token = tokens.next()?;
        if GIT_FLAGS_WITH_ARG.contains(&token) {
            tokens.next();
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        break token;
    };

    let (phrase, mutating) = match verb {
        "commit" => ("committing", true),
        "merge" => ("merging", true),
        "rebase" => ("rebasing", true),
        "cherry-pick" => ("cherry-picking", true),
        "revert" => ("reverting", true),
        "reset" => ("resetting", true),
        "push" => ("pushing", true),
        "pull" => ("pulling", true),
        "checkout" | "switch" => ("switching branches", true),
        "stash" => ("stashing", true),
        "fetch" => ("fetching", false),
        _ => return None,
    };
    Some((phrase.to_owned(), mutating))
}
