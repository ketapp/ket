//! Error types for `ket-core`.
//!
//! One variant per subsystem, so callers can match on *where* something failed
//! without string-matching a message. Deliberately no `anyhow` here — erasing the
//! error type is a choice for binaries, not for the library they build on.

use std::path::{Path, PathBuf};

/// The result type used throughout `ket-core`.
pub type Result<T, E = KetError> = std::result::Result<T, E>;

/// Anything that can go wrong inside ket.
#[derive(Debug, thiserror::Error)]
pub enum KetError {
    /// Configuration could not be read, parsed, or validated.
    #[error("config: {0}")]
    Config(String),

    /// A path ket depends on could not be resolved.
    #[error("could not resolve {what}: {why}")]
    Path {
        /// What was being resolved, e.g. `"home directory"`.
        what: &'static str,
        /// Why resolution failed.
        why: String,
    },

    /// A `git` invocation failed.
    ///
    /// Worktree mutations shell out to the real `git` binary rather than going
    /// through a library, so failures arrive as exit statuses and stderr.
    #[error("git {command} failed ({status}): {stderr}")]
    Git {
        /// The subcommand that failed, e.g. `"worktree add"`.
        command: String,
        /// Exit status, rendered.
        status: String,
        /// Whatever git wrote to stderr, trimmed.
        stderr: String,
    },

    /// A read-only git operation performed in-process by `gix` failed.
    ///
    /// Distinct from [`KetError::Git`] on purpose. That one carries an exit
    /// status and stderr because it came from the `git` binary, which is where
    /// every worktree *mutation* still goes; this one comes from the library
    /// used on read paths, and has neither.
    #[error("git ({what}): {why}")]
    Gix {
        /// What was being read, e.g. `"status"`.
        what: &'static str,
        /// What went wrong.
        why: String,
    },

    /// A project was referenced that ket does not know about.
    #[error("unknown project: {0}")]
    UnknownProject(String),

    /// A worktree was referenced that ket does not know about.
    #[error("unknown worktree: {0}")]
    UnknownWorktree(String),

    /// The requested operation conflicts with existing state.
    ///
    /// Distinct from a git failure: nothing went wrong, the request is just not
    /// something ket will do — a branch that already exists, a project with live
    /// worktrees, an ambiguous name.
    #[error("{0}")]
    Conflict(String),

    /// An agent could not be started, or died unexpectedly.
    #[error("agent {agent}: {why}")]
    Agent {
        /// The configured agent name, e.g. `"claude"`.
        agent: String,
        /// What went wrong.
        why: String,
    },

    /// Filesystem I/O failed.
    ///
    /// Always carries the path. A bare `NotFound` with no path is close to
    /// useless when debugging, and this tool touches a lot of paths.
    #[error("io: {source} ({path})")]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
}

impl KetError {
    /// Builds a [`KetError::Gix`] from any error the library returned.
    pub fn gix(what: &'static str, source: impl std::fmt::Display) -> Self {
        Self::Gix {
            what,
            why: source.to_string(),
        }
    }

    /// Builds an [`KetError::Io`] with the path attached.
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}
