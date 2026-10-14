//! `Workspace::open_pull_request` — committing a worktree's work, pushing
//! its branch and opening a pull request for it.
//!
//! The push is real: it lands in a bare repository in the sandbox, which is
//! a remote as far as git is concerned. `gh` is a shell script of the test's
//! own, handed in through `Workspace::with_gh_program`, that records what it
//! was asked and answers the way `gh` does. The commit shares its code with
//! `merge_worktree.rs`, which covers the template; this file covers what is
//! specific to the pull request path.

use std::fs;
use std::path::{Path, PathBuf};

use ket_core::event::Event;
use ket_core::project::ProjectSettings;
use ket_core::store::Store;
use ket_core::workspace::{PullRequestOptions, Workspace};

mod common;
use common::{Sandbox, commit_file, git, init_repo, rev};

/// A `gh` that keeps a diary.
struct FakeGh {
    script: PathBuf,
    log: PathBuf,
    tokens: PathBuf,
    list: PathBuf,
    body: PathBuf,
}

impl FakeGh {
    /// Writes the script into `sandbox`.
    fn new(sandbox: &Sandbox) -> Self {
        let gh = Self {
            script: sandbox.path("gh"),
            log: sandbox.path("gh.log"),
            tokens: sandbox.path("gh.tokens"),
            list: sandbox.path("gh.list"),
            body: sandbox.path("gh.body"),
        };
        // Paths are quoted: a sandbox directory name embeds a `ThreadId(N)`,
        // and unquoted parentheses are shell metacharacters.
        fs::write(
            &gh.script,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> "{log}"
printf '%s\n' "${{GH_TOKEN:-none}}" >> "{tokens}"
case "$1 $2" in
  "auth token") echo "tok-$6" ;;
  "pr list") if [ -f "{list}" ]; then cat "{list}"; else echo "[]"; fi ;;
  "pr create")
    cat > "{body}"
    echo "Creating pull request for $6 into $4 in o/r"
    echo ""
    echo "https://github.com/o/r/pull/77" ;;
  *) echo "unexpected: $*" >&2; exit 2 ;;
esac
"#,
                log = gh.log.display(),
                tokens = gh.tokens.display(),
                list = gh.list.display(),
                body = gh.body.display(),
            ),
        )
        .unwrap();
        make_executable(&gh.script);
        gh
    }

    /// Every call so far, one line of arguments each.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The `GH_TOKEN` each call saw, in order, `none` when it saw none.
    fn tokens(&self) -> Vec<String> {
        fs::read_to_string(&self.tokens)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Makes `pr list` answer with one open pull request, number `number`.
    fn with_open(&self, number: u64) {
        fs::write(
            &self.list,
            format!(r#"[{{"number": {number}, "url": "https://github.com/o/r/pull/{number}"}}]"#),
        )
        .unwrap();
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// A repository with one commit on `main`, pushed to a bare `origin` beside
/// it. Returns the repository and the remote.
fn repo_with_remote(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let remote = sandbox.path("remote.git");
    git(
        sandbox.root(),
        &["init", "--bare", "--quiet", remote.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    (repo, remote)
}

/// A workspace inside `sandbox` whose `gh` is `gh`.
fn workspace(sandbox: &Sandbox, gh: &FakeGh) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
    .with_gh_program(&gh.script)
}

fn options(title: &str) -> PullRequestOptions {
    PullRequestOptions {
        title: title.to_owned(),
        body: "Closes #1\n".to_owned(),
        draft: false,
    }
}

#[test]
fn uncommitted_work_is_committed_pushed_and_opened_as_a_pull_request() {
    let sandbox = Sandbox::new("pr-happy");
    let (repo, remote) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    fs::write(a.path.join("work.txt"), "done\n").unwrap();
    let mut events = ws.bus().subscribe();

    let report = ws
        .open_pull_request(&a.id, &options("Fix the thing"))
        .expect("open pull request");

    assert_eq!(report.worktree, a.id);
    assert_eq!(report.branch, "attempt-a");
    assert_eq!(report.base, "main");
    assert_eq!(report.remote, "origin");
    assert!(report.committed.is_some(), "uncommitted work was committed");
    assert!(!report.existed);
    assert_eq!(report.pull_request.number, 77);
    assert_eq!(report.pull_request.url, "https://github.com/o/r/pull/77");

    // The branch is on the remote, at the commit just made, and tracks it.
    assert_eq!(rev(&remote, "attempt-a"), rev(&a.path, "HEAD"));
    assert_eq!(
        git(
            &a.path,
            &["rev-parse", "--abbrev-ref", "attempt-a@{upstream}"]
        )
        .trim(),
        "origin/attempt-a"
    );

    // `gh` was asked whether one was open first, then to make one, with the
    // description on its stdin.
    let calls = gh.calls();
    assert_eq!(
        calls,
        [
            "pr list --head attempt-a --state open --json number,url",
            "pr create --base main --head attempt-a --title Fix the thing --body-file -",
        ]
    );
    assert_eq!(fs::read_to_string(&gh.body).unwrap(), "Closes #1\n");
    // No account was picked, so `gh` went as whoever it has active.
    assert!(gh.tokens().iter().all(|token| token == "none"));

    // The worktree remembers it, and the bus said so.
    let remembered = ws
        .worktrees(None)
        .unwrap()
        .into_iter()
        .find(|w| w.id == a.id)
        .unwrap()
        .pull_request
        .expect("the pull request is remembered on the worktree");
    assert_eq!(remembered, report.pull_request);
    let opened = std::iter::from_fn(|| events.try_recv().ok())
        .find(|envelope| matches!(envelope.event, Event::PullRequestOpened { .. }))
        .expect("a PullRequestOpened event");
    assert_eq!(
        opened.event,
        Event::PullRequestOpened {
            worktree_id: a.id.clone(),
            project_id: project.id.clone(),
            number: 77,
            url: "https://github.com/o/r/pull/77".to_owned(),
        }
    );
}

#[test]
fn a_draft_is_asked_for_as_one() {
    let sandbox = Sandbox::new("pr-draft");
    let (repo, _) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "work");

    let report = ws
        .open_pull_request(
            &a.id,
            &PullRequestOptions {
                draft: true,
                ..options("Draft it")
            },
        )
        .unwrap();

    assert!(report.committed.is_none(), "nothing was uncommitted");
    assert!(
        gh.calls()[1].ends_with("--body-file - --draft"),
        "{:?}",
        gh.calls()
    );
}

#[test]
fn a_pull_request_already_open_for_the_branch_is_pushed_to_not_doubled() {
    let sandbox = Sandbox::new("pr-existing");
    let (repo, remote) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    gh.with_open(5);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "work");

    let report = ws.open_pull_request(&a.id, &options("Again")).unwrap();

    assert!(report.existed);
    assert_eq!(report.pull_request.number, 5);
    assert_eq!(rev(&remote, "attempt-a"), rev(&a.path, "HEAD"));
    assert!(
        gh.calls().iter().all(|call| !call.starts_with("pr create")),
        "no second pull request was made: {:?}",
        gh.calls()
    );
    let remembered = ws
        .worktrees(None)
        .unwrap()
        .into_iter()
        .find(|w| w.id == a.id)
        .unwrap()
        .pull_request;
    assert_eq!(remembered.map(|pr| pr.number), Some(5));
}

#[test]
fn the_projects_github_account_is_the_one_gh_and_the_push_go_as() {
    let sandbox = Sandbox::new("pr-account");
    let (repo, _) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    ws.set_project_settings(
        &project.id,
        ProjectSettings {
            github_account: Some("sam".to_owned()),
            ..ProjectSettings::default()
        },
    )
    .unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "work");

    ws.open_pull_request(&a.id, &options("As sam")).unwrap();

    let calls = gh.calls();
    let tokens = gh.tokens();
    // Every call that acts was preceded by asking for sam's token, and
    // carried it; the token lookups themselves carry none.
    let acting: Vec<(&String, &String)> = calls
        .iter()
        .zip(&tokens)
        .filter(|(call, _)| call.starts_with("pr "))
        .collect();
    assert_eq!(acting.len(), 2, "{calls:?}");
    assert!(
        acting.iter().all(|(_, token)| *token == "tok-sam"),
        "{tokens:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call == "auth token --hostname github.com --user sam"),
        "{calls:?}"
    );
}

#[test]
fn a_repository_with_no_remote_is_refused_before_anything_is_committed() {
    let sandbox = Sandbox::new("pr-no-remote");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    fs::write(a.path.join("work.txt"), "done\n").unwrap();

    let err = ws
        .open_pull_request(&a.id, &options("Nowhere"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("no remote"), "{err}");
    assert!(gh.calls().is_empty(), "gh was never asked");
    assert!(
        git(&a.path, &["status", "--porcelain"]).contains("work.txt"),
        "the work is still uncommitted"
    );
}

#[test]
fn a_pull_request_needs_a_title() {
    let sandbox = Sandbox::new("pr-no-title");
    let (repo, _) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();

    let err = ws
        .open_pull_request(&a.id, &options("   "))
        .unwrap_err()
        .to_string();

    assert!(err.contains("needs a title"), "{err}");
    assert!(gh.calls().is_empty());
}

#[test]
fn a_push_that_fails_leaves_the_worktree_as_it_was() {
    let sandbox = Sandbox::new("pr-push-fails");
    let (repo, remote) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    commit_file(&a.path, "work.txt", "done\n", "work");
    // The remote goes away between the worktree being cut and the push.
    fs::remove_dir_all(&remote).unwrap();

    let err = ws
        .open_pull_request(&a.id, &options("Lost"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("could not push"), "{err}");
    assert!(
        gh.calls().iter().all(|call| !call.starts_with("pr create")),
        "no pull request was made for a branch that never arrived: {:?}",
        gh.calls()
    );
    let remembered = ws
        .worktrees(None)
        .unwrap()
        .into_iter()
        .find(|w| w.id == a.id)
        .unwrap()
        .pull_request;
    assert!(remembered.is_none());
}

#[test]
fn a_merge_left_half_done_in_the_worktree_is_refused() {
    let sandbox = Sandbox::new("pr-in-progress");
    let (repo, _) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "attempt-a", None, None)
        .unwrap();
    // Both sides change the same line, and the merge in the worktree stops
    // on the conflict.
    commit_file(&repo, "README.md", "# theirs\n", "theirs");
    commit_file(&a.path, "README.md", "# ours\n", "ours");
    assert!(
        common::try_git(&a.path, &["merge", "main"]).is_err(),
        "the merge conflicts"
    );

    let err = ws
        .open_pull_request(&a.id, &options("Mid-merge"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("in the middle of a"), "{err}");
    assert!(gh.calls().is_empty());
}

#[test]
fn the_draft_names_the_branch_the_base_and_the_remote_and_then_the_pull_request() {
    let sandbox = Sandbox::new("pr-draft-words");
    let (repo, _) = repo_with_remote(&sandbox);
    let gh = FakeGh::new(&sandbox);
    let ws = workspace(&sandbox, &gh);
    let project = ws.add_project(&repo).unwrap();
    let a = ws
        .create_worktree(&project.id, "142-fix-the-login", None, None)
        .unwrap();
    fs::write(a.path.join("work.txt"), "done\n").unwrap();

    let draft = ws.pull_request_draft(&a.id, &[]).unwrap();
    assert_eq!(draft.worktree, a.id);
    assert_eq!(draft.branch, "142-fix-the-login");
    assert_eq!(draft.base, "main");
    assert_eq!(draft.remote.as_deref(), Some("origin"));
    assert_eq!(draft.title, "Fix the login");
    assert_eq!(draft.body, "Closes #142\n");
    assert_eq!(draft.changes, 1);
    assert!(draft.existing.is_none());

    ws.open_pull_request(&a.id, &options("Fix the login"))
        .unwrap();

    let after = ws.pull_request_draft(&a.id, &[]).unwrap();
    assert_eq!(after.existing.map(|pr| pr.number), Some(77));
    assert_eq!(after.changes, 0, "the commit took the work");
}
