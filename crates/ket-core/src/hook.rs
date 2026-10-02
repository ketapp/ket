//! Lifecycle hooks: shell commands ket runs at fixed points, and lets veto
//! what happens next.
//!
//! A plugin system would let someone extend ket in any direction; a hook
//! covers the much smaller thing people actually ask for first — "run this
//! script when a worktree is created" — at a fraction of the design and
//! maintenance cost. Plugins stay deferred until hooks prove insufficient.
//!
//! There are six points, named after the events in [`crate::event`] they sit
//! next to:
//!
//! - `worktree.created`
//! - `worktree.provisioned`
//! - `worktree.removed`
//! - `agent.started`
//! - `agent.finished`
//! - `agent.permission_requested`
//!
//! Three properties make a hook something more than a notification:
//!
//! 1. **The exit code decides whether to proceed.** [`run`] returns
//!    [`KetError::Conflict`] the moment a hook exits non-zero, times out, or
//!    cannot be spawned at all — see "Failure policy" below. A hook that
//!    cannot affect the outcome is a log line with extra steps; this is what
//!    makes one useful instead.
//!
//! 2. **Context arrives on stdin, never in argv.** [`std::process::Command`]
//!    arguments are visible in the process table to every user on the
//!    machine (`ps -ef` on a shared box, or any process on a single-user
//!    one). A hook that wants to know which worktree fired, which branch, or
//!    which tool an agent asked to run gets that as a JSON object written to
//!    its stdin instead. Nothing this module does ever puts caller-supplied
//!    context into `command`.
//!
//! 3. **A hung hook cannot wedge a worktree.** Invariant 4 — every state is
//!    escapable — applies to a hook exactly as it applies to the
//!    post-provision command in [`crate::provision`]. Each [`HookSpec`]
//!    carries its own timeout; a hook that outruns it is killed and treated
//!    as a blocking failure, never waited on forever.
//!
//! # Precedence
//!
//! A hook point can be configured twice: once in the project's own
//! `.ket.toml` (via [`crate::config::HookConfig::for_repo`]), and once in the
//! user's global config. [`run`] runs **both** — project hooks first, then
//! global ones — rather than letting the more specific one replace the other.
//!
//! That is a deliberate departure from [`crate::config::ProvisionConfig::for_repo`],
//! which *does* let a repo-local file replace the global one wholesale. The
//! reasoning there does not transfer: merging two partial descriptions of
//! *one* resource (which directories to clone, where secrets live) produces a
//! result neither file actually describes, which is the wrong property for
//! something that decides where a `.env` ends up. A list of hooks has no such
//! hazard — each entry is an independent command, and concatenating two
//! independent lists is exactly as coherent as either list on its own. A
//! person's global "post to Slack when an agent finishes" hook should not go
//! silent just because a project also defines a `worktree.created` hook of
//! its own for something unrelated, like seeding a `.env` file.
//!
//! Project hooks run first, specifically so that a project's own gate — "run
//! the linter before the worktree is considered created" — gets to veto the
//! operation *before* a global, often side-effecting hook (a notification, an
//! audit log entry) reacts to something that, it turns out, did not actually
//! happen. The moment any hook blocks, the rest — project or global — do not
//! run at all: there is no reason to keep asking permission after the answer
//! is already no, and every additional process started is one more thing that
//! could hang.
//!
//! # Failure policy
//!
//! - Exit `0`: allowed, move on to the next configured hook.
//! - Any other exit code: [`KetError::Conflict`], naming the command, the
//!   point, and the exit code.
//! - Killed for exceeding `timeout_secs`: [`KetError::Conflict`], naming the
//!   timeout — never a hang, per invariant 4.
//! - The binary cannot be spawned at all (not on `PATH`, no permission to
//!   execute): [`KetError::Io`], the same mapping [`crate::provision`] uses
//!   for its post-provision command, because this is an environment problem
//!   rather than the hook's own decision.
//!
//! # Testability
//!
//! Real process spawning lives behind [`HookSpawner`], exactly the seam
//! [`crate::surface::Spawner`] uses for the same reason: a test that wants to
//! assert on the argv ket would run, or simulate a hook that hangs or blocks,
//! should not have to depend on a real binary existing on the machine running
//! the test.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::config::{HookConfig, HookSpec};
use crate::{KetError, Result};

/// How long to wait between polls when timing out a running hook.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long to wait for a hook's already-finished stdout to reach us over the
/// reader thread's channel.
///
/// Generous but bounded: the process is already dead (or killed) by the time
/// this is consulted, so the reader thread is only finishing a `read_to_end`
/// on a pipe with a known EOF. A wait here is not the timeout that protects
/// against a hung hook — [`ProcessHookSpawner::run`]'s own poll loop is — it
/// only guards against a reader thread that itself never gets scheduled.
const STDOUT_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// One point in a worktree's or agent's lifecycle where hooks can run.
///
/// Named after, and fired around, the corresponding [`crate::event::Event`]
/// variants. Kept as its own enum rather than a bare string so a call site
/// selects a point by value ket already knows about, instead of typing a
/// string that could drift from [`crate::config::HookConfig`]'s field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookPoint {
    /// A worktree was created.
    WorktreeCreated,
    /// A worktree finished provisioning.
    WorktreeProvisioned,
    /// A worktree was removed.
    WorktreeRemoved,
    /// An agent session started.
    AgentStarted,
    /// An agent session finished.
    AgentFinished,
    /// An agent asked to use a tool and policy did not settle it.
    AgentPermissionRequested,
}

impl HookPoint {
    /// The dotted name used in messages and sent to the hook as `point`.
    pub fn as_str(self) -> &'static str {
        match self {
            HookPoint::WorktreeCreated => "worktree.created",
            HookPoint::WorktreeProvisioned => "worktree.provisioned",
            HookPoint::WorktreeRemoved => "worktree.removed",
            HookPoint::AgentStarted => "agent.started",
            HookPoint::AgentFinished => "agent.finished",
            HookPoint::AgentPermissionRequested => "agent.permission_requested",
        }
    }

    /// The hooks configured for this point in `config`.
    fn hooks_in(self, config: &HookConfig) -> &[HookSpec] {
        match self {
            HookPoint::WorktreeCreated => &config.worktree_created,
            HookPoint::WorktreeProvisioned => &config.worktree_provisioned,
            HookPoint::WorktreeRemoved => &config.worktree_removed,
            HookPoint::AgentStarted => &config.agent_started,
            HookPoint::AgentFinished => &config.agent_finished,
            HookPoint::AgentPermissionRequested => &config.agent_permission_requested,
        }
    }
}

impl std::fmt::Display for HookPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a hook process actually did, as reported by a [`HookSpawner`].
///
/// Facts only — whether that counts as "allowed" or "blocked" is [`run`]'s
/// call, not the spawner's, so the same policy applies whether the process
/// really ran or a test faked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookExit {
    /// The process's exit code, if it exited normally. `None` means it was
    /// killed — either by [`HookSpawner`] itself for exceeding its timeout, or
    /// by a signal outside ket's control.
    pub exit_code: Option<i32>,
    /// Whether *this* run killed the process for exceeding its timeout, as
    /// opposed to `exit_code` being `None` for some other reason.
    pub timed_out: bool,
    /// Whatever the hook wrote to stdout, decoded lossily.
    ///
    /// Captured for diagnostics only — folded into a blocking error message
    /// so a person can see why without re-running the hook by hand. Never
    /// parsed as anything structured: a hook that prints garbage must not be
    /// able to crash the caller, only fail its own gate.
    pub stdout: String,
}

/// Starts a hook process and reports what it did.
///
/// This is the seam a test replaces, exactly as [`crate::surface::Spawner`]
/// is for the editor handoff: launching a real process from a unit test is
/// slow, needs a binary that may not exist on the machine running the test,
/// and is the wrong tool for asserting "the context reached stdin as JSON,
/// not as an argument" — a fake implementation can just record what it was
/// given.
pub trait HookSpawner: Send + Sync + std::fmt::Debug {
    /// Runs `command`, writing `stdin` to the process and waiting up to
    /// `timeout`.
    ///
    /// Must never block past `timeout` (invariant 4) — an implementation that
    /// launches a real process is responsible for polling and killing it
    /// itself, the way [`ProcessHookSpawner`] does.
    fn run(&self, command: &[String], stdin: &[u8], timeout: Duration) -> Result<HookExit>;
}

/// Runs a hook as a real child process, killing it if it outruns its timeout.
///
/// Stdin carries the JSON context and is closed as soon as it is written, so
/// a hook that reads to EOF before deciding never waits on more input that
/// will never come. Stdout is read on a background thread concurrently with
/// polling for exit, so a hook that writes a lot of diagnostic output cannot
/// deadlock against a full pipe buffer while this side is busy waiting.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessHookSpawner;

impl HookSpawner for ProcessHookSpawner {
    fn run(&self, command: &[String], stdin: &[u8], timeout: Duration) -> Result<HookExit> {
        let Some((program, args)) = command.split_first() else {
            // Nothing configured: vacuously allowed, matching
            // `crate::provision::run_post_command`'s treatment of an empty
            // post-provision command.
            return Ok(HookExit {
                exit_code: Some(0),
                timed_out: false,
                stdout: String::new(),
            });
        };

        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| KetError::io(program, e))?;

        // Write the context, then let `stdin` drop to close the pipe. A hook
        // blocked on reading until EOF must see one, or its own timeout would
        // not save it.
        if let Some(mut pipe) = child.stdin.take() {
            use std::io::Write;
            let _ = pipe.write_all(stdin);
        }

        let (tx, rx) = mpsc::channel();
        if let Some(mut out) = child.stdout.take() {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
        } else {
            let _ = tx.send(Vec::new());
        }

        let started = Instant::now();
        loop {
            match child.try_wait().map_err(|e| KetError::io(program, e))? {
                Some(status) => {
                    let stdout = rx.recv_timeout(STDOUT_JOIN_TIMEOUT).unwrap_or_default();
                    return Ok(HookExit {
                        exit_code: status.code(),
                        timed_out: false,
                        stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    });
                }
                None => {
                    if started.elapsed() >= timeout {
                        // A hung hook must never wedge the caller (invariant
                        // 4): kill it and report, rather than waiting longer.
                        let _ = child.kill();
                        let _ = child.wait();
                        let stdout = rx.recv_timeout(STDOUT_JOIN_TIMEOUT).unwrap_or_default();
                        return Ok(HookExit {
                            exit_code: None,
                            timed_out: true,
                            stdout: String::from_utf8_lossy(&stdout).into_owned(),
                        });
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
        }
    }
}

/// The JSON object a hook receives on stdin.
///
/// `point` is included so one script registered for several points — or
/// curious during development — can tell which one fired without ket having
/// to invoke it differently per point. `context` is whatever the call site
/// supplied; this module does not interpret it.
#[derive(Debug, Clone, serde::Serialize)]
struct HookPayload<'a> {
    point: &'static str,
    context: &'a serde_json::Value,
}

/// Runs every hook configured for `point`, project hooks before global ones,
/// stopping at the first one that does not allow the operation.
///
/// `project` and `global` are both consulted and both may contribute hooks
/// for the same point — see "Precedence" on the module docs for why neither
/// silently overrides the other. Passing [`HookConfig::default`] for
/// `project` is how a caller with no repo-local configuration says so.
///
/// `context` is serialized to JSON, wrapped with `point`, and written to each
/// hook's stdin — never appended to its argv. See "Failure policy" on the
/// module docs for exactly what each outcome maps to.
pub fn run(
    point: HookPoint,
    project: &HookConfig,
    global: &HookConfig,
    context: &serde_json::Value,
    spawner: &dyn HookSpawner,
) -> Result<()> {
    let payload = HookPayload {
        point: point.as_str(),
        context,
    };
    let stdin = serde_json::to_vec(&payload)
        .map_err(|e| KetError::Config(format!("could not encode hook context: {e}")))?;

    for spec in point.hooks_in(project).iter().chain(point.hooks_in(global)) {
        if spec.command.is_empty() {
            continue;
        }
        let exit = spawner.run(
            &spec.command,
            &stdin,
            Duration::from_secs(spec.timeout_secs),
        )?;
        settle(point, spec, exit)?;
    }

    Ok(())
}

/// Turns one hook's [`HookExit`] into the pass/fail decision [`run`] promises.
fn settle(point: HookPoint, spec: &HookSpec, exit: HookExit) -> Result<()> {
    let command = spec.command.join(" ");
    let output = exit.stdout.trim();
    let suffix = if output.is_empty() {
        String::new()
    } else {
        format!(": {output}")
    };

    if exit.timed_out {
        return Err(KetError::Conflict(format!(
            "hook `{command}` for {point} timed out after {}s and was killed{suffix}",
            spec.timeout_secs
        )));
    }

    match exit.exit_code {
        Some(0) => Ok(()),
        Some(code) => Err(KetError::Conflict(format!(
            "hook `{command}` for {point} exited {code}{suffix}"
        ))),
        None => Err(KetError::Conflict(format!(
            "hook `{command}` for {point} was terminated by a signal{suffix}"
        ))),
    }
}

/// A [`run`] with its spawner fixed, so a caller wires one up once — the real
/// [`ProcessHookSpawner`] in production, something else under test — instead
/// of threading it through every call site that fires a hook point.
#[derive(Debug, Clone)]
pub struct HookRunner {
    spawner: Arc<dyn HookSpawner>,
}

impl HookRunner {
    /// Builds a runner around any [`HookSpawner`].
    pub fn new(spawner: Arc<dyn HookSpawner>) -> Self {
        Self { spawner }
    }

    /// A runner that launches real processes.
    pub fn real() -> Self {
        Self::new(Arc::new(ProcessHookSpawner))
    }

    /// Runs the hooks configured for `point`. See [`run`] for the contract.
    pub fn run(
        &self,
        point: HookPoint,
        project: &HookConfig,
        global: &HookConfig,
        context: &serde_json::Value,
    ) -> Result<()> {
        run(point, project, global, context, self.spawner.as_ref())
    }
}

impl HookConfig {
    /// Loads this project's own hooks from `<root>/.ket.toml`, if present.
    ///
    /// Unlike [`crate::config::ProvisionConfig::for_repo`], a missing file or
    /// a missing `[hooks]` table is not "inherit the global config" — hooks
    /// from both sources run (see the module docs on [`crate::hook`] for why),
    /// so a project with none of its own simply contributes nothing here; the
    /// global hooks still run regardless.
    pub fn for_repo(root: &Path) -> Result<Self> {
        let path = root.join(".ket.toml");

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(KetError::io(&path, e)),
        };

        #[derive(serde::Deserialize)]
        struct RepoFile {
            #[serde(default)]
            hooks: HookConfig,
        }

        let parsed: RepoFile = toml::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?;

        Ok(parsed.hooks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A spawner that records every call and hands back a scripted result.
    #[derive(Debug, Default)]
    struct Recorder {
        calls: Mutex<Vec<(Vec<String>, Vec<u8>)>>,
        /// One scripted [`HookExit`] per call, consumed in order. Falls back
        /// to a plain success once exhausted.
        results: Mutex<Vec<HookExit>>,
    }

    impl Recorder {
        fn returning(results: Vec<HookExit>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                results: Mutex::new(results),
            }
        }

        fn calls(&self) -> Vec<(Vec<String>, Vec<u8>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn allowed() -> HookExit {
        HookExit {
            exit_code: Some(0),
            timed_out: false,
            stdout: String::new(),
        }
    }

    fn blocked() -> HookExit {
        HookExit {
            exit_code: Some(1),
            timed_out: false,
            stdout: String::new(),
        }
    }

    impl HookSpawner for Recorder {
        fn run(&self, command: &[String], stdin: &[u8], _timeout: Duration) -> Result<HookExit> {
            self.calls
                .lock()
                .unwrap()
                .push((command.to_vec(), stdin.to_vec()));
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                Ok(allowed())
            } else {
                Ok(results.remove(0))
            }
        }
    }

    fn spec(command: &[&str]) -> HookSpec {
        HookSpec {
            command: command.iter().map(|s| (*s).to_owned()).collect(),
            timeout_secs: 30,
        }
    }

    fn config_with(point: HookPoint, hooks: Vec<HookSpec>) -> HookConfig {
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
    fn a_hook_that_exits_zero_allows_the_operation() {
        let recorder = Recorder::returning(vec![allowed()]);
        let global = config_with(HookPoint::WorktreeCreated, vec![spec(&["true"])]);

        let result = run(
            HookPoint::WorktreeCreated,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        );

        assert!(result.is_ok());
        assert_eq!(recorder.calls().len(), 1);
    }

    #[test]
    fn a_hook_that_exits_non_zero_blocks_the_operation() {
        let recorder = Recorder::returning(vec![blocked()]);
        let global = config_with(HookPoint::WorktreeRemoved, vec![spec(&["false"])]);

        let result = run(
            HookPoint::WorktreeRemoved,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        );

        match result {
            Err(KetError::Conflict(message)) => {
                assert!(message.contains("worktree.removed"), "{message}");
                assert!(message.contains('1'), "{message}");
            }
            other => panic!("expected a blocking conflict, got {other:?}"),
        }
    }

    #[test]
    fn a_blocking_hooks_output_is_folded_into_the_error_for_diagnostics() {
        let recorder = Recorder::returning(vec![HookExit {
            exit_code: Some(2),
            timed_out: false,
            stdout: "lint failed on src/main.rs\n".to_owned(),
        }]);
        let global = config_with(HookPoint::AgentFinished, vec![spec(&["lint"])]);

        let result = run(
            HookPoint::AgentFinished,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        );

        match result {
            Err(KetError::Conflict(message)) => {
                assert!(message.contains("lint failed on src/main.rs"), "{message}");
            }
            other => panic!("expected a blocking conflict, got {other:?}"),
        }
    }

    #[test]
    fn garbage_stdout_from_an_allowed_hook_is_never_parsed_and_never_fails_the_run() {
        // The exit code is the whole contract; output is diagnostics only, and
        // must not be able to crash the caller by being invalid anything.
        let recorder = Recorder::returning(vec![HookExit {
            exit_code: Some(0),
            timed_out: false,
            stdout: "\u{0}not json at all {{{".to_owned(),
        }]);
        let global = config_with(HookPoint::AgentStarted, vec![spec(&["noisy"])]);

        let result = run(
            HookPoint::AgentStarted,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn context_reaches_the_hook_as_json_on_stdin_never_in_argv() {
        let recorder = Recorder::default();
        let global = config_with(
            HookPoint::AgentPermissionRequested,
            vec![spec(&["gate.sh"])],
        );
        let context = serde_json::json!({ "tool": "execute", "apiKey": "sk-super-secret" });

        run(
            HookPoint::AgentPermissionRequested,
            &HookConfig::default(),
            &global,
            &context,
            &recorder,
        )
        .unwrap();

        let calls = recorder.calls();
        assert_eq!(calls.len(), 1);
        let (argv, stdin) = &calls[0];

        assert_eq!(argv, &["gate.sh".to_owned()]);
        for arg in argv {
            assert!(
                !arg.contains("sk-super-secret"),
                "a secret leaked into argv: {argv:?}"
            );
        }

        let decoded: serde_json::Value = serde_json::from_slice(stdin).unwrap();
        assert_eq!(decoded["point"], "agent.permission_requested");
        assert_eq!(decoded["context"], context);
    }

    #[test]
    fn project_hooks_run_before_global_hooks_for_the_same_point() {
        let recorder = Recorder::default();
        let project = config_with(HookPoint::WorktreeCreated, vec![spec(&["project-hook"])]);
        let global = config_with(HookPoint::WorktreeCreated, vec![spec(&["global-hook"])]);

        run(
            HookPoint::WorktreeCreated,
            &project,
            &global,
            &serde_json::json!({}),
            &recorder,
        )
        .unwrap();

        let calls = recorder.calls();
        assert_eq!(calls.len(), 2, "both must run — see the precedence note");
        assert_eq!(calls[0].0, ["project-hook".to_owned()]);
        assert_eq!(calls[1].0, ["global-hook".to_owned()]);
    }

    #[test]
    fn a_blocking_project_hook_stops_the_global_hook_from_running_at_all() {
        // The chosen precedence: project first, and the first block ends the
        // whole run rather than continuing on to ask a second time.
        let recorder = Recorder::returning(vec![blocked()]);
        let project = config_with(HookPoint::WorktreeRemoved, vec![spec(&["project-gate"])]);
        let global = config_with(HookPoint::WorktreeRemoved, vec![spec(&["global-hook"])]);

        let result = run(
            HookPoint::WorktreeRemoved,
            &project,
            &global,
            &serde_json::json!({}),
            &recorder,
        );

        assert!(result.is_err());
        assert_eq!(
            recorder.calls().len(),
            1,
            "the global hook must not run once the project hook blocked"
        );
    }

    #[test]
    fn hooks_for_unrelated_points_do_not_run() {
        let recorder = Recorder::default();
        let global = config_with(HookPoint::WorktreeCreated, vec![spec(&["only-on-create"])]);

        run(
            HookPoint::WorktreeRemoved,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        )
        .unwrap();

        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn a_hook_hitting_its_configured_timeout_is_reported_as_timed_out() {
        let recorder = Recorder::returning(vec![HookExit {
            exit_code: None,
            timed_out: true,
            stdout: String::new(),
        }]);
        let global = config_with(HookPoint::AgentFinished, vec![spec(&["slow"])]);

        let result = run(
            HookPoint::AgentFinished,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        );

        match result {
            Err(KetError::Conflict(message)) => assert!(message.contains("timed out"), "{message}"),
            other => panic!("expected a timeout conflict, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_command_is_a_silent_no_op() {
        let recorder = Recorder::default();
        let global = config_with(
            HookPoint::WorktreeCreated,
            vec![HookSpec {
                command: Vec::new(),
                timeout_secs: 30,
            }],
        );

        let result = run(
            HookPoint::WorktreeCreated,
            &HookConfig::default(),
            &global,
            &serde_json::json!({}),
            &recorder,
        );

        assert!(result.is_ok());
        assert!(recorder.calls().is_empty());
    }

    #[test]
    fn every_hook_point_has_a_stable_dotted_name() {
        // Pinned deliberately: this string is both the config field mapping
        // and what a hook script sees as `point`.
        let cases = [
            (HookPoint::WorktreeCreated, "worktree.created"),
            (HookPoint::WorktreeProvisioned, "worktree.provisioned"),
            (HookPoint::WorktreeRemoved, "worktree.removed"),
            (HookPoint::AgentStarted, "agent.started"),
            (HookPoint::AgentFinished, "agent.finished"),
            (
                HookPoint::AgentPermissionRequested,
                "agent.permission_requested",
            ),
        ];
        for (point, expected) in cases {
            assert_eq!(point.as_str(), expected);
        }
    }

    #[test]
    fn a_missing_repo_local_config_contributes_no_project_hooks() {
        let dir = std::env::temp_dir().join(format!(
            "ket-hook-test-missing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = HookConfig::for_repo(&dir).unwrap();
        assert_eq!(config, HookConfig::default());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repo_local_ket_toml_contributes_its_own_hooks() {
        let dir = std::env::temp_dir().join(format!(
            "ket-hook-test-repo-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".ket.toml"),
            b"[hooks]\nworktree_created = [{ command = [\"./on-create.sh\"] }]\n",
        )
        .unwrap();

        let config = HookConfig::for_repo(&dir).unwrap();
        assert_eq!(config.worktree_created.len(), 1);
        assert_eq!(config.worktree_created[0].command, ["./on-create.sh"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_runner_wraps_run_with_a_fixed_spawner() {
        let recorder = Arc::new(Recorder::default());
        let runner = HookRunner::new(Arc::clone(&recorder) as Arc<dyn HookSpawner>);
        let global = config_with(HookPoint::AgentStarted, vec![spec(&["announce"])]);

        runner
            .run(
                HookPoint::AgentStarted,
                &HookConfig::default(),
                &global,
                &serde_json::json!({ "agent": "claude" }),
            )
            .unwrap();

        assert_eq!(recorder.calls().len(), 1);
    }
}
