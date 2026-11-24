//! What each start of an automation came to: started, done or failed, or
//! not started at all — skipped while the last run still worked, missed while
//! the Mac was asleep, or refused.
//!
//! Every automation's runs share one file, `automations/runs.json`, newest
//! first. A run is kept for [`KEEP_DAYS`], and an automation keeps at most
//! [`KEEP_EACH`] of them, so an hourly one that has run for a year doesn't
//! grow the file without end. The worktree a run made is not removed with
//! its record: that is the person's work, and goes when they remove it.

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::id::WorktreeId;
use crate::{Result, now_ms};

use super::{lock_in, read_json, write_json};

#[cfg(test)]
mod tests;

/// How long a run's record is kept.
pub const KEEP_DAYS: u64 = 90;

/// The most records one automation keeps.
pub const KEEP_EACH: usize = 200;

const RUNS_FILE: &str = "runs.json";

/// One start of an automation, or one that didn't happen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// The automation's id.
    pub automation: String,
    /// When it was due, in milliseconds since the epoch.
    pub due_ms: u64,
    /// When it started, or was skipped, missed or refused.
    pub at_ms: u64,
    /// What it came to.
    pub outcome: Outcome,
    /// The worktree it ran in, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeId>,
    /// That worktree's branch, which outlives the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// When its agent or command finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// The tokens it spent, where the agent reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// For a command, the line this run was started with: what `ket
    /// automation exec` runs, whatever the automation says by then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// What a run came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Started and still working.
    Started,
    /// Finished.
    Done,
    /// Ended badly.
    Failed {
        /// What went wrong, for a person.
        why: String,
    },
    /// Not started, on purpose: the last run was still working.
    Skipped {
        /// Why, for a person.
        why: String,
    },
    /// Not started: it came due while the Mac was asleep or off, and was too
    /// late to start by the time it woke.
    Missed {
        /// Why, for a person.
        why: String,
    },
    /// Not started: something refused — the project has gone, ket couldn't
    /// open.
    CouldNotStart {
        /// What, for a person.
        why: String,
    },
}

impl Outcome {
    /// Whether the run is still going.
    pub fn is_live(&self) -> bool {
        matches!(self, Outcome::Started)
    }

    /// `Done`, `Failed`, `Skipped`… as the runs table says it.
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Started => "Running",
            Outcome::Done => "Done",
            Outcome::Failed { .. } => "Failed",
            Outcome::Skipped { .. } => "Skipped",
            Outcome::Missed { .. } => "Missed",
            Outcome::CouldNotStart { .. } => "Couldn't start",
        }
    }
}

/// Every automation's runs, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Runs {
    /// Newest first.
    pub runs: Vec<Run>,
}

impl Runs {
    /// The runs under `root`, the automations directory. A missing file is
    /// no runs; a malformed one is an error, and is never written over.
    pub fn load_in(root: &Path) -> Result<Self> {
        read_json(&root.join(RUNS_FILE))
    }

    /// Re-reads the file, applies `change`, drops what is past keeping and
    /// writes it back. Returns the result.
    pub fn update_in(root: &Path, change: impl FnOnce(&mut Self)) -> Result<Self> {
        let _lock = lock_in(root, RUNS_FILE)?;
        let mut runs = Self::load_in(root)?;
        change(&mut runs);
        runs.prune(now_ms());
        write_json(&root.join(RUNS_FILE), &runs)?;
        Ok(runs)
    }

    /// When the file last changed, for a reader that reloads only then.
    pub fn modified_in(root: &Path) -> Option<SystemTime> {
        std::fs::metadata(root.join(RUNS_FILE))
            .and_then(|meta| meta.modified())
            .ok()
    }

    /// Adds `run` at the top.
    pub fn record(&mut self, run: Run) {
        self.runs.insert(0, run);
    }

    /// The run of `automation` that was due at `due_ms`, to fill in once it
    /// has a worktree or has finished.
    pub fn find_mut(&mut self, automation: &str, due_ms: u64) -> Option<&mut Run> {
        self.runs
            .iter_mut()
            .find(|run| run.automation == automation && run.due_ms == due_ms)
    }

    /// `automation`'s runs, newest first.
    pub fn of<'a>(&'a self, automation: &'a str) -> impl Iterator<Item = &'a Run> + 'a {
        self.runs
            .iter()
            .filter(move |run| run.automation == automation)
    }

    /// `automation`'s run that is still working, if it has one.
    pub fn live(&self, automation: &str) -> Option<&Run> {
        self.runs
            .iter()
            .find(|run| run.automation == automation && run.outcome.is_live())
    }

    /// Ends the run of `automation` due at `due_ms` as `outcome`, if it is
    /// still going — by whoever learns how it went: the window, when its
    /// worktree couldn't be made; a command's own terminal, with its exit
    /// status. A run already ended is left as it is.
    pub fn end(&mut self, automation: &str, due_ms: u64, outcome: Outcome, at_ms: u64) -> bool {
        match self.find_mut(automation, due_ms) {
            Some(run) if run.outcome.is_live() => {
                run.outcome = outcome;
                run.ended_ms = Some(at_ms);
                true
            }
            _ => false,
        }
    }

    /// Forgets every run of `automation`, when it is deleted.
    pub fn forget(&mut self, automation: &str) {
        self.runs.retain(|run| run.automation != automation);
    }

    /// Drops runs older than [`KEEP_DAYS`], and each automation's past its
    /// newest [`KEEP_EACH`]. A run still working is kept whatever its age.
    pub fn prune(&mut self, now_ms: u64) {
        let oldest = now_ms.saturating_sub(KEEP_DAYS * 24 * 60 * 60 * 1000);
        let mut kept: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        self.runs.retain(|run| {
            if run.outcome.is_live() {
                return true;
            }
            let count = kept.entry(run.automation.clone()).or_default();
            *count += 1;
            run.at_ms >= oldest && *count <= KEEP_EACH
        });
    }
}
