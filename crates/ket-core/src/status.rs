//! What changed in a worktree, and how far it has diverged from its base.
//!
//! This is the read path, and the first place in ket where `gix` does the work
//! instead of the `git` binary. The split is deliberate and stated in
//! [`crate::git`]: mutations shell out, because corrupting a worktree inside a
//! tool built on worktrees is unrecoverable; reads go in-process, because with
//! several projects each holding several worktrees, a status refresh is one
//! process spawn per worktree per refresh and that is the cost that shows up.
//!
//! Two bounds, so memory stays flat:
//!
//! - The file list is capped at [`MAX_CHANGED_FILES`]. A worktree where an agent
//!   ran `rm -rf src` has tens of thousands of changes, and none of them are
//!   worth holding in memory to render a list nobody will scroll.
//! - Nothing is cached. A [`WorktreeStatus`] is computed on demand and dropped;
//!   idle worktrees hold metadata only.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{KetError, Result};

/// Most changed files retained in one [`WorktreeStatus`].
///
/// Everything past this is counted but not kept — [`WorktreeStatus::truncated`]
/// says when that happened.
pub const MAX_CHANGED_FILES: usize = 1000;

/// Object cache used for the ahead/behind walk, in bytes.
///
/// `gix` recommends one for repeated commit lookups, and a revision walk is
/// nothing but repeated commit lookups. Bounded and per-call, so it is released
/// with the repository handle.
const OBJECT_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// How a file differs.
///
/// Named after what a person reading a change list wants to know, not after
/// git's internal categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// New in this worktree, and known to git.
    Added,
    /// Contents or mode changed.
    Modified,
    /// Gone.
    Deleted,
    /// Moved or copied, and git matched the two halves up.
    Renamed,
    /// A file became a symlink, or the reverse.
    TypeChange,
    /// Present on disk and not tracked. Ignored files are not included.
    Untracked,
    /// Left in a conflicted state by a merge or rebase.
    Conflicted,
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    /// Path relative to the worktree root. Untracked directories end in `/`.
    pub path: String,
    /// How it differs.
    pub kind: ChangeKind,
    /// Whether the change is staged.
    ///
    /// `true` means `HEAD` disagrees with the index, `false` that the index
    /// disagrees with the working tree. A path can legitimately appear twice
    /// with different values, exactly as it does in `git status`.
    pub staged: bool,
}

/// How far a worktree has moved relative to what it branched from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracking {
    /// The revision compared against, as it was named.
    pub base: String,
    /// Commits on this branch that the base does not have.
    pub ahead: u32,
    /// Commits on the base that this branch does not have.
    pub behind: u32,
}

/// A git operation caught mid-flight in a worktree.
///
/// Detected from the marker files git itself leaves in the git directory
/// while one of these is running or stopped on a conflict — the same files
/// `git status` reads to print "You are currently merging branch 'x'."
/// Nothing here says whether it is proceeding cleanly or stuck on a
/// conflict; [`ChangeKind::Conflicted`] files answer that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitOperation {
    /// `MERGE_HEAD` is present.
    Merge,
    /// `rebase-merge/` or `rebase-apply/` is present.
    Rebase,
    /// `CHERRY_PICK_HEAD` is present.
    CherryPick,
    /// `REVERT_HEAD` is present.
    Revert,
    /// `BISECT_LOG` is present.
    Bisect,
}

impl GitOperation {
    /// What to call it in a sentence — the noun, not the participle.
    ///
    /// "in the middle of a rebase", not "rebasing". The tag the sidebar draws
    /// wants the other grammar and keeps its own spelling; see
    /// `ket_ui::tree::git_operation_label`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Bisect => "bisect",
        }
    }
}

/// A worktree's read-side state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeStatus {
    /// Changed files, at most [`MAX_CHANGED_FILES`] of them, sorted by path.
    pub files: Vec<ChangedFile>,
    /// How many changes there were in total, including any not kept.
    pub total_changes: usize,
    /// Whether [`WorktreeStatus::files`] is a subset.
    ///
    /// When true the retained files are an arbitrary subset rather than the
    /// first N in path order: capping happens while collecting, because
    /// collecting everything in order to sort it is the unbounded buffer this
    /// cap exists to prevent.
    pub truncated: bool,
    /// Divergence from the base, or `None` if the base no longer resolves.
    ///
    /// A deleted branch or a pruned remote-tracking ref is an ordinary thing to
    /// come back to after a week away. It costs the ahead/behind numbers; it
    /// does not cost the status.
    pub tracking: Option<Tracking>,
    /// A merge, rebase, cherry-pick, revert, or bisect left mid-flight.
    ///
    /// This is not limited to `ket collapse` — anything that left these marker
    /// files behind counts, including a merge run by hand in the worktree's own
    /// terminal.
    pub in_progress: Option<GitOperation>,
}

impl WorktreeStatus {
    /// Whether anything at all differs from `HEAD`.
    pub fn is_dirty(&self) -> bool {
        self.total_changes > 0
    }
}

/// Checks the git directory for the marker files an in-progress operation
/// leaves behind.
///
/// Order matters only in the theoretical case of two markers coexisting
/// (git does not allow starting a second operation while one is unresolved,
/// but a hand-edited git directory could still manage it) — merge is checked
/// first because `MERGE_HEAD` is the one people hit constantly.
fn in_progress_operation(repo: &gix::Repository) -> Option<GitOperation> {
    let git_dir = repo.git_dir();
    let has = |name: &str| git_dir.join(name).exists();

    if has("MERGE_HEAD") {
        Some(GitOperation::Merge)
    } else if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        Some(GitOperation::Rebase)
    } else if has("CHERRY_PICK_HEAD") {
        Some(GitOperation::CherryPick)
    } else if has("REVERT_HEAD") {
        Some(GitOperation::Revert)
    } else if has("BISECT_LOG") {
        Some(GitOperation::Bisect)
    } else {
        None
    }
}

/// A cheap summary of a worktree's git-side state, from file metadata alone.
///
/// Two equal fingerprints mean nothing happened on the git side between them:
/// no commit, checkout, reset, staging, or merge, rebase, cherry-pick, revert
/// or bisect starting or ending. Each of those rewrites or creates one of the
/// files stamped here — a commit appends to the reflog and rewrites the index
/// even when every file was already staged.
///
/// It says nothing about edits to files in the working tree, which never touch
/// the git directory. Those are the file watcher's job; this is what lets a
/// caller skip [`of_worktree`] — a walk of the whole tree — for every worktree
/// whose git state it has already read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFingerprint {
    /// Size and modification time of each of [`FINGERPRINT_FILES`], in order;
    /// `None` for one that does not exist.
    stamps: Vec<Option<(u64, std::time::SystemTime)>>,
}

/// What [`git_fingerprint`] stamps, relative to the worktree's own git
/// directory: its `HEAD`, its index and its reflog, and every marker
/// [`in_progress_operation`] reads.
const FINGERPRINT_FILES: [&str; 9] = [
    "HEAD",
    "index",
    "logs/HEAD",
    "MERGE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
];

/// Stamps the worktree at `path`'s git directory. See [`GitFingerprint`].
///
/// `None` when the worktree is gone or has no git directory. A handful of
/// `stat` calls and, for a linked worktree, one read of its `.git` file: cheap
/// enough to run for every worktree on a timer, which [`of_worktree`] is not.
pub fn git_fingerprint(path: &Path) -> Option<GitFingerprint> {
    let git_dir = own_git_dir(path)?;
    let stamps = FINGERPRINT_FILES
        .iter()
        .map(|name| {
            let meta = std::fs::metadata(git_dir.join(name)).ok()?;
            Some((meta.len(), meta.modified().ok()?))
        })
        .collect();
    Some(GitFingerprint { stamps })
}

/// The git directory that belongs to the worktree at `path` itself.
///
/// The main checkout's `.git` is that directory. A linked worktree's `.git` is
/// a file naming its own directory under the main repository's
/// `.git/worktrees/`, which is where its `HEAD`, index and reflog live.
fn own_git_dir(path: &Path) -> Option<std::path::PathBuf> {
    let dot_git = path.join(".git");
    if std::fs::metadata(&dot_git).ok()?.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    let target = Path::new(target);
    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        path.join(target)
    })
}

/// Reads the status of the worktree at `path`.
///
/// `base` is the revision to measure divergence against — see
/// [`crate::worktree::Worktree::base_rev`], which is what decides whether that
/// is a moving branch or a frozen commit. Pass `None` to skip the walk entirely.
///
/// Ignored files are excluded and untracked directories are collapsed to a
/// single entry, matching `git status --ignored=no`. Both matter here rather
/// than being incidental: a provisioned worktree contains a `node_modules` of
/// 80,000 files, and listing it would make every status call proportional to the
/// size of the dependency tree.
pub fn of_worktree(path: &Path, base: Option<&str>) -> Result<WorktreeStatus> {
    let mut repo = gix::open(path).map_err(|e| KetError::gix("open", e))?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);

    let (files, total_changes) = changed_files(&repo)?;

    Ok(WorktreeStatus {
        truncated: total_changes > files.len(),
        files,
        total_changes,
        tracking: base.and_then(|base| tracking(&repo, base)),
        in_progress: in_progress_operation(&repo),
    })
}

/// Collects changed files, keeping at most [`MAX_CHANGED_FILES`].
///
/// Returns the kept files and the total number of changes seen. The iterator is
/// consumed in full even once the cap is reached: the total is worth having, and
/// abandoning it early would leave `gix`'s producer threads to notice on their
/// own.
fn changed_files(repo: &gix::Repository) -> Result<(Vec<ChangedFile>, usize)> {
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| KetError::gix("status", e))?
        .into_iter(Vec::new())
        .map_err(|e| KetError::gix("status", e))?;

    let mut files = Vec::new();
    let mut total = 0usize;

    for item in iter {
        let item = item.map_err(|e| KetError::gix("status", e))?;

        let Some(file) = classify(&item) else {
            continue;
        };

        total += 1;
        if files.len() < MAX_CHANGED_FILES {
            files.push(file);
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path).then(a.staged.cmp(&b.staged)));
    Ok((files, total))
}

/// Turns one `gix` status item into a change, or `None` if it is not one.
fn classify(item: &gix::status::Item) -> Option<ChangedFile> {
    use gix::status::index_worktree::Item as WorktreeItem;
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

    match item {
        // HEAD vs the index: staged.
        gix::status::Item::TreeIndex(change) => {
            use gix::diff::index::Change as TreeChange;
            let kind = match change {
                TreeChange::Addition { .. } => ChangeKind::Added,
                TreeChange::Deletion { .. } => ChangeKind::Deleted,
                TreeChange::Modification { .. } => ChangeKind::Modified,
                TreeChange::Rewrite { .. } => ChangeKind::Renamed,
            };
            Some(ChangedFile {
                path: change.location().to_string(),
                kind,
                staged: true,
            })
        }

        // The index vs the working tree: unstaged.
        gix::status::Item::IndexWorktree(WorktreeItem::Modification {
            rela_path, status, ..
        }) => {
            let kind = match status {
                EntryStatus::Conflict { .. } => ChangeKind::Conflicted,
                EntryStatus::IntentToAdd => ChangeKind::Added,
                EntryStatus::Change(Change::Removed) => ChangeKind::Deleted,
                EntryStatus::Change(Change::Type { .. }) => ChangeKind::TypeChange,
                EntryStatus::Change(Change::Modification { .. }) => ChangeKind::Modified,
                EntryStatus::Change(Change::SubmoduleModification(_)) => ChangeKind::Modified,
                // Purely a stat refresh gix would like written back. Nothing
                // about the file changed, so it is not a change.
                EntryStatus::NeedsUpdate(_) => return None,
            };
            Some(ChangedFile {
                path: rela_path.to_string(),
                kind,
                staged: false,
            })
        }

        gix::status::Item::IndexWorktree(WorktreeItem::DirectoryContents { entry, .. }) => {
            if entry.status != gix::dir::entry::Status::Untracked {
                return None;
            }

            // A collapsed untracked directory is one entry standing for its
            // whole contents. Without the trailing slash it reads as a file.
            let is_dir = matches!(
                entry.disk_kind,
                Some(gix::dir::entry::Kind::Directory | gix::dir::entry::Kind::Repository)
            );

            let mut path = entry.rela_path.to_string();
            if is_dir && !path.ends_with('/') {
                path.push('/');
            }

            Some(ChangedFile {
                path,
                kind: ChangeKind::Untracked,
                staged: false,
            })
        }

        gix::status::Item::IndexWorktree(WorktreeItem::Rewrite { dirwalk_entry, .. }) => {
            Some(ChangedFile {
                path: dirwalk_entry.rela_path.to_string(),
                kind: ChangeKind::Renamed,
                staged: false,
            })
        }
    }
}

/// Counts commits either side of the merge base with `base`.
///
/// Returns `None` rather than an error when anything fails to resolve: a base
/// branch that has since been deleted, or a worktree on an unborn HEAD, are both
/// ordinary states to find a week-old worktree in, and neither should stop the
/// rest of the status being reported.
fn tracking(repo: &gix::Repository, base: &str) -> Option<Tracking> {
    let head = repo.head_id().ok()?.detach();
    let base_id = repo.rev_parse_single(base).ok()?.detach();

    Some(Tracking {
        base: base.to_owned(),
        ahead: count_reachable(repo, head, base_id)?,
        behind: count_reachable(repo, base_id, head)?,
    })
}

/// Commits reachable from `tip` but not from `hidden` — `git rev-list tip ^hidden`.
fn count_reachable(
    repo: &gix::Repository,
    tip: gix::ObjectId,
    hidden: gix::ObjectId,
) -> Option<u32> {
    let walk = repo.rev_walk([tip]).with_hidden([hidden]).all().ok()?;

    let mut count = 0u32;
    for info in walk {
        info.ok()?;
        count = count.saturating_add(1);
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_format_is_flat_camel_case() {
        // Pinned: this is a view-model, and the TypeScript client in M1 is
        // written against exactly this shape.
        let status = WorktreeStatus {
            files: vec![ChangedFile {
                path: "src/main.rs".to_owned(),
                kind: ChangeKind::TypeChange,
                staged: true,
            }],
            total_changes: 1,
            truncated: false,
            tracking: Some(Tracking {
                base: "main".to_owned(),
                ahead: 3,
                behind: 0,
            }),
            in_progress: None,
        };

        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({
                "files": [{ "path": "src/main.rs", "kind": "typeChange", "staged": true }],
                "totalChanges": 1,
                "truncated": false,
                "tracking": { "base": "main", "ahead": 3, "behind": 0 },
                "inProgress": null,
            })
        );
    }

    #[test]
    fn a_clean_worktree_is_not_dirty() {
        let status = WorktreeStatus {
            files: Vec::new(),
            total_changes: 0,
            truncated: false,
            tracking: None,
            in_progress: None,
        };
        assert!(!status.is_dirty());
    }

    #[test]
    fn opening_something_that_is_not_a_repository_is_an_error_not_a_panic() {
        let result = of_worktree(Path::new("/"), None);
        assert!(matches!(result, Err(KetError::Gix { .. })), "{result:?}");
    }

    #[test]
    fn every_operation_has_a_noun_not_a_participle() {
        assert_eq!(GitOperation::Merge.label(), "merge");
        assert_eq!(GitOperation::Rebase.label(), "rebase");
        assert_eq!(GitOperation::CherryPick.label(), "cherry-pick");
        assert_eq!(GitOperation::Revert.label(), "revert");
        assert_eq!(GitOperation::Bisect.label(), "bisect");
    }

    /// A throwaway repository, removed when the guard drops.
    struct Repo(std::path::PathBuf);

    impl Repo {
        fn init(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ket-status-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            run(&dir, &["init", "--quiet", "--initial-branch=main"]);
            run(&dir, &["config", "user.email", "test@example.com"]);
            run(&dir, &["config", "user.name", "ket test"]);
            std::fs::write(dir.join("a.txt"), "one\n").unwrap();
            run(&dir, &["add", "."]);
            run(&dir, &["commit", "--quiet", "-m", "initial"]);
            Self(dir)
        }

        fn git_dir(&self) -> std::path::PathBuf {
            self.0.join(".git")
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn a_fingerprint_is_none_for_a_path_with_no_git_directory() {
        let dir = std::env::temp_dir().join(format!("ket-status-no-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(git_fingerprint(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fingerprint_is_stable_with_nothing_changed() {
        let repo = Repo::init("fp-stable");
        let a = git_fingerprint(&repo.0).unwrap();
        let b = git_fingerprint(&repo.0).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_fingerprint_changes_after_a_commit() {
        let repo = Repo::init("fp-commit");
        let before = git_fingerprint(&repo.0).unwrap();

        std::fs::write(repo.0.join("b.txt"), "two\n").unwrap();
        run(&repo.0, &["add", "."]);
        run(&repo.0, &["commit", "--quiet", "-m", "second"]);

        let after = git_fingerprint(&repo.0).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn a_fingerprint_is_unmoved_by_an_edit_that_never_touches_the_git_directory() {
        let repo = Repo::init("fp-worktree-edit");
        let before = git_fingerprint(&repo.0).unwrap();

        std::fs::write(repo.0.join("a.txt"), "edited but not staged\n").unwrap();

        let after = git_fingerprint(&repo.0).unwrap();
        assert_eq!(
            before, after,
            "a plain file edit never touches the git directory"
        );
    }

    #[test]
    fn a_merge_head_marker_is_reported_as_an_in_progress_merge() {
        let repo = Repo::init("in-progress-merge");
        std::fs::write(repo.git_dir().join("MERGE_HEAD"), "deadbeef\n").unwrap();

        let status = of_worktree(&repo.0, None).unwrap();
        assert_eq!(status.in_progress, Some(GitOperation::Merge));
    }

    #[test]
    fn a_rebase_merge_directory_is_reported_as_an_in_progress_rebase() {
        let repo = Repo::init("in-progress-rebase");
        std::fs::create_dir(repo.git_dir().join("rebase-merge")).unwrap();

        let status = of_worktree(&repo.0, None).unwrap();
        assert_eq!(status.in_progress, Some(GitOperation::Rebase));
    }

    #[test]
    fn a_cherry_pick_head_marker_is_reported_as_an_in_progress_cherry_pick() {
        let repo = Repo::init("in-progress-cherry-pick");
        std::fs::write(repo.git_dir().join("CHERRY_PICK_HEAD"), "deadbeef\n").unwrap();

        let status = of_worktree(&repo.0, None).unwrap();
        assert_eq!(status.in_progress, Some(GitOperation::CherryPick));
    }

    #[test]
    fn a_revert_head_marker_is_reported_as_an_in_progress_revert() {
        let repo = Repo::init("in-progress-revert");
        std::fs::write(repo.git_dir().join("REVERT_HEAD"), "deadbeef\n").unwrap();

        let status = of_worktree(&repo.0, None).unwrap();
        assert_eq!(status.in_progress, Some(GitOperation::Revert));
    }

    #[test]
    fn a_bisect_log_marker_is_reported_as_an_in_progress_bisect() {
        let repo = Repo::init("in-progress-bisect");
        std::fs::write(repo.git_dir().join("BISECT_LOG"), "git bisect start\n").unwrap();

        let status = of_worktree(&repo.0, None).unwrap();
        assert_eq!(status.in_progress, Some(GitOperation::Bisect));
    }

    #[test]
    fn nothing_in_progress_is_none() {
        let repo = Repo::init("in-progress-none");
        let status = of_worktree(&repo.0, None).unwrap();
        assert_eq!(status.in_progress, None);
    }

    #[test]
    fn a_deleted_base_costs_the_tracking_numbers_not_the_whole_status() {
        let repo = Repo::init("tracking-deleted-base");
        let status = of_worktree(&repo.0, Some("does-not-exist")).unwrap();
        assert!(status.tracking.is_none());
        assert!(!status.is_dirty());
    }
}
