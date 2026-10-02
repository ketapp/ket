//! Deferred worktree deletion: rename aside, delete on a worker thread,
//! sweep what a crash left behind.

use std::fs;
use std::time::{Duration, Instant};

use ket_core::worktree_trash::{
    TRASH_DIR_NAME, is_trash_entry_name, move_to_trash, restore_from_trash, schedule_deletion,
    sweep_stale,
};

mod common;
use common::Sandbox;

fn wait_until_gone(path: &std::path::Path) {
    let started = Instant::now();
    while path.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the trash worker never deleted {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_real_trash_entry_name_is_recognised() {
    // Shaped exactly like `entry_name()` produces: `wt-<millis>-<8 hex>`.
    assert!(is_trash_entry_name("wt-1234567890-0a1b2c3d"));
}

#[test]
fn names_that_do_not_match_the_shape_are_rejected() {
    assert!(!is_trash_entry_name("something-else"));
    assert!(!is_trash_entry_name("wt-"));
    assert!(!is_trash_entry_name("wt-123"));
    assert!(!is_trash_entry_name("wt--0a1b2c3d"), "empty millis");
    assert!(!is_trash_entry_name("wt-123-0a1b2c3"), "nonce too short");
    assert!(!is_trash_entry_name("wt-123-0a1b2c3dd"), "nonce too long");
    assert!(!is_trash_entry_name("wt-123-0a1b2c3z"), "nonce not hex");
    assert!(
        !is_trash_entry_name("wt-12a3-0a1b2c3d"),
        "millis not digits"
    );
}

#[test]
fn move_to_trash_renames_the_checkout_aside() {
    let sandbox = Sandbox::new("trash-move");
    let worktree = sandbox.path("project/attempt-a");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join("file.txt"), "work\n").unwrap();

    let entry = move_to_trash(&worktree).expect("rename should succeed");

    assert!(!worktree.exists(), "original path is gone");
    assert!(entry.is_dir());
    assert!(entry.join("file.txt").is_file());
    assert_eq!(entry.parent().unwrap().file_name().unwrap(), TRASH_DIR_NAME);
    assert_eq!(
        entry.parent().unwrap().parent().unwrap(),
        worktree.parent().unwrap()
    );
}

#[test]
fn move_to_trash_refuses_a_trash_root_that_is_a_file() {
    let sandbox = Sandbox::new("trash-blocked-root");
    let parent = sandbox.path("project");
    fs::create_dir_all(&parent).unwrap();
    // Occupy the trash root's path with a plain file instead of a directory.
    fs::write(parent.join(TRASH_DIR_NAME), "not a directory").unwrap();

    let worktree = parent.join("attempt-a");
    fs::create_dir_all(&worktree).unwrap();

    assert_eq!(move_to_trash(&worktree), None);
    assert!(worktree.exists(), "nothing moved on a refusal");
}

#[test]
fn move_to_trash_on_a_missing_checkout_leaves_no_empty_root_behind() {
    let sandbox = Sandbox::new("trash-missing-checkout");
    let parent = sandbox.path("project");
    fs::create_dir_all(&parent).unwrap();
    let worktree = parent.join("never-existed");

    assert_eq!(move_to_trash(&worktree), None);
    assert!(
        !parent.join(TRASH_DIR_NAME).exists(),
        "an empty trash root is cleaned up after a failed rename"
    );
}

#[test]
fn restore_from_trash_puts_the_checkout_back() {
    let sandbox = Sandbox::new("trash-restore");
    let worktree = sandbox.path("project/attempt-a");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join("file.txt"), "work\n").unwrap();

    let entry = move_to_trash(&worktree).unwrap();
    assert!(restore_from_trash(&entry, &worktree));

    assert!(worktree.join("file.txt").is_file());
    assert!(!entry.exists());
}

#[test]
fn restore_from_trash_fails_cleanly_for_a_path_that_is_not_there() {
    let sandbox = Sandbox::new("trash-restore-missing");
    let bogus_source = sandbox.path("nowhere");
    let destination = sandbox.path("destination");
    assert!(!restore_from_trash(&bogus_source, &destination));
}

#[test]
fn schedule_deletion_removes_the_entry_and_the_root_once_empty() {
    let sandbox = Sandbox::new("trash-schedule");
    let worktree = sandbox.path("project/attempt-a");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join("file.txt"), "work\n").unwrap();

    let entry = move_to_trash(&worktree).unwrap();
    let root = entry.parent().unwrap().to_path_buf();

    schedule_deletion(entry.clone());
    wait_until_gone(&entry);

    // The root itself is removed once its last entry is gone.
    let root_started = Instant::now();
    while root.exists() {
        assert!(
            root_started.elapsed() < Duration::from_secs(5),
            "root never cleaned up"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sweep_stale_leaves_a_freshly_trashed_entry_alone() {
    let sandbox = Sandbox::new("trash-sweep-fresh");
    let worktrees_root = sandbox.path("worktrees");
    let project = worktrees_root.join("proj1");
    let worktree = project.join("attempt-a");
    fs::create_dir_all(&worktree).unwrap();

    let entry = move_to_trash(&worktree).unwrap();
    sweep_stale(&worktrees_root);

    assert!(
        entry.exists(),
        "a fresh entry is nowhere near the stale bound"
    );
}

#[test]
fn sweep_stale_never_touches_a_foreign_entry_regardless_of_age() {
    let sandbox = Sandbox::new("trash-sweep-foreign");
    let worktrees_root = sandbox.path("worktrees");
    let project = worktrees_root.join("proj1");
    let trash = project.join(TRASH_DIR_NAME);
    fs::create_dir_all(&trash).unwrap();
    let foreign = trash.join("not-ours-at-all");
    fs::create_dir_all(&foreign).unwrap();

    sweep_stale(&worktrees_root);

    assert!(
        foreign.exists(),
        "an entry not shaped like ket's own is left alone"
    );
}

#[test]
fn sweep_stale_on_a_missing_worktrees_root_does_not_panic() {
    let sandbox = Sandbox::new("trash-sweep-missing-root");
    sweep_stale(&sandbox.path("does-not-exist"));
}
