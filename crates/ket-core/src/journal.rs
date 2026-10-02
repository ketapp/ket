//! An append-only record of the event bus.
//!
//! [`crate::event::EventBus`] is a `tokio::sync::broadcast` channel, which
//! reaches exactly as far as the process that owns it. That is the right shape
//! for the M1 shell, which links `ket-core` directly and subscribes in-process,
//! and it is useless for `ket events --follow`, which runs in a *different*
//! process from the `ket run` whose events it wants to watch.
//!
//! So the bus also writes here, and the CLI tails the file. One line of JSON per
//! envelope, which is the format `ket events` was always specified to emit —
//! the journal is not a second serialisation, it is the same one, landed on
//! disk.
//!
//! Writes are best-effort and never fail a publish. Losing a line from a log is
//! not a reason to fail the operation being logged.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::event::Envelope;

/// Size at which the journal is rotated.
///
/// Per the memory budget's principle that every accumulating structure gets a
/// bound: this one accumulates on disk rather than in memory, and would
/// otherwise grow for as long as ket is ever used.
pub const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;

/// An append-only JSONL record of published events.
#[derive(Debug, Clone)]
pub struct Journal {
    path: PathBuf,
}

impl Journal {
    /// A journal writing to `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where the journal is written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file holding the previous generation, after a rotation.
    pub fn rotated_path(&self) -> PathBuf {
        self.path.with_extension("jsonl.1")
    }

    /// Appends one envelope, ignoring any failure.
    ///
    /// The whole line is written in a single `write_all` on a handle opened with
    /// `O_APPEND`, so concurrent `ket` processes interleave whole lines rather
    /// than corrupting each other's. Several processes writing one journal is
    /// the normal case, not an edge case: `ket run` in two terminals plus a
    /// `ket events --follow` watching both is the workflow this exists for.
    pub fn append(&self, envelope: &Envelope) {
        if let Err(e) = self.try_append(envelope) {
            tracing::debug!(%e, path = %self.path.display(), "event journal append failed");
        }
    }

    /// The fallible half of [`Journal::append`].
    fn try_append(&self, envelope: &Envelope) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut line = serde_json::to_vec(envelope).map_err(std::io::Error::other)?;
        line.push(b'\n');

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(&line)?;

        // Checked after the write so a burst never blocks on a stat, and so the
        // file crosses the threshold rather than stopping just short of it.
        if file.metadata().map(|m| m.len()).unwrap_or(0) > MAX_JOURNAL_BYTES {
            let _ = fs::rename(&self.path, self.rotated_path());
        }

        Ok(())
    }

    /// Reads every envelope currently in the journal.
    ///
    /// Lines that fail to parse are skipped rather than failing the read: a
    /// partially written final line is normal when something is appending
    /// concurrently, and it should not cost the reader the rest of the file.
    pub fn read_all(&self) -> Vec<Envelope> {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };

        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}
