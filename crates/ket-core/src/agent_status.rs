//! What each pane's agent last said about itself — the one store of it.
//!
//! The process that runs an agent owns what is known about it, and
//! everything that shows it reads a [`AgentStatusSnapshot`] of that. With
//! the ket host enabled that process is the host, for the terminals it runs;
//! otherwise it is the window. Either way it is this type, so both modes mean
//! the same thing by a status.
//!
//! Keyed by pane — the `KET_PANE_KEY` a terminal hands its agent — rather
//! than by worktree. A worktree holds several panes, and two agents side by
//! side used to overwrite each other's reports: a `Ctrl-C` in one could not
//! even find its own turn to stop. The worktree is an attribute, which
//! [`crate::activity::Tracker`] rolls panes up by.
//!
//! Agent-agnostic. Claude Code and Codex share one [`HookEvent`] vocabulary,
//! so the only per-agent parts are Codex's worktree repair ([`Self::route`])
//! and which cancel sources an agent has: Codex reports `Interrupt` itself,
//! Claude Code reports nothing at all when a turn is cancelled.
//!
//! # Cancellation
//!
//! A cancel is a verdict on the lead agent, latched: the store holds it
//! against the same turn's restatements that arrive after it — a
//! `PostToolUse` delivered late would otherwise turn a cancelled pane
//! straight back to thinking. Hook times are when ket *received* a report,
//! not when the agent sent it, so ordering cannot come from timestamps; the
//! latch is what does it.
//!
//! A verdict comes from a Codex `Interrupt`, a `Ctrl-C` typed at a working
//! lead, or a phone's interrupt. A bare Escape is not one — it also dismisses
//! menus and clears input — so it stays an unlatched guess that the next
//! real report replaces outright. See [`StatusStore::escape`].

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::activity::{MIN_TOOL_VISIBLE_MS, REPORT_LINGERS_MS};
use crate::agent_hooks::{HookEvent, HookReport, PROMPT_TOOLS, PermissionQuestion};
use crate::event::AgentState;
use crate::subagents::{Roster, Subagent};

/// The pane a report with no pane of its own is filed under, followed by its
/// worktree: an agent ket did not launch, placed by its working directory.
const UNPLACED: &str = "worktree:";

/// Who decided a turn was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelSource {
    /// The agent said so itself: Codex's `Interrupt`.
    Provider,
    /// `Ctrl-C` typed into the pane while its lead was working.
    CtrlC,
    /// A paired phone's interrupt, which the host types as `Ctrl-C`.
    Phone,
}

/// A cancelled turn, held against that turn's late reports — see the module
/// docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelVerdict {
    /// Who cancelled it.
    pub source: CancelSource,
    /// When.
    pub at_ms: u64,
}

/// The tool call a lead last started, kept so a call that finishes between
/// two redraws is still seen — see [`MIN_TOOL_VISIBLE_MS`].
///
/// In the store rather than decided on the way in, because a snapshot may
/// carry a call's start and its end at once: a reader that only ever sees the
/// state after both would otherwise never show the call at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// What the agent said it was running.
    pub note: Option<String>,
    /// Whether it was `git` rewriting the worktree.
    pub mutating_git: bool,
    /// When it started.
    pub started_at_ms: u64,
    /// When it came back, once it has.
    pub ended_at_ms: Option<u64>,
}

/// One pane's agent, as its own hooks describe it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneAgentStatus {
    /// The pane's stable key.
    pub pane: String,
    /// The worktree its rows roll up under.
    pub worktree: String,
    /// Which agent.
    pub agent: String,
    /// The agent's own session id, when it has sent one.
    pub session: Option<String>,
    /// Where the lead is in its lifecycle.
    pub state: AgentState,
    /// What the lead said about that state, if anything.
    pub note: Option<String>,
    /// Whether the lead's tool in flight is `git` rewriting the worktree.
    pub mutating_git: bool,
    /// When the lead was last heard from. Never refreshed by a snapshot being
    /// passed on — only by the agent.
    pub at_ms: u64,
    /// A cancelled turn, while it is held.
    pub cancel: Option<CancelVerdict>,
    /// See [`ToolCall`].
    pub last_tool: Option<ToolCall>,
    /// What the lead is waiting on a person to allow, while it waits and its
    /// hook said. See [`crate::agent_hooks::PermissionQuestion`].
    #[serde(default)]
    pub question: Option<PermissionQuestion>,
    pub(crate) roster: Roster,
}

/// A new prompt's id: never `0`, never the same twice in a run, and not the
/// same run to run — a phone that answers across a host restart must not
/// match a prompt it never saw.
fn next_question_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let _ = NEXT.compare_exchange(
        0,
        // Nanoseconds since the epoch, shifted clear of the count below.
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |since| since.as_nanos() as u64))
            | 1,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    NEXT.fetch_add(1, Ordering::AcqRel)
}

impl PaneAgentStatus {
    fn new(pane: String, worktree: String, agent: String, at_ms: u64) -> Self {
        Self {
            pane,
            worktree,
            agent,
            session: None,
            state: AgentState::Idle,
            note: None,
            mutating_git: false,
            at_ms,
            cancel: None,
            last_tool: None,
            question: None,
            roster: Roster::default(),
        }
    }

    /// The lead's state as it should read at `now_ms`: the last tool call
    /// in place of "thinking" until it has been on screen for
    /// [`MIN_TOOL_VISIBLE_MS`], and otherwise exactly what was reported.
    ///
    /// Returns the state, its note, the git flag and when it began.
    pub fn shown(&self, now_ms: u64) -> (AgentState, Option<String>, bool, u64) {
        match &self.last_tool {
            Some(tool)
                if self.state == AgentState::Thinking
                    && now_ms.saturating_sub(tool.started_at_ms) < MIN_TOOL_VISIBLE_MS =>
            {
                (
                    AgentState::ExecutingTool,
                    tool.note.clone(),
                    tool.mutating_git,
                    tool.started_at_ms,
                )
            }
            _ => (self.state, self.note.clone(), self.mutating_git, self.at_ms),
        }
    }

    /// When anything in the pane — the lead or one of its subagents — was
    /// last heard from.
    pub fn heard_at_ms(&self) -> u64 {
        self.roster
            .heard_at_ms()
            .map_or(self.at_ms, |child| child.max(self.at_ms))
    }

    /// The subagents the lead has running.
    pub fn subagents(&self) -> Vec<Subagent> {
        self.roster.rows(&self.pane).collect()
    }

    /// Whether a report at `at_ms` finds this lead still believable without
    /// help — fresh, or in a state it leaves only by saying so.
    fn stands_at(&self, at_ms: u64) -> bool {
        at_ms.saturating_sub(self.heard_at_ms()) < REPORT_LINGERS_MS || settled(self.state)
    }

    /// Applies a cancel verdict: the lead stops, and so does everything the
    /// turn had running that sends no stop of its own.
    fn cancel_now(&mut self, source: CancelSource, at_ms: u64) {
        self.stop(at_ms);
        self.cancel = Some(CancelVerdict { source, at_ms });
    }

    /// Stops the lead without latching anything — the shared half of a
    /// verdict and of Escape's guess.
    fn stop(&mut self, at_ms: u64) {
        self.state = AgentState::Idle;
        self.note = None;
        self.question = None;
        self.mutating_git = false;
        self.at_ms = at_ms;
        // Whatever it was running belonged to the turn that has just ended,
        // and showing it for the rest of its minimum would undo this.
        self.last_tool = None;
        // So did its subagents, which send no stop when cancelled. One
        // running in the background that survived comes back with its next
        // event.
        self.roster.clear();
    }
}

/// Every pane's status at one moment, as the owner sends it to its readers.
///
/// Whole rather than a patch: there are a handful of panes, replacing them is
/// atomic, and a reader that reconnects cannot have missed an update.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusSnapshot {
    /// Which run of the owner this came from. A new host is a new epoch.
    pub epoch: u64,
    /// Bumped on every change within an epoch.
    pub revision: u64,
    /// Every pane the owner knows about.
    pub panes: Vec<PaneAgentStatus>,
}

impl AgentStatusSnapshot {
    /// Whether this should replace `current`: anything from another epoch
    /// does, and within one only a later revision.
    ///
    /// Another epoch is not compared by age: it is a host's start time, and
    /// a clock set back would lock a window out of the new host. A replaced
    /// connection's late snapshot is stopped before it gets here instead —
    /// see `host::client`.
    pub fn supersedes(&self, current: Option<&Self>) -> bool {
        current
            .is_none_or(|current| self.epoch != current.epoch || self.revision > current.revision)
    }
}

/// A worktree the owner can place a working directory in.
#[derive(Debug, Clone)]
struct Registered {
    id: String,
    path: PathBuf,
    canonical: PathBuf,
}

/// The canonical store — see the module docs.
#[derive(Debug, Default)]
pub struct StatusStore {
    epoch: u64,
    revision: u64,
    panes: BTreeMap<String, PaneAgentStatus>,
    /// Which pane each agent session was last seen in, for reports that
    /// arrive without a pane this owner has.
    sessions: HashMap<String, String>,
    /// Where each worktree is on disk — see [`Self::route`].
    worktrees: Vec<Registered>,
}

impl StatusStore {
    /// An empty store for one run of its owner.
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            ..Self::default()
        }
    }

    /// Replaces the worktrees [`Self::route`] can place a directory in.
    pub fn set_worktrees(&mut self, worktrees: impl IntoIterator<Item = (String, PathBuf)>) {
        self.worktrees = worktrees
            .into_iter()
            .map(|(id, path)| Registered {
                canonical: path.canonicalize().unwrap_or_else(|_| path.clone()),
                id,
                path,
            })
            .collect();
    }

    /// Binds a resumed agent session to the pane ket is opening for it.
    ///
    /// Codex may report with the pane saved in the original session. The
    /// session id in its payload is stable across resumes, so this explicit
    /// launch-time fact wins before the first report is routed.
    pub fn bind_session(&mut self, session: &str, pane: &str) {
        if !session.is_empty() && !pane.is_empty() {
            self.sessions.insert(session.to_owned(), pane.to_owned());
        }
    }

    /// Corrects the inherited worktree on a Codex report.
    ///
    /// Codex may keep serving threads from a shared process. A thread opened
    /// in another checkout then inherits the process's original
    /// `KET_WORKTREE_ID`, even though Codex puts the thread's real `cwd` in
    /// every event payload. That per-event fact wins whenever it names a
    /// known worktree. Only the worktree moves: the pane is the process that
    /// sent the report, which is where a keystroke cancelling it goes.
    pub fn route(&self, report: &mut HookReport) {
        if !report.agent.eq_ignore_ascii_case("codex") {
            return;
        }
        let Some(cwd) = report.cwd.as_deref() else {
            return;
        };
        let resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        if let Some(found) = self
            .worktrees
            .iter()
            .find(|w| w.path == *cwd || w.canonical == resolved)
        {
            report.worktree = Some(found.id.clone());
        }
    }

    /// Takes one report from an agent's hook. `known` says whether a pane key
    /// names a terminal this owner runs.
    ///
    /// A report with no worktree is dropped: ket's hooks are installed
    /// globally, so they also fire for sessions in directories ket knows
    /// nothing about. Returns whether anything changed.
    pub fn report(&mut self, report: &HookReport, known: impl Fn(&str) -> bool) -> bool {
        let mut report = report.clone();
        self.route(&mut report);
        let Some(worktree) = report.worktree.clone() else {
            return false;
        };
        let Some(pane) = self.resolve_pane(&report, &worktree, &known) else {
            return false;
        };
        let changed = self.apply(pane.clone(), worktree, &report);
        if changed {
            if let Some(session) = report.session.as_deref().filter(|id| !id.is_empty()) {
                self.sessions.insert(session.to_owned(), pane);
            }
            self.revision += 1;
        }
        changed
    }

    /// Which pane a report is about.
    ///
    /// The pane it names, when that is one of ours; else the pane its session
    /// was last seen in, while that is still ours. A report that names a pane
    /// neither way places — one whose terminal has already closed, say — is
    /// left unfiled rather than guessed at: filed anyway, a late tool call
    /// from a closed pane would light its worktree for the whole linger.
    /// One that names no pane at all is an agent ket did not launch, placed
    /// by its working directory, and has a pane of its worktree's.
    fn resolve_pane(
        &self,
        report: &HookReport,
        worktree: &str,
        known: &impl Fn(&str) -> bool,
    ) -> Option<String> {
        let named = report.pane.as_deref().filter(|pane| !pane.is_empty());
        let session = report.session.as_deref().filter(|id| !id.is_empty());

        if let Some(pane) = named.filter(|pane| known(pane)) {
            return Some(pane.to_owned());
        }
        if let Some(bound) = session
            .and_then(|id| self.sessions.get(id))
            .filter(|pane| known(pane))
        {
            return Some(bound.clone());
        }
        match named {
            Some(pane) => {
                tracing::debug!(
                    pane,
                    agent = %report.agent,
                    event = ?report.event,
                    "hook report from a pane this owner does not run; left unfiled"
                );
                None
            }
            None => Some(format!("{UNPLACED}{worktree}")),
        }
    }

    fn apply(&mut self, pane: String, worktree: String, report: &HookReport) -> bool {
        let at_ms = report.at_ms;

        // A subagent's report is about the subagent, never the lead: its tool
        // calls used to overwrite the lead's note, and one of several parallel
        // subagents stopping set the lead back to thinking.
        if let Some(child) = report.subagent.as_deref() {
            let entry = match self.panes.entry(pane) {
                Entry::Occupied(slot) => slot.into_mut(),
                Entry::Vacant(slot) => {
                    let pane = slot.key().clone();
                    let mut lead =
                        PaneAgentStatus::new(pane, worktree.clone(), report.agent.clone(), at_ms);
                    lead.state = AgentState::ExecutingTool;
                    slot.insert(lead)
                }
            };
            // A subagent only runs under a lead that is working. One ket has
            // not heard from — it was restarted mid-run — is taken to be.
            if !entry.stands_at(at_ms) {
                entry.state = AgentState::ExecutingTool;
                entry.note = None;
                entry.mutating_git = false;
                entry.at_ms = at_ms;
                entry.last_tool = None;
                entry.cancel = None;
            }
            entry.worktree = worktree;
            entry.roster.child(child, report);
            return true;
        }

        let state = report.event.state();
        let entry = match self.panes.entry(pane) {
            Entry::Occupied(slot) => slot.into_mut(),
            // Nothing to say about a pane ket has never heard from.
            Entry::Vacant(_) if state.is_none() => return false,
            Entry::Vacant(slot) => {
                let pane = slot.key().clone();
                slot.insert(PaneAgentStatus::new(
                    pane,
                    worktree.clone(),
                    report.agent.clone(),
                    at_ms,
                ))
            }
        };

        // The prompt a waiting subagent raised, announced again at the lead.
        // It is the subagent's, and its row already says so.
        if report.event == HookEvent::Notification && entry.roster.blocked() {
            return false;
        }
        if superseded(entry, report) {
            tracing::debug!(
                pane = %entry.pane,
                event = ?report.event,
                "report from a session this pane has moved on from"
            );
            return false;
        }
        entry.worktree = worktree;
        entry.agent.clone_from(&report.agent);
        if report.session.is_some() {
            entry.session.clone_from(&report.session);
        }
        entry.roster.lead(report);

        let Some(state) = state else {
            // A teammate going idle moves the roster and nothing else.
            return true;
        };

        if report.event == HookEvent::Interrupt {
            tracing::debug!(pane = %entry.pane, agent = %entry.agent, "turn cancelled: provider");
            entry.cancel_now(CancelSource::Provider, at_ms);
            return true;
        }

        if entry.cancel.is_some() {
            if restates_turn(report.event) {
                tracing::debug!(
                    pane = %entry.pane,
                    event = ?report.event,
                    "held against a cancelled turn"
                );
                return false;
            }
            entry.cancel = None;
        }

        // Another call's news while the lead waits on a person. Claude runs
        // calls that need no permission beside the one that does, and one of
        // those starting or finishing says nothing about the prompt still on
        // screen. Taken as the answer, it dropped the question: the next
        // `Notification` put the pane back to waiting with nothing to ask,
        // and a phone had no prompt to answer.
        if entry.state == AgentState::AwaitingPermission
            && matches!(
                report.event,
                HookEvent::PreTool | HookEvent::PostTool | HookEvent::PostToolFailure
            )
            && let (Some(asked), Some(call)) = (entry.question.as_ref(), report.call.as_ref())
            && !answers(asked, call)
        {
            tracing::debug!(
                pane = %entry.pane,
                event = ?report.event,
                "another call's report while a prompt is up"
            );
            return true;
        }

        // Narrowed to exactly a tool call coming back — see
        // `PaneAgentStatus::shown`. Matched on the event, not the state it
        // leads to: a failure, a refusal or a new prompt also reads
        // "thinking", and a failure on the heels of a tool call is the one
        // thing a row exists to surface fast, so those replace it at once.
        entry.last_tool = match state {
            AgentState::ExecutingTool => Some(ToolCall {
                note: report.note.clone(),
                mutating_git: report.mutating_git,
                started_at_ms: at_ms,
                ended_at_ms: None,
            }),
            AgentState::Thinking
                if report.event == HookEvent::PostTool
                    && entry.state == AgentState::ExecutingTool =>
            {
                entry.last_tool.take().map(|mut tool| {
                    tool.ended_at_ms.get_or_insert(at_ms);
                    tool
                })
            }
            _ => None,
        };
        // A `Notification` restating the wait says no more than that it
        // waits, and must not wipe what the `PermissionRequest` before it
        // said it was waiting on.
        entry.question = match state {
            AgentState::AwaitingPermission => report
                .question
                .as_deref()
                .cloned()
                .map(|mut question| {
                    question.id = next_question_id();
                    question
                })
                .or_else(|| {
                    (entry.state == AgentState::AwaitingPermission)
                        .then(|| entry.question.take())
                        .flatten()
                }),
            _ => None,
        };
        entry.state = state;
        entry.note.clone_from(&report.note);
        entry.mutating_git = report.mutating_git;
        entry.at_ms = at_ms;
        true
    }

    /// A cancel verdict from outside the agent: `Ctrl-C` or a phone.
    ///
    /// Only for a lead that is working. An interrupt typed at an idle prompt
    /// clears a half-written line, which is not a status's business, and one
    /// aimed at a build in another pane never reaches this one. Returns
    /// whether it applied.
    pub fn cancel(&mut self, pane: &str, source: CancelSource, now_ms: u64) -> bool {
        let Some(entry) = self.panes.get_mut(pane).filter(|e| working(e.state)) else {
            return false;
        };
        tracing::debug!(pane, agent = %entry.agent, ?source, "turn cancelled: inferred");
        entry.cancel_now(source, now_ms);
        self.revision += 1;
        true
    }

    /// Escape pressed at a working lead: a guess that the turn was cancelled.
    ///
    /// Not a verdict and not latched. It is Claude Code's own interrupt key,
    /// and Claude sends no hook when a turn is cancelled, so without it a
    /// cancelled Claude turn reads "thinking" until its evidence expires. But
    /// Escape also dismisses menus and clears input, so any real report that
    /// follows replaces this outright: a wrong guess costs one delivery.
    pub fn escape(&mut self, pane: &str, now_ms: u64) -> bool {
        let Some(entry) = self.panes.get_mut(pane).filter(|e| working(e.state)) else {
            return false;
        };
        tracing::debug!(pane, agent = %entry.agent, "turn stopped: escape");
        entry.stop(now_ms);
        self.revision += 1;
        true
    }

    /// Forgets a pane whose terminal has closed or exited.
    pub fn clear(&mut self, pane: &str) -> bool {
        if self.panes.remove(pane).is_none() {
            return false;
        }
        self.sessions.retain(|_, bound| bound != pane);
        self.revision += 1;
        true
    }

    /// Every pane, in key order.
    pub fn entries(&self) -> impl Iterator<Item = &PaneAgentStatus> {
        self.panes.values()
    }

    /// One pane.
    pub fn get(&self, pane: &str) -> Option<&PaneAgentStatus> {
        self.panes.get(pane)
    }

    /// Everything, for a reader.
    pub fn snapshot(&self) -> AgentStatusSnapshot {
        AgentStatusSnapshot {
            epoch: self.epoch,
            revision: self.revision,
            panes: self.panes.values().cloned().collect(),
        }
    }
}

/// Whether `call`'s report is about the call `asked` is waiting on.
///
/// A question or a plan is matched on its tool alone. Its input on the way
/// back is not the input it asked with — the answers are written into an
/// `AskUserQuestion`'s, and a plan can be edited before it is approved — so a
/// subject compared there missed the answer, held every call after it as
/// "another call's", and left a working pane reading "waiting" until its turn
/// ended. A pane shows one prompt at a time, so the tool is enough. A
/// permission keeps its subject: the calls running beside one are often the
/// same tool.
fn answers(asked: &PermissionQuestion, call: &PermissionQuestion) -> bool {
    asked.tool == call.tool
        && (PROMPT_TOOLS.contains(&asked.tool.as_str()) || asked.subject == call.subject)
}

/// Whether a lead is mid-turn, so that stopping it means something.
fn working(state: AgentState) -> bool {
    matches!(state, AgentState::Thinking | AgentState::ExecutingTool)
}

/// Whether a live process is enough to keep saying this about an agent — see
/// [`crate::activity::Tracker`]'s linger rule.
///
/// A settled state is one the agent will not leave without saying so. An
/// unsettled one is mid-turn, and a turn can end without a word.
pub(crate) fn settled(state: AgentState) -> bool {
    match state {
        AgentState::Idle | AgentState::AwaitingPermission | AgentState::Authenticating => true,
        AgentState::Starting | AgentState::Thinking | AgentState::ExecutingTool => false,
    }
}

/// Whether a lead's report comes from a session older than the one its pane
/// has moved on to — a `/clear`, a resume, a new agent — and arrived late.
///
/// Only a new session's `SessionStart` moves an occupied pane to it. An
/// initial prompt still establishes an empty pane, but every agent ket
/// installs hooks for announces its session start; accepting a prompt as a
/// later handoff would let a delayed prompt from the old session take the
/// pane back. Anything else from another session is the old one still
/// talking, and would overwrite the new one.
fn superseded(entry: &PaneAgentStatus, report: &HookReport) -> bool {
    let held = entry.session.as_deref().filter(|id| !id.is_empty());
    let from = report.session.as_deref().filter(|id| !id.is_empty());
    held.zip(from).is_some_and(|(held, from)| held != from)
        && report.event != HookEvent::SessionStart
}

/// Whether an event is the cancelled turn still talking — ignored while a
/// verdict is held.
///
/// Everything else with a state releases the verdict and applies: a new
/// prompt, a new session, or the agent's own word that the turn is over or
/// waiting on a person.
fn restates_turn(event: HookEvent) -> bool {
    matches!(
        event,
        HookEvent::PreTool
            | HookEvent::PostTool
            | HookEvent::PostToolFailure
            | HookEvent::PermissionDenied
            | HookEvent::Notification
            | HookEvent::SubagentStart
            | HookEvent::SubagentStop
    )
}
