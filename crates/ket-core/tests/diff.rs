//! `ket diff` against real `git`.
//!
//! Invariant 5: the diff is checked against what git actually reports, not
//! against our beliefs about diffing. Agreement is measured with
//! `git diff --numstat`, which is the precise form of "matches `git diff` in
//! meaning" — byte equality with git's output is not the goal, since git also
//! emits `index` and mode lines that carry no information a reviewer reads.

mod common;

use std::collections::BTreeMap;
use std::fs;

use common::{Sandbox, commit_file, git, init_repo, rev};
use ket_core::diff::{self, LineMark};
use ket_core::status::ChangeKind;

/// Per-path `(added, removed)` as `git diff --numstat <base>` reports it.
fn git_numstat(repo: &std::path::Path, base: &str) -> BTreeMap<String, (usize, usize)> {
    git(repo, &["diff", "--numstat", base])
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let added = parts.next()?;
            let removed = parts.next()?;
            let path = parts.next()?;
            // git writes "-" for binary files.
            Some((
                path.to_owned(),
                (
                    added.parse().unwrap_or_default(),
                    removed.parse().unwrap_or_default(),
                ),
            ))
        })
        .collect()
}

/// Per-path `(added, removed)` as ket reports it.
fn ket_numstat(d: &diff::WorktreeDiff) -> BTreeMap<String, (usize, usize)> {
    d.files
        .iter()
        .map(|f| (f.path.clone(), f.line_counts()))
        .collect()
}

#[test]
fn ket_and_git_agree_on_a_committed_change() {
    let sandbox = Sandbox::new("diff-committed");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    commit_file(&repo, "src/lib.rs", "one\ntwo\nthree\n", "add lib");
    let base = "main";

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "src/lib.rs", "one\nTWO\nthree\nfour\n", "edit lib");

    let ours = diff::of_worktree(&repo, base).expect("diff");

    assert_eq!(ket_numstat(&ours), git_numstat(&repo, base));
}

#[test]
fn uncommitted_work_counts_the_same_as_committed() {
    let sandbox = Sandbox::new("diff-uncommitted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "src/lib.rs", "one\ntwo\nthree\n", "add lib");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);

    // Left dirty on purpose: agents differ in whether they commit, and a
    // reviewer comparing three attempts should not have to care.
    fs::write(repo.join("src/lib.rs"), "one\nTWO\nthree\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    assert_eq!(ket_numstat(&ours), git_numstat(&repo, "main"));
    assert_eq!(ours.total_files, 1);
    assert_eq!(ours.files[0].line_counts(), (1, 1));
}

#[test]
fn a_change_split_across_a_commit_and_the_working_tree_is_reported_once() {
    let sandbox = Sandbox::new("diff-split");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "a.txt", "1\n2\n3\n", "add a");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "a.txt", "1\nTWO\n3\n", "commit half");
    fs::write(repo.join("a.txt"), "1\nTWO\nTHREE\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // One entry, not two: the file changed once as far as the base is concerned.
    assert_eq!(ours.files.len(), 1);
    assert_eq!(ket_numstat(&ours), git_numstat(&repo, "main"));
}

#[test]
fn an_added_file_is_reported_as_added() {
    let sandbox = Sandbox::new("diff-added");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "new.txt", "hello\n", "add new");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let file = ours
        .files
        .iter()
        .find(|f| f.path == "new.txt")
        .expect("new.txt");

    assert_eq!(file.kind, ChangeKind::Added);
    assert_eq!(file.line_counts(), (1, 0));
    assert!(file.to_unified().contains("--- /dev/null"));
}

#[test]
fn a_deleted_file_is_reported_as_deleted() {
    let sandbox = Sandbox::new("diff-deleted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "gone.txt", "bye\n", "add gone");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    git(&repo, &["rm", "--quiet", "gone.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "remove gone"]);

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let file = ours
        .files
        .iter()
        .find(|f| f.path == "gone.txt")
        .expect("gone.txt");

    assert_eq!(file.kind, ChangeKind::Deleted);
    assert_eq!(file.line_counts(), (0, 1));
    assert!(file.to_unified().contains("+++ /dev/null"));
}

#[test]
fn an_untracked_file_is_included_even_though_git_diff_would_skip_it() {
    let sandbox = Sandbox::new("diff-untracked");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::write(repo.join("scratch.rs"), "fn main() {}\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // A deliberate departure from `git diff`, which ignores untracked files.
    // An agent that writes a new file without staging it has still done work,
    // and a review that silently omitted it would be wrong.
    assert!(git_numstat(&repo, "main").is_empty());
    let file = ours
        .files
        .iter()
        .find(|f| f.path == "scratch.rs")
        .expect("scratch.rs present");
    assert_eq!(file.kind, ChangeKind::Untracked);
    assert_eq!(file.line_counts(), (1, 0));
}

#[test]
fn an_untracked_directory_is_expanded_into_its_files() {
    let sandbox = Sandbox::new("diff-untracked-dir");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::create_dir_all(repo.join("newmod/deep")).expect("mkdir");
    fs::write(repo.join("newmod/mod.rs"), "pub mod deep;\n").expect("write");
    fs::write(repo.join("newmod/deep/inner.rs"), "pub fn f() {}\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let paths: Vec<&str> = ours.files.iter().map(|f| f.path.as_str()).collect();

    // git status collapses this to a single "newmod/" entry. A diff that
    // reported one nameless directory would hide the entire change.
    assert!(paths.contains(&"newmod/mod.rs"), "{paths:?}");
    assert!(paths.contains(&"newmod/deep/inner.rs"), "{paths:?}");
    assert!(!paths.iter().any(|p| p.ends_with('/')), "{paths:?}");
}

#[test]
fn a_directory_is_never_reported_as_a_changed_file() {
    let sandbox = Sandbox::new("diff-nodirs");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "src/lib.rs", "one\n", "add lib");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "src/lib.rs", "two\n", "edit lib");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let paths: Vec<&str> = ours.files.iter().map(|f| f.path.as_str()).collect();

    // The tree-to-tree walk reports "src" alongside "src/lib.rs"; only the
    // blob is a change anyone can read.
    assert_eq!(paths, vec!["src/lib.rs"], "{paths:?}");
}

#[test]
fn a_binary_file_is_reported_without_hunks() {
    let sandbox = Sandbox::new("diff-binary");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::write(repo.join("blob.bin"), [0u8, 1, 2, 0, 255, 0]).expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "add binary"]);

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let file = ours
        .files
        .iter()
        .find(|f| f.path == "blob.bin")
        .expect("blob.bin");

    assert!(file.binary);
    assert!(file.hunks.is_empty());
    assert!(file.to_unified().contains("Binary files"));
}

#[test]
fn an_unchanged_worktree_produces_an_empty_diff() {
    let sandbox = Sandbox::new("diff-clean");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    assert!(ours.is_empty());
    assert!(ours.files.is_empty());
    assert_eq!(ours.to_unified(), "");
}

#[test]
fn a_file_touched_and_reverted_is_not_reported() {
    let sandbox = Sandbox::new("diff-reverted");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "a.txt", "original\n", "add a");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::write(repo.join("a.txt"), "changed\n").expect("write");
    fs::write(repo.join("a.txt"), "original\n").expect("revert");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // The path is a candidate — status noticed it — but the contents match, so
    // it must not appear. Comparing content rather than trusting the candidate
    // set is what makes that true.
    assert!(ours.is_empty(), "expected no changes, got {:?}", ours.files);
}

#[test]
fn paths_with_spaces_survive_the_round_trip() {
    let sandbox = Sandbox::new("diff-spaces");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "a dir/a file.txt", "one\n", "add spaced");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::write(repo.join("a dir/a file.txt"), "one\ntwo\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    assert_eq!(ket_numstat(&ours), git_numstat(&repo, "main"));
    assert_eq!(ours.files[0].path, "a dir/a file.txt");
}

#[test]
fn the_unified_rendering_carries_hunk_headers_and_prefixes() {
    let sandbox = Sandbox::new("diff-render");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "a.txt", "1\n2\n3\n4\n5\n", "add a");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::write(repo.join("a.txt"), "1\n2\nTHREE\n4\n5\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let text = ours.to_unified();

    assert!(text.contains("diff --git a/a.txt b/a.txt"), "{text}");
    assert!(text.contains("--- a/a.txt"), "{text}");
    assert!(text.contains("+++ b/a.txt"), "{text}");
    assert!(text.contains("@@ -"), "{text}");
    assert!(text.contains("-3"), "{text}");
    assert!(text.contains("+THREE"), "{text}");
    // Context lines keep their leading space.
    assert!(text.contains(" 1"), "{text}");
}

#[test]
fn a_base_that_does_not_resolve_is_an_error_rather_than_an_empty_diff() {
    let sandbox = Sandbox::new("diff-badbase");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let err = diff::of_worktree(&repo, "no-such-ref").expect_err("should fail");

    // Silently returning "no changes" for a base that is gone would read as
    // "the agent did nothing", which is the most misleading answer available.
    assert!(
        err.to_string().to_lowercase().contains("base"),
        "unhelpful error: {err}"
    );
}

// ---------------------------------------------------------------------------
// Renames
// ---------------------------------------------------------------------------

#[test]
fn a_move_with_no_edits_is_one_rename_rather_than_a_deletion_and_an_addition() {
    let sandbox = Sandbox::new("diff-rename");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "old.rs", "one\ntwo\nthree\n", "add old");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    git(&repo, &["mv", "old.rs", "new.rs"]);
    git(&repo, &["commit", "--quiet", "-m", "move it"]);

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // git pairs the halves too, and counts the result as one file with nothing
    // added and nothing removed. `-M` is git's default since 2.9; spelling it
    // out keeps the test from depending on the developer's `diff.renames`.
    assert_eq!(
        git(&repo, &["diff", "--numstat", "-M", "main"]).trim(),
        "0\t0\told.rs => new.rs"
    );

    assert_eq!(ours.total_files, 1, "{:?}", ours.files);
    let file = &ours.files[0];
    assert_eq!(file.kind, ChangeKind::Renamed);
    assert_eq!(file.path, "new.rs");
    assert_eq!(file.old_path.as_deref(), Some("old.rs"));
    assert!(file.hunks.is_empty());
    assert_eq!(file.line_counts(), (0, 0));

    let text = file.to_unified();
    assert!(text.contains("diff --git a/old.rs b/new.rs"), "{text}");
    assert!(text.contains("rename from old.rs"), "{text}");
    assert!(text.contains("rename to new.rs"), "{text}");
}

#[test]
fn a_move_left_dirty_is_paired_the_same_as_a_committed_one() {
    let sandbox = Sandbox::new("diff-rename-dirty");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "old.rs", "one\ntwo\nthree\n", "add old");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::rename(repo.join("old.rs"), repo.join("new.rs")).expect("rename");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // git sees half of it: the deletion is in the working tree, the arrival is
    // untracked, and `git diff` does not look at untracked files, so its rename
    // detection has nothing to pair the deletion with.
    assert_eq!(
        git_numstat(&repo, "main").into_iter().collect::<Vec<_>>(),
        vec![("old.rs".to_owned(), (0, 3))]
    );

    // Pairing happens after both halves are assembled, so whether the agent
    // committed the move makes no difference to what a reviewer sees.
    assert_eq!(ours.total_files, 1, "{:?}", ours.files);
    assert_eq!(ours.files[0].kind, ChangeKind::Renamed);
    assert_eq!(ours.files[0].path, "new.rs");
    assert_eq!(ours.files[0].old_path.as_deref(), Some("old.rs"));
}

#[test]
fn a_move_with_edits_is_a_deletion_and_an_addition_where_git_calls_it_a_rename() {
    let sandbox = Sandbox::new("diff-rename-edited");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(
        &repo,
        "old.rs",
        "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n",
        "add old",
    );

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    git(&repo, &["mv", "old.rs", "new.rs"]);
    fs::write(
        repo.join("new.rs"),
        "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\n",
    )
    .expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "move and edit"]);

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // git's diffcore-rename pairs these, at 87% similar.
    let theirs = git(&repo, &["diff", "--numstat", "-M", "main"]);
    assert!(theirs.contains("old.rs => new.rs"), "{theirs}");

    // A deliberate departure: ket only claims a rename it can prove by content,
    // so a move with edits reads as a deletion and an addition. The reasoning is
    // on `collapse_renames` in `diff.rs` — briefly, similarity scoring is
    // quadratic in the size of the change, and git stops doing it silently once
    // the change is large enough, which would make the category a file is
    // reported under depend on how much else the agent touched.
    let kinds: Vec<(&str, ChangeKind)> = ours
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("new.rs", ChangeKind::Added),
            ("old.rs", ChangeKind::Deleted)
        ]
    );
    assert!(ours.files.iter().all(|f| f.old_path.is_none()));
}

// ---------------------------------------------------------------------------
// Submodules
// ---------------------------------------------------------------------------

/// Records a gitlink at `sub` pointing to commit `at`, and commits it.
///
/// Built by hand rather than with `git submodule add`, which would have to clone
/// over the `file` transport that git refuses by default.
fn add_submodule(repo: &std::path::Path, at: &str) {
    fs::write(
        repo.join(".gitmodules"),
        "[submodule \"sub\"]\n\tpath = sub\n\turl = ./sub\n",
    )
    .expect("write .gitmodules");
    git(repo, &["add", ".gitmodules"]);
    git(
        repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{at},sub"),
        ],
    );
    git(repo, &["commit", "--quiet", "-m", "add submodule"]);
}

#[test]
fn a_submodule_pointer_change_is_reported_the_way_git_reports_it() {
    let sandbox = Sandbox::new("diff-submodule");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let sub = repo.join("sub");
    init_repo(&sub);
    commit_file(&sub, "s.txt", "one\n", "s1");
    let first = rev(&sub, "HEAD");
    commit_file(&sub, "s.txt", "two\n", "s2");
    let second = rev(&sub, "HEAD");

    add_submodule(&repo, &second);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    git(&sub, &["checkout", "--quiet", &first]);

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // A gitlink is a tree entry holding a commit id that belongs to another
    // repository. Reading it as a blob finds nothing, which is how a submodule
    // bump used to disappear from the diff entirely. git renders it as a
    // one-line change, and rendering the same line is what makes --numstat
    // agree rather than being a coincidence.
    assert_eq!(ket_numstat(&ours), git_numstat(&repo, "main"));

    let file = ours.files.iter().find(|f| f.path == "sub").expect("sub");
    assert_eq!(file.kind, ChangeKind::Modified);
    assert_eq!(file.line_counts(), (1, 1));

    let text = file.to_unified();
    assert!(
        text.contains(&format!("-Subproject commit {second}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("+Subproject commit {first}")),
        "{text}"
    );
}

#[test]
fn a_submodule_that_was_never_checked_out_is_not_reported_as_deleted() {
    let sandbox = Sandbox::new("diff-submodule-empty");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let sub = repo.join("sub");
    init_repo(&sub);
    let head = rev(&sub, "HEAD");
    add_submodule(&repo, &head);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    // What a clone without `--recurse-submodules` leaves behind.
    fs::remove_dir_all(&sub).expect("remove sub");
    fs::create_dir(&sub).expect("recreate sub");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // "the agent removed the submodule" and "nobody ran submodule update" are
    // very different reports, and only one of them is true.
    assert!(git_numstat(&repo, "main").is_empty());
    assert!(ours.is_empty(), "{:?}", ours.files);
}

#[test]
fn an_untracked_nested_repository_is_one_entry_rather_than_all_of_its_files() {
    let sandbox = Sandbox::new("diff-nested-repo");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    let vendor = repo.join("vendor");
    init_repo(&vendor);
    commit_file(&vendor, "a.txt", "one\n", "a");
    let head = rev(&vendor, "HEAD");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let paths: Vec<&str> = ours.files.iter().map(|f| f.path.as_str()).collect();

    // An agent that cloned something into the worktree made one change worth
    // reviewing. Expanding the directory would fill the diff with that
    // repository's files, and with the contents of its `.git`.
    assert_eq!(paths, vec!["vendor"], "{paths:?}");
    assert_eq!(ours.files[0].kind, ChangeKind::Untracked);
    let text = ours.files[0].to_unified();
    assert!(
        text.contains(&format!("+Subproject commit {head}")),
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// Symlinks
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_file_that_becomes_a_symlink_and_the_reverse_are_both_type_changes() {
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new("diff-typechange");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    fs::write(repo.join("target.txt"), "target contents\n").expect("write");
    fs::write(repo.join("becomes-link.txt"), "hello\nworld\n").expect("write");
    symlink("target.txt", repo.join("becomes-file.txt")).expect("symlink");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "add both"]);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::remove_file(repo.join("becomes-link.txt")).expect("remove");
    symlink("target.txt", repo.join("becomes-link.txt")).expect("symlink");
    fs::remove_file(repo.join("becomes-file.txt")).expect("remove");
    fs::write(repo.join("becomes-file.txt"), "now a real file\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    assert_eq!(ket_numstat(&ours), git_numstat(&repo, "main"));

    let link = ours
        .files
        .iter()
        .find(|f| f.path == "becomes-link.txt")
        .expect("becomes-link.txt");
    assert_eq!(link.kind, ChangeKind::TypeChange);
    // A symlink's content is the path it points at, which is what git stores.
    // Following the link instead would have produced the contents of a file
    // nobody touched.
    let text = link.to_unified();
    assert!(text.contains("+target.txt"), "{text}");
    assert!(!text.contains("target contents"), "{text}");

    let file = ours
        .files
        .iter()
        .find(|f| f.path == "becomes-file.txt")
        .expect("becomes-file.txt");
    assert_eq!(file.kind, ChangeKind::TypeChange);
    assert!(file.to_unified().contains("-target.txt"));

    // git renders each of these as a deletion followed by an addition under the
    // same path. One entry per path is what a file list wants, and the line
    // counts come out the same either way, which is why --numstat still agrees.
    assert_eq!(ours.total_files, 2, "{:?}", ours.files);
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_is_an_addition_rather_than_nothing_at_all() {
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new("diff-dangling");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    symlink("nowhere.txt", repo.join("broken.lnk")).expect("symlink");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // Asking whether the path "is a file" follows the link, and a link to
    // nothing is not a file — so this used to read as no change at all.
    let file = ours
        .files
        .iter()
        .find(|f| f.path == "broken.lnk")
        .expect("broken.lnk");
    assert_eq!(file.kind, ChangeKind::Untracked);
    assert_eq!(file.line_counts(), (1, 0));
    assert!(file.to_unified().contains("+nowhere.txt"));
}

// ---------------------------------------------------------------------------
// Line endings
// ---------------------------------------------------------------------------

#[test]
fn a_crlf_file_reports_only_the_line_that_changed() {
    let sandbox = Sandbox::new("diff-crlf");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.autocrlf", "false"]);
    commit_file(&repo, "crlf.txt", "a\r\nb\r\nc\r\nd\r\n", "add crlf");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    fs::write(repo.join("crlf.txt"), "a\r\nB\r\nc\r\nd\r\n").expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");

    assert_eq!(ket_numstat(&ours), git_numstat(&repo, "main"));
    assert_eq!(ours.files[0].line_counts(), (1, 1));
    // The carriage returns belong to the line ending, not to the text, and a
    // renderer that kept them would put a stray CR in the middle of a hunk.
    assert!(!ours.to_unified().contains('\r'));
}

#[test]
fn core_autocrlf_does_not_turn_an_untouched_file_into_a_whole_file_rewrite() {
    let sandbox = Sandbox::new("diff-autocrlf");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.autocrlf", "true"]);
    commit_file(&repo, "lf.txt", "a\nb\nc\nd\n", "add lf");

    // With autocrlf on, git stores LF and checks out CRLF. Force the checkout so
    // the working tree holds what a colleague on Windows would have.
    fs::remove_file(repo.join("lf.txt")).expect("remove");
    git(&repo, &["checkout", "--quiet", "--", "lf.txt"]);
    let on_disk = fs::read(repo.join("lf.txt")).expect("read");
    assert!(on_disk.windows(2).any(|w| w == b"\r\n"), "expected CRLF");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // Comparing the stored blob against the bytes on disk compares the two
    // halves of a conversion git is performing on purpose, and reports every
    // line of an untouched file as rewritten.
    assert!(git_numstat(&repo, "main").is_empty());
    assert!(ours.is_empty(), "{:?}", ours.files);
}

#[test]
fn a_gitattributes_eol_setting_does_not_turn_an_untouched_file_into_a_rewrite() {
    let sandbox = Sandbox::new("diff-eol-attr");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.autocrlf", "false"]);

    fs::write(repo.join(".gitattributes"), "*.txt text eol=crlf\n").expect("write");
    commit_file(&repo, "win.txt", "a\nb\nc\nd\n", "add win");

    fs::remove_file(repo.join("win.txt")).expect("remove");
    git(&repo, &["checkout", "--quiet", "--", "win.txt"]);
    let on_disk = fs::read(repo.join("win.txt")).expect("read");
    assert!(on_disk.windows(2).any(|w| w == b"\r\n"), "expected CRLF");

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    let ours = diff::of_worktree(&repo, "main").expect("diff");

    // The same conversion, driven by attributes rather than by config: whichever
    // way a repository asks for it, the answer must not change.
    assert!(git_numstat(&repo, "main").is_empty());
    assert!(ours.is_empty(), "{:?}", ours.files);
}

// ---------------------------------------------------------------------------
// Very large files
// ---------------------------------------------------------------------------

#[test]
fn a_file_over_the_read_limit_is_reported_as_changed_without_being_read() {
    let sandbox = Sandbox::new("diff-huge");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    let huge = "x".repeat(diff::MAX_FILE_BYTES as usize + 1);
    fs::write(repo.join("huge.txt"), &huge).expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let file = ours
        .files
        .iter()
        .find(|f| f.path == "huge.txt")
        .expect("huge.txt");

    // Its diff could not be kept even if it were computed, so it is neither read
    // nor interned — but it is still reported, because a reviewer needs to know
    // the file changed.
    assert!(file.truncated);
    assert!(file.hunks.is_empty());
    assert_eq!(file.kind, ChangeKind::Untracked);
    assert!(file.to_unified().contains("diff omitted"));
}

#[test]
fn one_large_file_does_not_starve_the_files_after_it() {
    let sandbox = Sandbox::new("diff-budget-order");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    git(&repo, &["checkout", "--quiet", "-b", "feature"]);

    // Sorted by path, so the big one is diffed first and gets first call on the
    // shared budget. Big enough that spending it greedily would leave the second
    // file nothing, small enough to still be worth reading.
    let line = format!("{}\n", "x".repeat(99));
    let big = line.repeat(diff::MAX_DIFF_BYTES * 15 / 16 / line.len());
    let small = line.repeat(diff::MAX_DIFF_BYTES / 8 / line.len());
    assert!(big.len() as u64 <= diff::MAX_FILE_BYTES);
    assert!(small.len() > diff::MAX_DIFF_BYTES - big.len());

    fs::write(repo.join("a-big.txt"), &big).expect("write");
    fs::write(repo.join("z-small.txt"), &small).expect("write");

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    let big_file = ours
        .files
        .iter()
        .find(|f| f.path == "a-big.txt")
        .expect("a-big.txt");
    let small_file = ours
        .files
        .iter()
        .find(|f| f.path == "z-small.txt")
        .expect("z-small.txt");

    // The big one is over its per-file share, so it is dropped whole and spends
    // nothing, rather than being kept and leaving the next file blank.
    assert!(big_file.truncated);
    assert!(big_file.hunks.is_empty());
    assert!(!small_file.truncated, "the second file was starved");
    assert_eq!(
        small_file.line_counts(),
        (small.lines().count(), 0),
        "the second file lost lines"
    );

    let kept: usize = ours
        .files
        .iter()
        .flat_map(|f| f.hunks.iter())
        .flat_map(|h| h.lines.iter())
        .map(|l| l.text.len() + 1)
        .sum();
    assert!(kept <= diff::MAX_DIFF_BYTES, "{kept} bytes kept");
}

#[test]
fn more_files_than_the_cap_are_reported_as_truncated_with_the_true_total() {
    let sandbox = Sandbox::new("diff-file-cap");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);

    for i in 0..diff::MAX_DIFF_FILES + 1 {
        fs::write(repo.join(format!("f{i:04}.txt")), "content\n").expect("write");
    }

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    assert_eq!(ours.total_files, diff::MAX_DIFF_FILES + 1);
    assert_eq!(ours.files.len(), diff::MAX_DIFF_FILES);
    assert!(ours.truncated);
}

#[test]
fn exactly_the_cap_is_not_truncated() {
    let sandbox = Sandbox::new("diff-file-cap-exact");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);

    for i in 0..diff::MAX_DIFF_FILES {
        fs::write(repo.join(format!("f{i:04}.txt")), "content\n").expect("write");
    }

    let ours = diff::of_worktree(&repo, "main").expect("diff");
    assert_eq!(ours.total_files, diff::MAX_DIFF_FILES);
    assert_eq!(ours.files.len(), diff::MAX_DIFF_FILES);
    assert!(!ours.truncated);
}

// ---------------------------------------------------------------------------
// file_of_worktree
// ---------------------------------------------------------------------------

#[test]
fn file_of_worktree_matches_the_same_files_entry_in_the_whole_diff() {
    let sandbox = Sandbox::new("diff-single-file");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    commit_file(&repo, "src/lib.rs", "one\ntwo\nthree\n", "add lib");
    git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    commit_file(&repo, "src/lib.rs", "one\nTWO\nthree\n", "edit lib");
    fs::write(repo.join("untouched.txt"), "same\n").expect("write");
    git(&repo, &["add", "untouched.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "add untouched", "-a"]);

    let whole = diff::of_worktree(&repo, "main").expect("diff");
    let from_whole = whole
        .files
        .iter()
        .find(|f| f.path == "src/lib.rs")
        .expect("src/lib.rs in the whole diff");

    let alone = diff::file_of_worktree(&repo, "main", "src/lib.rs")
        .expect("diff")
        .expect("src/lib.rs changed");

    assert_eq!(alone.kind, from_whole.kind);
    assert_eq!(alone.line_counts(), from_whole.line_counts());
}

#[test]
fn file_of_worktree_is_none_for_a_file_that_did_not_change() {
    let sandbox = Sandbox::new("diff-single-file-unchanged");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "same.txt", "content\n", "add same");

    let result = diff::file_of_worktree(&repo, "main", "same.txt").expect("diff");
    assert!(result.is_none());
}

#[test]
fn file_of_worktree_is_none_for_a_path_that_does_not_exist_on_either_side() {
    let sandbox = Sandbox::new("diff-single-file-missing");
    let repo = sandbox.path("repo");
    init_repo(&repo);

    let result = diff::file_of_worktree(&repo, "main", "never-existed.txt").expect("diff");
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------
// line_marks
// ---------------------------------------------------------------------------

#[test]
fn line_marks_flags_an_added_line() {
    let sandbox = Sandbox::new("diff-marks-added");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "f.txt", "one\ntwo\n", "add f");

    let marks = diff::line_marks(&repo.join("f.txt"), "one\ntwo\nthree\n").expect("marks");
    assert_eq!(marks.at(0), None);
    assert_eq!(marks.at(1), None);
    assert_eq!(marks.at(2), Some(LineMark::Added));
    assert!(!marks.is_empty());
}

#[test]
fn line_marks_flags_a_modified_line() {
    let sandbox = Sandbox::new("diff-marks-modified");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "f.txt", "one\ntwo\nthree\n", "add f");

    let marks = diff::line_marks(&repo.join("f.txt"), "one\nTWO\nthree\n").expect("marks");
    assert_eq!(marks.at(0), None);
    assert_eq!(marks.at(1), Some(LineMark::Modified));
    assert_eq!(marks.at(2), None);
}

#[test]
fn line_marks_hangs_a_deletion_on_the_line_that_closed_over_it() {
    let sandbox = Sandbox::new("diff-marks-removed-above");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "f.txt", "one\ntwo\nthree\n", "add f");

    let marks = diff::line_marks(&repo.join("f.txt"), "one\nthree\n").expect("marks");
    assert_eq!(marks.at(0), None);
    assert_eq!(marks.at(1), Some(LineMark::RemovedAbove));
}

#[test]
fn line_marks_hangs_a_trailing_deletion_on_the_last_line() {
    let sandbox = Sandbox::new("diff-marks-removed-below");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "f.txt", "one\ntwo\nthree\n", "add f");

    let marks = diff::line_marks(&repo.join("f.txt"), "one\ntwo\n").expect("marks");
    assert_eq!(marks.at(0), None);
    assert_eq!(marks.at(1), Some(LineMark::RemovedBelow));
}

#[test]
fn line_marks_is_empty_for_unchanged_text() {
    let sandbox = Sandbox::new("diff-marks-unchanged");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "f.txt", "one\ntwo\n", "add f");

    let marks = diff::line_marks(&repo.join("f.txt"), "one\ntwo\n").expect("marks");
    assert!(marks.is_empty());
    assert_eq!(marks.at(0), None);
}

#[test]
fn line_marks_treats_every_line_as_added_when_head_has_no_commit_yet() {
    let sandbox = Sandbox::new("diff-marks-unborn");
    let repo = sandbox.path("repo");
    fs::create_dir_all(&repo).expect("mkdir");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "ket test"]);
    fs::write(repo.join("f.txt"), "").expect("write");

    let marks = diff::line_marks(&repo.join("f.txt"), "one\ntwo\n").expect("marks");
    assert_eq!(marks.at(0), Some(LineMark::Added));
    assert_eq!(marks.at(1), Some(LineMark::Added));
}

#[test]
fn line_marks_is_empty_inside_a_bare_repository() {
    // `worktree_relative` returns `None` for a bare repository — there is no
    // working tree to be relative to — which is the realistic way a path
    // resolves to "outside any working tree" without `gix::discover` itself
    // failing to find a repository at all.
    let sandbox = Sandbox::new("diff-marks-bare");
    let bare = sandbox.path("bare.git");
    git(
        sandbox.root(),
        &["init", "--bare", "--quiet", bare.to_str().unwrap()],
    );
    let file = bare.join("f.txt");

    let marks = diff::line_marks(&file, "one\ntwo\n").expect("marks");
    assert!(marks.is_empty());
}

#[test]
fn line_marks_is_empty_for_text_over_the_size_limit() {
    let sandbox = Sandbox::new("diff-marks-oversized");
    let repo = sandbox.path("repo");
    init_repo(&repo);
    commit_file(&repo, "f.txt", "one\n", "add f");

    let huge = "x".repeat(diff::MAX_FILE_BYTES as usize + 1);
    let marks = diff::line_marks(&repo.join("f.txt"), &huge).expect("marks");
    assert!(marks.is_empty());
}
