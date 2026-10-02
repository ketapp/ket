//! Deleting a worktree without making anyone wait for it.
//!
//! `git worktree remove` deletes the checkout inline. For a Rust worktree that
//! is several gigabytes of `target/`, and the caller — the sidebar, usually —
//! sits there until it finishes. Measured in the seconds-to-tens-of-seconds
//! range, which is long enough that deleting a worktree stops feeling free,
//! and a cleanup that does not feel free is one that does not happen. That is
//! the whole mechanism by which merged worktrees pile up.
//!
//! So the expensive part is moved off the critical path. Renaming the checkout
//! into a hidden sibling directory is a metadata operation that takes no
//! measurable time whatever the checkout holds, and the recursive delete then
//! runs on a worker thread after the caller has already been told the worktree
//! is gone. Git's registration is cleared in between, so nothing observes the
//! half-way state.
//!
//! The trash root is a *sibling* of the worktree, never a shared location, so
//! the rename can never cross a filesystem boundary and silently become a
//! copy. If it fails anyway the caller is told, and falls back to letting git
//! delete in place — slow, but correct.
//!
//! Entries can outlive the process that made them: a kill between the rename
//! and the worker draining leaves one behind. [`sweep_stale`] is the backstop,
//! and is why nothing here treats a failed delete as worth reporting.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The hidden directory trashed checkouts are renamed into.
pub const TRASH_DIR_NAME: &str = ".ket-worktree-trash";

/// How long an entry is left alone before a sweep will delete it.
///
/// Not the same worry as [`crate::pack::sweep_temps`], which must not delete a
/// live write. Nothing needs a trash entry — it is deregistered from git and
/// dropped from the store before it is ever queued. The one window that
/// matters is the sub-second gap in which [`move_to_trash`] has renamed a
/// checkout and its caller may still need [`restore_from_trash`] to put it
/// back. An hour is enormously more than that gap and still far less than the
/// gap to the next launch, which is when the entries a crash left behind get
/// collected.
const TRASH_STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// Where trashed checkouts belonging to `worktree_path` are kept.
fn trash_root(worktree_path: &Path) -> Option<PathBuf> {
    Some(worktree_path.parent()?.join(TRASH_DIR_NAME))
}

/// A name no concurrent removal can collide with.
///
/// Same-named worktrees in two ket processes can be removed in the same
/// millisecond, so the timestamp alone is not enough; the pid separates the
/// processes and the counter separates removals within one.
fn entry_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nonce =
        u64::from(std::process::id()) ^ (u64::from(now.subsec_nanos()) << 8) ^ (count << 32);

    format!("wt-{}-{:08x}", now.as_millis(), nonce as u32)
}

/// Whether `name` is an entry this module created.
///
/// A sweep deletes only what matches. Anything else that found its way into a
/// trash root is somebody else's, and is left exactly where it is.
pub fn is_trash_entry_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("wt-") else {
        return false;
    };
    let Some((millis, nonce)) = rest.split_once('-') else {
        return false;
    };
    !millis.is_empty()
        && millis.bytes().all(|b| b.is_ascii_digit())
        && nonce.len() == 8
        && nonce.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Renames a checkout aside so the caller can return before it is deleted.
///
/// `None` when the rename is not available — a different filesystem, or a
/// trash root that is not a plain directory — and the caller must let git
/// delete the worktree in place instead.
pub fn move_to_trash(worktree_path: &Path) -> Option<PathBuf> {
    let root = trash_root(worktree_path)?;

    if let Err(e) = std::fs::create_dir_all(&root) {
        tracing::warn!(error = %e, path = %root.display(), "could not open a worktree trash root");
        return None;
    }

    // Deliberately `symlink_metadata`: a symlink here would make the rename
    // land wherever it points, which is the one place a later recursive delete
    // must never be aimed.
    let usable = std::fs::symlink_metadata(&root)
        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink());
    if !usable {
        tracing::warn!(path = %root.display(), "refusing a worktree trash root that is not a directory");
        return None;
    }

    let entry = root.join(entry_name());
    if let Err(e) = std::fs::rename(worktree_path, &entry) {
        tracing::warn!(
            error = %e,
            path = %worktree_path.display(),
            "deferred deletion unavailable; deleting in place"
        );
        // Leave no empty root behind when the rename never happened. This
        // fails harmlessly when another removal has queued something in it.
        let _ = std::fs::remove_dir(&root);
        return None;
    }

    Some(entry)
}

/// Undoes [`move_to_trash`], leaving the worktree exactly as it was.
///
/// The caller needs this when clearing git's registration fails: the checkout
/// has been renamed but git still believes in the old path, and putting it
/// back is the only way to leave a state that anything can act on.
pub fn restore_from_trash(trash_path: &Path, worktree_path: &Path) -> bool {
    match std::fs::rename(trash_path, worktree_path) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %worktree_path.display(),
                "could not restore a worktree from the trash"
            );
            false
        }
    }
}

/// The worker every queued deletion is handed to.
///
/// One thread, so a burst of removals cannot saturate the disk while somebody
/// is still trying to work. `None` when the thread could not be spawned, which
/// makes the caller delete on its own thread instead.
fn queue() -> Option<&'static Sender<PathBuf>> {
    static QUEUE: OnceLock<Option<Sender<PathBuf>>> = OnceLock::new();

    QUEUE
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<PathBuf>();
            std::thread::Builder::new()
                .name("ket-worktree-trash".to_owned())
                .spawn(move || {
                    for path in rx {
                        delete_now(&path);
                    }
                })
                .map_err(|e| {
                    tracing::warn!(error = %e, "could not start the worktree trash worker");
                })
                .ok()
                .map(|_| tx)
        })
        .as_ref()
}

/// Queues a trashed checkout for deletion in the background.
///
/// Returns immediately. A process that exits before the worker drains leaves
/// the entry on disk, where the next launch's [`sweep_stale`] collects it.
pub fn schedule_deletion(trash_path: PathBuf) {
    if let Some(tx) = queue()
        && tx.send(trash_path.clone()).is_ok()
    {
        return;
    }
    delete_now(&trash_path);
}

/// Deletes one trashed entry, and the root with it when it was the last.
fn delete_now(trash_path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(trash_path) {
        // Only a warning: the entry is already invisible to everything, and a
        // sweep will find it again.
        tracing::warn!(
            error = %e,
            path = %trash_path.display(),
            "could not delete a trashed worktree"
        );
        return;
    }

    // Tidies the root away once it holds nothing, which is what lets the
    // project directory above it be pruned in turn. Fails harmlessly while
    // another entry is still queued.
    if let Some(root) = trash_path.parent() {
        let _ = std::fs::remove_dir(root);
    }
}

/// Deletes trash left behind by a previous run.
///
/// Every failure is ignored on purpose, exactly as in
/// [`crate::pack::sweep_temps`]: this is housekeeping, a directory that cannot
/// be read is not worth troubling anybody with, and the next launch tries
/// again.
pub fn sweep_stale(worktrees_root: &Path) {
    let Ok(projects) = std::fs::read_dir(worktrees_root) else {
        return;
    };

    for project in projects.flatten() {
        let root = project.path().join(TRASH_DIR_NAME);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let matches = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(is_trash_entry_name);
            if !matches {
                continue;
            }

            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .and_then(|at| at.elapsed().map_err(std::io::Error::other))
                .is_ok_and(|age| age > TRASH_STALE_AFTER);
            if !stale {
                continue;
            }

            tracing::debug!(path = %path.display(), "sweeping a worktree left by a previous run");
            let _ = std::fs::remove_dir_all(&path);
        }

        let _ = std::fs::remove_dir(&root);
    }
}
