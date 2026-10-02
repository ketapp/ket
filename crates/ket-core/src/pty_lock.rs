//! The one lock every `openpty(3)` call in this crate must go through.
//!
//! `openpty` is not thread-safe on macOS — it goes through `ptsname`, which
//! returns a pointer into static storage — so two threads opening a pty at the
//! same instant intermittently get a failure whose errno decodes to nothing at
//! all (`Unknown error: -6`). Opening several at once is ket's *normal* case: a
//! project with several worktrees restores a terminal pane and starts an agent
//! in each of them together, which is precisely the shape that trips it.
//!
//! [`terminal`](crate::terminal) and [`agent::pty`](crate::agent::pty) both open
//! ptys, on their own threads, and neither is the only place it happens. A lock
//! private to one of them would not protect the other: two threads can still
//! call `openpty` at the same instant, one from each module, and the bug comes
//! back in exactly the configuration that matters — an agent running in a
//! worktree that also has a terminal open. So there is exactly one lock here,
//! and both modules acquire it around their call.
//!
//! Held for a couple of syscalls, once per pty. Nothing about opening a
//! terminal or starting an agent is on a hot path, and the alternative is a
//! pane or a session that fails to open every so often for no reason anyone can
//! reproduce.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serialises every `openpty(3)` call in the process.
static OPEN_PTY: Mutex<()> = Mutex::new(());

/// Acquires the process-wide pty-open lock.
///
/// Hold the returned guard across the `openpty` call and nothing longer — see
/// the module documentation for why the lock exists at all. Poisoning is
/// ignored: a panic while some *other* pty was being opened says nothing about
/// whether this one is safe to open, and refusing every future pty because one
/// thread once panicked would be a strange way to fail safe.
pub fn lock() -> MutexGuard<'static, ()> {
    OPEN_PTY.lock().unwrap_or_else(PoisonError::into_inner)
}
