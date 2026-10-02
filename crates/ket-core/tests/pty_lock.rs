//! Both pty-opening code paths, racing each other.
//!
//! `crates/ket-core/tests/terminal.rs` already pins that opening many terminals
//! at once succeeds, but that alone does not pin the bug this crate actually
//! has: `terminal.rs` and `agent::pty` each call `openpty(3)`, and `openpty` is
//! not thread-safe on macOS. Two *separate* locks, one per module, would not
//! protect either of them — a thread opening a terminal and a thread starting
//! an agent can still call `openpty` at the same instant, one from each module,
//! which is exactly the shape that matters in ket: a worktree with a terminal
//! pane open while an agent starts running in it, times several worktrees
//! restoring together.
//!
//! This test opens ptys from both `LocalTerminal::open` and `agent::pty::run` on
//! separate OS threads at the same time. It only proves anything if the lock is
//! genuinely shared: with a lock private to each module (or no lock at all) it
//! fails intermittently with an `openpty` error whose errno decodes to nothing
//! at all, the same `Unknown error: -6` `terminal.rs`'s own regression test was
//! written against.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::Sandbox;
use ket_core::agent::pty;
use ket_core::agent::{SessionContext, cancellation};
use ket_core::config::{AgentSpec, AgentTimeouts, PermissionConfig, Transport};
use ket_core::event::EventBus;
use ket_core::id::SessionId;
use ket_core::terminal::{LocalTerminal, TerminalSize, TerminalSpec};

/// An agent that answers a prompt and exits, run over a pty.
fn echo_agent() -> AgentSpec {
    AgentSpec {
        name: "echo".to_owned(),
        transport: Transport::Pty,
        command: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "read line; printf 'answering: %s\\n' \"$line\"".to_owned(),
        ],
        env: BTreeMap::new(),
        env_remove: Vec::new(),
        launch: None,
    }
}

/// Runs one agent session to completion, on whatever thread calls this.
///
/// A tiny current-thread runtime rather than `#[tokio::test]`: the point of
/// this test is genuine OS-thread simultaneity at the moment `openpty` is
/// called, which `std::thread::scope` guarantees and a shared tokio executor's
/// scheduling does not.
fn run_agent_session(cwd: std::path::PathBuf, index: usize) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    runtime.block_on(async {
        let spec = echo_agent();
        let bus = Arc::new(EventBus::new(16));
        let (_handle, cancel) = cancellation();
        let ctx = Arc::new(SessionContext::new(
            SessionId::new(format!("pty-lock-agent-{index}")),
            cwd,
            bus,
            AgentTimeouts::default(),
            PermissionConfig::default(),
            cancel,
        ));

        pty::run(&spec, &ctx, "ping")
            .await
            .expect("agent pty session");
    });
}

/// A terminal running a trivial script, sized for a small pane.
fn terminal_spec(cwd: &std::path::Path, index: usize) -> TerminalSpec {
    TerminalSpec {
        env_remove: Vec::new(),
        command: "sh".to_owned(),
        args: vec!["-c".to_owned(), format!("printf 'pane {index}\\n'")],
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
    }
}

#[test]
fn terminals_and_agent_sessions_opening_ptys_together_all_succeed() {
    let sandbox = Sandbox::new("pty-lock-shared");
    let cwd = sandbox.root().to_path_buf();

    std::thread::scope(|scope| {
        for index in 0..8 {
            let cwd = cwd.clone();
            scope.spawn(move || run_agent_session(cwd, index));
        }

        for index in 0..8 {
            let cwd = cwd.clone();
            scope.spawn(move || {
                LocalTerminal::open(&terminal_spec(&cwd, index)).expect("open terminal");
            });
        }
    });
}
