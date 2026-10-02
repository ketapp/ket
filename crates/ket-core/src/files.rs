//! Reading a worktree's files, for the explorer and the file finder.
//!
//! Two shapes, because the two surfaces want different things. The explorer
//! expands one directory at a time and wants everything in it — including the
//! empty directories a flat list would lose. The finder wants every file at
//! once and does not care about structure. Building the tree out of the flat
//! list would have saved a function and cost the empty directories, which are
//! exactly the ones a person is about to put a file into.
//!
//! Both know the repository's ignore rules, which means git's rules rather
//! than an approximation of them: `.gitignore` at every level, the repository's
//! own `info/exclude`, and the user's global file. They put them to opposite
//! uses. The finder leaves ignored files out of the index altogether: nobody is
//! looking for a file in `target/`, and a build directory would be most of what
//! the index held. The explorer keeps them and marks them — see
//! [`Entry::ignored`] — because a tree is a picture of a directory, and one
//! that silently omits a file the reader can see in a terminal beside it is a
//! tree that lies about what is on disk. What to do about the mark is the
//! explorer's business; hiding it is not this module's decision to make.
//!
//! Both are bounded. A directory with a hundred thousand files in it is rare
//! but not hypothetical, and the shell draws this on a click.

use std::path::{Path, PathBuf};

use crate::error::{KetError, Result};

/// Entries returned for one directory.
///
/// Past this the listing is one nobody reads, and an unbounded `Vec` here
/// becomes an unbounded number of elements in the sidebar.
pub const MAX_DIR_ENTRIES: usize = 2_000;

/// Files the finder will index.
///
/// Chosen so the index stays a few megabytes on the largest repository anyone
/// opens in a tool like this, rather than so it fits any particular one.
pub const MAX_INDEX_FILES: usize = 20_000;

/// What a directory entry is, as far as the explorer needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    /// A directory, which can be expanded.
    Directory,
    /// A regular file, which can be opened.
    File,
    /// A symlink, shown but never followed.
    ///
    /// Reported rather than resolved: following one turns a listing into a walk
    /// of somewhere else entirely, and a cycle into a hang.
    Symlink,
}

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The final path component, which is what the tree shows.
    pub name: String,
    /// Slash-separated path from the worktree root. Empty for the root itself.
    ///
    /// Slash-separated on every platform because it is an identity — the key
    /// the explorer remembers a directory's expanded state under — and not a
    /// path to hand to the filesystem. Use [`Entry::path`] for that.
    pub rela: String,
    /// Absolute path, for opening the file.
    pub path: PathBuf,
    /// File, directory, or symlink.
    pub kind: EntryKind,
    /// Whether git's ignore rules match this entry.
    ///
    /// Always `false` where there is no repository, because nothing then
    /// defines what ignoring would mean.
    pub ignored: bool,
}

/// Every file under a worktree, flattened, for a fuzzy finder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileIndex {
    /// Slash-separated paths from the worktree root, sorted.
    paths: Vec<String>,
    /// Whether [`MAX_INDEX_FILES`] cut the walk short.
    truncated: bool,
}

impl FileIndex {
    /// The indexed paths, sorted.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Whether the walk hit [`MAX_INDEX_FILES`] and stopped.
    ///
    /// Surfaced rather than swallowed: a finder that silently cannot see half
    /// the repository is worse than one that says so.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// How many files were indexed.
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// Whether nothing was indexed.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Joins a relative directory onto a root, refusing anything that escapes it.
///
/// The relative path arrives from the shell, which got it from a previous
/// listing — but "it came from us" is not a property this function can check,
/// and a `..` here reads a directory the user never opened.
fn resolve(root: &Path, rela: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in rela.split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." {
            return Err(KetError::Config(format!(
                "path escapes the worktree: {rela}"
            )));
        }
        path.push(part);
    }
    Ok(path)
}

/// Decides which entries git would ignore, for one worktree.
///
/// Rebuilt per listing rather than held: `gix`'s stack borrows the repository
/// it came from, so keeping one means keeping both in a struct that borrows
/// itself. Rebuilding also picks up a `.gitignore` the user just edited, which
/// a cached stack would not.
struct Excludes {
    /// The repository the rules come from.
    repo: gix::Repository,
}

impl Excludes {
    /// Opens the repository at `root`, or `None` when there is not one.
    ///
    /// Not an error: a directory that is not a repository still lists, it just
    /// has nothing to ignore by.
    fn open(root: &Path) -> Option<Self> {
        let repo = gix::open(root).ok()?;
        Some(Self { repo })
    }

    /// Calls `visit` with a predicate that answers "would git ignore this?".
    ///
    /// Takes a closure rather than returning the stack for the borrow reason in
    /// the type's own documentation.
    fn with<T>(&self, visit: impl FnOnce(&mut dyn FnMut(&str, bool) -> bool) -> T) -> Result<T> {
        let index = self
            .repo
            .index_or_empty()
            .map_err(|e| KetError::Config(format!("reading the index: {e}")))?;
        let mut stack = self
            .repo
            .excludes(
                &index,
                None,
                gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            )
            .map_err(|e| KetError::Config(format!("reading ignore rules: {e}")))?;

        let mut excluded = |rela: &str, is_dir: bool| {
            let mode = if is_dir {
                gix::index::entry::Mode::DIR
            } else {
                gix::index::entry::Mode::FILE
            };
            // A path the rules cannot be evaluated for is shown, not hidden: the
            // failure mode of guessing "ignored" is a file that has vanished
            // from the explorer with no way to ask why.
            stack
                .at_entry(rela, Some(mode))
                .map(|platform| platform.is_excluded())
                .unwrap_or(false)
        };

        Ok(visit(&mut excluded))
    }
}

/// Lists one directory of a worktree, marking what git ignores.
///
/// Marked, not dropped — see the module doc. `.git` is the one exception, and
/// it is not an ignore rule that hides it.
///
/// `rela` is slash-separated and relative to `root`; empty lists the root.
/// Directories sort before files, then by name, case-insensitively — the order
/// every file explorer uses, and the one a person scans by.
///
/// Never recurses: the explorer asks again when a directory is expanded, so the
/// cost of opening a project is the cost of its top level.
pub fn read_dir(root: &Path, rela: &str) -> Result<Vec<Entry>> {
    let raw = raw_entries(root, rela)?;

    // One stack for the whole directory rather than one per entry: building it
    // reads every `.gitignore` on the way down.
    let entries = match Excludes::open(root) {
        Some(excludes) => excludes.with(|excluded| mark_ignored(raw, excluded))?,
        None => raw,
    };

    Ok(present(entries))
}

/// One directory's entries, before anything has decided what git ignores.
///
/// Split out so a sweep over several directories can build the ignore rules
/// once for all of them — see [`read_dirs`].
fn raw_entries(root: &Path, rela: &str) -> Result<Vec<Entry>> {
    let dir = resolve(root, rela)?;

    let mut raw = Vec::new();
    let reader = std::fs::read_dir(&dir).map_err(|e| KetError::io(&dir, e))?;
    for entry in reader {
        // One unreadable entry must not lose the other four hundred.
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            // A name that is not UTF-8 has no `rela` to key state by, and
            // showing it as replacement characters would make two different
            // files look like one.
            continue;
        };

        // `.git` is never shown. It is not ignored by git's own rules — it is
        // simply not part of anyone's project.
        if rela.is_empty() && name == ".git" {
            continue;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };

        let child = if rela.is_empty() {
            name.clone()
        } else {
            format!("{rela}/{name}")
        };
        raw.push(Entry {
            name,
            rela: child,
            path: entry.path(),
            kind,
            // Filled in below, where the rules are read once for the whole
            // directory rather than once per entry.
            ignored: false,
        });
    }

    Ok(raw)
}

/// Reads several directories, building the ignore rules once for all of them.
///
/// [`read_dir`] opens the repository and reads its index every time it is
/// called, because one listing is all it has to amortise that over. A caller
/// re-reading everything on screen — the file tree on the shell's tick — calls
/// it once per visible directory and pays for that index read once per
/// directory, which scales with the size of the *repository* rather than the
/// size of what is being listed. This reads the rules once for the sweep.
///
/// Answers are returned in the order asked for, each one whatever [`read_dir`]
/// would have returned for it. A directory that has gone since it was last
/// listed is its own `Err`, not the end of the sweep: the rest of the tree is
/// still true.
pub fn read_dirs(root: &Path, relas: &[String]) -> Vec<(String, Result<Vec<Entry>>)> {
    let raw: Vec<(String, Result<Vec<Entry>>)> = relas
        .iter()
        .map(|rela| (rela.clone(), raw_entries(root, rela)))
        .collect();

    let marked = match Excludes::open(root) {
        Some(excludes) => excludes.with(|excluded| {
            raw.into_iter()
                .map(|(rela, entries)| (rela, entries.map(|e| mark_ignored(e, excluded))))
                .collect::<Vec<_>>()
        }),
        // Not a repository: nothing defines what ignoring would mean, and the
        // listings stand as they are.
        None => Ok(raw),
    };

    match marked {
        Ok(listings) => listings
            .into_iter()
            .map(|(rela, entries)| (rela, entries.map(present)))
            .collect(),
        // The rules could not be read at all. Every listing is still a real
        // listing; they simply do not know what is ignored.
        Err(e) => {
            tracing::warn!(%e, root = %root.display(), "listing without ignore rules");
            relas
                .iter()
                .map(|rela| (rela.clone(), raw_entries(root, rela).map(present)))
                .collect()
        }
    }
}

/// Applies the ignore predicate to a listing.
fn mark_ignored(raw: Vec<Entry>, excluded: &mut dyn FnMut(&str, bool) -> bool) -> Vec<Entry> {
    raw.into_iter()
        .map(|entry| Entry {
            ignored: excluded(&entry.rela, entry.kind == EntryKind::Directory),
            ..entry
        })
        .collect()
}

/// Orders a listing the way the tree shows it, and bounds it.
///
/// Directories first, then by name folded to lowercase — the order a person
/// reading a file tree expects, rather than the order the filesystem happened
/// to hand back.
fn present(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by(|a, b| {
        let after_folders = |entry: &Entry| entry.kind != EntryKind::Directory;
        after_folders(a)
            .cmp(&after_folders(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    entries.truncate(MAX_DIR_ENTRIES);
    entries
}

/// Every non-ignored file under `root`, for the finder.
///
/// Tracked and untracked alike, because a file you created two minutes ago is
/// the one you are most likely to be looking for.
///
/// Stops at [`MAX_INDEX_FILES`] and says so in [`FileIndex::truncated`].
pub fn index(root: &Path) -> Result<FileIndex> {
    index_with_limit(root, MAX_INDEX_FILES)
}

/// Every non-ignored file for content search, without the fuzzy finder's cap.
pub(crate) fn search_index(root: &Path) -> Result<FileIndex> {
    index_with_limit(root, usize::MAX)
}

/// Builds the shared flat index with the caller's surface-specific bound.
fn index_with_limit(root: &Path, limit: usize) -> Result<FileIndex> {
    let repo = match gix::open(root) {
        Ok(repo) => repo,
        // Not a repository: walk it plainly rather than refusing. Nothing is
        // ignored because nothing defines what "ignored" would mean.
        Err(_) => return index_without_git(root, limit),
    };

    let git_index = repo
        .index_or_empty()
        .map_err(|e| KetError::Config(format!("reading the index: {e}")))?;
    let options = repo
        .dirwalk_options()
        .map_err(|e| KetError::Config(format!("reading walk settings: {e}")))?
        // Every file, rather than a collapsed `src/`: the finder matches on
        // whole paths, and a directory is not something it can offer.
        .emit_untracked(gix::dir::walk::EmissionMode::Matching)
        .emit_ignored(None)
        .emit_empty_directories(false)
        .emit_pruned(false)
        .emit_tracked(true);

    let walk = repo
        .dirwalk_iter(git_index, None::<&str>, Default::default(), options)
        .map_err(|e| KetError::Config(format!("walking the worktree: {e}")))?;

    let mut paths = Vec::new();
    let mut truncated = false;
    for item in walk {
        let Ok(item) = item else { continue };
        // Directories and symlinks are not things the finder can open. `None`
        // is a tracked entry the walk did not stat, which is a file.
        if !matches!(
            item.entry.disk_kind,
            Some(gix::dir::entry::Kind::File) | None
        ) {
            continue;
        }
        if paths.len() >= limit {
            truncated = true;
            break;
        }
        paths.push(item.entry.rela_path.to_string());
    }

    paths.sort();
    paths.dedup();
    Ok(FileIndex { paths, truncated })
}

/// The walk for a directory that is not a git repository.
///
/// Bounded by the same limit, so an accidental symlink loop or a home
/// directory cannot turn the finder into a hang — [`read_dir`] does not follow
/// symlinks, so only the count is doing that work.
fn index_without_git(root: &Path, limit: usize) -> Result<FileIndex> {
    let mut paths = Vec::new();
    let mut queue = vec![String::new()];
    let mut truncated = false;

    while let Some(rela) = queue.pop() {
        let Ok(entries) = read_dir(root, &rela) else {
            continue;
        };
        for entry in entries {
            // Nothing is ignored on this path — there is no repository to say
            // so — but the finder's contract is that the index holds no
            // ignored file, and a later caller must not be able to break it by
            // reaching this walk with rules in force.
            if entry.ignored {
                continue;
            }
            match entry.kind {
                // On the off chance one appears below the root.
                EntryKind::Directory if entry.name != ".git" => queue.push(entry.rela),
                EntryKind::File => {
                    if paths.len() >= limit {
                        truncated = true;
                        queue.clear();
                        break;
                    }
                    paths.push(entry.rela);
                }
                _ => {}
            }
        }
    }

    paths.sort();
    Ok(FileIndex { paths, truncated })
}
