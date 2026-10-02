//! What a checkout costs on disk, and how much of that is regenerable.
//!
//! A worktree's size is not a fact ket can look up. There is no cheap call for
//! it on any filesystem it runs on — the number is the sum of a full walk, and
//! for a Rust checkout that walk is an eight-gigabyte `target/` of a few
//! hundred thousand files. So everything here is written to be called
//! deliberately, off the window's thread, and cached by whoever asked.
//!
//! **The split matters more than the total.** Almost all of what a worktree
//! costs is build output, and build output is regenerable by definition —
//! which makes clearing it a different kind of act from deleting the checkout.
//! One costs a rebuild; the other costs the work. A number that does not
//! separate them cannot tell you that.
//!
//! **Which directories those are is declared, never guessed** — see
//! [`crate::config::StorageConfig`]. Deleting several gigabytes on a hunch
//! about what a directory name means is not something to do on a hunch.
//!
//! Nothing here follows a symlink, for the reason
//! [`crate::provision::teardown`] does not: a provisioned worktree can hold a
//! link to the primary checkout's `node_modules`, and following it would first
//! count a directory this worktree does not own and then, on a clear, delete
//! the copy every worktree shares.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::provision::safe_join;

/// What a checkout is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Footprint {
    /// Every byte under it.
    pub total: u64,
    /// The part of that which is declared build output.
    pub build: u64,
}

impl Footprint {
    /// What would still be there once the build output was cleared.
    pub fn kept(self) -> u64 {
        self.total.saturating_sub(self.build)
    }
}

/// One directory a clear would remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clearable {
    /// Where it is, relative to the checkout's root.
    pub path: String,
    /// What it holds.
    pub bytes: u64,
}

/// Compiles the declared patterns into a matcher.
///
/// A pattern that will not compile is dropped with a warning rather than
/// failing the whole scan: one typo in a config list should cost that entry,
/// not every size in the sidebar.
fn matcher(build_dirs: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in build_dirs {
        match Glob::new(pattern) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(e) => {
                tracing::warn!(%e, pattern, "ignoring an unparsable build_dirs pattern");
            }
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Measures a checkout, separating its build output from the rest.
///
/// One walk for both numbers. A matched directory is measured whole and not
/// descended into — a `target/` inside a `target/` is already counted.
pub fn measure(worktree: &Path, build_dirs: &[String]) -> Footprint {
    let set = matcher(build_dirs);
    let mut out = Footprint::default();
    walk(worktree, Path::new(""), &set, &mut out);
    out
}

/// The walk behind [`measure`].
fn walk(dir: &Path, rel: &Path, set: &GlobSet, out: &mut Footprint) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // `symlink_metadata`, never `metadata` — see the module docs.
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }

        let child = rel.join(entry.file_name());

        if meta.is_dir() {
            if set.is_match(&child) {
                let bytes = tree_bytes(&path);
                out.total += bytes;
                out.build += bytes;
            } else {
                walk(&path, &child, set, out);
            }
        } else {
            out.total += meta.len();
        }
    }
}

/// Every byte under one directory, symlinks unfollowed.
fn tree_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    let mut bytes = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        bytes += if meta.is_dir() {
            tree_bytes(&path)
        } else {
            meta.len()
        };
    }
    bytes
}

/// What clearing this checkout would remove, largest first.
///
/// Separate from [`clear`] because the reader is shown this list and asked
/// before anything is deleted: the whole safety of the action rests on the
/// claim that everything in it is regenerable, and a claim worth making is
/// worth showing.
pub fn plan_clear(worktree: &Path, build_dirs: &[String]) -> Vec<Clearable> {
    let set = matcher(build_dirs);
    let mut found = Vec::new();
    collect(worktree, Path::new(""), &set, &mut found);
    found.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    found
}

/// The walk behind [`plan_clear`].
fn collect(dir: &Path, rel: &Path, set: &GlobSet, found: &mut Vec<Clearable>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }

        let child = rel.join(entry.file_name());

        if set.is_match(&child) {
            found.push(Clearable {
                path: child.to_string_lossy().into_owned(),
                bytes: tree_bytes(&path),
            });
        } else {
            collect(&path, &child, set, found);
        }
    }
}

/// What one clear managed to do.
///
/// The two numbers are separate because a clear can half-work: a directory
/// held open by a running build refuses to go while the rest of the plan
/// leaves without trouble. Reporting only the bytes would let that land as an
/// unqualified success, which is the one reading it must not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cleared {
    /// What the directories that went were holding.
    pub freed: u64,
    /// How many planned directories are still there. A directory that was
    /// already gone counts as neither: nothing was reclaimed and nothing is
    /// left to reclaim.
    pub failed: usize,
}

/// Deletes the planned directories, reporting what went and what would not.
///
/// Takes the plan rather than recomputing it, so what is deleted is exactly
/// what the reader was shown and agreed to — a second walk could find a
/// directory that appeared in between and remove something nobody named.
///
/// Every entry is re-checked on the way in. The plan came from a walk of this
/// same tree, so an escape is not reachable from a correct caller, but this
/// function deletes whole directories and checks anyway.
pub fn clear(worktree: &Path, plan: &[Clearable]) -> Cleared {
    let mut cleared = Cleared::default();

    for item in plan {
        let Some(path) = safe_join(worktree, &item.path) else {
            tracing::warn!(path = %item.path, "refusing a build directory outside the worktree");
            cleared.failed += 1;
            continue;
        };
        let Ok(meta) = path.symlink_metadata() else {
            continue; // Already gone.
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }

        match std::fs::remove_dir_all(&path) {
            Ok(()) => cleared.freed += item.bytes,
            Err(e) => {
                tracing::warn!(%e, path = %path.display(), "could not clear a build directory");
                cleared.failed += 1;
            }
        }
    }

    cleared
}

/// Sizes as the chrome shows them.
///
/// One decimal below ten and none above, so a column of them stays the same
/// width and the eye compares the digits rather than re-reading the unit:
/// `8.5 GB`, `142 MB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GB"), (1 << 20, "MB"), (1 << 10, "KB")];

    for (scale, unit) in UNITS {
        if bytes >= scale {
            let value = bytes as f64 / scale as f64;
            return if value < 10.0 {
                format!("{value:.1} {unit}")
            } else {
                format!("{value:.0} {unit}")
            };
        }
    }

    format!("{bytes} B")
}
