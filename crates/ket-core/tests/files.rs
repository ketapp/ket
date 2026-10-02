//! What the explorer and the file finder see.
//!
//! Checked against a real repository built by the real `git` binary, and in
//! two places against `git`'s own answer for what is ignored — the point of
//! using git's rules is to agree with git, which is not something a
//! hand-written fixture can assert on its own.

mod common;

use std::fs;

use common::{Sandbox, commit_file, git, init_repo};
use ket_core::files::{EntryKind, MAX_DIR_ENTRIES, index, read_dir, read_dirs};

/// Names of the entries in a directory, in the order the explorer draws them.
fn names(root: &std::path::Path, rela: &str) -> Vec<String> {
    read_dir(root, rela)
        .expect("listing")
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

/// Whether the listing of `rela` marks `name` as ignored.
///
/// Panics when it is not listed at all, which is the failure the marking is
/// there to prevent.
fn ignored(root: &std::path::Path, rela: &str, name: &str) -> bool {
    read_dir(root, rela)
        .expect("listing")
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed in {rela:?}"))
        .ignored
}

#[test]
fn directories_come_before_files_and_both_sort_by_name() {
    let sandbox = Sandbox::new("files-order");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("zebra")).unwrap();
    fs::create_dir_all(repo.join("Alpha")).unwrap();
    fs::write(repo.join("beta.rs"), "").unwrap();
    fs::write(repo.join("Aardvark.rs"), "").unwrap();

    // Directories first; within each group, case-insensitively by name, so
    // `Alpha` does not sort above `zebra` merely for being capitalised.
    assert_eq!(
        names(&repo, ""),
        ["Alpha", "zebra", "Aardvark.rs", "beta.rs", "README.md"]
    );
}

#[test]
fn the_git_directory_is_never_listed() {
    let sandbox = Sandbox::new("files-dotgit");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    assert!(repo.join(".git").exists(), "fixture has no .git");
    assert!(!names(&repo, "").contains(&".git".to_owned()));
}

#[test]
fn an_ignored_path_is_listed_but_marked_and_git_agrees_it_is_ignored() {
    let sandbox = Sandbox::new("files-ignored");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, ".gitignore", "target/\n*.log\n", "ignore rules");
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target/binary"), "").unwrap();
    fs::write(repo.join("noisy.log"), "").unwrap();
    fs::write(repo.join("kept.rs"), "").unwrap();

    // The tree shows what is on disk; the mark is what says how git sees it.
    let listed = names(&repo, "");
    assert!(listed.contains(&"kept.rs".to_owned()), "{listed:?}");
    assert!(listed.contains(&"target".to_owned()), "{listed:?}");
    assert!(listed.contains(&"noisy.log".to_owned()), "{listed:?}");

    assert!(ignored(&repo, "", "target"));
    assert!(ignored(&repo, "", "noisy.log"));
    assert!(!ignored(&repo, "", "kept.rs"));

    // Marked for the explorer, still absent from the finder: an ignored
    // directory is most of what a repository holds and none of what anyone
    // searches.
    let indexed = index(&repo).unwrap();
    assert!(
        !indexed
            .paths()
            .iter()
            .any(|path| path.starts_with("target/")),
        "{:?}",
        indexed.paths()
    );

    // The rules we applied are git's, so git must reach the same verdict.
    let seen = git(&repo, &["status", "--porcelain", "--ignored"]);
    assert!(seen.contains("target/"), "{seen}");
    assert!(seen.contains("noisy.log"), "{seen}");
}

#[test]
fn a_nested_gitignore_applies_to_its_own_directory() {
    let sandbox = Sandbox::new("files-nested-ignore");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("crate/src")).unwrap();
    commit_file(&repo, "crate/.gitignore", "generated.rs\n", "nested rules");
    fs::write(repo.join("crate/generated.rs"), "").unwrap();
    fs::write(repo.join("crate/src/generated.rs"), "").unwrap();
    fs::write(repo.join("generated.rs"), "").unwrap();

    // The rule is scoped to `crate/`, so the root's file of the same name is
    // not marked by it.
    assert!(!ignored(&repo, "", "generated.rs"));
    assert!(ignored(&repo, "crate", "generated.rs"));
    // And it applies below `crate/` too, which is git's behaviour for a bare
    // name without a slash.
    assert!(ignored(&repo, "crate/src", "generated.rs"));
}

#[test]
fn an_empty_directory_is_listed_because_it_is_where_a_file_is_about_to_go() {
    let sandbox = Sandbox::new("files-empty-dir");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("scratch")).unwrap();

    // Git cannot track it and the finder cannot offer it, but the explorer
    // shows the filesystem, not the index.
    assert!(names(&repo, "").contains(&"scratch".to_owned()));
    assert!(read_dir(&repo, "scratch").unwrap().is_empty());
}

#[test]
fn a_symlink_is_reported_rather_than_followed() {
    let sandbox = Sandbox::new("files-symlink");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    let outside = sandbox.path("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "").unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("link")).unwrap();

    let entries = read_dir(&repo, "").unwrap();
    let link = entries
        .iter()
        .find(|entry| entry.name == "link")
        .expect("link missing");
    assert_eq!(link.kind, EntryKind::Symlink);

    // A symlink is not a directory the walk descends into, so nothing outside
    // the worktree reaches the finder.
    let indexed = index(&repo).unwrap();
    assert!(
        !indexed.paths().iter().any(|path| path.contains("secret")),
        "{:?}",
        indexed.paths()
    );
}

#[test]
fn a_path_that_climbs_out_of_the_worktree_is_refused() {
    let sandbox = Sandbox::new("files-escape");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::write(sandbox.path("outside.txt"), "").unwrap();

    assert!(read_dir(&repo, "..").is_err());
    assert!(read_dir(&repo, "src/../..").is_err());
}

#[test]
fn the_index_holds_tracked_and_new_files_but_not_ignored_ones() {
    let sandbox = Sandbox::new("files-index");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, ".gitignore", "target/\n", "ignore rules");
    commit_file(&repo, "src/main.rs", "fn main() {}\n", "source");
    fs::create_dir_all(repo.join("target")).unwrap();
    fs::write(repo.join("target/binary"), "").unwrap();
    // Untracked, and the file most likely to be the one you are looking for.
    fs::write(repo.join("src/scratch.rs"), "").unwrap();

    let indexed = index(&repo).unwrap();
    let paths = indexed.paths();
    assert!(paths.contains(&"src/main.rs".to_owned()), "{paths:?}");
    assert!(paths.contains(&"src/scratch.rs".to_owned()), "{paths:?}");
    assert!(paths.contains(&".gitignore".to_owned()), "{paths:?}");
    assert!(!paths.iter().any(|path| path.starts_with("target/")));
    assert!(!paths.iter().any(|path| path.starts_with(".git/")));
    assert!(!indexed.truncated());

    // Sorted, so the finder's output is stable between runs.
    let mut sorted = paths.to_vec();
    sorted.sort();
    assert_eq!(paths, sorted.as_slice());
}

#[test]
fn a_directory_that_is_not_a_repository_still_lists_and_indexes() {
    let sandbox = Sandbox::new("files-no-repo");
    let plain = sandbox.path("plain");
    fs::create_dir_all(plain.join("nested")).unwrap();
    fs::write(plain.join("a.txt"), "").unwrap();
    fs::write(plain.join("nested/b.txt"), "").unwrap();

    assert_eq!(names(&plain, ""), ["nested", "a.txt"]);
    let indexed = index(&plain).unwrap();
    assert_eq!(indexed.paths(), ["a.txt", "nested/b.txt"]);
}

#[test]
fn read_dirs_lists_several_directories_with_one_shared_ignore_pass() {
    let sandbox = Sandbox::new("files-read-dirs");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::create_dir_all(repo.join("docs")).unwrap();
    fs::write(repo.join("src/main.rs"), "").unwrap();
    fs::write(repo.join("docs/readme.md"), "").unwrap();
    fs::write(repo.join(".gitignore"), "src/main.rs\n").unwrap();

    let listings = read_dirs(&repo, &["src".to_owned(), "docs".to_owned(), "".to_owned()]);

    assert_eq!(listings.len(), 3);
    let src = listings
        .iter()
        .find(|(rela, _)| rela == "src")
        .unwrap()
        .1
        .as_ref()
        .expect("src listing");
    assert!(
        src.iter().find(|e| e.name == "main.rs").unwrap().ignored,
        "the shared ignore pass should still mark files per directory"
    );

    let docs = listings
        .iter()
        .find(|(rela, _)| rela == "docs")
        .unwrap()
        .1
        .as_ref()
        .expect("docs listing");
    assert!(!docs.iter().find(|e| e.name == "readme.md").unwrap().ignored);
}

#[test]
fn read_dirs_reports_a_missing_directory_without_losing_the_others() {
    let sandbox = Sandbox::new("files-read-dirs-missing");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/main.rs"), "").unwrap();

    let listings = read_dirs(&repo, &["src".to_owned(), "does-not-exist".to_owned()]);

    let (_, src_result) = listings.iter().find(|(rela, _)| rela == "src").unwrap();
    assert!(src_result.is_ok(), "the directory that exists still lists");

    let (_, missing_result) = listings
        .iter()
        .find(|(rela, _)| rela == "does-not-exist")
        .unwrap();
    assert!(missing_result.is_err(), "the missing one is its own error");
}

#[test]
fn a_directory_listing_is_bounded_at_max_dir_entries() {
    let sandbox = Sandbox::new("files-dir-cap");
    let plain = sandbox.path("plain");
    fs::create_dir_all(&plain).unwrap();
    for i in 0..MAX_DIR_ENTRIES + 5 {
        fs::write(plain.join(format!("f{i:05}.txt")), "").unwrap();
    }

    let entries = read_dir(&plain, "").unwrap();
    assert_eq!(entries.len(), MAX_DIR_ENTRIES);
}

#[test]
fn a_path_with_a_trailing_slash_still_resolves() {
    let sandbox = Sandbox::new("files-trailing-slash");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("src/main.rs"), "").unwrap();

    assert_eq!(names(&repo, "src/"), ["main.rs"]);
}
