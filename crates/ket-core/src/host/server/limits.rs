//! Keeping the ledger of sessions a plan limit has stopped — see
//! [`crate::limits`] for what a limit is and what the ledger holds.
//!
//! Three things happen here, on a thread of their own so that nothing reads
//! a file while a hook report is being applied:
//!
//! - **Every turn that ends is judged.** The status store notes each end as
//!   its report is applied ([`crate::agent_status::TurnEnd`]); this asks
//!   whether it ended on a limit, from the account its hook carried or, for
//!   an agent whose hook carries none, from the end of the session's own
//!   file. A limit marks the session; any other end takes its mark off.
//! - **A Codex turn nobody reported the end of is ended.** Codex has no
//!   event for a turn that fails, so a pane whose plan ran out mid-turn went
//!   on reading "working" for good. A Codex pane that is at work and has
//!   gone quiet has its rollout looked at; if the rollout says the turn is
//!   over, that is reported here as the `StopFailure` Codex never sent, and
//!   takes the same road as any other.
//! - **Each pane is told what its session is stopped at**, so a window and a
//!   phone both read it off the status they already have.
//!
//! The host asks no provider anything for this. When a limit resets is read
//! from what has already arrived: Claude's status line, the quota Codex
//! writes into its rollout, and the usage a window last published.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use super::{Host, lock, pass_on_hooks};
use crate::agent_hooks::{HookEvent, HookReport, StatusLine};
use crate::agent_status::{PaneAgentStatus, TurnEnd};
use crate::conversation::{Ended, Transcript};
use crate::event::AgentState;
use crate::limits::{Hit, Ledger, Limit, Mark, classify, resets_at};
use crate::rate_limits::{Provider, RateWindow};

/// How often the ledger is tended.
const TICK: Duration = Duration::from_secs(1);

/// Every how many ticks quiet Codex panes are looked at, and reset times
/// still unknown are looked for again.
const SLOW_EVERY: u32 = 3;

/// How long a Codex pane at work has to have said nothing before its rollout
/// is read: a turn in flight reports a tool every few seconds.
const SILENT_MS: u64 = 5_000;

/// How long ago the rollout has to have recorded the turn's end. Codex's own
/// `Stop`, if it sends one for this turn, arrives well inside this.
const SETTLED_MS: u64 = 3_000;

/// How far before a `Stop` Codex can have recorded the error it stopped on
/// and still be about that turn.
const SAME_TURN_MS: u64 = 120_000;

/// How old the usage a window published may be and still say when a window
/// resets. A window refreshes it every five minutes.
const READING_FRESH_MS: u64 = 15 * 60 * 1000;

/// The ledger and what feeds it — a field of [`Host`].
#[derive(Debug, Default)]
pub(super) struct Limits {
    ledger: Mutex<Ledger>,
    /// Where the ledger is kept between runs. `None` in tests, which keep
    /// nothing.
    file: Option<PathBuf>,
    /// Turns that have ended and not been judged yet.
    ended: Mutex<Vec<TurnEnd>>,
    /// Claude's plan windows, as each pane's status line last gave them.
    windows: Mutex<HashMap<String, Vec<RateWindow>>>,
}

impl Limits {
    /// The ledger as the last run left it.
    pub(super) fn load() -> Self {
        let file = super::super::dir().ok().map(|dir| dir.join("limits.json"));
        let ledger = file
            .as_deref()
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            ledger: Mutex::new(ledger),
            file,
            ..Self::default()
        }
    }

    /// Takes in what the hook listener just heard: the turns that ended, to
    /// be judged on the next tick, and the status lines, for the plan
    /// windows Claude puts in them.
    pub(super) fn heard(&self, ended: Vec<TurnEnd>, lines: &[StatusLine]) {
        if !ended.is_empty() {
            lock(&self.ended).extend(ended);
        }
        for line in lines {
            let Some(pane) = line.pane.as_deref() else {
                continue;
            };
            if let Some(read) = crate::rate_limits::snapshot_from_statusline(&line.payload) {
                lock(&self.windows).insert(pane.to_owned(), read.windows);
            }
        }
    }

    fn save(&self) {
        let Some(file) = self.file.as_deref() else {
            return;
        };
        let written = serde_json::to_vec_pretty(&*lock(&self.ledger))
            .map_err(super::host_error)
            .and_then(|bytes| super::super::identity::write_private(file, &bytes));
        if let Err(error) = written {
            tracing::warn!(%error, "ket host: the limits ledger was not saved");
        }
    }
}

impl Host {
    /// The limit the session last seen in the pane keyed `key` is stopped
    /// at: for a tab that is asleep, which has a key and no status.
    pub(in crate::host) fn limit_in(&self, key: &str) -> Option<Limit> {
        lock(&self.limits.ledger).in_pane(key).cloned()
    }
}

/// What the worker reads from an agent's own files. A trait so its tests can
/// say what a session's file holds without there being one.
pub(super) trait Sessions {
    /// See [`Transcript::ended_on`].
    fn ended_on(&self, agent: &str, session: &str) -> Option<Ended>;
    /// See [`Transcript::quota`].
    fn quota(&self, agent: &str, session: &str) -> Vec<RateWindow>;
    /// See [`Transcript::written`].
    fn written(&self, agent: &str, session: &str) -> Option<u64>;
    /// Which of the agent's accounts the session belongs to — see
    /// [`Mark::account`].
    fn account(&self, agent: &str, session: &str) -> String;
    /// Which of them the usage a window publishes is read for, spelled as
    /// [`Self::account`] spells it.
    fn usage_account(&self, agent: &str) -> String;
}

/// The agents' real files.
struct Files;

impl Sessions for Files {
    fn ended_on(&self, agent: &str, session: &str) -> Option<Ended> {
        Transcript::locate(agent, session)?.ended_on()
    }

    fn quota(&self, agent: &str, session: &str) -> Vec<RateWindow> {
        Transcript::locate(agent, session).map_or_else(Vec::new, |found| found.quota())
    }

    fn written(&self, agent: &str, session: &str) -> Option<u64> {
        Transcript::locate(agent, session)?.written()
    }

    fn account(&self, agent: &str, session: &str) -> String {
        // `<config>/projects/<project>/<session>.jsonl`: the config
        // directory is the login.
        match Transcript::locate(agent, session) {
            Some(Transcript::Claude(path)) => path
                .ancestors()
                .nth(3)
                .map(|dir| dir.display().to_string())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn usage_account(&self, agent: &str) -> String {
        // A window reads Claude's usage for the default login (see
        // `rate_limits::claude`), and a project's own `claude` may use
        // another.
        if harness(agent) == "claude" {
            crate::sessions::claude_config_dir().display().to_string()
        } else {
            String::new()
        }
    }
}

/// What one run of the worker remembers between ticks.
#[derive(Debug, Default)]
pub(super) struct Watch {
    ticks: u32,
    /// How long each quiet Codex session's rollout was when it was last
    /// read and found to say nothing: unchanged, it still says nothing.
    looked: HashMap<String, u64>,
}

/// Tends the ledger until the host exits.
pub(super) fn run(host: &Host) {
    let mut watch = Watch::default();
    loop {
        std::thread::sleep(TICK);
        pass(host, &mut watch, &Files, crate::now_ms());
    }
}

/// One tick — see the module docs.
pub(super) fn pass(host: &Host, watch: &mut Watch, sessions: &dyn Sessions, now_ms: u64) {
    watch.ticks = watch.ticks.wrapping_add(1);
    let slow = watch.ticks.is_multiple_of(SLOW_EVERY);
    if slow {
        unstick(host, watch, sessions, now_ms);
    }
    let ended = std::mem::take(&mut *lock(&host.limits.ended));
    let mut changed = false;
    for end in &ended {
        changed |= judge(host, sessions, end, now_ms);
    }
    changed |= tend(host, sessions, slow, now_ms);
    if changed {
        host.limits.save();
    }
}

/// Marks the session of a turn that ended on a limit, and takes the mark off
/// one that ended any other way. Returns whether the ledger changed.
fn judge(host: &Host, sessions: &dyn Sessions, end: &TurnEnd, now_ms: u64) -> bool {
    // An interrupted turn was stopped by a person, and says nothing about
    // whether the next one will be refused.
    let (Some(session), false) = (end.session.as_deref(), end.event == HookEvent::Interrupt) else {
        return false;
    };
    let codex = harness(&end.agent) == "codex";
    let failure = match (&end.failure, end.event) {
        (Some(failure), _) => Some(failure.clone()),
        // Grok's hook says only that the turn failed.
        (None, HookEvent::StopFailure) => sessions
            .ended_on(&end.agent, session)
            .map(|ended| ended.failure),
        // Codex has no failure event: if it sends anything for a turn that
        // ran out of plan, it is a plain `Stop`.
        (None, HookEvent::Stop) if codex => sessions
            .ended_on(&end.agent, session)
            .filter(|ended| {
                ended
                    .at_ms
                    .is_none_or(|at| at.saturating_add(SAME_TURN_MS) >= end.at_ms)
            })
            .map(|ended| ended.failure),
        _ => None,
    };
    let Some((failure, hit)) =
        failure.and_then(|failure| classify(&end.agent, &failure).map(|hit| (failure, hit)))
    else {
        return lock(&host.limits.ledger).clear(&end.agent, session);
    };
    let mark = Mark {
        agent: end.agent.clone(),
        session: session.to_owned(),
        account: sessions.account(&end.agent, session),
        pane: end.pane.clone(),
        limit: Limit {
            scope: hit.scope,
            window: hit.window,
            since_ms: end.at_ms,
            resets_at_ms: reset(host, sessions, &end.agent, session, &end.pane, hit, now_ms),
            said: failure.message,
        },
    };
    tracing::info!(
        pane = %end.pane,
        agent = %end.agent,
        scope = ?hit.scope,
        resets_at_ms = ?mark.limit.resets_at_ms,
        "a turn ended on a limit"
    );
    lock(&host.limits.ledger).hit(mark)
}

/// When the window `hit` ran out of resets, from whatever has said: the
/// agent's own report of its plan first, then the usage a window published,
/// while that is recent and of the session's own account.
fn reset(
    host: &Host,
    sessions: &dyn Sessions,
    agent: &str,
    session: &str,
    pane: &str,
    hit: Hit,
    now_ms: u64,
) -> Option<u64> {
    let own = match harness(agent).as_str() {
        "claude" => lock(&host.limits.windows)
            .get(pane)
            .cloned()
            .unwrap_or_default(),
        "codex" => sessions.quota(agent, session),
        _ => Vec::new(),
    };
    resets_at(hit, &own, now_ms).or_else(|| {
        // Another login's reading would be a wrong time, said as a fact.
        if sessions.account(agent, session) != sessions.usage_account(agent) {
            return None;
        }
        let provider = provider(agent)?;
        let published = host
            .usage()
            .into_iter()
            .find(|read| read.provider == provider)?;
        let fresh = published
            .fetched_at_ms
            .is_some_and(|at| now_ms.saturating_sub(at) <= READING_FRESH_MS);
        fresh
            .then(|| resets_at(hit, &published.windows, now_ms))
            .flatten()
    })
}

/// Ends the turn of any Codex pane whose rollout says it is over while the
/// pane still reads as working — see the module docs.
fn unstick(host: &Host, watch: &mut Watch, sessions: &dyn Sessions, now_ms: u64) {
    let panes = host.agent_status().panes;
    let quiet: Vec<&PaneAgentStatus> = panes
        .iter()
        .filter(|pane| {
            harness(&pane.agent) == "codex"
                && matches!(pane.state, AgentState::Thinking | AgentState::ExecutingTool)
                && now_ms.saturating_sub(pane.heard_at_ms()) >= SILENT_MS
        })
        .collect();
    watch.looked.retain(|session, _| {
        quiet
            .iter()
            .any(|pane| pane.session.as_deref() == Some(session))
    });
    for pane in quiet {
        let Some(session) = pane.session.as_deref() else {
            continue;
        };
        let written = sessions.written(&pane.agent, session);
        if written.is_some() && watch.looked.get(session) == written.as_ref() {
            continue;
        }
        let mut nothing = || {
            if let Some(written) = written {
                watch.looked.insert(session.to_owned(), written);
            }
        };
        // Over, by the turn's own record, and not the turn before this one.
        let Some((ended, at)) = sessions
            .ended_on(&pane.agent, session)
            .filter(|ended| ended.closed)
            .and_then(|ended| ended.at_ms.map(|at| (ended, at)))
            .filter(|&(_, at)| at >= pane.recency_at_ms)
        else {
            nothing();
            continue;
        };
        if now_ms.saturating_sub(at) < SETTLED_MS {
            // Too new to be sure Codex will not report it itself: look again.
            continue;
        }
        tracing::info!(pane = %pane.pane, "a Codex turn ended on an error it never reported");
        let report = HookReport {
            agent: pane.agent.clone(),
            event: HookEvent::StopFailure,
            worktree: Some(pane.worktree.clone()),
            cwd: None,
            pane: Some(pane.pane.clone()),
            session: Some(session.to_owned()),
            tool_name: None,
            note: None,
            mutating_git: false,
            command: None,
            at_ms: now_ms,
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
            failure: Some(Box::new(ended.failure)),
        };
        let known = host.pane_keys();
        let (changed, ended) = {
            let mut status = lock(&host.status);
            // The pane as it was read, still: a prompt or a tool heard while
            // the rollout was read is a turn going on, not the one that ended.
            let still = status.get(&pane.pane).is_some_and(|now| {
                now.session == pane.session
                    && now.state == pane.state
                    && now.recency_at_ms == pane.recency_at_ms
                    && now.heard_at_ms() == pane.heard_at_ms()
            });
            if !still {
                continue;
            }
            let changed = status.report(&report, |pane| known.contains(pane));
            (changed, status.take_ended())
        };
        if changed {
            host.broadcast_status();
            // A window keeps its own count of who is working, from reports.
            pass_on_hooks(host, vec![report], Vec::new());
        }
        host.limits.heard(ended, &[]);
    }
}

/// Keeps the ledger true to the panes, and the panes to the ledger: a
/// session that is running tools again loses its mark, a reset time that has
/// since been said is filled in, old marks go, and every pane's status says
/// what its session is stopped at. Returns whether the ledger changed.
fn tend(host: &Host, sessions: &dyn Sessions, slow: bool, now_ms: u64) -> bool {
    let panes = host.agent_status().panes;
    let mut changed = false;

    // What the panes say about the marked sessions.
    let mut going = Vec::new();
    let mut undated = Vec::new();
    {
        let ledger = lock(&host.limits.ledger);
        for pane in &panes {
            let Some(session) = pane.session.as_deref() else {
                continue;
            };
            let Some(limit) = ledger.of(&pane.agent, session) else {
                continue;
            };
            if pane.state == AgentState::ExecutingTool && pane.at_ms > limit.since_ms {
                going.push((pane.agent.clone(), session.to_owned()));
            } else if slow && limit.resets_at_ms.is_none() {
                let hit = Hit {
                    scope: limit.scope,
                    window: limit.window,
                };
                undated.push((pane, session.to_owned(), hit));
            }
        }
    }
    // Read outside the ledger's lock: this opens files.
    let dated: Vec<_> = undated
        .into_iter()
        .filter_map(|(pane, session, hit)| {
            reset(
                host,
                sessions,
                &pane.agent,
                &session,
                &pane.pane,
                hit,
                now_ms,
            )
            .map(|at| (pane.agent.clone(), session, at))
        })
        .collect();

    let wanted: Vec<(String, Option<Limit>)> = {
        let mut ledger = lock(&host.limits.ledger);
        for (agent, session) in &going {
            changed |= ledger.clear(agent, session);
        }
        for (agent, session, at) in &dated {
            changed |= ledger.learn_reset(agent, session, *at);
        }
        changed |= ledger.prune(now_ms);
        for pane in &panes {
            if let Some(session) = pane.session.as_deref() {
                changed |= ledger.seen_in(&pane.agent, session, &pane.pane);
            }
        }
        panes
            .iter()
            .filter_map(|pane| {
                let limit = pane
                    .session
                    .as_deref()
                    .and_then(|session| ledger.of(&pane.agent, session))
                    .cloned();
                (limit != pane.limit).then(|| (pane.pane.clone(), limit))
            })
            .collect()
    };
    if !wanted.is_empty() {
        let moved = {
            let mut status = lock(&host.status);
            wanted.into_iter().fold(false, |moved, (pane, limit)| {
                status.set_limit(&pane, limit) | moved
            })
        };
        if moved {
            host.broadcast_status();
        }
    }
    // A closed pane's status line says nothing about the next one's plan.
    lock(&host.limits.windows).retain(|pane, _| panes.iter().any(|open| &open.pane == pane));
    changed
}

/// The harness of `agent`, lower-cased.
fn harness(agent: &str) -> String {
    crate::agents::harness(agent).to_ascii_lowercase()
}

/// The provider whose plan `agent` spends.
fn provider(agent: &str) -> Option<Provider> {
    match harness(agent).as_str() {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        "opencode" => Some(Provider::OpenCode),
        "grok" => Some(Provider::Grok),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
