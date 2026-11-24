//! The host's clock for automations — see [`crate::automation`].
//!
//! The host keeps the time rather than a window because it outlives one: a
//! person can close every ket window and an automation still starts. Each
//! second of housekeeping asks [`Clock::tick`] whether one has come due:
//!
//! - **On time, or late within its catch-up window:** handed to a window
//!   as work, as `ket start` hands it — unless the automation's last run is
//!   still working and it skips while busy. With no window open, ket is
//!   opened in the background, and the start waits for a window; past its
//!   catch-up window it becomes missed.
//! - **Missed:** recorded, and the start dealt with.
//!
//! After the Mac wakes, nothing starts for two minutes: a Mac that wakes for
//! a moment in the night to check mail goes back to sleep before an agent
//! could do anything, and a run started then would sit frozen mid-turn.
//!
//! A run is followed until its worktree has gone quiet — no agent at work, no
//! command running — and then recorded as done.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::super::*;
use crate::activity::Foreground;
use crate::automation::{Automation, Automations, Due, Outcome, Place, Run, Runs, Task};
use crate::event::AgentState;
use crate::host::wire::AutomationStart;
use crate::id::{ProjectId, WorktreeId};
use crate::worktree::Worktree;

/// After a wake, how long nothing starts — see the module's docs.
const SETTLE: SignedDuration = SignedDuration::from_mins(2);

/// How much further the wall clock may run than the host's own between two
/// looks before the host takes it that the Mac slept. The host's clock stops
/// while the Mac sleeps; the wall clock doesn't.
const SLEPT: SignedDuration = SignedDuration::from_secs(30);

/// How often the Mac's time zone is read again, for one that travels.
const TIME_ZONE_EVERY: Duration = Duration::from_secs(10 * 60);

/// How long after asking macOS to open ket it is asked again, while no
/// window has come.
const OPEN_AGAIN: Duration = Duration::from_secs(2 * 60);

/// How often runs still going are looked at.
const FOLLOW_EVERY: Duration = Duration::from_secs(15);

/// How long a run is given to get going — its worktree made, its agent
/// started — before quiet in it means anything.
const GETTING_GOING: Duration = Duration::from_secs(10 * 60);

/// How long a run's worktree has to stay quiet to count as done, so an agent
/// between two tool calls isn't taken for finished.
const QUIET_FOR: Duration = Duration::from_secs(60);

/// How long a Run now waits for a ket window before it is given up on.
const RUN_NOW_WAIT: Duration = Duration::from_secs(5 * 60);

/// The clock's state between ticks: the automations as last read, and what
/// it has seen of the time.
pub(in crate::host) struct Clock {
    root: Option<PathBuf>,
    /// ket's state file; `None` is the real one. Tests name their own.
    state: Option<PathBuf>,
    seen: Option<SystemTime>,
    automations: Automations,
    tz: TimeZone,
    tz_read: Instant,
    last: Option<(Instant, Timestamp)>,
    settle_until: Option<Timestamp>,
    opened: Option<Instant>,
    followed: Option<Instant>,
    /// Since when each live run's worktree has been quiet: (automation, due).
    quiet: HashMap<(String, u64), Instant>,
    /// Branches handed to a window for a new worktree, by project, and when:
    /// taken while the window makes it, so two starts sent together never
    /// get the same name.
    reserved: Vec<(ProjectId, String, Instant)>,
}

/// What became of handing a start to a window.
enum Handed {
    /// A window has it, making a worktree on this branch — or opening a tab
    /// in this worktree.
    To {
        branch: Option<String>,
        worktree: Option<WorktreeId>,
    },
    /// No window is open to take it.
    NoWindow,
    /// It can't be started at all, and why.
    Refused(String),
}

impl Clock {
    /// A clock reading the automations directory.
    pub(super) fn new() -> Self {
        Self::at(crate::paths::automations_dir().ok(), None)
    }

    /// A clock reading the automations under `root`, or none, and ket's
    /// state from `state`, or the real state file.
    pub(in crate::host) fn at(root: Option<PathBuf>, state: Option<PathBuf>) -> Self {
        Self {
            root,
            state,
            seen: None,
            automations: Automations::default(),
            tz: TimeZone::system(),
            tz_read: Instant::now(),
            last: None,
            settle_until: None,
            opened: None,
            followed: None,
            quiet: HashMap::new(),
            reserved: Vec::new(),
        }
    }

    /// Whether the host must stay up for an automation: one has a start to
    /// come, one that came is still to be dealt with, or one was asked to
    /// run now. A one-off whose moment passed before it was made does not.
    pub(in crate::host) fn keeps_alive(&self) -> bool {
        let now = Timestamp::now();
        self.automations.automations.iter().any(|automation| {
            automation.run_now_ms.is_some()
                || automation.next_start(now, &self.tz).is_some()
                || automation.due(now, &self.tz).is_some()
        })
    }

    /// One look at the clock: starts what is due, records what was missed,
    /// follows what is running.
    pub(in crate::host) fn tick(&mut self, host: &Host) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.reload(&root);
        let now = Timestamp::now();
        self.watch_for_sleep(Instant::now(), now);
        if self.automations.automations.is_empty() {
            return;
        }
        if self.tz_read.elapsed() >= TIME_ZONE_EVERY {
            self.tz = TimeZone::system();
            self.tz_read = Instant::now();
        }
        if self.followed.is_none_or(|at| at.elapsed() >= FOLLOW_EVERY) {
            self.followed = Some(Instant::now());
            self.follow_runs(host, &root);
        }
        // Run now first, and even while settling after a wake: a person
        // asked for it, so the Mac is in use.
        let asked: Vec<(String, u64)> = self
            .automations
            .automations
            .iter()
            .filter_map(|automation| Some((automation.id.clone(), automation.run_now_ms?)))
            .collect();
        for (id, asked_ms) in asked {
            self.run_now(host, &root, &id, asked_ms);
        }
        if self.settle_until.is_some_and(|until| now < until) {
            return;
        }
        let due: Vec<String> = self
            .automations
            .automations
            .iter()
            .filter(|automation| automation.due(now, &self.tz).is_some())
            .map(|automation| automation.id.clone())
            .collect();
        for id in due {
            self.take(host, &root, &id);
        }
    }

    /// Re-reads the automations when another process has written them.
    fn reload(&mut self, root: &Path) {
        let modified = Automations::modified_in(root);
        if modified.is_some() && modified == self.seen {
            return;
        }
        match Automations::load_in(root) {
            Ok(automations) => {
                self.automations = automations;
                self.seen = modified;
            }
            Err(error) => tracing::warn!(%error, "automations: can't read them"),
        }
    }

    /// Notices a sleep — the wall clock having run on further than the
    /// host's own since the last look — and holds starts back after it.
    fn watch_for_sleep(&mut self, mono: Instant, wall: Timestamp) {
        if let Some((was_mono, was_wall)) = self.last {
            let host_ran = SignedDuration::try_from(mono.duration_since(was_mono))
                .unwrap_or(SignedDuration::MAX);
            let wall_ran = wall.duration_since(was_wall);
            if wall_ran - host_ran > SLEPT {
                tracing::info!("automations: the Mac slept; holding starts for two minutes");
                self.settle_until = wall.checked_add(SETTLE).ok();
            }
        }
        self.last = Some((mono, wall));
    }

    /// Deals with automation `id`'s start, if one is due. Asked of the
    /// automation as saved now, not as it was when the look began: one
    /// paused or rescheduled since — by the window, the phone, another
    /// start's write — is taken as it now is.
    fn take(&mut self, host: &Host, root: &Path, id: &str) {
        self.reload(root);
        let Some(automation) = self.automations.get(id).cloned() else {
            return;
        };
        let Some(due) = automation.due(Timestamp::now(), &self.tz) else {
            return;
        };
        let at = due.at();
        if let Due::Missed(_) = due {
            let why = format!(
                "the Mac was asleep, or ket wasn't running, at {}",
                clock_label(at, &self.tz)
            );
            self.settle(root, &automation, at, Outcome::Missed { why });
            return;
        }
        if automation.skip_while_busy && self.still_working(host, root, &automation) {
            let why = "the last run was still working".to_owned();
            self.settle(root, &automation, at, Outcome::Skipped { why });
            return;
        }
        match self.dispatch(host, root, &automation, at) {
            Handed::To { .. } => {
                self.opened = None;
                self.mark_handled(root, &automation, at);
            }
            // Left undealt with: the next look tries again, and a start
            // that waits too long for a window becomes missed.
            Handed::NoWindow => self.open_ket(),
            Handed::Refused(_) => self.mark_handled(root, &automation, at),
        }
    }

    /// Starts automation `id` now, as someone asked at `asked_ms`, whatever
    /// its schedule — paused or not, its last run working or not.
    fn run_now(&mut self, host: &Host, root: &Path, id: &str, asked_ms: u64) {
        let Some(automation) = self.automations.get(id).cloned() else {
            return;
        };
        // The start is the moment it was asked for, which is also its run's
        // due time: the window reports back by it.
        let at = i64::try_from(asked_ms)
            .ok()
            .and_then(|ms| Timestamp::from_millisecond(ms).ok())
            .unwrap_or_else(Timestamp::now);
        match self.dispatch(host, root, &automation, at) {
            Handed::To { .. } => self.opened = None,
            Handed::NoWindow => {
                let waited = Duration::from_millis(crate::now_ms().saturating_sub(asked_ms));
                if waited < RUN_NOW_WAIT {
                    self.open_ket();
                    return;
                }
                let why = "no ket window opened to start it".to_owned();
                self.record(root, &automation, at, Outcome::CouldNotStart { why });
            }
            Handed::Refused(_) => {}
        }
        let cleared = Automations::update_in(root, |all| {
            if let Some(automation) = all.get_mut(id)
                && automation.run_now_ms == Some(asked_ms)
            {
                automation.run_now_ms = None;
            }
            Ok(())
        });
        match cleared {
            Ok(automations) => {
                // Read again next look rather than taking the file's time for
                // this copy's: a window or the CLI may have written since.
                self.automations = automations;
                self.seen = None;
            }
            Err(error) => {
                tracing::warn!(%error, "automations: can't clear a Run now");
                if let Some(automation) = self.automations.get_mut(id) {
                    automation.run_now_ms = None;
                }
            }
        }
    }

    /// Records the run before handing it to a window — with the command it
    /// runs, which is what `ket automation exec` runs — so the window can end
    /// it the moment it learns it failed; then fills in where it went, or
    /// takes it back when no window took it, or ends it as refused.
    fn dispatch(
        &mut self,
        host: &Host,
        root: &Path,
        automation: &Automation,
        at: Timestamp,
    ) -> Handed {
        if windows(&host.outs).is_empty() {
            return Handed::NoWindow;
        }
        let due_ms = u64::try_from(at.as_millisecond()).unwrap_or(0);
        let command = match &automation.task {
            Task::Command { line } => Some(line.clone()),
            Task::Prompt { .. } => None,
        };
        let reserved = Run {
            automation: automation.id.clone(),
            due_ms,
            at_ms: crate::now_ms(),
            outcome: Outcome::Started,
            worktree: None,
            branch: None,
            ended_ms: None,
            tokens: None,
            command,
        };
        if let Err(error) = Runs::update_in(root, |runs| runs.record(reserved)) {
            tracing::warn!(%error, "automations: can't record a run");
        }
        let handed = self.hand_over(host, automation, at);
        let id = automation.id.as_str();
        let written = Runs::update_in(root, |runs| match &handed {
            Handed::To { branch, worktree } => {
                if let Some(run) = runs.find_mut(id, due_ms) {
                    run.branch.clone_from(branch);
                    if run.worktree.is_none() {
                        run.worktree.clone_from(worktree);
                    }
                }
            }
            Handed::NoWindow => runs.runs.retain(|run| {
                !(run.automation == id && run.due_ms == due_ms && run.outcome.is_live())
            }),
            Handed::Refused(why) => {
                runs.end(
                    id,
                    due_ms,
                    Outcome::CouldNotStart { why: why.clone() },
                    crate::now_ms(),
                );
            }
        });
        if let Err(error) = written {
            tracing::warn!(%error, "automations: can't record where a run went");
        }
        handed
    }

    /// Records a start that didn't happen — missed or skipped — and that it
    /// has been dealt with.
    fn settle(&mut self, root: &Path, automation: &Automation, at: Timestamp, outcome: Outcome) {
        self.record(root, automation, at, outcome);
        self.mark_handled(root, automation, at);
    }

    /// Records what the start due `at` came to.
    fn record(&self, root: &Path, automation: &Automation, at: Timestamp, outcome: Outcome) {
        let run = Run {
            automation: automation.id.clone(),
            due_ms: u64::try_from(at.as_millisecond()).unwrap_or(0),
            at_ms: crate::now_ms(),
            outcome,
            worktree: None,
            branch: None,
            ended_ms: None,
            tokens: None,
            command: None,
        };
        if let Err(error) = Runs::update_in(root, |runs| runs.record(run)) {
            tracing::warn!(%error, "automations: can't record a run");
        }
    }

    /// Records that the start due `at` has been dealt with — for the
    /// schedule `automation` had, only: one changed meanwhile keeps its own
    /// starts, and a one-off moved to a new moment isn't made finished by
    /// the old one.
    fn mark_handled(&mut self, root: &Path, automation: &Automation, at: Timestamp) {
        let id = automation.id.clone();
        let when = automation.when.clone();
        match Automations::update_in(root, |all| {
            if let Some(saved) = all.get_mut(&id)
                && saved.when == when
            {
                saved.handled(at);
            }
            Ok(())
        }) {
            Ok(automations) => {
                // Read again next look rather than taking the file's time for
                // this copy's: a window or the CLI may have written since.
                self.automations = automations;
                self.seen = None;
            }
            Err(error) => {
                // Kept in memory at least, so this run of the host doesn't
                // start it again every second.
                tracing::warn!(%error, "automations: can't record a start as dealt with");
                if let Some(saved) = self.automations.get_mut(&id)
                    && saved.when == when
                {
                    saved.handled(at);
                }
            }
        }
    }

    /// Hands the start to a window, as `ket start` hands work.
    fn hand_over(&mut self, host: &Host, automation: &Automation, at: Timestamp) -> Handed {
        let handed = self.send_start(host, automation, at);
        if let Handed::To {
            branch: Some(branch),
            worktree: None,
        } = &handed
        {
            self.reserved
                .retain(|(_, _, at)| at.elapsed() < GETTING_GOING);
            self.reserved
                .push((automation.project.clone(), branch.clone(), Instant::now()));
        }
        handed
    }

    fn send_start(&self, host: &Host, automation: &Automation, at: Timestamp) -> Handed {
        let state = match self.store().and_then(|store| store.load()) {
            Ok(state) => state,
            Err(error) => return Handed::Refused(format!("ket's state couldn't be read: {error}")),
        };
        let Some(project) = state
            .projects
            .iter()
            .find(|project| project.id == automation.project)
        else {
            return Handed::Refused("its project isn't in ket any more".to_owned());
        };
        let (base, worktree, branch) = match &automation.place {
            Place::NewWorktree { base } => {
                let git = crate::git::Git::new(&project.root);
                let reserved: Vec<&str> = self
                    .reserved
                    .iter()
                    .filter(|(held, _, at)| held == &project.id && at.elapsed() < GETTING_GOING)
                    .map(|(_, branch, _)| branch.as_str())
                    .collect();
                let branch = automation.unused_branch(at, &self.tz, |name| {
                    reserved.contains(&name) || git.branch_exists(name)
                });
                (base.clone(), None, Some(branch))
            }
            Place::Worktree { id } => {
                // Its own project's: a worktree of another one is never where
                // this automation runs, whatever was saved.
                let Some(worktree) = state.worktrees.iter().find(|worktree| {
                    &worktree.id == id && worktree.project_id == automation.project
                }) else {
                    return Handed::Refused(
                        "its worktree has been removed, or isn't in its project".to_owned(),
                    );
                };
                (
                    None,
                    Some(worktree.id.clone()),
                    Some(worktree.branch.clone()),
                )
            }
        };
        let outs = windows(&host.outs);
        if outs.is_empty() {
            return Handed::NoWindow;
        }
        let (prompt, agent, model, effort, shell) = match &automation.task {
            Task::Prompt {
                text,
                agent,
                model,
                effort,
            } => (
                text.clone(),
                agent.clone(),
                model.clone(),
                effort.clone(),
                false,
            ),
            Task::Command { line } => (line.clone(), None, None, None, true),
        };
        let Ok(frame) = wire::json(&Reply::StartWork {
            project: automation.project.as_str().to_owned(),
            prompt,
            agent,
            note: None,
            // What a window from before automations names it.
            from: WorkSource::Cli,
            base,
            model,
            effort,
            worktree: worktree.as_ref().map(|id| id.as_str().to_owned()),
            shell,
            automation: Some(AutomationStart {
                id: automation.id.clone(),
                name: automation.name.clone(),
                branch: branch.clone().unwrap_or_default(),
                economy: automation.economy,
                due_ms: u64::try_from(at.as_millisecond()).unwrap_or(0),
            }),
        }) else {
            return Handed::Refused("the work couldn't be written for the window".to_owned());
        };
        // A window gone between the listing and the write takes nothing: the
        // next one is asked, and with none, the start waits as with no window.
        if !outs.iter().any(|out| out.send(&frame)) {
            return Handed::NoWindow;
        }
        tracing::info!(automation = %automation.id, "automations: handed a start to a window");
        Handed::To { branch, worktree }
    }

    /// Asks macOS to open ket, in the background, so a window comes to take
    /// a start — no more than once in [`OPEN_AGAIN`]. Only from inside the
    /// app: a host run from a development build has no app to open, and its
    /// starts wait for a window, then go missed.
    fn open_ket(&mut self) {
        if self.opened.is_some_and(|at| at.elapsed() < OPEN_AGAIN) {
            return;
        }
        self.opened = Some(Instant::now());
        let Some(app) = std::env::current_exe().ok().as_deref().and_then(app_bundle) else {
            return;
        };
        if cfg!(test) {
            return;
        }
        tracing::info!(app = %app.display(), "automations: opening ket for a start");
        if let Err(error) = std::process::Command::new("/usr/bin/open")
            .arg("-g")
            .arg(&app)
            .spawn()
        {
            tracing::warn!(%error, "automations: couldn't open ket");
        }
    }

    /// ket's state store.
    fn store(&self) -> crate::Result<crate::store::Store> {
        match &self.state {
            Some(path) => Ok(crate::store::Store::at(path.clone())),
            None => crate::store::Store::open_default(),
        }
    }

    /// Every worktree ket knows, or none when its state can't be read.
    fn known_worktrees(&self) -> Vec<Worktree> {
        self.store()
            .and_then(|store| store.load())
            .map(|state| state.worktrees)
            .unwrap_or_default()
    }

    /// Whether `automation`'s last run is still working.
    fn still_working(&self, host: &Host, root: &Path, automation: &Automation) -> bool {
        let Ok(runs) = Runs::load_in(root) else {
            return false;
        };
        let Some(run) = runs.live(&automation.id) else {
            return false;
        };
        let started = Duration::from_millis(crate::now_ms().saturating_sub(run.at_ms));
        match run_worktree(run, &automation.project, &self.known_worktrees()) {
            Some(worktree) => working(host, worktree.as_str()),
            // Its worktree isn't made yet: still getting going, unless that
            // has taken far too long.
            None => started < GETTING_GOING,
        }
    }

    /// Records each live run that has gone quiet for [`QUIET_FOR`] as done.
    fn follow_runs(&mut self, host: &Host, root: &Path) {
        let Ok(runs) = Runs::load_in(root) else {
            return;
        };
        let live: Vec<Run> = runs
            .runs
            .iter()
            .filter(|run| run.outcome.is_live())
            .cloned()
            .collect();
        self.quiet.retain(|(id, due), _| {
            live.iter()
                .any(|run| &run.automation == id && run.due_ms == *due)
        });
        if live.is_empty() {
            return;
        }
        let worktrees = self.known_worktrees();
        let mut done: Vec<(String, u64, Option<WorktreeId>)> = Vec::new();
        let mut found: Vec<(String, u64, WorktreeId)> = Vec::new();
        for run in &live {
            let Some(project) = self
                .automations
                .get(&run.automation)
                .map(|automation| automation.project.clone())
            else {
                continue;
            };
            let worktree = run_worktree(run, &project, &worktrees);
            if run.worktree.is_none()
                && let Some(worktree) = &worktree
            {
                found.push((run.automation.clone(), run.due_ms, worktree.clone()));
            }
            let started = Duration::from_millis(crate::now_ms().saturating_sub(run.at_ms));
            let key = (run.automation.clone(), run.due_ms);
            let quiet = match &worktree {
                Some(worktree) => !working(host, worktree.as_str()),
                None => started >= GETTING_GOING,
            };
            if !quiet || started < Duration::from_secs(60) {
                self.quiet.remove(&key);
                continue;
            }
            let since = *self.quiet.entry(key.clone()).or_insert_with(Instant::now);
            if since.elapsed() >= QUIET_FOR {
                self.quiet.remove(&key);
                done.push((key.0, key.1, worktree));
            }
        }
        if done.is_empty() && found.is_empty() {
            return;
        }
        let ended = crate::now_ms();
        let result = Runs::update_in(root, |runs| {
            // Each run's worktree, once it has one: what the runs table links.
            for (automation, due, worktree) in &found {
                if let Some(run) = runs.find_mut(automation, *due) {
                    run.worktree.get_or_insert_with(|| worktree.clone());
                }
            }
            // Only a run still going: the window or the command may have
            // ended it as failed since it was read.
            for (automation, due, worktree) in &done {
                if runs.end(automation, *due, Outcome::Done, ended)
                    && let Some(run) = runs.find_mut(automation, *due)
                    && run.worktree.is_none()
                {
                    run.worktree.clone_from(worktree);
                }
            }
        });
        if let Err(error) = result {
            tracing::warn!(%error, "automations: can't record a run as done");
        }
    }
}

/// The worktree a run is in: recorded, or found by the branch it was given
/// among `project`'s — another project's worktree on a branch of the same
/// name is never it.
fn run_worktree(run: &Run, project: &ProjectId, worktrees: &[Worktree]) -> Option<WorktreeId> {
    run.worktree.clone().or_else(|| {
        let branch = run.branch.as_deref()?;
        worktrees
            .iter()
            .find(|worktree| &worktree.project_id == project && worktree.branch == branch)
            .map(|worktree| worktree.id.clone())
    })
}

/// Whether anything is at work in `worktree`: an agent thinking, running a
/// tool, starting or waiting on a person; or a command running in one of its
/// terminals.
pub(in crate::host) fn working(host: &Host, worktree: &str) -> bool {
    let agent = host.agent_status().panes.iter().any(|pane| {
        pane.worktree == worktree
            && matches!(
                pane.state,
                AgentState::Starting
                    | AgentState::Authenticating
                    | AgentState::Thinking
                    | AgentState::ExecutingTool
                    | AgentState::AwaitingPermission
            )
    });
    // A command, by what its shell is running. Only a shell's: an agent's
    // tab always has the agent in its foreground, at work or not, and an
    // agent that has finished its turn is idle by its own word above.
    agent
        || host.listing().iter().any(|(_, key, terminal)| {
            key.as_deref().is_some_and(|key| {
                let mut parts = key.split('|');
                parts.next() == Some(worktree) && parts.next() == Some("shell")
            }) && matches!(terminal.foreground(), Some(Foreground::Running(_)))
        })
}

/// The `.app` an executable is inside, if it is inside one.
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|dir| dir.extension().is_some_and(|extension| extension == "app"))
        .map(Path::to_path_buf)
}

/// `2:00 AM` on the Mac's clock.
fn clock_label(at: Timestamp, tz: &TimeZone) -> String {
    let local = at.to_zoned(tz.clone());
    let hour = u8::try_from(local.hour()).unwrap_or(0);
    let minute = u8::try_from(local.minute()).unwrap_or(0);
    crate::automation::Clock::new(hour, minute).map_or_else(String::new, |clock| clock.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_is_dealt_with_only_for_the_schedule_it_came_from() {
        use crate::automation::{Clock as Time, Day, When};
        let root = std::env::temp_dir().join(format!("ket-mark-handled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tz = TimeZone::system();
        let today = Timestamp::now().to_zoned(tz.clone()).date();
        let once = |day| When::Once {
            day: Day::new(day),
            time: Time::new(9, 0).unwrap(),
        };
        let saved = Automation::new(
            &[],
            ProjectId::new("p"),
            "Once",
            Task::Command {
                line: "true".to_owned(),
            },
            once(today.tomorrow().unwrap()),
        );
        Automations::update_in(&root, |all| {
            all.automations.push(saved.clone());
            Ok(())
        })
        .unwrap();
        let mut clock = Clock::at(Some(root.clone()), None);
        // The start came from the schedule before it was moved to tomorrow.
        let mut before = saved.clone();
        before.when = once(today);
        clock.mark_handled(&root, &before, Timestamp::now());
        let handled = |root: &Path| {
            Automations::load_in(root)
                .unwrap()
                .get(&saved.id)
                .unwrap()
                .handled_ms
        };
        assert!(handled(&root).is_none(), "the moved one-off still starts");
        clock.mark_handled(&root, &saved, Timestamp::now());
        assert!(handled(&root).is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_an_executable_inside_an_app_has_an_app_to_open() {
        let inside = Path::new("/Applications/ket.app/Contents/MacOS/ket-ui");
        assert_eq!(
            app_bundle(inside),
            Some(PathBuf::from("/Applications/ket.app"))
        );
        assert_eq!(
            app_bundle(Path::new("/opt/src/ket/target/debug/ket-ui")),
            None
        );
    }

    #[test]
    fn a_wall_clock_far_ahead_of_the_hosts_is_a_sleep() {
        let mut clock = Clock::at(None, None);
        let mono = Instant::now();
        let wall = Timestamp::now();
        clock.watch_for_sleep(mono, wall);
        let later = wall.checked_add(SignedDuration::from_secs(1)).unwrap();
        clock.watch_for_sleep(mono + Duration::from_secs(1), later);
        assert!(clock.settle_until.is_none(), "an ordinary second");
        let woke = later.checked_add(SignedDuration::from_hours(6)).unwrap();
        clock.watch_for_sleep(mono + Duration::from_secs(2), woke);
        assert_eq!(clock.settle_until, woke.checked_add(SETTLE).ok());
    }
}
