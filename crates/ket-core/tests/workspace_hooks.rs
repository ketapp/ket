//! Lifecycle hooks wired into the operations they gate.
//!
//! `hook.rs`'s own unit tests cover the mechanism in isolation — precedence,
//! the exit-code contract, secrets never reaching argv — against a fake
//! spawner, and `tests/hook.rs` covers the same mechanism against real
//! processes. Neither ever calls a [`Workspace`] method: they exercise
//! [`ket_core::hook::run`] directly with a `HookConfig` built by hand.
//!
//! What is worth checking here is the wiring itself: that each call site
//! actually reads the project's `.ket.toml` and the global config, that a
//! blocking hook stops the operation and leaves nothing half-done, that a
//! passing hook is invisible to the caller, and that the JSON context each
//! call site sends actually describes what just happened.

mod common;

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Sandbox, init_repo};
use ket_core::KetError;
use ket_core::config::{AgentSpec, Config, HookConfig, HookSpec, Transport};
use ket_core::event::{Envelope, Event, SessionOutcome};
use ket_core::hook::{HookExit, HookRunner, HookSpawner};
use ket_core::store::Store;
use ket_core::workspace::Workspace;

/// A spawner that records every hook it is asked to run, and fails (exits
/// `1`) exactly the programs named via [`FakeHooks::block`].
///
/// Keyed on the program name a test configured for a particular hook point,
/// rather than on the point itself: a call site is what decides which point
/// fires and with what command, so asserting against the command actually
/// spawned is the more end-to-end check.
#[derive(Debug, Default)]
struct FakeHooks {
    calls: Mutex<Vec<(Vec<String>, Vec<u8>)>>,
    blocked: Mutex<HashSet<String>>,
}

impl FakeHooks {
    fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Every future call naming `program` as its command fails the operation
    /// it gates.
    fn block(&self, program: &str) {
        self.blocked.lock().unwrap().insert(program.to_owned());
    }

    fn calls(&self) -> Vec<(Vec<String>, Vec<u8>)> {
        self.calls.lock().unwrap().clone()
    }

    /// The decoded `{"point": ..., "context": ...}` payload sent to every
    /// call naming `program`, in call order.
    fn contexts_for(&self, program: &str) -> Vec<serde_json::Value> {
        self.calls()
            .into_iter()
            .filter(|(command, _)| command.first().map(String::as_str) == Some(program))
            .map(|(_, stdin)| serde_json::from_slice(&stdin).expect("hook stdin is valid JSON"))
            .collect()
    }
}

impl HookSpawner for FakeHooks {
    fn run(
        &self,
        command: &[String],
        stdin: &[u8],
        _timeout: Duration,
    ) -> ket_core::Result<HookExit> {
        self.calls
            .lock()
            .unwrap()
            .push((command.to_vec(), stdin.to_vec()));

        let blocked = command
            .first()
            .is_some_and(|program| self.blocked.lock().unwrap().contains(program));

        Ok(HookExit {
            exit_code: Some(if blocked { 1 } else { 0 }),
            timed_out: false,
            stdout: String::new(),
        })
    }
}

/// A hook spec naming `program` as its whole command.
fn hook(program: &str) -> HookSpec {
    HookSpec {
        command: vec![program.to_owned()],
        timeout_secs: 5,
    }
}

/// Config with `hooks` set and nothing to provision, so `create_and_provision`
/// stays instant — matching `ket-core/tests/agent.rs`'s own fixture.
fn config_with_hooks(hooks: HookConfig) -> Config {
    let mut config = Config {
        hooks,
        ..Config::default()
    };
    config.provision.directories.clear();
    config.provision.files.clear();
    config
}

/// A workspace whose hooks spawn through `spawner` instead of real processes.
fn workspace(sandbox: &Sandbox, config: Config, spawner: &Arc<FakeHooks>) -> Workspace {
    Workspace::with_hooks(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        config,
        HookRunner::new(Arc::clone(spawner) as Arc<dyn HookSpawner>),
    )
}

/// An "agent" that reads the prompt and answers, then exits — the same
/// fixture `tests/agent.rs` uses, so a real `sh` process is enough to reach
/// `agent.started` and `agent.finished` without needing a real model.
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

/// Drains everything published so far.
fn drain(events: &mut tokio::sync::broadcast::Receiver<Envelope>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        out.push(envelope.event);
    }
    out
}

fn expect_conflict_naming(err: &KetError, needle: &str) {
    match err {
        KetError::Conflict(message) => {
            assert!(
                message.contains(needle),
                "{message:?} does not name {needle}"
            );
        }
        other => panic!("expected a conflict naming {needle}, got {other:?}"),
    }
}

// ---- worktree.created -------------------------------------------------

#[test]
fn a_blocking_worktree_created_hook_leaves_no_worktree_behind() {
    let sandbox = Sandbox::new("hook-create-block");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    spawner.block("gate-create");
    let config = config_with_hooks(HookConfig {
        worktree_created: vec![hook("gate-create")],
        ..HookConfig::default()
    });
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let err = ws
        .create_worktree(&project.id, "task", None, None)
        .expect_err("a blocking hook must fail worktree creation");
    expect_conflict_naming(&err, "worktree.created");
    expect_conflict_naming(&err, "gate-create");

    assert!(
        ws.worktrees(None).unwrap().is_empty(),
        "no worktree record may survive a blocked worktree.created hook"
    );

    let branches = common::git(&repo, &["branch", "--list", "task"]);
    assert!(
        branches.trim().is_empty(),
        "the branch survived a blocked hook: {branches:?}"
    );

    let listed = common::git(&repo, &["worktree", "list", "--porcelain"]);
    assert!(
        !listed.contains("task"),
        "a worktree checkout survived a blocked hook: {listed}"
    );
}

#[test]
fn a_passing_worktree_created_hook_lets_creation_proceed_and_sees_the_right_context() {
    let sandbox = Sandbox::new("hook-create-pass");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let config = config_with_hooks(HookConfig {
        worktree_created: vec![hook("announce-create")],
        ..HookConfig::default()
    });
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "task", None, None)
        .expect("a passing hook must not block creation");

    assert!(worktree.path.is_dir());

    let contexts = spawner.contexts_for("announce-create");
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["point"], "worktree.created");
    assert_eq!(
        contexts[0]["context"]["worktreeId"].as_str(),
        Some(worktree.id.as_str())
    );
    assert_eq!(
        contexts[0]["context"]["projectId"].as_str(),
        Some(project.id.as_str())
    );
    assert_eq!(contexts[0]["context"]["branch"].as_str(), Some("task"));
}

#[test]
fn a_projects_own_ket_toml_hook_is_consulted_by_worktree_created() {
    // The wiring question the mechanism's own tests cannot ask: does the call
    // site actually load `HookConfig::for_repo`, or only the global config?
    let sandbox = Sandbox::new("hook-create-repo-local");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    std::fs::write(
        repo.join(".ket.toml"),
        "[hooks]\nworktree_created = [{ command = [\"gate-repo-local\"] }]\n",
    )
    .unwrap();

    let spawner = FakeHooks::new();
    spawner.block("gate-repo-local");
    let ws = workspace(&sandbox, config_with_hooks(HookConfig::default()), &spawner);

    let project = ws.add_project(&repo).expect("add project");
    // A committed `.ket.toml` is code the repo ships, not something adding
    // the project consents to run — see `automation_trusted_...` below.
    ws.set_automation_trusted(&project.id, true).unwrap();
    let err = ws
        .create_worktree(&project.id, "task", None, None)
        .expect_err("the repo's own .ket.toml hook must be consulted");
    expect_conflict_naming(&err, "gate-repo-local");

    assert!(ws.worktrees(None).unwrap().is_empty());
}

#[test]
fn an_untrusted_projects_ket_toml_hook_is_not_run() {
    // The other half of the wiring question above: a project added but never
    // explicitly trusted must not execute a cloned repo's committed hooks.
    let sandbox = Sandbox::new("hook-create-untrusted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    std::fs::write(
        repo.join(".ket.toml"),
        "[hooks]\nworktree_created = [{ command = [\"gate-repo-local\"] }]\n",
    )
    .unwrap();

    let spawner = FakeHooks::new();
    spawner.block("gate-repo-local");
    let ws = workspace(&sandbox, config_with_hooks(HookConfig::default()), &spawner);

    let project = ws.add_project(&repo).expect("add project");
    assert!(
        !ws.automation_trusted(&project.id).unwrap(),
        "a freshly registered project must not be trusted by default"
    );

    ws.create_worktree(&project.id, "task", None, None)
        .expect("an untrusted repo's own hook must not block creation");

    assert!(spawner.contexts_for("gate-repo-local").is_empty());
}

// ---- worktree.provisioned ----------------------------------------------

#[test]
fn a_blocking_worktree_provisioned_hook_leaves_the_worktree_present_but_unprovisioned() {
    let sandbox = Sandbox::new("hook-provision-block");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let config = config_with_hooks(HookConfig {
        worktree_provisioned: vec![hook("gate-provision")],
        ..HookConfig::default()
    });
    let ws = workspace(&sandbox, config, &spawner);
    spawner.block("gate-provision");

    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "task", None, None)
        .expect("create worktree");

    let err = ws
        .provision_worktree(&worktree.id)
        .expect_err("a blocking hook must fail provisioning");
    expect_conflict_naming(&err, "worktree.provisioned");

    let refreshed = ws
        .worktrees(None)
        .unwrap()
        .into_iter()
        .find(|w| w.id == worktree.id)
        .expect("the worktree record itself must survive");
    assert!(
        !refreshed.is_provisioned(),
        "a blocked provisioning hook must not mark the worktree ready"
    );
    assert!(
        worktree.path.is_dir(),
        "the checkout itself must survive a blocked provisioning hook"
    );
}

#[test]
fn a_passing_worktree_provisioned_hook_lets_provisioning_proceed() {
    let sandbox = Sandbox::new("hook-provision-pass");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let config = config_with_hooks(HookConfig {
        worktree_provisioned: vec![hook("announce-provision")],
        ..HookConfig::default()
    });
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "task", None, None)
        .expect("create worktree");

    ws.provision_worktree(&worktree.id)
        .expect("a passing hook must not block provisioning");

    let refreshed = ws
        .worktrees(None)
        .unwrap()
        .into_iter()
        .find(|w| w.id == worktree.id)
        .unwrap();
    assert!(refreshed.is_provisioned());

    let contexts = spawner.contexts_for("announce-provision");
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["point"], "worktree.provisioned");
    assert_eq!(
        contexts[0]["context"]["worktreeId"].as_str(),
        Some(worktree.id.as_str())
    );
}

// ---- worktree.removed ---------------------------------------------------

#[test]
fn a_blocking_worktree_removed_hook_leaves_the_worktree_fully_intact() {
    let sandbox = Sandbox::new("hook-remove-block");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let config = config_with_hooks(HookConfig {
        worktree_removed: vec![hook("gate-remove")],
        ..HookConfig::default()
    });
    let ws = workspace(&sandbox, config, &spawner);
    spawner.block("gate-remove");

    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "task", None, None)
        .expect("create worktree");

    let err = ws
        .remove_worktree(&worktree.id, false, true)
        .expect_err("a blocking hook must fail removal");
    expect_conflict_naming(&err, "worktree.removed");

    assert!(
        worktree.path.is_dir(),
        "the checkout must survive a blocked removal hook"
    );
    assert_eq!(ws.worktrees(None).unwrap().len(), 1);

    let branches = common::git(&repo, &["branch", "--list", "task"]);
    assert!(!branches.trim().is_empty(), "the branch must survive too");
}

#[test]
fn a_passing_worktree_removed_hook_lets_removal_proceed() {
    let sandbox = Sandbox::new("hook-remove-pass");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let config = config_with_hooks(HookConfig {
        worktree_removed: vec![hook("announce-remove")],
        ..HookConfig::default()
    });
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "task", None, None)
        .expect("create worktree");

    ws.remove_worktree(&worktree.id, false, true)
        .expect("a passing hook must not block removal");

    assert!(!worktree.path.exists());
    assert!(ws.worktrees(None).unwrap().is_empty());

    let contexts = spawner.contexts_for("announce-remove");
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["point"], "worktree.removed");
    assert_eq!(
        contexts[0]["context"]["worktreeId"].as_str(),
        Some(worktree.id.as_str())
    );
    assert_eq!(contexts[0]["context"]["branch"].as_str(), Some("task"));
}

// ---- agent.started / agent.finished ------------------------------------

#[tokio::test]
async fn a_blocking_agent_started_hook_leaves_no_session_behind() {
    let sandbox = Sandbox::new("hook-agent-start-block");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    spawner.block("gate-start");
    let mut config = config_with_hooks(HookConfig {
        agent_started: vec![hook("gate-start")],
        ..HookConfig::default()
    });
    config.agents = vec![echo_agent()];
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let (worktree, _) = ws
        .create_and_provision(&project.id, "work", None, None)
        .expect("create and provision");

    let mut events = ws.bus().subscribe();
    let err = ws
        .run_agent(&worktree.id, Some("echo"), "hello")
        .await
        .expect_err("a blocking hook must stop the agent from starting");
    expect_conflict_naming(&err, "agent.started");

    assert!(
        ws.sessions(None).unwrap().is_empty(),
        "no session record may survive a blocked agent.started hook"
    );

    let published = drain(&mut events);
    assert!(
        published.is_empty(),
        "nothing should have been announced: {published:?}"
    );
}

#[tokio::test]
async fn a_passing_agent_started_hook_lets_the_session_run() {
    let sandbox = Sandbox::new("hook-agent-start-pass");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let mut config = config_with_hooks(HookConfig {
        agent_started: vec![hook("announce-start")],
        ..HookConfig::default()
    });
    config.agents = vec![echo_agent()];
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let (worktree, _) = ws
        .create_and_provision(&project.id, "work", None, None)
        .expect("create and provision");

    let outcome = ws
        .run_agent(&worktree.id, Some("echo"), "hello there")
        .await
        .expect("a passing hook must not block the session");
    assert_eq!(outcome, SessionOutcome::Completed);

    let contexts = spawner.contexts_for("announce-start");
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["point"], "agent.started");
    assert_eq!(contexts[0]["context"]["agent"].as_str(), Some("echo"));
    assert_eq!(
        contexts[0]["context"]["worktreeId"].as_str(),
        Some(worktree.id.as_str())
    );
}

#[tokio::test]
async fn a_blocking_agent_finished_hook_still_leaves_the_session_recorded_as_ended() {
    // `agent.finished` cannot undo an agent's turn — it already happened, in
    // the worktree, whatever the outcome. "Blocks" here means the hook's
    // failure is surfaced to the caller of `run_agent`, not that the session
    // is left dangling: invariant 4 requires it end regardless.
    let sandbox = Sandbox::new("hook-agent-finish-block");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    spawner.block("gate-finish");
    let mut config = config_with_hooks(HookConfig {
        agent_finished: vec![hook("gate-finish")],
        ..HookConfig::default()
    });
    config.agents = vec![echo_agent()];
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let (worktree, _) = ws
        .create_and_provision(&project.id, "work", None, None)
        .expect("create and provision");

    let err = ws
        .run_agent(&worktree.id, Some("echo"), "hello there")
        .await
        .expect_err("a blocking agent.finished hook must surface as an error");
    expect_conflict_naming(&err, "agent.finished");

    let sessions = ws.sessions(None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(
        sessions[0].status.is_ended(),
        "the session must not be left Live just because its finishing hook failed"
    );
}

#[tokio::test]
async fn a_passing_agent_finished_hook_sees_the_real_outcome() {
    let sandbox = Sandbox::new("hook-agent-finish-pass");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let spawner = FakeHooks::new();
    let mut config = config_with_hooks(HookConfig {
        agent_finished: vec![hook("announce-finish")],
        ..HookConfig::default()
    });
    config.agents = vec![echo_agent()];
    let ws = workspace(&sandbox, config, &spawner);

    let project = ws.add_project(&repo).expect("add project");
    let (worktree, _) = ws
        .create_and_provision(&project.id, "work", None, None)
        .expect("create and provision");

    let outcome = ws
        .run_agent(&worktree.id, Some("echo"), "hello there")
        .await
        .expect("a passing hook must not block finishing");
    assert_eq!(outcome, SessionOutcome::Completed);

    let contexts = spawner.contexts_for("announce-finish");
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["point"], "agent.finished");
    assert_eq!(
        contexts[0]["context"]["outcome"]["kind"].as_str(),
        Some("completed")
    );
}
