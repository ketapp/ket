//! What a worktree would bring to its base, for reviewing on a phone:
//! changed files, their status, and each file's own diff.
//!
//! With no `base` given, this is measured against `HEAD` — uncommitted work
//! only, which is what the repository's own checkout (which has no base) is
//! measured against. With a `base`, it is measured from the commit the
//! branch last shared with it, so committed work counts too.

use std::fs;

use ket_core::review::{Status, changes, diff};

mod common;
use common::{Sandbox, commit_file, git, init_repo};

#[test]
fn a_clean_worktree_has_no_changes() {
    let sandbox = Sandbox::new("review-clean");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    assert!(changes(&repo, None).unwrap().is_empty());
}

#[test]
fn an_uncommitted_edit_is_reported_as_modified() {
    let sandbox = Sandbox::new("review-uncommitted-edit");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("README.md"), "changed\n").unwrap();

    let found = changes(&repo, None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "README.md");
    assert_eq!(found[0].status, Status::Modified);
    assert_eq!(found[0].lines, Some((1, 1)));
}

#[test]
fn a_committed_change_is_visible_against_an_earlier_base() {
    let sandbox = Sandbox::new("review-committed");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "start", "main"]);
    commit_file(&repo, "README.md", "changed\n", "edit readme");

    let found = changes(&repo, Some("start")).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "README.md");
    assert_eq!(found[0].status, Status::Modified);
}

#[test]
fn a_new_committed_file_is_reported_as_added() {
    let sandbox = Sandbox::new("review-added-committed");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["branch", "start", "main"]);
    commit_file(&repo, "new.txt", "line one\nline two\n", "add new file");

    let found = changes(&repo, Some("start")).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "new.txt");
    assert_eq!(found[0].status, Status::Added);
    assert_eq!(found[0].lines, Some((2, 0)));
}

#[test]
fn a_deleted_file_is_reported_as_deleted() {
    let sandbox = Sandbox::new("review-deleted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "doomed.txt", "bye\n", "add doomed file");
    git(&repo, &["branch", "start", "main"]);
    fs::remove_file(repo.join("doomed.txt")).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "remove doomed file"]);

    let found = changes(&repo, Some("start")).unwrap();
    assert!(
        found
            .iter()
            .any(|f| f.path == "doomed.txt" && f.status == Status::Deleted)
    );
}

#[test]
fn an_untracked_file_is_reported_as_added_with_its_line_count() {
    let sandbox = Sandbox::new("review-untracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("scratch.txt"), "a\nb\nc\n").unwrap();

    let found = changes(&repo, None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "scratch.txt");
    assert_eq!(found[0].status, Status::Added);
    assert_eq!(found[0].lines, Some((3, 0)));
}

#[test]
fn results_are_sorted_by_path() {
    let sandbox = Sandbox::new("review-sorted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("zebra.txt"), "z\n").unwrap();
    fs::write(repo.join("apple.txt"), "a\n").unwrap();

    let found = changes(&repo, None).unwrap();
    let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["apple.txt", "zebra.txt"]);
}

#[test]
fn a_binary_untracked_file_has_no_line_count() {
    let sandbox = Sandbox::new("review-binary-untracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("image.bin"), [0u8, 1, 2, 3, 0, 4]).unwrap();

    let found = changes(&repo, None).unwrap();
    assert_eq!(found[0].lines, None);
}

#[test]
fn changes_are_measured_from_the_branch_point_not_the_bases_moving_tip() {
    let sandbox = Sandbox::new("review-base");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "feature.txt", "work\n", "feature work");
    git(&repo, &["checkout", "--quiet", "main"]);
    commit_file(&repo, "unrelated.txt", "noise\n", "main moves on");
    git(&repo, &["checkout", "--quiet", "feature"]);

    // Against `main`, only the feature branch's own file shows up — the
    // merge base is the branch point, not main's current, moved-on tip.
    let found = changes(&repo, Some("main")).unwrap();
    let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["feature.txt"]);
}

#[test]
fn a_bare_repository_with_no_head_yet_has_no_changes() {
    let sandbox = Sandbox::new("review-no-head");
    let repo = sandbox.path("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);

    assert!(changes(&repo, None).unwrap().is_empty());
}

#[test]
fn diff_returns_the_patch_for_an_uncommitted_edit() {
    let sandbox = Sandbox::new("review-diff-modified");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("README.md"), "changed content\n").unwrap();

    let (text, truncated) = diff(&repo, None, "README.md").unwrap();
    assert!(!truncated);
    assert!(text.contains("changed content"), "{text}");
}

#[test]
fn diff_returns_the_whole_file_for_an_untracked_addition() {
    let sandbox = Sandbox::new("review-diff-untracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(repo.join("new.txt"), "brand new content\n").unwrap();

    let (text, _) = diff(&repo, None, "new.txt").unwrap();
    assert!(text.contains("brand new content"), "{text}");
}

#[test]
fn diff_refuses_a_path_with_no_changes() {
    let sandbox = Sandbox::new("review-diff-no-changes");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    assert!(diff(&repo, None, "README.md").is_err());
}
