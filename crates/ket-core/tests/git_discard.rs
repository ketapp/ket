//! `Git::discard`, the Git view's Discard, and `Git::head_stamp`.
//!
//! Discard throws work away, so each case checks both what went and what
//! stayed.

use std::fs;

use ket_core::git::Git;

mod common;
use common::{Sandbox, commit_file, git, init_repo};

fn status(repo: &std::path::Path) -> String {
    git(repo, &["status", "--porcelain"])
}

#[test]
fn an_edited_file_goes_back_to_head_and_its_neighbours_are_left_alone() {
    let sandbox = Sandbox::new("discard-edit");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "keep.txt", "keep\n", "keep");
    fs::write(repo.join("README.md"), "changed\n").unwrap();
    fs::write(repo.join("keep.txt"), "also changed\n").unwrap();

    Git::new(&repo).discard(&["README.md"]).unwrap();

    assert_eq!(
        fs::read_to_string(repo.join("README.md")).unwrap(),
        "# fixture\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("keep.txt")).unwrap(),
        "also changed\n"
    );
    assert_eq!(status(&repo).trim(), "M keep.txt");
}

#[test]
fn a_staged_edit_is_unstaged_as_well_as_undone() {
    let sandbox = Sandbox::new("discard-staged");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("README.md"), "staged\n").unwrap();
    git(&repo, &["add", "README.md"]);

    Git::new(&repo).discard(&["README.md"]).unwrap();

    assert!(status(&repo).is_empty(), "{}", status(&repo));
}

#[test]
fn a_staged_new_file_is_removed_from_the_index_and_the_disk() {
    let sandbox = Sandbox::new("discard-added");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("new.txt"), "new\n").unwrap();
    git(&repo, &["add", "new.txt"]);

    Git::new(&repo).discard(&["new.txt"]).unwrap();

    assert!(!repo.join("new.txt").exists());
    assert!(status(&repo).is_empty());
}

#[test]
fn an_untracked_file_is_deleted() {
    let sandbox = Sandbox::new("discard-untracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("scratch.txt"), "scratch\n").unwrap();

    Git::new(&repo).discard(&["scratch.txt"]).unwrap();

    assert!(!repo.join("scratch.txt").exists());
}

#[test]
fn a_staged_deletion_is_brought_back() {
    let sandbox = Sandbox::new("discard-deleted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["rm", "--quiet", "README.md"]);
    assert!(!repo.join("README.md").exists());

    Git::new(&repo).discard(&["README.md"]).unwrap();

    assert_eq!(
        fs::read_to_string(repo.join("README.md")).unwrap(),
        "# fixture\n"
    );
    assert!(status(&repo).is_empty());
}

#[test]
fn a_glob_shaped_name_discards_only_itself() {
    let sandbox = Sandbox::new("discard-literal");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "*.txt", "star\n", "a file named star");
    commit_file(&repo, "other.txt", "other\n", "other");
    fs::write(repo.join("*.txt"), "star changed\n").unwrap();
    fs::write(repo.join("other.txt"), "other changed\n").unwrap();

    Git::new(&repo).discard(&["*.txt"]).unwrap();

    assert_eq!(fs::read_to_string(repo.join("*.txt")).unwrap(), "star\n");
    assert_eq!(
        fs::read_to_string(repo.join("other.txt")).unwrap(),
        "other changed\n",
        "a pathspec glob would have reset this too"
    );
}

#[test]
fn an_untracked_directory_is_refused_rather_than_removed_recursively() {
    let sandbox = Sandbox::new("discard-dir");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("build/out")).unwrap();
    fs::write(repo.join("build/out/a.o"), "obj").unwrap();

    assert!(Git::new(&repo).discard(&["build"]).is_err());
    assert!(repo.join("build/out/a.o").exists());
}

#[test]
fn several_paths_of_different_kinds_go_in_one_call() {
    let sandbox = Sandbox::new("discard-many");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("README.md"), "changed\n").unwrap();
    fs::write(repo.join("scratch.txt"), "scratch\n").unwrap();

    Git::new(&repo)
        .discard(&["README.md", "scratch.txt"])
        .unwrap();

    assert!(status(&repo).is_empty(), "{}", status(&repo));
}

#[test]
fn head_stamp_is_the_short_id_and_commit_time_and_none_when_unborn() {
    let sandbox = Sandbox::new("head-stamp");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let (id, at_ms) = Git::new(&repo).head_stamp().unwrap();
    assert_eq!(id, git(&repo, &["rev-parse", "--short", "HEAD"]).trim());
    let seconds: u64 = git(&repo, &["log", "-1", "--format=%ct"])
        .trim()
        .parse()
        .unwrap();
    assert_eq!(at_ms, seconds * 1000);

    let unborn = sandbox.path("unborn");
    fs::create_dir_all(&unborn).unwrap();
    git(&unborn, &["init", "--quiet"]);
    assert_eq!(Git::new(&unborn).head_stamp(), None);
    assert_eq!(Git::new(&unborn).root(), unborn.as_path());
}
