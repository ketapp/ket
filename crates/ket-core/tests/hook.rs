//! Lifecycle hooks against real processes.
//!
//! The unit tests inside `hook.rs` cover the mechanism — precedence, the exit
//! code contract, secrets never reaching argv — against a fake spawner, the
//! same way `provision.rs`'s unit tests cover its mechanism. What is worth
//! checking here, against a real `std::process::Command`, is the two claims
//! that only mean something with a real OS process behind them: a hook that
//! never exits does not wedge the caller, and the JSON context a hook reads on
//! its real stdin is exactly what was sent.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::Sandbox;
use ket_core::KetError;
use ket_core::config::{HookConfig, HookSpec};
use ket_core::hook::{HookPoint, HookRunner, HookSpawner, ProcessHookSpawner};

fn hook(command: &[&str], timeout_secs: u64) -> HookSpec {
    HookSpec {
        command: command.iter().map(|s| (*s).to_owned()).collect(),
        timeout_secs,
    }
}

fn global_only(point: HookPoint, hooks: Vec<HookSpec>) -> HookConfig {
    let mut config = HookConfig::default();
    match point {
        HookPoint::WorktreeCreated => config.worktree_created = hooks,
        HookPoint::WorktreeProvisioned => config.worktree_provisioned = hooks,
        HookPoint::WorktreeRemoved => config.worktree_removed = hooks,
        HookPoint::AgentStarted => config.agent_started = hooks,
        HookPoint::AgentFinished => config.agent_finished = hooks,
        HookPoint::AgentPermissionRequested => config.agent_permission_requested = hooks,
    }
    config
}

#[test]
fn a_hung_hook_is_killed_at_its_timeout_and_blocks_rather_than_wedging_the_caller() {
    // Invariant 4: every state is escapable, including a call site waiting on
    // a hook that was configured to run forever.
    let runner = HookRunner::real();
    let global = global_only(HookPoint::AgentFinished, vec![hook(&["sleep", "60"], 1)]);

    let started = Instant::now();
    let result = runner.run(
        HookPoint::AgentFinished,
        &HookConfig::default(),
        &global,
        &serde_json::json!({}),
    );
    let elapsed = started.elapsed();

    match result {
        Err(KetError::Conflict(message)) => {
            assert!(message.contains("timed out"), "{message}");
        }
        other => panic!("expected a timeout conflict, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(10),
        "the timeout did not fire promptly: waited {elapsed:?}"
    );
}

#[test]
fn a_hook_binary_that_does_not_exist_fails_promptly_rather_than_hanging() {
    let runner = HookRunner::real();
    let global = global_only(
        HookPoint::WorktreeCreated,
        vec![hook(&["ket-no-such-hook-binary-9f3a"], 30)],
    );

    let started = Instant::now();
    let result = runner.run(
        HookPoint::WorktreeCreated,
        &HookConfig::default(),
        &global,
        &serde_json::json!({}),
    );

    assert!(matches!(result, Err(KetError::Io { .. })), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?} to report a missing hook binary",
        started.elapsed()
    );
}

#[test]
fn a_successful_hook_within_its_timeout_lets_the_operation_proceed() {
    let runner = HookRunner::real();
    let global = global_only(HookPoint::WorktreeRemoved, vec![hook(&["true"], 5)]);

    let result = runner.run(
        HookPoint::WorktreeRemoved,
        &HookConfig::default(),
        &global,
        &serde_json::json!({}),
    );

    assert!(result.is_ok());
}

#[test]
fn a_hook_that_exits_non_zero_really_blocks_the_operation() {
    let runner = HookRunner::real();
    let global = global_only(HookPoint::WorktreeProvisioned, vec![hook(&["false"], 5)]);

    let result = runner.run(
        HookPoint::WorktreeProvisioned,
        &HookConfig::default(),
        &global,
        &serde_json::json!({}),
    );

    assert!(matches!(result, Err(KetError::Conflict(_))));
}

#[test]
fn the_real_spawner_writes_context_to_the_hooks_actual_stdin_as_json() {
    // `cat` is the simplest possible hook: whatever comes in on stdin comes
    // back out on stdout. Round-tripping the context through a real process
    // this way is the strongest evidence available that it travels as JSON on
    // stdin, not as an argument the process table would expose.
    let context = serde_json::json!({
        "worktreeId": "wt-42",
        "secret": "sk-should-never-be-in-argv",
    });

    let exit = ProcessHookSpawner
        .run(
            &["cat".to_owned()],
            &serde_json::to_vec(&context).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

    assert_eq!(exit.exit_code, Some(0));
    let decoded: serde_json::Value = serde_json::from_str(&exit.stdout).unwrap();
    assert_eq!(decoded, context);
}

#[test]
fn project_and_global_hooks_both_run_as_real_processes_project_first() {
    // Two real scripts, each appending their own name to a shared file. The
    // resulting order is the precedence rule made observable outside the
    // mock the unit tests use.
    let sandbox = Sandbox::new("hook-precedence");
    let log = sandbox.path("order.log");

    let project_script = write_appender(&sandbox, "project.sh", "project", &log);
    let global_script = write_appender(&sandbox, "global.sh", "global", &log);

    let project = HookConfig {
        worktree_created: vec![hook(&[project_script.to_str().unwrap()], 5)],
        ..HookConfig::default()
    };
    let global = HookConfig {
        worktree_created: vec![hook(&[global_script.to_str().unwrap()], 5)],
        ..HookConfig::default()
    };

    let runner = HookRunner::real();
    runner
        .run(
            HookPoint::WorktreeCreated,
            &project,
            &global,
            &serde_json::json!({}),
        )
        .unwrap();

    assert_eq!(std::fs::read_to_string(&log).unwrap(), "project\nglobal\n");
}

#[test]
fn a_blocking_project_hook_prevents_the_global_hook_from_ever_running() {
    let sandbox = Sandbox::new("hook-precedence-block");
    let log = sandbox.path("order.log");

    let project_script = write_blocking_appender(&sandbox, "project-blocks.sh", "project", &log);
    let global_script = write_appender(&sandbox, "global-unreached.sh", "global", &log);

    let project = HookConfig {
        worktree_removed: vec![hook(&[project_script.to_str().unwrap()], 5)],
        ..HookConfig::default()
    };
    let global = HookConfig {
        worktree_removed: vec![hook(&[global_script.to_str().unwrap()], 5)],
        ..HookConfig::default()
    };

    let runner = HookRunner::real();
    let result = runner.run(
        HookPoint::WorktreeRemoved,
        &project,
        &global,
        &serde_json::json!({}),
    );

    assert!(matches!(result, Err(KetError::Conflict(_))));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "project\n",
        "the global hook ran even though the project hook already blocked"
    );
}

/// Writes an executable shell script that appends `line` to `log` and exits 0.
fn write_appender(
    sandbox: &Sandbox,
    name: &str,
    line: &str,
    log: &std::path::Path,
) -> std::path::PathBuf {
    let script = sandbox.path(name);
    std::fs::write(
        &script,
        // The log path is quoted: a sandbox directory name embeds a
        // `ThreadId(N)` for uniqueness, and unquoted parentheses are shell
        // metacharacters that would otherwise break the redirection.
        format!(
            "#!/bin/sh\ncat >/dev/null\necho {line} >> \"{}\"\n",
            log.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    script
}

/// Like [`write_appender`], but exits non-zero after logging — a project hook
/// that vetoes the operation rather than merely observing it.
fn write_blocking_appender(
    sandbox: &Sandbox,
    name: &str,
    line: &str,
    log: &std::path::Path,
) -> std::path::PathBuf {
    let script = sandbox.path(name);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat >/dev/null\necho {line} >> \"{}\"\nexit 1\n",
            log.display()
        ),
    )
    .unwrap();
    make_executable(&script);
    script
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &std::path::Path) {}

/// Keeps `ProcessHookSpawner` (used directly above) tied to the trait it
/// implements, so a refactor that quietly narrows its API is caught here too.
#[test]
fn the_real_spawner_is_a_hookspawner() {
    fn assert_impl<T: HookSpawner>() {}
    assert_impl::<ProcessHookSpawner>();
}

/// `Arc<dyn HookSpawner>` is what `HookRunner::new` actually takes; pin the
/// shape so the constructor and the trait do not drift apart silently.
#[test]
fn a_runner_can_be_built_from_a_shared_spawner() {
    let _runner = HookRunner::new(Arc::new(ProcessHookSpawner) as Arc<dyn HookSpawner>);
}
