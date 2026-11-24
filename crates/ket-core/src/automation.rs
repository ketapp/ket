//! Automations: work that starts itself — an agent given a prompt, or a
//! command in a terminal — once at a set time, or on a repeat.
//!
//! Every automation belongs to a project and is kept, with all the others,
//! in one file under [`paths::automations_dir`]; what each start came to is
//! kept beside it (see [`runs`]):
//!
//! ```text
//! automations/automations.json
//! automations/runs.json
//! ```
//!
//! One file rather than one per project because the list is read whole: the
//! Automations dialog shows every project's, sorted by when each starts next,
//! and the host's clock asks all of them each second. It is private to this
//! person, like the rest of the data directory.
//!
//! This module only keeps and answers questions about them. The host decides
//! when one is due and starts it; the shell and the phone show and edit them.
//!
//! The `_in` variants take the automations directory explicitly, so the file
//! handling is exercisable without a home.

pub mod runs;
pub mod when;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::config::write_atomically;
use crate::id::{ProjectId, WorktreeId};
use crate::{KetError, Result, now_ms, paths};

pub use runs::{Outcome, Run, Runs};
pub use when::{Clock, Day, Dow, Due, When};

const AUTOMATIONS_FILE: &str = "automations.json";

/// One automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    /// ket's own: base 36 of the moment it was made.
    pub id: String,
    /// What it is called in lists, and what its runs' branches are named
    /// after.
    pub name: String,
    /// The project it runs in.
    pub project: ProjectId,
    /// What it does.
    pub task: Task,
    /// When it starts.
    pub when: When,
    /// Where each run works.
    #[serde(default)]
    pub place: Place,
    /// The Economy level its agent starts at; `None` leaves it to the
    /// project's choice, as a start from the phone does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub economy: Option<u8>,
    /// Whether it starts at all. A paused one keeps its place in the list.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Whether a start is skipped while the last run is still working, rather
    /// than a second one working beside it.
    #[serde(default = "yes")]
    pub skip_while_busy: bool,
    /// How late, in minutes, a start missed while the Mac was asleep or ket
    /// was not running may still be taken. `None` is [`When::default_catch_up`];
    /// `Some(0)` never takes one late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up_minutes: Option<u32>,
    /// When it was made, in milliseconds since the epoch.
    pub created_ms: u64,
    /// When it was last changed.
    pub updated_ms: u64,
    /// The latest start already dealt with — started, skipped or missed — so
    /// none is dealt with twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handled_ms: Option<u64>,
    /// When it was last turned back on, so a start it slept through while
    /// paused is never taken late — see [`Automation::set_enabled`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_ms: Option<u64>,
    /// When someone asked for it to start now, whatever its schedule — Run
    /// now. The host starts it on its next look and clears this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_now_ms: Option<u64>,
}

fn yes() -> bool {
    true
}

/// What an automation does each time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    /// An agent, started with this prompt.
    Prompt {
        /// What the agent is told.
        text: String,
        /// The agent's name; `None` is the project's or ket's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        /// The model; `None` is the agent's own default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// The effort; `None` is the agent's own default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    /// A shell command, typed into a fresh terminal.
    Command {
        /// The command line.
        line: String,
    },
}

/// Where each run of an automation works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Place {
    /// A new worktree each run.
    NewWorktree {
        /// The branch it starts from; `None` is the project's default branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    /// A new tab in this worktree each run.
    Worktree {
        /// The worktree.
        id: WorktreeId,
    },
}

impl Default for Place {
    fn default() -> Self {
        Place::NewWorktree { base: None }
    }
}

impl Automation {
    /// A new automation, on, with an id none of `taken` has.
    pub fn new(
        taken: &[Automation],
        project: ProjectId,
        name: &str,
        task: Task,
        when: When,
    ) -> Self {
        let now = now_ms();
        let mut stamp = now;
        let id = loop {
            let id = base36(stamp);
            if !taken.iter().any(|automation| automation.id == id) {
                break id;
            }
            stamp += 1;
        };
        Self {
            id,
            name: name.trim().to_owned(),
            project,
            task,
            when,
            place: Place::default(),
            economy: None,
            enabled: true,
            skip_while_busy: true,
            catch_up_minutes: None,
            created_ms: now,
            updated_ms: now,
            handled_ms: None,
            resumed_ms: None,
            run_now_ms: None,
        }
    }

    /// Asks the host to start it now, whatever its schedule, paused or not.
    pub fn ask_to_run(&mut self) {
        self.run_now_ms = Some(now_ms());
    }

    /// Pauses it, or turns it back on. Turned back on, it starts from its
    /// next start: nothing that came due while it was paused is taken late.
    pub fn set_enabled(&mut self, on: bool) {
        if on && !self.enabled {
            self.resumed_ms = Some(now_ms());
        }
        self.enabled = on;
        self.updated_ms = now_ms();
    }

    /// How late a missed start may still be taken.
    pub fn catch_up(&self) -> SignedDuration {
        match self.catch_up_minutes {
            Some(minutes) => SignedDuration::from_mins(i64::from(minutes)),
            None => self.when.default_catch_up(),
        }
    }

    /// When it starts next, after `now`; `None` while paused, or for a
    /// one-off that has had its start.
    pub fn next_start(&self, now: Timestamp, tz: &TimeZone) -> Option<Timestamp> {
        if !self.enabled || self.finished() {
            return None;
        }
        self.when.next_after(now, tz)
    }

    /// Whether a start has come due that hasn't been dealt with — see
    /// [`When::due`]. Never for a paused automation, nor a one-off that has
    /// had its start (whatever the clock now says its moment is, after a
    /// change of time zone), nor for a start that came while it was paused.
    pub fn due(&self, now: Timestamp, tz: &TimeZone) -> Option<Due> {
        if !self.enabled || self.finished() {
            return None;
        }
        let since = [self.handled_ms, Some(self.created_ms), self.resumed_ms]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(0);
        let since = Timestamp::from_millisecond(i64::try_from(since).ok()?).ok()?;
        self.when.due(since, now, self.catch_up(), tz)
    }

    /// Records that the start due at `at` has been dealt with.
    pub fn handled(&mut self, at: Timestamp) {
        let at = u64::try_from(at.as_millisecond()).unwrap_or(0);
        self.handled_ms = Some(self.handled_ms.map_or(at, |was| was.max(at)));
    }

    /// A one-off that has had its start.
    pub fn finished(&self) -> bool {
        self.when.is_once() && self.handled_ms.is_some()
    }

    /// The branch a run due at `at` works on: the name's first words, and the
    /// day — `audit-dependencies-1120` — so each day's run has its own. An
    /// hourly one adds the time, and a one-off needs neither.
    ///
    /// Not necessarily free: two automations with alike names, or last
    /// year's branch for the same day, can hold it. The starter takes
    /// [`Automation::unused_branch`] instead.
    pub fn branch_for(&self, at: Timestamp, tz: &TimeZone) -> String {
        let stem = crate::git::branch_stem(&self.name);
        let stem = if stem.is_empty() {
            "automation".to_owned()
        } else {
            stem
        };
        let local = at.to_zoned(tz.clone());
        match self.when {
            When::Once { .. } => stem,
            When::Hourly { .. } => format!("{stem}-{}", local.strftime("%m%d-%H%M")),
            _ => format!("{stem}-{}", local.strftime("%m%d")),
        }
    }

    /// [`Automation::branch_for`], or that with `-2`, `-3`… after it, the
    /// first `taken` says no branch has.
    pub fn unused_branch(
        &self,
        at: Timestamp,
        tz: &TimeZone,
        taken: impl Fn(&str) -> bool,
    ) -> String {
        let branch = self.branch_for(at, tz);
        if !taken(&branch) {
            return branch;
        }
        (2..)
            .map(|n| format!("{branch}-{n}"))
            .find(|candidate| !taken(candidate))
            .unwrap_or(branch)
    }
}

/// Every automation, in the order they were made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Automations {
    /// Oldest first.
    pub automations: Vec<Automation>,
}

impl Automations {
    /// Every automation. A missing file is none; a malformed one is an
    /// error, and is never written over.
    pub fn load() -> Result<Self> {
        Self::load_in(&paths::automations_dir()?)
    }

    /// The same, under an explicit automations directory.
    pub fn load_in(root: &Path) -> Result<Self> {
        read_json(&file_in(root))
    }

    /// Re-reads the file, applies `change` and writes it back, so an edit in
    /// one window never drops what the host or another window just saved.
    /// Nothing is written when `change` fails. Returns the result.
    pub fn update(change: impl FnOnce(&mut Self) -> Result<()>) -> Result<Self> {
        Self::update_in(&paths::automations_dir()?, change)
    }

    /// The same, under an explicit automations directory.
    pub fn update_in(root: &Path, change: impl FnOnce(&mut Self) -> Result<()>) -> Result<Self> {
        let _lock = lock_in(root, AUTOMATIONS_FILE)?;
        let mut automations = Self::load_in(root)?;
        change(&mut automations)?;
        for automation in &automations.automations {
            automation.when.check().map_err(KetError::Conflict)?;
        }
        write_json(&file_in(root), &automations)?;
        Ok(automations)
    }

    /// Deletes automation `id` and the record of its runs. The worktrees its
    /// runs made are left alone.
    pub fn delete_in(root: &Path, id: &str) -> Result<()> {
        // The history first: one that can't be read can't be cleaned up, and
        // a failed delete shouldn't have taken the automation already.
        Runs::load_in(root)?;
        Self::update_in(root, |all| {
            all.automations.retain(|automation| automation.id != id);
            Ok(())
        })?;
        Runs::update_in(root, |runs| runs.forget(id))?;
        Ok(())
    }

    /// The same, in the automations directory.
    pub fn delete(id: &str) -> Result<()> {
        Self::delete_in(&paths::automations_dir()?, id)
    }

    /// When the file last changed, or `None` when there is none — for a
    /// reader that reloads only when another process has written it.
    pub fn modified() -> Option<SystemTime> {
        Self::modified_in(&paths::automations_dir().ok()?)
    }

    /// The same, under an explicit automations directory.
    pub fn modified_in(root: &Path) -> Option<SystemTime> {
        std::fs::metadata(file_in(root))
            .and_then(|meta| meta.modified())
            .ok()
    }

    /// Automation `id`.
    pub fn get(&self, id: &str) -> Option<&Automation> {
        self.automations
            .iter()
            .find(|automation| automation.id == id)
    }

    /// Automation `id`, to change.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Automation> {
        self.automations
            .iter_mut()
            .find(|automation| automation.id == id)
    }

    /// The ones that will start, soonest first, each with its next start.
    /// Paused ones and finished one-offs are left out.
    pub fn upcoming(&self, now: Timestamp, tz: &TimeZone) -> Vec<(&Automation, Timestamp)> {
        let mut upcoming: Vec<_> = self
            .automations
            .iter()
            .filter_map(|automation| Some((automation, automation.next_start(now, tz)?)))
            .collect();
        upcoming.sort_by_key(|(automation, at)| (*at, automation.created_ms));
        upcoming
    }
}

fn file_in(root: &Path) -> PathBuf {
    root.join(AUTOMATIONS_FILE)
}

/// Holds `name`'s lock under `root` until dropped, so the host and every
/// window read, change and write the file one at a time — a window's edit
/// never drops the start the host just recorded, nor the other way round.
fn lock_in(root: &Path, name: &str) -> Result<crate::store::LockGuard> {
    std::fs::create_dir_all(root).map_err(|e| KetError::io(root, e))?;
    crate::store::LockGuard::acquire(root.join(format!("{name}.lock")))
}

/// `path` read as JSON; a missing file is the default.
fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(KetError::io(path, e)),
    }
}

/// Written readable by this person alone: prompts and command lines are
/// theirs.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| KetError::Config(e.to_string()))?;
    write_atomically(path, &text, true)
}

fn base36(mut n: u64) -> String {
    let mut digits = Vec::new();
    loop {
        let digit = u32::try_from(n % 36).unwrap_or(0);
        digits.push(char::from_digit(digit, 36).unwrap_or('0'));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}
