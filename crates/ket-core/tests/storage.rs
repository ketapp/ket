//! What a checkout costs on disk, and clearing its regenerable build output.

use std::fs;

use ket_core::storage::{Clearable, clear, format_bytes, measure, plan_clear};

mod common;
use common::Sandbox;

fn write(path: &std::path::Path, bytes: usize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

#[test]
fn measure_counts_every_file_once() {
    let sandbox = Sandbox::new("storage-measure-total");
    let root = sandbox.path("worktree");
    write(&root.join("a.txt"), 100);
    write(&root.join("sub").join("b.txt"), 50);

    let footprint = measure(&root, &[]);
    assert_eq!(footprint.total, 150);
    assert_eq!(footprint.build, 0);
    assert_eq!(footprint.kept(), 150);
}

#[test]
fn measure_separates_build_output_by_glob() {
    let sandbox = Sandbox::new("storage-measure-build");
    let root = sandbox.path("worktree");
    write(&root.join("src").join("main.rs"), 20);
    write(&root.join("target").join("debug").join("binary"), 1000);

    let footprint = measure(&root, &["target".to_owned()]);
    assert_eq!(footprint.total, 1020);
    assert_eq!(footprint.build, 1000);
    assert_eq!(footprint.kept(), 20);
}

#[test]
fn measure_does_not_descend_into_a_matched_directory() {
    // A `target/` inside a `target/` is already counted once the outer one
    // is measured whole.
    let sandbox = Sandbox::new("storage-measure-nested");
    let root = sandbox.path("worktree");
    write(
        &root.join("target").join("nested").join("target").join("f"),
        500,
    );

    let footprint = measure(&root, &["target".to_owned()]);
    assert_eq!(footprint.total, 500);
    assert_eq!(footprint.build, 500);
}

#[test]
fn measure_ignores_symlinks() {
    let sandbox = Sandbox::new("storage-measure-symlink");
    let root = sandbox.path("worktree");
    fs::create_dir_all(&root).unwrap();
    let real = sandbox.path("elsewhere.txt");
    write(&real, 999);

    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, root.join("link.txt")).unwrap();

    let footprint = measure(&root, &[]);
    assert_eq!(footprint.total, 0, "a symlink's target is not counted");
}

#[test]
fn measure_on_a_missing_directory_is_zero() {
    let sandbox = Sandbox::new("storage-measure-missing");
    let footprint = measure(&sandbox.path("does-not-exist"), &[]);
    assert_eq!(footprint.total, 0);
    assert_eq!(footprint.build, 0);
}

#[test]
fn an_unparsable_build_dirs_pattern_is_dropped_without_matching_everything() {
    let sandbox = Sandbox::new("storage-measure-bad-pattern");
    let root = sandbox.path("worktree");
    write(&root.join("target").join("f"), 500);

    // `[` alone is not a valid glob; it must be skipped rather than panic or
    // match every directory.
    let footprint = measure(&root, &["[".to_owned()]);
    assert_eq!(footprint.build, 0);
    assert_eq!(footprint.total, 500);
}

#[test]
fn footprint_kept_saturates_rather_than_underflows() {
    let footprint = ket_core::storage::Footprint {
        total: 10,
        build: 50,
    };
    assert_eq!(footprint.kept(), 0);
}

#[test]
fn plan_clear_lists_matched_directories_largest_first() {
    let sandbox = Sandbox::new("storage-plan-sort");
    let root = sandbox.path("worktree");
    write(&root.join("target").join("f"), 100);
    write(&root.join("node_modules").join("f"), 500);

    let plan = plan_clear(&root, &["target".to_owned(), "node_modules".to_owned()]);
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].path, "node_modules");
    assert_eq!(plan[0].bytes, 500);
    assert_eq!(plan[1].path, "target");
    assert_eq!(plan[1].bytes, 100);
}

#[test]
fn plan_clear_breaks_size_ties_by_path() {
    let sandbox = Sandbox::new("storage-plan-tie");
    let root = sandbox.path("worktree");
    write(&root.join("zzz").join("f"), 100);
    write(&root.join("aaa").join("f"), 100);

    let plan = plan_clear(&root, &["zzz".to_owned(), "aaa".to_owned()]);
    assert_eq!(plan[0].path, "aaa");
    assert_eq!(plan[1].path, "zzz");
}

#[test]
fn plan_clear_only_names_directories_not_files() {
    let sandbox = Sandbox::new("storage-plan-files-only");
    let root = sandbox.path("worktree");
    // A file that happens to match the pattern's name is not a directory to
    // clear.
    write(&root.join("target"), 10);

    let plan = plan_clear(&root, &["target".to_owned()]);
    assert!(plan.is_empty());
}

#[test]
fn clear_removes_planned_directories_and_reports_freed_bytes() {
    let sandbox = Sandbox::new("storage-clear-happy");
    let root = sandbox.path("worktree");
    write(&root.join("target").join("f"), 100);

    let plan = plan_clear(&root, &["target".to_owned()]);
    let result = clear(&root, &plan);

    assert_eq!(result.freed, 100);
    assert_eq!(result.failed, 0);
    assert!(!root.join("target").exists());
}

#[test]
fn clear_leaves_unplanned_directories_alone() {
    let sandbox = Sandbox::new("storage-clear-scoped");
    let root = sandbox.path("worktree");
    write(&root.join("target").join("f"), 100);
    write(&root.join("node_modules").join("f"), 50);

    // Only `target` was planned, so `node_modules` must survive even though
    // it would also match a build-dirs pattern.
    let plan = plan_clear(&root, &["target".to_owned()]);
    clear(&root, &plan);

    assert!(root.join("node_modules").exists());
}

#[test]
fn clear_counts_an_already_gone_directory_as_neither_freed_nor_failed() {
    let sandbox = Sandbox::new("storage-clear-already-gone");
    let root = sandbox.path("worktree");
    fs::create_dir_all(&root).unwrap();

    let plan = vec![Clearable {
        path: "target".to_owned(),
        bytes: 100,
    }];
    let result = clear(&root, &plan);

    assert_eq!(result.freed, 0);
    assert_eq!(result.failed, 0);
}

#[test]
fn clear_refuses_a_planned_path_that_escapes_the_worktree() {
    let sandbox = Sandbox::new("storage-clear-escape");
    let root = sandbox.path("worktree");
    fs::create_dir_all(&root).unwrap();
    write(&sandbox.path("outside").join("f"), 10);

    let plan = vec![Clearable {
        path: "../outside".to_owned(),
        bytes: 10,
    }];
    let result = clear(&root, &plan);

    assert_eq!(result.failed, 1);
    assert_eq!(result.freed, 0);
    assert!(sandbox.path("outside").exists(), "the escape was refused");
}

#[test]
fn format_bytes_picks_the_largest_unit_that_fits() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(999), "999 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
}

#[test]
fn format_bytes_drops_the_decimal_at_ten_and_above() {
    assert_eq!(format_bytes(9 * 1024 * 1024 + 900_000), "9.9 MB");
    assert_eq!(format_bytes(10 * 1024 * 1024), "10 MB");
    assert_eq!(format_bytes(142 * 1024 * 1024), "142 MB");
}
