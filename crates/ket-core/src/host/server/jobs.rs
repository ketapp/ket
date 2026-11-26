//! Keeping each agent pane's status true to what its agent left running — see
//! [`crate::jobs`] for how a job is found, and [`crate::jobs::claude`] for
//! what Claude's transcript adds.
//!
//! **In the background means outliving the turn.** In the process tree a
//! command the agent is waiting on looks exactly like one it left running;
//! only the agent's turn tells them apart. So a command counts as a job when
//! its agent is not at work, when it started before the turn now going on, or
//! when Claude's transcript says Claude ran it in the background — and, once
//! it has counted, it counts until it ends.
//!
//! **An ended job is shown for [`LINGER_MS`]**, with how it ended where the
//! agent said. Claude does, the moment the command ends; for anything else it
//! is only gone. A command Claude ran in the background and that ended before
//! a tick could see it is shown ended from its transcript alone.
//!
//! **Stop signals only what the status lists.** A window or a phone names a
//! job by its pid and the time it started, and the host ends it only if the
//! pane's status lists it as running and the process with that pid is still
//! the one that started then — see [`crate::jobs::stop`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use super::{Host, lock};
use crate::event::AgentState;
use crate::jobs::claude::{Tail, Told};
use crate::jobs::{self, Candidate, Ended, Job, Names, Outcome};
use crate::terminal::LocalTerminal;

/// How often the panes are looked at.
const TICK: Duration = Duration::from_secs(2);

/// Every how many ticks the ports of young jobs are read again; a job that
/// has just been found has its read at once. A server binds its port within
/// moments of starting, so after [`PORTS_YOUNG_MS`] a job's ports are read
/// only every [`PORTS_LATE_EVERY`] ticks — `lsof` is the dearest thing a
/// tick does, and a dev server can be left running all day.
const PORTS_EVERY: u32 = 5;

/// How long a job is young enough to have its ports read every
/// [`PORTS_EVERY`] ticks.
const PORTS_YOUNG_MS: u64 = 3 * 60 * 1000;

/// Every how many ticks every running job's ports are read: five minutes.
const PORTS_LATE_EVERY: u32 = 150;

/// Every how many ticks a Claude session whose transcript was not found is
/// looked for again: it is written once the session has said something.
const LOOK_EVERY: u32 = 5;

/// Which look at a missing transcript may search every project folder
/// Claude has, once, for a session filed away from its agent's directory.
/// Not the first: a new session has not written its file yet.
const SEARCH_ON_LOOK: u32 = 2;

/// How long an ended job is still shown.
pub(crate) const LINGER_MS: u64 = 3 * 60 * 1000;

/// How long after Claude asked for a command its process can start: a
/// command can wait on a person to allow it.
const ALLOWED_WITHIN_MS: u64 = 30 * 60 * 1000;

/// How long before Claude's record of asking its process can seem to start:
/// the two clocks are read apart.
const SKEW_MS: u64 = 5_000;

/// What a window or a phone stopped — a field of [`Host`] — so that the job,
/// once gone, reads as stopped rather than only gone.
#[derive(Debug, Default)]
pub(super) struct Jobs {
    stopped: Mutex<HashSet<(i32, u64)>>,
}

impl Host {
    /// Ends a job pane `pane` lists as running, at a window's or a phone's
    /// word. Returns whether it was signalled.
    pub(in crate::host) fn stop_job(&self, pane: &str, pid: i32, started_at_ms: u64) -> bool {
        let listed = lock(&self.status).get(pane).is_some_and(|status| {
            status.jobs.iter().any(|job| {
                job.pid == pid && job.started_at_ms == started_at_ms && job.ended.is_none()
            })
        });
        if !listed || pid <= 0 || !jobs::stop(pid, started_at_ms) {
            return false;
        }
        lock(&self.jobs.stopped).insert((pid, started_at_ms));
        tracing::info!(pane, pid, "a background job was stopped");
        true
    }

    /// [`Self::stop_job`] for a job a phone names by its terminal's id.
    pub(in crate::host) fn stop_job_in(&self, terminal: u64, pid: i32, started_at_ms: u64) -> bool {
        self.key(terminal)
            .is_some_and(|key| self.stop_job(&key, pid, started_at_ms))
    }

    /// The process in front in each of `panes`, by pane key: what its
    /// agent's commands are found under. A terminal at its shell's prompt has
    /// none, and no other terminal is asked.
    fn fronts(&self, panes: &[Facts]) -> HashMap<String, i32> {
        let terminals: Vec<(String, Arc<LocalTerminal>)> = lock(&self.terminals)
            .values()
            .filter(|entry| !entry.killed && !entry.terminal.is_closed())
            .filter_map(|entry| {
                let key = entry.key.as_ref()?;
                panes
                    .iter()
                    .any(|pane| &pane.pane == key)
                    .then(|| (key.clone(), entry.terminal.clone()))
            })
            .collect();
        terminals
            .into_iter()
            .filter_map(|(key, terminal)| {
                let front = terminal.foreground_group().filter(|&front| front > 0)?;
                let shell = terminal.pid().and_then(|pid| i32::try_from(pid).ok());
                (Some(front) != shell).then_some((key, front))
            })
            .collect()
    }
}

/// One job as the worker keeps it: what is shown, and which of Claude's
/// commands it is.
#[derive(Debug, Clone)]
struct Tracked {
    job: Job,
    told: Option<String>,
}

/// What a tick reads of a pane's status: these and nothing else, so it
/// copies five fields rather than the whole status every two seconds.
struct Facts {
    pane: String,
    agent: String,
    session: Option<String>,
    state: AgentState,
    recency_at_ms: u64,
}

/// What the worker keeps about one pane between ticks.
#[derive(Default)]
struct Pane {
    /// The Claude session `tail` reads the transcript of.
    session: Option<String>,
    tail: Option<Tail>,
    /// How many times this session's transcript has been looked for and not
    /// found.
    looks: u32,
    /// Jobs running, then jobs ended, each oldest first.
    jobs: Vec<Tracked>,
    /// Commands seen at work in a turn, by process, with what they run once
    /// that has been read: not jobs unless they outlive the turn or Claude
    /// says it ran them in the background, and not read again.
    passed: HashMap<Candidate, Option<String>>,
}

/// What one run of the worker remembers between ticks.
#[derive(Default)]
struct Watch {
    ticks: u32,
    panes: HashMap<String, Pane>,
    names: Names,
}

/// Tends the panes' jobs until the host exits.
pub(super) fn run(host: &Host) {
    let mut watch = Watch::default();
    loop {
        std::thread::sleep(TICK);
        pass(host, &mut watch, crate::now_ms());
    }
}

/// One tick — see the module docs.
fn pass(host: &Host, watch: &mut Watch, now_ms: u64) {
    watch.ticks = watch.ticks.wrapping_add(1);
    let panes: Vec<Facts> = lock(&host.status)
        .entries()
        .map(|status| Facts {
            pane: status.pane.clone(),
            agent: status.agent.clone(),
            session: status.session.clone(),
            state: status.state,
            recency_at_ms: status.recency_at_ms,
        })
        .collect();
    let fronts = host.fronts(&panes);
    watch
        .panes
        .retain(|key, _| panes.iter().any(|pane| &pane.pane == key));

    // What each pane has running that has not been placed yet, and whether
    // it has outlived a turn — the alternative being that Claude may have run
    // it in the background, which its command has to bear out.
    let mut new: Vec<(String, Candidate, bool)> = Vec::new();
    let mut live: HashMap<String, Vec<Candidate>> = HashMap::new();
    for status in &panes {
        let pane = watch.panes.entry(status.pane.clone()).or_default();
        let front = fronts.get(&status.pane).copied();
        follow(pane, status, front, watch.ticks);
        let found = front
            .map(|front| jobs::candidates(front, &mut watch.names))
            .unwrap_or_default();
        for candidate in &found {
            let placed = pane.jobs.iter().any(|tracked| {
                tracked.job.ended.is_none()
                    && tracked.job.pid == candidate.pid
                    && tracked.job.started_at_ms == candidate.started_at_ms
            });
            if placed {
                continue;
            }
            let outlived = !at_work(status) || candidate.started_at_ms < status.recency_at_ms;
            if outlived || asked_for(pane, candidate) {
                new.push((status.pane.clone(), *candidate, outlived));
            } else {
                pane.passed.entry(*candidate).or_default();
            }
        }
        live.insert(status.pane.clone(), found);
    }
    watch.names.settle();

    // One `ps` for everything new whose command is not known yet.
    let pids: Vec<i32> = new
        .iter()
        .filter(|(key, candidate, _)| {
            watch
                .panes
                .get(key)
                .and_then(|pane| pane.passed.get(candidate))
                .is_none_or(Option::is_none)
        })
        .map(|(_, candidate, _)| candidate.pid)
        .collect();
    let commands = jobs::commands(&pids);
    let mut found_new = false;
    for (key, candidate, outlived) in new {
        let Some(pane) = watch.panes.get_mut(&key) else {
            continue;
        };
        let known = pane.passed.get(&candidate).cloned().flatten();
        let Some(command) = known.or_else(|| commands.get(&candidate.pid).cloned()) else {
            continue;
        };
        let told = pane
            .tail
            .as_ref()
            .and_then(|tail| told_for(tail.told(), &pane.jobs, &command, candidate));
        // At work and not Claude's background command: one it is waiting on.
        if !outlived && told.is_none() {
            pane.passed.insert(candidate, Some(command));
            continue;
        }
        pane.passed.remove(&candidate);
        pane.jobs.push(Tracked {
            job: Job {
                pid: candidate.pid,
                started_at_ms: candidate.started_at_ms,
                command,
                description: told.and_then(|told| told.description.clone()),
                ports: Vec::new(),
                ended: None,
            },
            told: told.map(|told| told.id.clone()),
        });
        found_new = true;
    }

    // What has ended since, and what Claude has said about how.
    let stopped = std::mem::take(&mut *lock(&host.jobs.stopped));
    for (key, pane) in &mut watch.panes {
        let running = live.get(key).map(Vec::as_slice).unwrap_or_default();
        end(pane, running, &stopped, now_ms);
        pane.passed
            .retain(|candidate, _| running.contains(candidate));
    }
    // A stop the worker has not seen the end of yet is kept for the next tick.
    {
        let still: HashSet<(i32, u64)> = watch
            .panes
            .values()
            .flat_map(|pane| &pane.jobs)
            .filter(|tracked| tracked.job.ended.is_none())
            .map(|tracked| (tracked.job.pid, tracked.job.started_at_ms))
            .filter(|job| stopped.contains(job))
            .collect();
        lock(&host.jobs.stopped).extend(still);
    }

    if watch.ticks.is_multiple_of(PORTS_LATE_EVERY) {
        serving(watch, |_| true);
    } else if found_new || watch.ticks.is_multiple_of(PORTS_EVERY) {
        serving(watch, |job| {
            now_ms.saturating_sub(job.started_at_ms) < PORTS_YOUNG_MS
        });
    }

    // Compared where they are kept, and copied only where they differ:
    // most ticks, nothing does.
    let moved = {
        let mut status = lock(&host.status);
        let mut moved = false;
        for (key, pane) in &watch.panes {
            let same = status.get(key).is_none_or(|now| {
                now.jobs
                    .iter()
                    .eq(pane.jobs.iter().map(|tracked| &tracked.job))
            });
            if !same {
                moved |= status.set_jobs(key, shown(pane));
            }
        }
        moved
    };
    if moved {
        host.broadcast_status();
    }
}

/// Whether the pane's agent is in a turn.
fn at_work(status: &Facts) -> bool {
    matches!(
        status.state,
        AgentState::Thinking | AgentState::ExecutingTool | AgentState::AwaitingPermission
    )
}

/// Keeps a Claude pane's transcript read, and starts over when the pane's
/// session changes.
///
/// The transcript is looked for where Claude files it for the directory the
/// agent runs in (`front`'s), which is two lookups. Only once, on the
/// [`SEARCH_ON_LOOK`]th look, is every project folder searched for it — there
/// can be thousands — for a session resumed from somewhere else.
fn follow(pane: &mut Pane, status: &Facts, front: Option<i32>, ticks: u32) {
    let claude = crate::agents::harness(&status.agent).eq_ignore_ascii_case("claude");
    let session = status.session.clone().filter(|_| claude);
    let changed = session != pane.session;
    if changed {
        pane.session = session;
        pane.tail = None;
        pane.looks = 0;
    }
    let Some(session) = pane.session.as_deref() else {
        return;
    };
    if pane.tail.is_none() && (changed || ticks.is_multiple_of(LOOK_EVERY)) {
        let path = front
            .and_then(jobs::cwd_of)
            .and_then(|cwd| jobs::claude::transcript_at(&cwd, session))
            .or_else(|| {
                (pane.looks == SEARCH_ON_LOOK)
                    .then(|| crate::conversation::Transcript::locate(&status.agent, session))
                    .flatten()
                    .and_then(|found| match found {
                        crate::conversation::Transcript::Claude(path) => Some(path),
                        _ => None,
                    })
            });
        pane.looks = pane.looks.saturating_add(1);
        pane.tail = path.map(Tail::new);
    }
    if let Some(tail) = pane.tail.as_mut() {
        tail.poll();
    }
}

/// Whether Claude has a background command still running that `candidate`
/// could be: asked for before it started, and not yet said to be over.
fn asked_for(pane: &Pane, candidate: &Candidate) -> bool {
    pane.tail.as_ref().is_some_and(|tail| {
        tail.told()
            .iter()
            .any(|told| told.ended.is_none() && in_time(told, candidate))
    })
}

fn in_time(told: &Told, candidate: &Candidate) -> bool {
    told.started_at_ms <= candidate.started_at_ms + SKEW_MS
        && candidate.started_at_ms <= told.started_at_ms + ALLOWED_WITHIN_MS
}

/// The command of Claude's that `candidate` runs: the same command, asked for
/// in time, and not already another job's — the latest such, when Claude ran
/// one command twice.
fn told_for<'a>(
    told: &'a [Told],
    jobs: &[Tracked],
    command: &str,
    candidate: Candidate,
) -> Option<&'a Told> {
    told.iter()
        .filter(|told| told.command == command && in_time(told, &candidate))
        .filter(|told| {
            !jobs
                .iter()
                .any(|tracked| tracked.told.as_deref() == Some(told.id.as_str()))
        })
        .max_by_key(|told| told.started_at_ms)
}

/// Ends the pane's jobs that are no longer running, says how each ended as
/// Claude says it, adds what Claude ran and finished between two looks, and
/// lets go of what ended more than [`LINGER_MS`] ago.
fn end(pane: &mut Pane, running: &[Candidate], stopped: &HashSet<(i32, u64)>, now_ms: u64) {
    let told: &[Told] = pane.tail.as_ref().map(Tail::told).unwrap_or_default();
    let said = |id: Option<&str>| {
        let id = id?;
        let ending = told.iter().find(|told| told.id == id)?.ended?;
        Some(Ended {
            at_ms: ending.at_ms,
            outcome: ending.status.into(),
            exit: ending.exit,
        })
    };
    for tracked in &mut pane.jobs {
        let job = &mut tracked.job;
        match job.ended {
            None => {
                let still = running.iter().any(|candidate| {
                    candidate.pid == job.pid && candidate.started_at_ms == job.started_at_ms
                });
                if still {
                    continue;
                }
                job.ports.clear();
                let mut ended = said(tracked.told.as_deref()).unwrap_or(Ended {
                    at_ms: now_ms,
                    outcome: Outcome::Gone,
                    exit: None,
                });
                // Stopped from here, it reads as stopped: Claude calls a
                // command ended by a signal failed.
                if stopped.contains(&(job.pid, job.started_at_ms)) {
                    ended.outcome = Outcome::Stopped;
                }
                job.ended = Some(ended);
            }
            // Gone a tick before Claude said how.
            Some(Ended {
                outcome: Outcome::Gone,
                ..
            }) => {
                if let Some(ended) = said(tracked.told.as_deref()) {
                    job.ended = Some(ended);
                }
            }
            Some(_) => {}
        }
    }
    let mut unseen: Vec<Tracked> = told
        .iter()
        .filter(|told| {
            told.ended
                .is_some_and(|ended| now_ms.saturating_sub(ended.at_ms) < LINGER_MS)
        })
        .filter(|told| {
            !pane
                .jobs
                .iter()
                .any(|tracked| tracked.told.as_deref() == Some(told.id.as_str()))
        })
        .map(|told| Tracked {
            job: Job {
                pid: 0,
                started_at_ms: told.started_at_ms,
                command: told.command.clone(),
                description: told.description.clone(),
                ports: Vec::new(),
                ended: said(Some(&told.id)),
            },
            told: Some(told.id.clone()),
        })
        .collect();
    pane.jobs.append(&mut unseen);
    pane.jobs.retain(|tracked| {
        tracked
            .job
            .ended
            .is_none_or(|ended| now_ms.saturating_sub(ended.at_ms) < LINGER_MS)
    });
    pane.jobs.sort_by_key(|tracked| {
        (
            tracked.job.ended.is_some(),
            tracked.job.started_at_ms,
            tracked.job.pid,
        )
    });
    pane.jobs.truncate(jobs::MOST);
}

/// Reads the ports of the running jobs `which` picks, in one `lsof`. A job it
/// passes over keeps the ports it was last read with.
fn serving(watch: &mut Watch, which: impl Fn(&Job) -> bool) {
    let read = |job: &Job| job.ended.is_none() && job.pid > 0 && which(job);
    let groups: Vec<i32> = watch
        .panes
        .values()
        .flat_map(|pane| &pane.jobs)
        .filter(|tracked| read(&tracked.job))
        .map(|tracked| tracked.job.pid)
        .collect();
    if groups.is_empty() {
        return;
    }
    let ports = jobs::listening(&groups);
    for tracked in watch.panes.values_mut().flat_map(|pane| &mut pane.jobs) {
        if read(&tracked.job) {
            tracked.job.ports = ports.get(&tracked.job.pid).cloned().unwrap_or_default();
        }
    }
}

/// What the pane's status says it has: its jobs, as the person is shown them.
fn shown(pane: &Pane) -> Vec<Job> {
    pane.jobs
        .iter()
        .map(|tracked| tracked.job.clone())
        .collect()
}
