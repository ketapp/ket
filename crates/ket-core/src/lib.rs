//! `ket-core` — the engine.
//!
//! Everything ket can do lives here. This crate compiles and is fully exercisable
//! with no UI whatsoever: both `ket-cli` and the native shell `ket-ui` are
//! *clients* of this crate, not privileged parts of it.
//!
//! That is why the shell could be swapped
//! from a browser to `gpui` without the engine noticing.

pub mod activity;
pub mod admin;
pub mod agent;
pub mod agent_hooks;
pub mod agent_status;
pub mod agents;
pub mod app_icon;
pub mod backlog;
pub mod branch_cleanup;
pub mod buffer;
pub mod build_info;
pub mod capture;
pub mod changelog;
pub mod command;
pub mod config;
pub mod conversation;
pub mod cow;
pub mod diff;
pub mod error;
pub mod event;
pub mod feedback;
pub mod files;
pub mod git;
pub mod hook;
pub mod host;
pub mod id;
pub mod jev;
pub mod journal;
pub mod keybinding;
pub mod logging;
pub mod ollama;
pub mod pack;
pub mod paths;
pub mod project;
pub mod provision;
pub mod pty_lock;
pub mod rate_limits;
pub mod rates;
pub mod review;
pub mod sessions;
pub mod shell;
pub mod slug;
pub mod snippets;
pub mod status;
pub mod storage;
pub mod store;
pub mod subagents;
pub mod surface;
pub mod syntax;
pub mod terminal;
pub mod text_search;
pub mod theme;
pub mod usage;
pub mod workspace;
pub mod worktree;
pub mod worktree_trash;

pub use error::{KetError, Result};
/// The Android Emulator, for the emulator tab — see the crate.
pub use ket_android as android;

/// Milliseconds since the Unix epoch.
///
/// A clock before 1970 is not a case worth propagating an error for; it saturates.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
