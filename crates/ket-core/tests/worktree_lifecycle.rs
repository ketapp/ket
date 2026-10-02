//! End-to-end tests against a real git repository.
//!
//! The unit tests elsewhere cover parsing and slugging in isolation. These check
//! the thing that actually matters: that what ket believes agrees with what git
//! believes, on a real repo, including for branch names that break naive path
//! construction.

use std::fs;

use ket_core::git::Git;
use ket_core::store::Store;
use ket_core::workspace::Workspace;

mod common;
use common::{Sandbox, git, init_repo};

/// A workspace whose state and worktrees live entirely inside `sandbox`.
fn workspace(sandbox: &Sandbox) -> Workspace {
    Workspace::with_paths(
        Store::at(sandbox.path("state.json")),
        sandbox.path("worktrees"),
    )
}

#[test]
fn create_list_and_remove_round_trip() {
    let sandbox = Sandbox::new("roundtrip");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    assert_eq!(project.default_base, "main");

    let worktree = ws
        .create_worktree(&project.id, "task-one", None, None)
        .expect("create worktree");

    assert!(worktree.path.is_dir());
    assert_eq!(ws.worktrees(Some(&project.id)).unwrap().len(), 1);

    ws.remove_worktree(&worktree.id, false, true)
        .expect("remove worktree");

    assert!(!worktree.path.exists());
    assert!(ws.worktrees(Some(&project.id)).unwrap().is_empty());
}

#[test]
fn ket_and_git_agree_on_what_worktrees_exist() {
    // The contract check: our registry against git's own view, including a
    // worktree git created that ket knows nothing about.
    let sandbox = Sandbox::new("agree");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    ws.create_worktree(&project.id, "ket-made", None, None)
        .unwrap();

    // One made by hand, outside ket.
    let manual = sandbox.path("manual");
    git(
        &repo,
        &["worktree", "add", "-b", "by-hand", manual.to_str().unwrap()],
    );

    let from_git = Git::new(&repo).worktree_list().expect("git worktree list");
    let branches: Vec<_> = from_git.iter().filter_map(|w| w.branch_name()).collect();

    assert!(branches.contains(&"main"), "got {branches:?}");
    assert!(branches.contains(&"ket-made"), "got {branches:?}");
    assert!(branches.contains(&"by-hand"), "got {branches:?}");

    // ket reports only what it created, and does not claim the manual one.
    let ours = ws.worktrees(Some(&project.id)).unwrap();
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0].branch, "ket-made");
}

#[test]
fn a_slashed_branch_occupies_one_directory_not_two() {
    let sandbox = Sandbox::new("slashes");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let nested = ws
        .create_worktree(&project.id, "feature/login", None, None)
        .unwrap();

    // Used raw, `feature/login` would create `.../feature/login` — two levels.
    // The slug keeps it to one component directly under the project directory.
    let under_project = nested
        .path
        .strip_prefix(sandbox.path("worktrees").join(project.id.as_str()))
        .expect("worktree must live under its project directory");
    assert_eq!(under_project.components().count(), 1, "got {nested:?}");

    // The branch name itself is preserved verbatim, slashes and all — only the
    // path is slugged.
    assert_eq!(nested.branch, "feature/login");
    // `--format` rather than plain `--list`: git prefixes `+` for a branch that
    // is checked out in another worktree, which every ket branch always is.
    assert_eq!(
        git(
            &repo,
            &[
                "branch",
                "--list",
                "feature/login",
                "--format=%(refname:short)"
            ]
        )
        .trim(),
        "feature/login"
    );
}

#[test]
fn branches_that_flatten_to_the_same_text_get_separate_directories() {
    // `a/b` and `a-b` can both exist in git at once, and a naive slug maps both
    // to `a-b`. This is the collision that actually reaches us in practice.
    let sandbox = Sandbox::new("flatten");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let slashed = ws.create_worktree(&project.id, "a/b", None, None).unwrap();
    let dashed = ws.create_worktree(&project.id, "a-b", None, None).unwrap();

    assert_ne!(slashed.path, dashed.path);
    assert!(slashed.path.is_dir());
    assert!(dashed.path.is_dir());
    assert_eq!(slashed.path.parent(), dashed.path.parent());
}

#[test]
fn a_git_ref_directory_file_conflict_is_reported_and_leaves_nothing_behind() {
    // Git stores refs as file paths, so `refs/heads/feature/login` makes
    // `feature` a directory and a branch named `feature` becomes impossible.
    // ket cannot fix that, but it must fail cleanly rather than half-creating.
    let sandbox = Sandbox::new("df-conflict");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    ws.create_worktree(&project.id, "feature/login", None, None)
        .unwrap();

    let conflict = ws.create_worktree(&project.id, "feature", None, None);
    assert!(
        conflict.is_err(),
        "expected git to refuse, got {conflict:?}"
    );

    // The registry matches reality: one worktree, and no orphan directory.
    let listed = ws.worktrees(Some(&project.id)).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].branch, "feature/login");

    let project_dir = sandbox.path("worktrees").join(project.id.as_str());
    let entries: Vec<_> = fs::read_dir(&project_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "orphan directory left behind: {entries:?}"
    );
}

#[test]
fn creating_a_worktree_on_an_existing_branch_is_refused() {
    let sandbox = Sandbox::new("dup-branch");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    ws.create_worktree(&project.id, "taken", None, None)
        .unwrap();
    let second = ws.create_worktree(&project.id, "taken", None, None);

    assert!(second.is_err(), "expected a conflict, got {second:?}");
    // And the failure left nothing behind.
    assert_eq!(ws.worktrees(Some(&project.id)).unwrap().len(), 1);
}

#[test]
fn removing_a_dirty_worktree_requires_force() {
    // Losing an agent's unreviewed work by accident is the worst thing this tool
    // could do, so the refusal is load-bearing.
    let sandbox = Sandbox::new("dirty");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "dirty-work", None, None)
        .unwrap();

    fs::write(worktree.path.join("README.md"), b"uncommitted change\n").unwrap();

    assert!(
        ws.remove_worktree(&worktree.id, false, false).is_err(),
        "a dirty worktree must not be removed without force"
    );
    assert!(worktree.path.is_dir());

    ws.remove_worktree(&worktree.id, true, false)
        .expect("force removal should succeed");
    assert!(!worktree.path.exists());
}

#[test]
fn prune_drops_worktrees_whose_directories_vanished() {
    let sandbox = Sandbox::new("prune");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let kept = ws.create_worktree(&project.id, "kept", None, None).unwrap();
    let gone = ws.create_worktree(&project.id, "gone", None, None).unwrap();

    // Simulate someone running `rm -rf` on it.
    fs::remove_dir_all(&gone.path).unwrap();

    let pruned = ws.prune().expect("prune");
    assert_eq!(pruned, vec![gone.id.clone()]);

    let remaining = ws.worktrees(Some(&project.id)).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, kept.id);
}

#[test]
fn removing_the_last_worktree_leaves_no_empty_directory_behind() {
    // Across many projects these accumulate forever otherwise.
    let sandbox = Sandbox::new("no-litter");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws.create_worktree(&project.id, "temp", None, None).unwrap();

    let project_dir = sandbox.path("worktrees").join(project.id.as_str());
    assert!(project_dir.is_dir());

    ws.remove_worktree(&worktree.id, false, true).unwrap();

    assert!(
        !project_dir.exists(),
        "left an empty project directory behind"
    );
    // The worktrees root itself must survive.
    assert!(sandbox.path("worktrees").is_dir());
}

#[test]
fn a_project_directory_with_other_worktrees_is_kept() {
    let sandbox = Sandbox::new("keep-shared");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let first = ws.create_worktree(&project.id, "one", None, None).unwrap();
    let second = ws.create_worktree(&project.id, "two", None, None).unwrap();

    ws.remove_worktree(&first.id, false, true).unwrap();

    let project_dir = sandbox.path("worktrees").join(project.id.as_str());
    assert!(project_dir.is_dir(), "removed a directory still in use");
    assert!(second.path.is_dir());
}

#[test]
fn adding_the_same_project_twice_is_idempotent() {
    let sandbox = Sandbox::new("idempotent");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let first = ws.add_project(&repo).unwrap();
    // From a subdirectory, which must resolve to the same project.
    let sub = repo.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let second = ws.add_project(&sub).unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(ws.projects().unwrap().len(), 1);
}

#[test]
fn two_projects_with_the_same_directory_name_stay_separate() {
    // The multi-project case: `api` in two different places.
    let sandbox = Sandbox::new("same-name");
    let a = sandbox.path("one/api");
    let b = sandbox.path("two/api");
    init_repo(&a);
    init_repo(&b);

    let ws = workspace(&sandbox);
    let pa = ws.add_project(&a).unwrap();
    let pb = ws.add_project(&b).unwrap();

    assert_ne!(pa.id, pb.id);
    assert_eq!(pa.name, "api");
    assert_eq!(pb.name, "api");

    let wa = ws
        .create_worktree(&pa.id, "shared-branch-name", None, None)
        .unwrap();
    let wb = ws
        .create_worktree(&pb.id, "shared-branch-name", None, None)
        .unwrap();

    assert_ne!(wa.path, wb.path);
    assert!(wa.path.is_dir() && wb.path.is_dir());
}

#[test]
fn removing_a_project_with_live_worktrees_is_refused_without_force() {
    let sandbox = Sandbox::new("project-rm");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    ws.create_worktree(&project.id, "live", None, None).unwrap();

    assert!(ws.remove_project(&project.id, false).is_err());
    assert_eq!(ws.projects().unwrap().len(), 1);

    ws.remove_project(&project.id, true).expect("force removal");
    assert!(ws.projects().unwrap().is_empty());
}

#[test]
fn a_project_whose_repository_disappeared_degrades_instead_of_failing() {
    let sandbox = Sandbox::new("vanished");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).unwrap();
    let worktree = ws
        .create_worktree(&project.id, "orphan", None, None)
        .unwrap();

    fs::remove_dir_all(&repo).unwrap();

    // Listing still works; the project simply reports itself unavailable.
    let projects = ws.projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert!(!projects[0].is_available());

    // And its registry entry can still be cleaned up.
    ws.remove_worktree(&worktree.id, false, false)
        .expect("record removal should still succeed");
    assert!(ws.worktrees(Some(&project.id)).unwrap().is_empty());
}

#[test]
fn resolve_project_prefers_cwd_then_recency() {
    let sandbox = Sandbox::new("resolve");
    let a = sandbox.path("alpha");
    let b = sandbox.path("beta");
    init_repo(&a);
    init_repo(&b);

    let ws = workspace(&sandbox);
    let pa = ws.add_project(&a).unwrap();
    let pb = ws.add_project(&b).unwrap();

    // Inside a project's tree, that project wins.
    assert_eq!(ws.resolve_project(None, &a).unwrap().id, pa.id);
    assert_eq!(ws.resolve_project(None, &b).unwrap().id, pb.id);

    // Outside any of them, the most recently touched wins.
    ws.touch_project(&pa.id).unwrap();
    let outside = sandbox.path("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    assert_eq!(ws.resolve_project(None, &outside).unwrap().id, pa.id);

    // An explicit name still wins over both.
    assert_eq!(ws.resolve_project(Some("beta"), &a).unwrap().id, pb.id);
}

// ---- base refs ------------------------------------------------------------
//
// `git worktree add -b <branch> <path> <base>` accepts far more than a branch
// name for `base`, and each of these arrives with a different consequence for
// how far the worktree has diverged later.

#[test]
fn a_tag_is_a_valid_base_and_is_frozen_at_creation() {
    let sandbox = Sandbox::new("base-tag");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let tagged = common::rev(&repo, "HEAD");
    git(&repo, &["tag", "v1.0"]);
    // Move main past the tag, so basing on the tag is observably different from
    // basing on the branch.
    common::commit_file(&repo, "after.txt", "later\n", "after the tag");

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "from-tag", Some("v1.0"), None)
        .expect("create from a tag");

    assert_eq!(common::rev(&worktree.path, "HEAD"), tagged);
    assert_eq!(worktree.base, "v1.0");
    assert_eq!(worktree.base_commit.as_deref(), Some(tagged.as_str()));
}

#[test]
fn a_remote_tracking_ref_is_a_valid_base() {
    let sandbox = Sandbox::new("base-remote");
    let upstream = sandbox.path("upstream");
    init_repo(&upstream);
    common::commit_file(&upstream, "upstream.txt", "theirs\n", "upstream work");
    let upstream_head = common::rev(&upstream, "HEAD");

    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(
        &repo,
        &["remote", "add", "origin", &upstream.to_string_lossy()],
    );
    git(&repo, &["fetch", "--quiet", "origin"]);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "from-remote", Some("origin/main"), None)
        .expect("create from a remote-tracking ref");

    assert_eq!(common::rev(&worktree.path, "HEAD"), upstream_head);
    // The ref is kept as written, so divergence tracks `origin/main` as it moves
    // rather than being pinned to today's tip.
    assert_eq!(worktree.base_rev(), Some("origin/main"));
}

#[test]
fn a_detached_head_repository_can_still_be_registered_and_branched() {
    // No `origin/HEAD`, no `main`, no `master`, and a detached checkout. Every
    // heuristic for "the default branch" comes up empty, and refusing to
    // register the project would make it unusable in ket for no good reason.
    let sandbox = Sandbox::new("base-detached");
    let repo = sandbox.path("repo");
    common::init_repo_on(&repo, "trunk");
    common::commit_file(&repo, "second.txt", "two\n", "second");

    let detached_at = common::rev(&repo, "HEAD~1");
    git(&repo, &["checkout", "--quiet", &detached_at]);

    let ws = workspace(&sandbox);
    let project = ws
        .add_project(&repo)
        .expect("a detached repo is registrable");
    assert_eq!(project.default_base, "HEAD");

    let worktree = ws
        .create_worktree(&project.id, "from-detached", None, None)
        .expect("create from a detached HEAD");

    assert_eq!(common::rev(&worktree.path, "HEAD"), detached_at);

    // `HEAD` inside the new worktree means the worktree's own branch, so the
    // frozen commit is the only honest thing to measure against.
    assert_eq!(worktree.base, "HEAD");
    assert_eq!(worktree.base_rev(), Some(detached_at.as_str()));
}

#[test]
fn a_base_ref_that_does_not_exist_is_refused_and_leaves_nothing_behind() {
    let sandbox = Sandbox::new("base-missing");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let result = ws.create_worktree(&project.id, "doomed", Some("no-such-ref"), None);
    assert!(result.is_err(), "expected a refusal, got {result:?}");
    assert!(ws.worktrees(Some(&project.id)).unwrap().is_empty());
    assert!(
        ws.project_state(&project.id).unwrap().open.is_empty(),
        "a failed creation must not leave the project pointing at it"
    );
}

// ---- paths containing spaces ----------------------------------------------

#[test]
fn spaces_in_every_path_are_handled_end_to_end() {
    // The repository root, the worktrees root, and the state file all contain
    // spaces. Nothing here shells out through an interpreter, so this should
    // hold — but "should" is what a test is for, and one `format!` into a shell
    // string anywhere would break all of it at once.
    let sandbox = Sandbox::new("spaces");
    let repo = sandbox.path("my projects/api service");
    init_repo(&repo);

    let ws = Workspace::with_paths(
        Store::at(sandbox.path("state file.json")),
        sandbox.path("work trees"),
    );

    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "feature/login", None, None)
        .expect("create worktree");

    assert!(worktree.path.is_dir());

    // Branch names cannot themselves contain spaces — git's own ref format
    // forbids them — so the exposure is entirely in the paths around them.
    assert!(worktree.path.to_string_lossy().contains(' '));

    // git agrees, which is the part that matters.
    let listed = Git::new(&repo).worktree_list().expect("git worktree list");
    assert!(
        listed.iter().any(|w| w.path == worktree.path),
        "git does not know about {}: {listed:?}",
        worktree.path.display()
    );

    ws.remove_worktree(&worktree.id, false, true)
        .expect("remove worktree");
    assert!(!worktree.path.exists());
}

// ---- submodules -----------------------------------------------------------

/// A repository with one submodule, committed but not checked out anywhere new.
fn init_repo_with_submodule(sandbox: &Sandbox) -> std::path::PathBuf {
    let sub = sandbox.path("sub");
    init_repo(&sub);

    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(
        &repo,
        &[
            // Local paths as submodule sources are refused by default since
            // CVE-2022-39253. The fixture is ours, so allow it here only.
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            &sub.to_string_lossy(),
            "libs/sub",
        ],
    );
    git(&repo, &["commit", "--quiet", "-m", "add submodule"]);
    repo
}

#[test]
fn a_worktree_of_a_repo_with_submodules_is_created_and_removed_normally() {
    // Uninitialised submodules are the default state of a fresh worktree: git
    // creates the directory and leaves it empty.
    let sandbox = Sandbox::new("submodule-plain");
    let repo = init_repo_with_submodule(&sandbox);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "sub-work", None, None)
        .expect("create worktree");

    let submodule_dir = worktree.path.join("libs/sub");
    assert!(submodule_dir.is_dir(), "submodule directory should exist");
    assert_eq!(
        fs::read_dir(&submodule_dir).unwrap().count(),
        0,
        "git worktree add does not populate submodules"
    );

    ws.remove_worktree(&worktree.id, false, true)
        .expect("remove worktree");
    assert!(!worktree.path.exists());
}

#[test]
fn a_checked_out_submodule_blocks_removal_with_an_explanation() {
    // git refuses outright — "working trees containing submodules cannot be
    // moved or removed" — which says nothing about what to do next. ket says it
    // before touching anything, and names the flag.
    let sandbox = Sandbox::new("submodule-live");
    let repo = init_repo_with_submodule(&sandbox);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let worktree = ws
        .create_worktree(&project.id, "sub-live", None, None)
        .expect("create worktree");

    git(
        &worktree.path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--quiet",
        ],
    );
    assert!(worktree.path.join("libs/sub/README.md").is_file());

    let refused = ws.remove_worktree(&worktree.id, false, false);
    let message = refused.expect_err("expected a refusal").to_string();
    assert!(
        message.contains("submodule") && message.contains("--force"),
        "unhelpful refusal: {message}"
    );
    assert!(worktree.path.is_dir(), "the refusal must change nothing");

    ws.remove_worktree(&worktree.id, true, true)
        .expect("force removal should succeed");
    assert!(!worktree.path.exists());
}

#[test]
fn worktree_order_is_empty_until_set_and_then_round_trips() {
    let sandbox = Sandbox::new("wt-order");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws.create_worktree(&project.id, "a", None, None).unwrap();
    let b = ws.create_worktree(&project.id, "b", None, None).unwrap();

    assert!(ws.worktree_order(&project.id).unwrap().is_empty());

    ws.set_worktree_order(&project.id, &[b.id.clone(), a.id.clone()])
        .unwrap();
    assert_eq!(
        ws.worktree_order(&project.id).unwrap(),
        vec![b.id.clone(), a.id.clone()]
    );

    // The stored order overrides the default recency sort.
    let listed: Vec<_> = ws
        .worktrees(Some(&project.id))
        .unwrap()
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(listed, vec![b.id, a.id]);
}

#[test]
fn set_worktree_order_drops_ids_from_another_project() {
    let sandbox = Sandbox::new("wt-order-scoped");
    let repo_a = sandbox.path("a");
    let repo_b = sandbox.path("b");
    init_repo(&repo_a);
    init_repo(&repo_b);

    let ws = workspace(&sandbox);
    let p1 = ws.add_project(&repo_a).expect("add a");
    let p2 = ws.add_project(&repo_b).expect("add b");
    let mine = ws.create_worktree(&p1.id, "mine", None, None).unwrap();
    let theirs = ws.create_worktree(&p2.id, "theirs", None, None).unwrap();

    ws.set_worktree_order(&p1.id, &[mine.id.clone(), theirs.id])
        .unwrap();

    assert_eq!(ws.worktree_order(&p1.id).unwrap(), vec![mine.id]);
}

#[test]
fn an_order_naming_nothing_clears_the_record() {
    let sandbox = Sandbox::new("wt-order-clear");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");
    let a = ws.create_worktree(&project.id, "a", None, None).unwrap();

    ws.set_worktree_order(&project.id, &[a.id]).unwrap();
    assert!(!ws.worktree_order(&project.id).unwrap().is_empty());

    ws.set_worktree_order(&project.id, &[]).unwrap();
    assert!(ws.worktree_order(&project.id).unwrap().is_empty());
}

#[test]
fn worktree_order_on_an_unknown_project_is_an_error() {
    let sandbox = Sandbox::new("wt-order-unknown");
    let ws = workspace(&sandbox);
    let bogus = ket_core::id::ProjectId::new("does-not-exist");

    assert!(ws.worktree_order(&bogus).is_err());
    assert!(ws.set_worktree_order(&bogus, &[]).is_err());
}

#[test]
fn create_worktree_prepared_reports_what_the_base_went_through() {
    let sandbox = Sandbox::new("wt-prepared");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let (worktree, prepared) = ws
        .create_worktree_prepared(&project.id, "attempt", None, None)
        .expect("create prepared");

    assert_eq!(worktree.branch, "attempt");
    // No remote configured, so there is nothing to have fetched.
    assert!(matches!(prepared.fetch, ket_core::git::Fetch::NoRemote));
}

#[test]
fn create_worktree_prepared_with_economy_rejects_an_unknown_level() {
    let sandbox = Sandbox::new("wt-prepared-bad-economy");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let ws = workspace(&sandbox);
    let project = ws.add_project(&repo).expect("add project");

    let err = ws
        .create_worktree_prepared_with_economy(
            &project.id,
            "attempt",
            None,
            None,
            Some("not-a-real-level"),
        )
        .expect_err("should refuse");
    assert!(err.to_string().contains("Economy"), "{err}");

    // Nothing left behind on disk or in the registry for the refused attempt.
    assert!(ws.worktrees(Some(&project.id)).unwrap().is_empty());
    assert!(!ket_core::git::Git::new(&repo).branch_exists("attempt"));
}
