//! When ket reads git status, and for which worktrees.
//!
//! A status read is a walk of the whole working tree, and it used to run for
//! every worktree in every project on every tick — whether or not
//! anything had changed, and whether or not anyone was looking. With a handful
//! of worktrees that was the periodic burst in every CPU sample, and it grew
//! with each one added. It also spawned a `git` process per dirty worktree to
//! count lines, which never showed up as ket at all.
//!
//! Now a worktree is read in full only when there is a reason to:
//!
//! - **Its git state moved.** Every tick stamps each worktree's git directory
//!   — `HEAD`, the index, the reflog, the merge and rebase markers — which is a
//!   few `stat` calls, not a walk. A commit, checkout, staging or merge changes
//!   the stamp and that worktree alone is read. See
//!   [`ket_core::status::git_fingerprint`].
//! - **It is selected and its files changed.** The selected worktree is
//!   watched, and a burst of writes becomes one read once it settles — but
//!   never sooner than [`SELECTED_MIN_GAP`] after the last one, so an agent
//!   writing continuously cannot run status back to back.
//! - **The safety sweep.** Every [`SWEEP_EVERY`], everything is read, which
//!   catches what neither of the above sees: edits in a worktree that is not
//!   selected, which touch no git file until they are staged.
//!
//! The same plan throughout: read git only for the worktree on screen, on
//! file events, with a floor between reads and a slow safety interval.
//!
//! The right panel's Git view and an open diff tab show the selected
//! worktree, so they are refreshed when it is read rather than on every tick.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Context, Task, WeakEntity};
use ket_core::id::WorktreeId;
use ket_core::status::GitFingerprint;
use ket_core::surface::FileWatcher;

use crate::Shell;

/// How often every worktree is read regardless.
///
/// The latency of the one thing nothing else sees: a file edited, and not yet
/// staged, in a worktree that is not the selected one.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// The least time between two reads of the selected worktree that its own
/// file changes asked for.
const SELECTED_MIN_GAP: Duration = Duration::from_secs(3);

/// How long a burst of file events is left to settle before it counts as one
/// change. A save is several events — a write, a rename, a permissions touch.
const SETTLE: Duration = Duration::from_millis(125);

/// How long the watch thread waits for an event before checking whether it is
/// still wanted, which bounds how long it outlives its worktree's selection.
const WATCH_POLL: Duration = Duration::from_secs(1);

/// What the shell remembers between reads. See the module docs.
#[derive(Default)]
pub(crate) struct GitWatch {
    /// Each worktree's fingerprint as of its last full read. Absent means it
    /// has never been read; `Some(None)` means it was missing then.
    seen: HashMap<WorktreeId, Option<GitFingerprint>>,
    /// When every worktree was last read in full.
    swept_at: Option<Instant>,
    /// The watch on the selected worktree.
    watched: Option<Watched>,
    /// Whether the selected worktree's files changed since it was last read.
    selected_changed: bool,
    /// When the selected worktree was last read in full.
    selected_read_at: Option<Instant>,
    /// A read booked for when [`SELECTED_MIN_GAP`] has passed.
    retry: Option<Task<()>>,
}

/// A watch on one worktree's files.
///
/// Dropping it ends the watch: the task holds the only receiver, and the
/// thread that holds the watcher leaves once nothing is listening.
struct Watched {
    id: WorktreeId,
    _signal: Task<()>,
}

/// One worktree's result from a read: its new fingerprint, and what its
/// status read found — `None` when its fingerprint said nothing had moved.
struct Read {
    id: WorktreeId,
    fingerprint: Option<GitFingerprint>,
    row: Option<Row>,
}

/// What a full status read of one worktree puts on its sidebar row.
struct Row {
    missing: bool,
    dirty: bool,
    changes: usize,
    marks: std::sync::Arc<crate::tree::ChangeMarks>,
    line_changes: Option<(u32, u32)>,
    in_progress: Option<ket_core::status::GitOperation>,
}

impl Shell {
    /// Keeps the file watch on whichever worktree is selected.
    ///
    /// A newly selected worktree is read at the next opportunity rather than
    /// waiting for the sweep: it is the one on screen.
    pub(crate) fn follow_selected_worktree(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .selection
            .and_then(|s| self.projects.get(s.project)?.worktrees.get(s.worktree))
            .map(|node| (node.id.clone(), node.path.clone()));

        match selected {
            None => self.git_watch.watched = None,
            Some((id, _))
                if self
                    .git_watch
                    .watched
                    .as_ref()
                    .is_some_and(|watched| watched.id == id) => {}
            Some((id, path)) => {
                self.git_watch.watched = Some(watch(id, path, cx));
                self.git_watch.selected_changed = true;
            }
        }
    }

    /// Reads git status for every worktree that has a reason to be read —
    /// see the module docs — off the window's thread.
    ///
    /// Safe to call as often as anything likes: one read runs at a time, a
    /// worktree with nothing new is skipped after a few `stat` calls, and the
    /// selected worktree's own changes wait out [`SELECTED_MIN_GAP`].
    pub(crate) fn read_git_status(&mut self, cx: &mut Context<Self>) {
        // One read at a time. A call that arrives while one is running is not
        // lost: the read looks again when it lands, if anything is pending.
        if self.reading_git_status {
            return;
        }

        let sweep = self
            .git_watch
            .swept_at
            .is_none_or(|at| at.elapsed() >= SWEEP_EVERY);
        let selected = self.git_watch.watched.as_ref().map(|w| w.id.clone());

        let mut selected_due = self.git_watch.selected_changed;
        if selected_due
            && let Some(wait) = self
                .git_watch
                .selected_read_at
                .and_then(|at| SELECTED_MIN_GAP.checked_sub(at.elapsed()))
        {
            // Too soon after the last read. Book the rest of the gap once;
            // whatever else changes meanwhile rides along with it.
            if self.git_watch.retry.is_none() {
                self.git_watch.retry = Some(cx.spawn(
                    async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
                        cx.background_executor().timer(wait).await;
                        let _ = shell.update(cx, |shell, cx| {
                            shell.git_watch.retry = None;
                            shell.read_git_status(cx);
                        });
                    },
                ));
            }
            selected_due = false;
            // And nothing else either, unless the sweep is due: without this,
            // every burst in the gap re-fingerprinted every worktree in every
            // project. The booked read does all of that when the gap ends.
            if !sweep {
                return;
            }
        }

        let wanted: Vec<(WorktreeId, PathBuf, Option<Option<GitFingerprint>>, bool)> = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .map(|node| {
                let force = sweep || (selected_due && selected.as_ref() == Some(&node.id));
                (
                    node.id.clone(),
                    node.path.clone(),
                    self.git_watch.seen.get(&node.id).cloned(),
                    force,
                )
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        if selected_due {
            self.git_watch.selected_changed = false;
        }

        self.reading_git_status = true;
        let read = cx.background_executor().spawn(async move {
            wanted
                .into_iter()
                .map(|(id, path, seen, force)| {
                    // Stamped before the walk, so anything that lands during
                    // it shows up as a change next time rather than being
                    // absorbed into a stamp taken after.
                    let fingerprint = ket_core::status::git_fingerprint(&path);
                    let unchanged = !force && seen.as_ref() == Some(&fingerprint);
                    let row = (!unchanged).then(|| read_row(&path));
                    Read {
                        id,
                        fingerprint,
                        row,
                    }
                })
                .collect::<Vec<_>>()
        });

        cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let reads = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.reading_git_status = false;
                if sweep {
                    shell.git_watch.swept_at = Some(Instant::now());
                }

                let mut changed = false;
                let mut selected_read = false;
                for Read {
                    id,
                    fingerprint,
                    row,
                } in reads
                {
                    let Some(row) = row else {
                        continue;
                    };
                    changed = true;
                    selected_read |= selected.as_ref() == Some(&id);
                    // Written back by id rather than by position: the sidebar
                    // may have been reloaded while this was walking, and an
                    // index into the list it started from would name a
                    // different row.
                    for node in shell
                        .projects
                        .iter_mut()
                        .flat_map(|project| project.worktrees.iter_mut())
                        .filter(|node| node.id == id)
                    {
                        node.missing = row.missing;
                        node.dirty = row.dirty;
                        node.changes = row.changes;
                        node.marks = row.marks.clone();
                        node.line_changes = row.line_changes;
                        node.in_progress = row.in_progress;
                    }
                    shell.git_watch.seen.insert(id, fingerprint);
                }

                if selected_read {
                    shell.git_watch.selected_read_at = Some(Instant::now());
                    // They show the selected worktree, and it may have moved.
                    shell.refresh_git_panel(cx);
                    shell.refresh_open_diff(cx);
                }
                // Changes that arrived while this was reading.
                if shell.git_watch.selected_changed {
                    shell.read_git_status(cx);
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// Reads one worktree's row in full: a status walk, and for a dirty tree the
/// line counts.
fn read_row(path: &std::path::Path) -> Row {
    let missing = !path.is_dir();
    let status = (!missing)
        .then(|| ket_core::status::of_worktree(path, None).ok())
        .flatten();
    let dirty = status.as_ref().is_some_and(|s| s.total_changes > 0);
    let changes = status.as_ref().map(crate::tree::changed_paths).unwrap_or(0);
    let marks = std::sync::Arc::new(
        status
            .as_ref()
            .map(crate::tree::ChangeMarks::of)
            .unwrap_or_default(),
    );
    // Only for the trees that have something in them. A clean worktree's
    // answer is known without asking, and asking anyway would put a git
    // process per worktree on every read.
    let line_changes = dirty
        .then(|| ket_core::git::Git::new(path).line_changes().ok())
        .flatten();
    Row {
        missing,
        dirty,
        changes,
        marks,
        line_changes,
        in_progress: status.and_then(|s| s.in_progress),
    }
}

/// Starts watching the worktree at `path` for file changes, which mark it as
/// changed and ask for a read.
///
/// The watcher runs on a thread of its own: it blocks waiting for events, and
/// a blocked task would hold one of gpui's executor threads for good.
///
/// Only changes status could see wake it. A build writes thousands of files
/// into `target/` a second, git ignores every one of them, and each burst used
/// to set off a read — ket at a full core for as long as a `cargo build` ran
/// in the selected worktree. See [`status_blind`]. A
/// worktree that cannot be watched — gone, or out of watch descriptors — is
/// simply not watched; the sweep still reads it.
/// Whether a change under `root` is one git status cannot show: build output
/// and dependency trees, which are all but always ignored, and the git
/// directory, whose own changes the fingerprint check on every tick already
/// catches.
///
/// By path component rather than by asking git, which would cost a process or
/// a gix index load per burst — the very cost this exists to avoid. A project
/// that tracks files in a directory named like these still has them read by
/// the sweep and the tick.
fn status_blind(root: &std::path::Path, path: &std::path::Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some("target" | "node_modules" | ".git")
        )
    })
}

fn watch(id: WorktreeId, path: PathBuf, cx: &mut Context<Shell>) -> Watched {
    let (tx, mut rx) = tokio::sync::watch::channel(());
    let _ = std::thread::Builder::new()
        .name("ket-git-watch".into())
        .spawn(move || {
            let Ok(watcher) = FileWatcher::watch(&path) else {
                return;
            };
            let root = path.canonicalize().unwrap_or(path);
            while !tx.is_closed() {
                let Some(first) = watcher.next_within(WATCH_POLL) else {
                    continue;
                };
                std::thread::sleep(SETTLE);
                // Before the drain, which clears it: a watcher that dropped
                // events cannot say what they were, so they all count.
                let overflowed = watcher.overflowed();
                let rest = watcher.drain();
                let counts = overflowed
                    || std::iter::once(&first)
                        .chain(&rest)
                        .any(|change| !status_blind(&root, &change.path));
                if counts && tx.send(()).is_err() {
                    return;
                }
            }
        });

    let signal = cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
        while rx.changed().await.is_ok() {
            let alive = shell.update(cx, |shell, cx| {
                shell.git_watch.selected_changed = true;
                shell.read_git_status(cx);
            });
            if alive.is_err() {
                return;
            }
        }
    });
    Watched {
        id,
        _signal: signal,
    }
}
