//! Provisioning, end to end against real repositories.
//!
//! The unit tests in `provision.rs` cover the mechanism. These cover the thing
//! that actually matters: that a worktree ket hands to an agent is one the agent
//! can work in, and that it arrives fast enough not to think about.

use std::fs;
use std::path::Path;
use std::time::Instant;

use ket_core::config::{Config, DirStrategy, DirectorySpec, ProvisionConfig};
use ket_core::store::Store;
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, git};

/// A repository with a committed file, an untracked `.env`, and dependencies.
fn init_repo(path: &Path) {
    common::init_repo(path);
    fs::write(path.join(".gitignore"), b"node_modules/\n.env\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "ignore deps"]);

    // Untracked, and required for the project to run.
    fs::write(path.join(".env"), b"API_KEY=super-secret-value\n").unwrap();
    fs::create_dir_all(path.join("node_modules/left-pad")).unwrap();
    fs::write(
        path.join("node_modules/left-pad/index.js"),
        b"module.exports = () => {};\n",
    )
    .unwrap();
}

fn provision_config() -> ProvisionConfig {
    ProvisionConfig {
        directories: vec![DirectorySpec {
            path: "node_modules".to_owned(),
            strategy: DirStrategy::Clone,
        }],
        files: vec![".env".to_owned()],
        post_command: Vec::new(),
        post_timeout_secs: 60,
    }
}

fn workspace(sandbox: &Sandbox) -> Workspace {
    workspace_with(sandbox, provision_config())
}

fn workspace_with(sandbox: &Sandbox, provision: ProvisionConfig) -> Workspace {
    let config = Config {
        provision,
        ..Config::default()
    };
    Workspace::with_config(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        config,
    )
}

#[test]
fn a_provisioned_worktree_is_one_an_agent_can_actually_work_in() {
    // The acceptance test for the whole epic: dependencies and untracked config
    // are present, with no manual setup.
    let sandbox = Sandbox::new("usable");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let (worktree, report) = ws
        .create_and_provision(&project.id, "task", None, None)
        .unwrap();
    let report = report.expect("provisioning should have run");

    assert!(worktree.is_provisioned());
    assert!(
        worktree
            .path
            .join("node_modules/left-pad/index.js")
            .is_file()
    );
    assert_eq!(
        fs::read(worktree.path.join(".env")).unwrap(),
        b"API_KEY=super-secret-value\n"
    );
    // The tracked checkout is intact too.
    assert!(worktree.path.join("README.md").is_file());
    assert_eq!(report.files, vec![".env".to_owned()]);
}

#[test]
fn an_unprovisioned_worktree_is_visibly_not_ready() {
    // Starting an agent here would waste a session on a missing node_modules,
    // so the state has to be distinguishable rather than merely absent.
    let sandbox = Sandbox::new("not-ready");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let worktree = ws.create_worktree(&project.id, "bare", None, None).unwrap();
    assert!(!worktree.is_provisioned());
    assert!(!worktree.path.join("node_modules").exists());

    ws.provision_worktree(&worktree.id).unwrap();

    let refreshed = ws
        .worktrees(Some(&project.id))
        .unwrap()
        .into_iter()
        .find(|w| w.id == worktree.id)
        .unwrap();
    assert!(refreshed.is_provisioned());
}

#[test]
fn provisioning_is_idempotent_and_re_runnable() {
    let sandbox = Sandbox::new("rerun");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let (worktree, _) = ws
        .create_and_provision(&project.id, "task", None, None)
        .unwrap();

    // An agent installs something into its own worktree.
    fs::write(
        worktree.path.join("node_modules/left-pad/index.js"),
        b"locally modified",
    )
    .unwrap();

    ws.provision_worktree(&worktree.id).unwrap();

    assert_eq!(
        fs::read(worktree.path.join("node_modules/left-pad/index.js")).unwrap(),
        b"locally modified",
        "re-provisioning must not clobber the worktree's own state"
    );
}

#[test]
fn two_worktrees_get_independent_dependency_trees() {
    // Three agents on one task is the core use case; if they shared
    // node_modules, concurrent installs would corrupt it for all of them.
    let sandbox = Sandbox::new("independent");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let (a, _) = ws
        .create_and_provision(&project.id, "attempt-a", None, None)
        .unwrap();
    let (b, _) = ws
        .create_and_provision(&project.id, "attempt-b", None, None)
        .unwrap();

    fs::write(a.path.join("node_modules/left-pad/index.js"), b"agent a").unwrap();

    assert_eq!(
        fs::read(b.path.join("node_modules/left-pad/index.js")).unwrap(),
        b"module.exports = () => {};\n",
        "one agent's install leaked into another's worktree"
    );
    // And the source checkout is untouched.
    assert_eq!(
        fs::read(repo.join("node_modules/left-pad/index.js")).unwrap(),
        b"module.exports = () => {};\n"
    );
}

#[test]
fn a_repo_local_ket_toml_overrides_the_global_config() {
    let sandbox = Sandbox::new("repo-local");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    // This project does not want node_modules cloned at all.
    fs::write(
        repo.join(".ket.toml"),
        b"[provision]\ndirectories = []\nfiles = []\npost_command = []\npost_timeout_secs = 30\n",
    )
    .unwrap();

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let (worktree, report) = ws
        .create_and_provision(&project.id, "task", None, None)
        .unwrap();

    let report = report.expect("provisioning should have run");
    assert!(report.directories.is_empty());
    assert!(report.files.is_empty());
    assert!(!worktree.path.join("node_modules").exists());
    assert!(!worktree.path.join(".env").exists());
}

#[test]
fn a_failing_post_command_leaves_the_worktree_unprovisioned_but_present() {
    // The checkout may already hold work, so a failed `npm install` must not
    // destroy it — but it must not be reported as ready either.
    let sandbox = Sandbox::new("post-fail");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let mut provision = provision_config();
    provision.post_command = vec!["false".to_owned()];
    let ws = Workspace::with_config(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
        Config {
            provision,
            ..Config::default()
        },
    );

    let project = ws.add_project(&repo).unwrap();
    let (worktree, report) = ws
        .create_and_provision(&project.id, "task", None, None)
        .unwrap();

    assert!(report.is_none(), "provisioning should have failed");
    assert!(worktree.path.is_dir(), "the checkout must survive");
    assert!(!worktree.is_provisioned());
}

#[test]
fn provisioning_a_realistic_dependency_tree_stays_within_budget() {
    // A measured gate. If provisioning a worktree is slow
    // enough to think about, the parallel-worktree premise stops working.
    //
    // 20,000 files is a mid-sized node_modules. Copy-on-write should make this
    // roughly independent of file count; a regression to per-file copying shows
    // up here as a multi-second result.
    const BUDGET_MS: u128 = 2_000;
    const FILES: usize = 20_000;

    let sandbox = Sandbox::new("budget");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let modules = repo.join("node_modules");
    for i in 0..FILES {
        let dir = modules.join(format!("pkg{}", i / 50));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("f{i}.js")), vec![b'x'; 2048]).unwrap();
    }

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let started = Instant::now();
    let (worktree, report) = ws
        .create_and_provision(&project.id, "perf", None, None)
        .unwrap();
    let elapsed = started.elapsed();

    let report = report.expect("provisioning should have run");
    assert!(worktree.is_provisioned());

    if report.degraded_to_full_copy() {
        // A non-APFS or network-mounted temp dir. The budget is meaningless
        // there, so report rather than failing spuriously.
        eprintln!("copy-on-write unavailable here; skipping the timing assertion");
        return;
    }

    assert!(
        elapsed.as_millis() < BUDGET_MS,
        "provisioning {FILES} files took {}ms, over the {BUDGET_MS}ms budget \
         (provision step alone: {}ms)",
        elapsed.as_millis(),
        report.duration_ms
    );
}

// ---- teardown -------------------------------------------------------------

#[test]
fn removing_a_worktree_removes_what_provisioning_created() {
    let sandbox = Sandbox::new("teardown");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let (worktree, report) = ws
        .create_and_provision(&project.id, "torn-down", None, None)
        .unwrap();

    assert!(report.is_some(), "provisioning should have run");
    assert!(
        worktree
            .path
            .join("node_modules/left-pad/index.js")
            .is_file()
    );
    assert!(worktree.path.join(".env").is_file());

    ws.remove_worktree(&worktree.id, false, true)
        .expect("a provisioned worktree removes cleanly");

    assert!(!worktree.path.exists());
    // And the primary checkout is untouched — it is the source everything else
    // was cloned from.
    assert!(repo.join("node_modules/left-pad/index.js").is_file());
    assert!(repo.join(".env").is_file());
}

#[test]
fn a_symlinked_dependency_is_unlinked_rather_than_followed() {
    // The failure this prevents is not subtle: following the link would delete
    // the primary checkout's node_modules, which every other worktree was
    // cloned from and which nothing would rebuild.
    let sandbox = Sandbox::new("teardown-symlink");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace_with(
        &sandbox,
        ProvisionConfig {
            directories: vec![DirectorySpec {
                path: "node_modules".to_owned(),
                strategy: DirStrategy::Symlink,
            }],
            ..provision_config()
        },
    );

    let project = ws.add_project(&repo).unwrap();
    let (worktree, _) = ws
        .create_and_provision(&project.id, "linked", None, None)
        .unwrap();
    assert!(worktree.path.join("node_modules").is_symlink());

    ws.remove_worktree(&worktree.id, false, true)
        .expect("a symlink-provisioned worktree removes cleanly");

    assert!(
        repo.join("node_modules/left-pad/index.js").is_file(),
        "teardown followed a symlink out of the worktree"
    );
}

#[test]
fn a_symlinked_dependency_does_not_force_the_user_into_force() {
    // The near-universal `node_modules/` ignore rule has a trailing slash, so it
    // matches directories only. A symlink is not a directory, so git sees an
    // untracked file and refuses to remove the worktree. Without teardown
    // running first, every symlink-provisioned worktree would need `--force` —
    // the same flag that discards an agent's unreviewed work.
    let sandbox = Sandbox::new("teardown-noforce");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    assert!(
        fs::read_to_string(repo.join(".gitignore"))
            .unwrap()
            .contains("node_modules/"),
        "the fixture must use the trailing-slash form for this to mean anything"
    );

    let ws = workspace_with(
        &sandbox,
        ProvisionConfig {
            directories: vec![DirectorySpec {
                path: "node_modules".to_owned(),
                strategy: DirStrategy::Symlink,
            }],
            ..provision_config()
        },
    );

    let project = ws.add_project(&repo).unwrap();
    let (worktree, _) = ws
        .create_and_provision(&project.id, "linked", None, None)
        .unwrap();

    // Confirm the trap is real: git by itself refuses this worktree.
    let refused = common::try_git(
        &repo,
        &["worktree", "remove", &worktree.path.to_string_lossy()],
    );
    assert!(
        refused.is_err(),
        "git removed a symlink-provisioned worktree unaided; the premise has changed"
    );

    ws.remove_worktree(&worktree.id, false, false)
        .expect("ket removes it without --force");
    assert!(!worktree.path.exists());
}

#[test]
fn a_directory_the_repository_tracks_is_never_recorded_as_provisioned() {
    // `vendor/` is committed in plenty of Go projects. Provisioning leaves it
    // alone, and teardown must therefore never see it: deleting it would be
    // deleting the checkout's own files.
    let sandbox = Sandbox::new("teardown-tracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    common::commit_file(
        &repo,
        "vendor/lib/thing.go",
        "package lib\n",
        "vendor a dep",
    );

    let ws = workspace_with(
        &sandbox,
        ProvisionConfig {
            directories: vec![DirectorySpec {
                path: "vendor".to_owned(),
                strategy: DirStrategy::Clone,
            }],
            ..provision_config()
        },
    );

    let project = ws.add_project(&repo).unwrap();
    let (worktree, report) = ws
        .create_and_provision(&project.id, "tracked", None, None)
        .unwrap();
    let report = report.expect("provisioning should have run");

    assert!(
        !report.created_paths().contains(&"vendor".to_owned()),
        "a tracked directory must not be reported as created: {report:?}"
    );

    let stored = ws
        .worktrees(Some(&project.id))
        .unwrap()
        .into_iter()
        .find(|w| w.id == worktree.id)
        .unwrap();
    assert!(!stored.provisioned_paths.contains(&"vendor".to_owned()));

    // And the tracked files are still there, untouched by provisioning.
    assert!(worktree.path.join("vendor/lib/thing.go").is_file());
}

// ---- concurrency ----------------------------------------------------------

#[test]
fn two_worktrees_provision_concurrently_from_one_source() {
    // Running several agents at once is the entire point, so the first thing
    // anyone will do is create two worktrees at the same time. Both clone from
    // the same `node_modules`, and neither may see the other's writes.
    let sandbox = Sandbox::new("concurrent");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();

    let a = ws
        .create_worktree(&project.id, "agent-a", None, None)
        .unwrap();
    let b = ws
        .create_worktree(&project.id, "agent-b", None, None)
        .unwrap();

    let (ra, rb) = std::thread::scope(|scope| {
        let ta = scope.spawn(|| ws.provision_worktree(&a.id));
        let tb = scope.spawn(|| ws.provision_worktree(&b.id));
        (ta.join().unwrap(), tb.join().unwrap())
    });

    ra.expect("worktree a provisions");
    rb.expect("worktree b provisions");

    // Both are marked provisioned in the registry: neither store write was lost
    // to the other.
    for worktree in ws.worktrees(Some(&project.id)).unwrap() {
        assert!(
            worktree.is_provisioned(),
            "{} was not recorded as provisioned",
            worktree.id
        );
        assert_eq!(worktree.provisioned_paths, vec![".env", "node_modules"]);
    }

    // And the copies are independent, which is what makes two concurrent
    // installs safe.
    fs::write(a.path.join("node_modules/left-pad/index.js"), b"agent a").unwrap();
    assert_ne!(
        fs::read(b.path.join("node_modules/left-pad/index.js")).unwrap(),
        b"agent a"
    );
    assert!(repo.join("node_modules/left-pad/index.js").is_file());
}

// ---- paths that break naive string handling -------------------------------

#[test]
fn a_repository_path_containing_glob_metacharacters_still_copies_its_env() {
    // `.env` is found by globbing `<repo>/<pattern>`. A repository living under
    // a directory with brackets in its name would have those read as a character
    // class, the glob would match nothing, and the secret would silently not be
    // copied — leaving the agent with a broken environment and no error.
    let sandbox = Sandbox::new("glob-meta");
    let repo = sandbox.path("archive [2024]/api");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let (worktree, report) = ws
        .create_and_provision(&project.id, "bracketed", None, None)
        .unwrap();

    let report = report.expect("provisioning should have run");
    assert_eq!(report.files, vec![".env".to_owned()]);
    assert_eq!(
        fs::read(worktree.path.join(".env")).unwrap(),
        b"API_KEY=super-secret-value\n"
    );
}

// ---- repository automation trust -------------------------------------

#[test]
fn an_untrusted_repos_post_command_does_not_run_and_the_global_one_does_not_run_either() {
    // A repo's own `[provision]` section replaces the global config outright
    // (see `ProvisionConfig::for_repo`). Gating only the repo's command must
    // not resurrect the global one in its place: that is still a command the
    // project never opted into running inside its checkout.
    let sandbox = Sandbox::new("untrusted-post-command");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(
        repo.join(".ket.toml"),
        "[provision]\npost_command = [\"touch\", \"from-repo\"]\n",
    )
    .unwrap();

    let mut global = provision_config();
    global.post_command = vec!["touch".to_owned(), "from-global".to_owned()];
    let ws = workspace_with(&sandbox, global);

    let project = ws.add_project(&repo).unwrap();
    assert!(
        !ws.automation_trusted(&project.id).unwrap(),
        "a freshly registered project must not be trusted by default"
    );
    let (worktree, report) = ws
        .create_and_provision(&project.id, "task", None, None)
        .unwrap();

    let report = report.expect("declarative provisioning still runs");
    assert!(report.post_command_ms.is_none());
    assert!(!worktree.path.join("from-repo").exists());
    assert!(!worktree.path.join("from-global").exists());
    assert!(worktree.is_provisioned());
}

#[test]
fn trusting_the_project_lets_its_own_post_command_run() {
    let sandbox = Sandbox::new("trusted-post-command");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(
        repo.join(".ket.toml"),
        "[provision]\npost_command = [\"touch\", \"from-repo\"]\n",
    )
    .unwrap();

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.set_automation_trusted(&project.id, true).unwrap();

    let (worktree, report) = ws
        .create_and_provision(&project.id, "task", None, None)
        .unwrap();

    let report = report.expect("provisioning should have run");
    assert!(report.post_command_ms.is_some());
    assert!(worktree.path.join("from-repo").is_file());
}
