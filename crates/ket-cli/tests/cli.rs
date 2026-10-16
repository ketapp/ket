//! The `ket` binary, end to end.
//!
//! The unit tests beside `main.rs` prove every registry command has an argv
//! and every argv a command; none of them runs a handler. These do: each test
//! builds a throwaway repository, runs the real binary against it, and reads
//! what it printed and what it left on disk.
//!
//! Every run is sandboxed. `XDG_DATA_HOME`, `XDG_CONFIG_HOME`,
//! `XDG_CACHE_HOME` and `HOME` all point inside the test's own directory, so
//! nothing here can touch the state of whoever runs the suite, and `KET_HOST=0`
//! keeps any path that might reach for a host from starting one. Worktrees are
//! registered in the fixture repository's own `.git`, which goes with the
//! sandbox.
//!
//! No test here contacts a host, an agent or GitHub: `ket host`, `ket run` and
//! a `ket pr` that gets as far as `gh` are left to the core tests that can fake
//! them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway directory tree with its own data, config, cache and home, and
/// a repository with one commit on `main`. Removed when the test ends.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "ket-cli-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // macOS puts the temp dir behind /private; canonicalise so paths
        // compare equal to the ones git and ket print.
        let root = root.canonicalize().unwrap();
        for dir in ["data", "config", "cache", "home"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let sandbox = Self { root };
        init_repo(&sandbox.repo());
        sandbox
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn repo(&self) -> PathBuf {
        self.path("repo")
    }

    /// Runs `ket args…` from `cwd`.
    fn ket_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_ket"))
            .args(args)
            .current_dir(cwd)
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("HOME", self.path("home"))
            .env("KET_HOST", "0")
            .env_remove("KET_LOG")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .output()
            .expect("run ket");
        Run(output)
    }

    /// Runs `ket args…` from the sandbox root, outside any repository.
    fn ket(&self, args: &[&str]) -> Run {
        self.ket_in(&self.root, args)
    }

    /// Registers the fixture repository and returns its project id.
    fn add_project(&self) -> String {
        let repo = self.repo();
        let run = self.ket(&["project", "add", repo.to_str().unwrap()]).ok();
        run.first_word()
    }

    /// Creates a worktree on `branch` without provisioning, returning its id
    /// and path.
    fn add_worktree(&self, project: &str, branch: &str) -> (String, PathBuf) {
        let run = self
            .ket_in(&self.repo(), &["worktree", "add", branch, "--no-provision"])
            .ok();
        let line = run.stdout();
        let mut words = line.split_whitespace();
        let id = words.next().expect("an id").to_owned();
        let path = PathBuf::from(words.next().expect("a path"));
        assert!(
            id.contains(branch),
            "worktree {id} is for {branch} in {project}"
        );
        (id, path)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// What one run of `ket` did.
struct Run(Output);

impl Run {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    fn code(&self) -> Option<i32> {
        self.0.status.code()
    }

    /// Asserts success and returns itself.
    #[track_caller]
    fn ok(self) -> Self {
        assert!(
            self.0.status.success(),
            "ket failed ({:?})\nstdout:\n{}\nstderr:\n{}",
            self.0.status,
            self.stdout(),
            self.stderr()
        );
        self
    }

    /// Asserts failure, that the error is reported as `ket: …`, and returns
    /// the message.
    #[track_caller]
    fn err(self) -> String {
        assert!(
            !self.0.status.success(),
            "ket should have failed\nstdout:\n{}",
            self.stdout()
        );
        let stderr = self.stderr();
        assert!(stderr.contains("ket: "), "errors are prefixed: {stderr}");
        stderr
    }

    fn first_word(&self) -> String {
        self.stdout()
            .split_whitespace()
            .next()
            .expect("output")
            .to_owned()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "--initial-branch=main", "--quiet"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "ket test"]);
    fs::write(path.join("README.md"), "# fixture\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "--quiet", "-m", "initial"]);
}

fn commit_file(repo: &Path, rel: &str, contents: &str, message: &str) {
    fs::write(repo.join(rel), contents).unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", message]);
}

// ---- projects ---------------------------------------------------------------

#[test]
fn a_project_is_added_listed_configured_and_removed() {
    let sandbox = Sandbox::new("project");
    let empty = sandbox.ket(&["project", "list"]).ok();
    assert!(empty.stdout().is_empty());
    assert!(empty.stderr().contains("no projects registered"));

    let project = sandbox.add_project();
    assert!(project.starts_with("repo"), "{project}");

    let listed = sandbox.ket(&["project", "list"]).ok().stdout();
    assert!(listed.contains(&project), "{listed}");
    assert!(
        listed.contains(sandbox.repo().to_str().unwrap()),
        "{listed}"
    );
    // Available, with no worktrees yet.
    assert!(listed.starts_with(' '), "{listed}");

    let set = sandbox
        .ket(&[
            "project",
            "set",
            &project,
            "--name",
            "Fixture",
            "--color",
            "#eab308",
            "--discovered",
            "hide",
            "--automation",
            "trust",
        ])
        .ok()
        .stdout();
    for expected in [
        "name=Fixture",
        "color=#eab308",
        "base=main",
        "discovered=hide",
        "automation=trust",
    ] {
        assert!(set.contains(expected), "{expected} in {set}");
    }

    let removed = sandbox.ket(&["project", "rm", &project]).ok().stdout();
    assert_eq!(removed.trim(), format!("removed {project}"));
    assert!(
        sandbox.ket(&["project", "list"]).ok().stdout().is_empty(),
        "the project is gone"
    );
}

#[test]
fn a_project_setting_out_of_range_is_refused() {
    let sandbox = Sandbox::new("project-bad-base");
    let project = sandbox.add_project();
    let err = sandbox
        .ket(&["project", "set", &project, "--base", "no-such-branch"])
        .err();
    assert!(err.contains("no-such-branch"), "{err}");
}

#[test]
fn a_directory_that_is_not_a_repository_is_not_a_project() {
    let sandbox = Sandbox::new("project-not-repo");
    let plain = sandbox.path("plain");
    fs::create_dir_all(&plain).unwrap();
    sandbox
        .ket(&["project", "add", plain.to_str().unwrap()])
        .err();
}

#[test]
fn the_state_lands_in_the_sandboxed_data_directory() {
    let sandbox = Sandbox::new("project-state");
    sandbox.add_project();
    let state = sandbox.path("data/ket/state.json");
    assert!(state.is_file(), "state is written under XDG_DATA_HOME");
    let text = fs::read_to_string(state).unwrap();
    assert!(text.contains(sandbox.repo().to_str().unwrap()));
}

// ---- worktrees --------------------------------------------------------------

#[test]
fn a_worktree_is_created_listed_opened_closed_and_removed() {
    let sandbox = Sandbox::new("worktree");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "fix-it");
    assert!(path.is_dir());
    assert_eq!(git(&path, &["branch", "--show-current"]).trim(), "fix-it");

    // Not provisioned: listed with `~`, and the project counts it.
    let listed = sandbox.ket(&["worktree", "list"]).ok().stdout();
    let row = listed.lines().find(|l| l.contains(&id)).expect("listed");
    assert!(row.starts_with('~'), "{row}");
    assert!(row.contains("fix-it"), "{row}");
    let projects = sandbox.ket(&["project", "list"]).ok().stdout();
    assert!(projects.contains(" 1 "), "{projects}");

    let opened = sandbox.ket(&["worktree", "open", &id]).ok().stdout();
    assert_eq!(opened.trim(), format!("opened {id}"));
    let listed = sandbox.ket(&["worktree", "list"]).ok().stdout();
    let row = listed.lines().find(|l| l.contains(&id)).unwrap();
    assert_eq!(&row[1..2], "*", "the open worktree has focus: {row}");

    let closed = sandbox.ket(&["worktree", "close", &id]).ok().stdout();
    assert_eq!(closed.trim(), format!("closed {id}"));

    let removed = sandbox
        .ket(&["worktree", "rm", &id, "--force", "--delete-branch"])
        .ok()
        .stdout();
    assert_eq!(removed.trim(), format!("removed {id}"));
    let listed = sandbox.ket(&["worktree", "list"]);
    assert!(listed.stderr().contains("no worktrees"));
    assert!(
        !git(&sandbox.repo(), &["branch", "--list", "fix-it"]).contains("fix-it"),
        "the branch was deleted"
    );
}

#[test]
fn provisioning_a_worktree_reports_how_long_it_took() {
    let sandbox = Sandbox::new("worktree-provision");
    sandbox.add_project();
    let created = sandbox
        .ket_in(&sandbox.repo(), &["worktree", "add", "provisioned"])
        .ok()
        .stdout();
    assert!(created.contains("provisioned in "), "{created}");
    let id = created.split_whitespace().next().unwrap().to_owned();

    // Ready, so listed without a mark.
    let listed = sandbox.ket(&["worktree", "list"]).ok().stdout();
    let row = listed.lines().find(|l| l.contains(&id)).unwrap();
    assert!(row.starts_with(' '), "{row}");

    let again = sandbox.ket(&["worktree", "provision", &id]).ok().stdout();
    assert!(again.contains("provisioned in "), "{again}");
}

#[test]
fn status_and_diff_describe_what_changed_against_the_base() {
    let sandbox = Sandbox::new("worktree-diff");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "change");

    let clean = sandbox.ket(&["diff", &id]).ok();
    assert!(clean.stdout().is_empty());
    assert!(clean.stderr().contains("no changes against"));

    fs::write(path.join("README.md"), "# fixture\nmore\n").unwrap();
    fs::write(path.join("new.txt"), "a\nb\n").unwrap();

    let status = sandbox.ket(&["worktree", "status", &id]).ok().stdout();
    assert!(status.contains("M worktree README.md"), "{status}");
    assert!(status.contains("? worktree new.txt"), "{status}");
    assert!(
        status.contains("2 change(s); 0 ahead, 0 behind"),
        "{status}"
    );

    let unified = sandbox.ket(&["diff", &id]).ok().stdout();
    assert!(unified.contains("+more"), "{unified}");
    assert!(unified.contains("new.txt"), "{unified}");

    let stat = sandbox.ket(&["diff", &id, "--stat"]).ok().stdout();
    assert!(stat.contains("README.md"), "{stat}");
    assert!(stat.contains("2 file(s), +3 -0"), "{stat}");

    // From inside the worktree, the worktree is implied.
    let implied = sandbox.ket_in(&path, &["diff", "--stat"]).ok().stdout();
    assert_eq!(implied, stat);
}

#[test]
fn merge_commits_the_work_and_lands_it_in_the_base() {
    let sandbox = Sandbox::new("worktree-merge");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "landing");
    fs::write(path.join("work.txt"), "done\n").unwrap();

    let merged = sandbox.ket(&["merge", &id]).ok().stdout();
    assert!(
        merged.contains(&format!("committed 1 file(s) in {id}")),
        "{merged}"
    );
    assert!(merged.contains("merged landing into main"), "{merged}");
    assert!(sandbox.repo().join("work.txt").is_file());

    let again = sandbox.ket(&["merge", &id]).ok().stdout();
    assert!(
        again.contains(&format!("nothing to commit in {id}")),
        "{again}"
    );
    assert!(again.contains("landing was already in main"), "{again}");
}

#[test]
fn merge_refuses_a_primary_checkout_on_another_branch() {
    let sandbox = Sandbox::new("worktree-merge-wrong-branch");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "landing");
    fs::write(path.join("work.txt"), "done\n").unwrap();
    git(&sandbox.repo(), &["switch", "--quiet", "-c", "elsewhere"]);

    let err = sandbox.ket(&["merge", &id]).err();
    assert!(err.contains("check out `main`"), "{err}");
}

#[test]
fn merge_into_a_dirty_primary_checkout_needs_force() {
    let sandbox = Sandbox::new("worktree-merge-dirty");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "landing");
    fs::write(path.join("work.txt"), "done\n").unwrap();
    fs::write(sandbox.repo().join("scratch.txt"), "mine\n").unwrap();

    let err = sandbox.ket(&["merge", &id]).err();
    assert!(err.contains("Pass --force to merge anyway"), "{err}");

    let forced = sandbox.ket(&["merge", &id, "--force"]).ok().stdout();
    assert!(forced.contains("merged landing into main"), "{forced}");
    assert!(sandbox.repo().join("scratch.txt").is_file(), "left alone");
}

#[test]
fn collapse_keeps_one_attempt_and_discards_the_rest() {
    let sandbox = Sandbox::new("worktree-collapse");
    let project = sandbox.add_project();
    let (winner, winner_path) = sandbox.add_worktree(&project, "attempt-a");
    let (loser, loser_path) = sandbox.add_worktree(&project, "attempt-b");
    commit_file(&winner_path, "a.txt", "a\n", "a");
    commit_file(&loser_path, "b.txt", "b\n", "b");

    let collapsed = sandbox.ket(&["collapse", &winner]).ok().stdout();
    assert!(
        collapsed.contains("merged attempt-a into main"),
        "{collapsed}"
    );
    assert!(
        collapsed.contains(&format!("discarded {loser}")),
        "{collapsed}"
    );
    assert!(sandbox.repo().join("a.txt").is_file());
    assert!(!sandbox.repo().join("b.txt").exists());
    assert!(!loser_path.exists());
}

#[test]
fn prune_forgets_a_worktree_whose_directory_is_gone() {
    let sandbox = Sandbox::new("worktree-prune");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "vanished");

    let nothing = sandbox.ket(&["worktree", "prune"]).ok();
    assert!(nothing.stderr().contains("nothing to prune"));

    fs::remove_dir_all(&path).unwrap();
    let listed = sandbox.ket(&["worktree", "list"]).ok().stdout();
    let row = listed.lines().find(|l| l.contains(&id)).unwrap();
    assert!(row.starts_with('!'), "missing on disk: {row}");

    let pruned = sandbox.ket(&["worktree", "prune"]).ok().stdout();
    assert_eq!(pruned.trim(), format!("pruned {id}"));
    git(&sandbox.repo(), &["worktree", "prune"]);
}

#[test]
fn an_unknown_worktree_is_an_error_not_a_silent_success() {
    let sandbox = Sandbox::new("worktree-unknown");
    sandbox.add_project();
    for args in [
        &["worktree", "status", "nope"][..],
        &["diff", "nope"],
        &["merge", "nope"],
        &["worktree", "rm", "nope"],
    ] {
        let err = sandbox.ket(args).err();
        assert!(err.contains("nope"), "{args:?}: {err}");
    }
}

#[test]
fn pr_with_no_remote_is_refused_before_anything_is_committed() {
    let sandbox = Sandbox::new("worktree-pr-no-remote");
    let project = sandbox.add_project();
    let (id, path) = sandbox.add_worktree(&project, "142-fix-the-login");
    fs::write(path.join("work.txt"), "done\n").unwrap();

    let err = sandbox.ket(&["pr", &id]).err();
    assert!(err.contains("no remote to push to"), "{err}");
    assert!(
        git(&path, &["status", "--porcelain"]).contains("work.txt"),
        "the work is still uncommitted"
    );
}

// ---- backlog ----------------------------------------------------------------

#[test]
fn a_backlog_note_goes_through_its_whole_life() {
    let sandbox = Sandbox::new("backlog");
    sandbox.add_project();
    let repo = sandbox.repo();

    let empty = sandbox.ket_in(&repo, &["backlog", "list"]).ok();
    assert!(empty.stderr().contains("no backlog notes"));

    let first = sandbox
        .ket_in(
            &repo,
            &[
                "backlog",
                "add",
                "Fix the login",
                "--body",
                "it loops",
                "--tag",
                "ui",
            ],
        )
        .ok();
    let first_id = first.first_word();
    assert!(
        first.stdout().contains("Fix the login  #ui"),
        "{}",
        first.stdout()
    );
    let second_id = sandbox
        .ket_in(&repo, &["backlog", "add", "Write the docs"])
        .ok()
        .first_word();

    let listed = sandbox.ket_in(&repo, &["backlog", "list"]).ok().stdout();
    assert!(listed.contains("Fix the login"), "{listed}");
    assert!(listed.contains("Write the docs"), "{listed}");

    let tagged = sandbox
        .ket_in(&repo, &["backlog", "tag", &second_id, "docs"])
        .ok()
        .stdout();
    assert_eq!(tagged.trim(), format!("{second_id}  #docs"));
    let by_tag = sandbox
        .ket_in(&repo, &["backlog", "list", "--tag", "docs"])
        .ok()
        .stdout();
    assert!(by_tag.contains("Write the docs") && !by_tag.contains("Fix the login"));
    let untagged = sandbox
        .ket_in(&repo, &["backlog", "tag", &second_id, "docs", "--remove"])
        .ok()
        .stdout();
    assert_eq!(untagged.trim(), format!("{second_id}  no tags"));

    let moved = sandbox
        .ket_in(
            &repo,
            &["backlog", "move", &second_id, "--before", &first_id],
        )
        .ok()
        .stdout();
    assert!(moved.contains(&format!("moved {second_id} before {first_id}")));
    let order = sandbox.ket_in(&repo, &["backlog", "list"]).ok().stdout();
    assert!(
        order.find("Write the docs").unwrap() < order.find("Fix the login").unwrap(),
        "{order}"
    );

    let done = sandbox
        .ket_in(&repo, &["backlog", "done", &first_id])
        .ok()
        .stdout();
    assert_eq!(done.trim(), format!("done {first_id}"));
    let open = sandbox.ket_in(&repo, &["backlog", "list"]).ok().stdout();
    assert!(
        !open.contains("Fix the login"),
        "done notes are hidden: {open}"
    );
    let all = sandbox
        .ket_in(&repo, &["backlog", "list", "--all"])
        .ok()
        .stdout();
    let row = all.lines().find(|l| l.contains("Fix the login")).unwrap();
    assert!(row.starts_with('x'), "{row}");

    sandbox
        .ket_in(&repo, &["backlog", "reopen", &first_id])
        .ok();
    assert!(
        sandbox
            .ket_in(&repo, &["backlog", "list"])
            .ok()
            .stdout()
            .contains("Fix the login")
    );

    let removed = sandbox
        .ket_in(&repo, &["backlog", "rm", &first_id])
        .ok()
        .stdout();
    assert_eq!(removed.trim(), format!("removed {first_id}"));
}

#[test]
fn an_unknown_note_is_an_error() {
    let sandbox = Sandbox::new("backlog-unknown");
    sandbox.add_project();
    for verb in ["done", "reopen", "rm"] {
        let err = sandbox
            .ket_in(&sandbox.repo(), &["backlog", verb, "no-such-note"])
            .err();
        assert!(
            err.contains("no backlog note no-such-note"),
            "{verb}: {err}"
        );
    }
}

#[test]
fn a_backlog_can_move_into_the_repository_and_back() {
    let sandbox = Sandbox::new("backlog-store");
    sandbox.add_project();
    let repo = sandbox.repo();
    sandbox
        .ket_in(&repo, &["backlog", "add", "Shared note"])
        .ok();

    let shared = sandbox
        .ket_in(&repo, &["backlog", "store", "repo"])
        .ok()
        .stdout();
    assert!(shared.contains("1 note(s) in"), "{shared}");
    assert!(repo.join(".ket/backlog").is_dir(), "notes are in the repo");
    assert!(
        sandbox
            .ket_in(&repo, &["backlog", "list"])
            .ok()
            .stdout()
            .contains("Shared note"),
        "still listed from the repo"
    );

    let private = sandbox
        .ket_in(&repo, &["backlog", "store", "private"])
        .ok()
        .stdout();
    assert!(private.contains("1 note(s), kept privately"), "{private}");

    // Refused by the argument parser before anything is read or moved.
    let refused = sandbox.ket_in(&repo, &["backlog", "store", "somewhere"]);
    assert_eq!(refused.code(), Some(2));
    assert!(
        refused
            .stderr()
            .contains("[possible values: private, repo]"),
        "{}",
        refused.stderr()
    );
}

// ---- the rest ---------------------------------------------------------------

#[test]
fn config_lives_in_the_sandbox_and_shows_as_toml() {
    let sandbox = Sandbox::new("config");
    let path = sandbox.ket(&["config", "path"]).ok().stdout();
    assert_eq!(
        path.trim(),
        sandbox.path("config/ket/config.toml").to_str().unwrap()
    );
    let shown = sandbox.ket(&["config", "show"]).ok().stdout();
    assert!(shown.contains("[merge]"), "{shown}");
    assert!(shown.contains("commit_template"), "{shown}");
}

#[test]
fn doctor_names_the_sandboxed_paths() {
    let sandbox = Sandbox::new("doctor");
    // Whether every agent is installed depends on the machine; the paths
    // and the shape of the report do not.
    let run = sandbox.ket(&["doctor"]);
    let out = run.stdout();
    assert!(
        out.contains(sandbox.path("data/ket").to_str().unwrap()),
        "{out}"
    );
    assert!(out.contains("\nrequired\n"), "{out}");
    assert!(out.contains("\nagents\n"), "{out}");
    assert!(out.contains("git"), "{out}");
    assert!(matches!(run.code(), Some(0 | 1)), "{:?}", run.code());
}

#[test]
fn events_lists_what_the_journal_recorded() {
    let sandbox = Sandbox::new("events");
    let project = sandbox.add_project();
    let (id, _) = sandbox.add_worktree(&project, "journalled");

    let events = sandbox.ket(&["events"]).ok().stdout();
    let types: Vec<String> = events
        .lines()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).expect("json lines");
            value["type"].as_str().unwrap_or_default().to_owned()
        })
        .collect();
    assert!(types.iter().any(|t| t == "projectRegistered"), "{types:?}");
    assert!(types.iter().any(|t| t == "worktreeCreated"), "{types:?}");
    assert!(events.contains(&id), "{events}");
}

#[test]
fn commands_lists_the_catalogue_and_marks_what_needs_a_selection() {
    let sandbox = Sandbox::new("commands");
    let all = sandbox.ket(&["commands"]).ok().stdout();
    assert!(all.contains("worktree.merge"), "{all}");
    assert!(all.contains("worktree.pull_request"), "{all}");
    assert!(all.contains("command(s)"), "{all}");

    // Outside any project, a worktree command needs one named.
    let outside = sandbox
        .ket(&["commands", "--category", "worktree"])
        .ok()
        .stdout();
    let row = outside
        .lines()
        .find(|l| l.contains("worktree.status"))
        .unwrap();
    assert!(row.starts_with('!'), "{row}");

    // Inside a worktree, it applies.
    let project = sandbox.add_project();
    let (_, path) = sandbox.add_worktree(&project, "here");
    let inside = sandbox
        .ket_in(
            &path,
            &["commands", "--applicable", "--category", "worktree"],
        )
        .ok()
        .stdout();
    assert!(inside.contains("worktree.status"), "{inside}");
    assert!(inside.lines().all(|l| !l.starts_with('!')), "{inside}");

    let err = sandbox.ket(&["commands", "--category", "nonsense"]).err();
    assert!(err.contains("unknown category nonsense"), "{err}");
}

#[test]
fn ps_with_no_sessions_says_so_and_reaps_nothing() {
    let sandbox = Sandbox::new("ps");
    let listed = sandbox.ket(&["ps"]).ok();
    assert!(listed.stdout().is_empty());
    assert!(listed.stderr().contains("no sessions"));
    let reaped = sandbox.ket(&["ps", "--reap"]).ok().stdout();
    assert_eq!(reaped.trim(), "reaped 0 session record(s)");
}

#[test]
fn filter_passes_short_output_through_and_keeps_the_exit_status() {
    let sandbox = Sandbox::new("filter");
    let echoed = sandbox
        .ket(&["filter", "--", "echo", "hello"])
        .ok()
        .stdout();
    assert_eq!(echoed.trim(), "hello");

    let failed = sandbox.ket(&["filter", "--", "sh", "-c", "exit 3"]);
    assert_eq!(failed.code(), Some(3), "{}", failed.stderr());

    sandbox.ket(&["recall", "no-such-output"]).err();
}
