//! What a worktree changed, relative to the revision it branched from.
//!
//! [`crate::status`] answers "which files differ"; this answers "how". The two
//! are separate because they have different costs: a status refresh runs
//! constantly and touches metadata, while a diff reads file contents and is
//! computed only for a worktree someone is actually looking at.
//!
//! The comparison is **base to working tree**, not `HEAD` to working tree. An
//! agent may or may not have committed its work — some do, some leave
//! everything staged, some leave it dirty — and a reviewer comparing three
//! attempts does not care which. Diffing against the base makes all three
//! comparable regardless of how each agent chose to leave the worktree.
//!
//! Bounds, so memory stays flat: [`MAX_DIFF_FILES`] files and
//! [`MAX_DIFF_BYTES`] of hunk text in total, of which no single file may take
//! more than [`MAX_FILE_HUNK_BYTES`], and content over [`MAX_FILE_BYTES`] is
//! never read at all. An agent that regenerates a lock file produces a diff
//! nobody will read to the end, and holding all of it to render a list is
//! exactly the unbounded buffer the budget exists to prevent.
//!
//! # Where this deliberately differs from `git diff`
//!
//! Every one of these is asserted in `tests/diff.rs`, so a change of behaviour
//! shows up as a failing test rather than as a quietly different answer.
//!
//! - **Untracked files are included.** `git diff` skips them. An agent that
//!   writes a new file without staging it has still done work.
//! - **A type change is one entry, not two.** git renders a file that became a
//!   symlink as a deletion followed by an addition, under the same path. Here it
//!   is a single [`ChangeKind::TypeChange`] whose hunks go straight from the old
//!   contents to the link target, which is the shape a file list wants.
//! - **Renames are only claimed when the content is identical.** git's
//!   `diffcore-rename` also pairs a deletion with an addition it judges 50%
//!   similar. See [`collapse_renames`] for why ket does not.
//!
//! Everything else — submodule pointers, CRLF and `core.autocrlf`, binary
//! detection — is meant to agree with git exactly, and is checked against
//! `git diff --numstat` rather than against our beliefs about git.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use gix::bstr::ByteSlice;
use gix::diff::blob::unified_diff::{ConsumeHunk, ContextSize, DiffLineKind, HunkHeader};
use gix::diff::blob::{Algorithm, InternedInput, UnifiedDiff};
use serde::{Deserialize, Serialize};

use crate::status::{self, ChangeKind};
use crate::{KetError, Result};

/// Most files retained in one [`WorktreeDiff`].
pub const MAX_DIFF_FILES: usize = 500;

/// Most bytes of hunk text retained across one [`WorktreeDiff`].
pub const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

/// Most bytes of hunk text any one file may take out of [`MAX_DIFF_BYTES`].
///
/// The budget is spent in path order, so without a per-file share the first file
/// in the list can consume all of it and every file after it comes back empty —
/// a reviewer scrolling past a regenerated lock file would find the change they
/// were actually looking for blank. A quarter means the first four files always
/// get hunks, whatever the first one turns out to be.
pub const MAX_FILE_HUNK_BYTES: usize = MAX_DIFF_BYTES / 4;

/// Largest content, on either side, that is read in order to be diffed.
///
/// Twice the whole hunk budget: a file this size could not have its diff kept
/// even if it were computed, so reading it, interning every line and running the
/// diff would be work whose only possible outcome is being thrown away. Such a
/// file is reported as changed with [`FileDiff::truncated`] set.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Lines of unchanged context kept either side of a change.
///
/// Three matches `git diff`'s default, which matters because the verification
/// for this module is agreement with `git`.
pub const CONTEXT_LINES: u32 = 3;

/// How many leading bytes are examined when deciding whether a file is binary.
///
/// Matches git's own sniffing window, so that a file git calls binary is not one
/// ket tries to render as text.
const BINARY_SNIFF_BYTES: usize = 8000;

/// Which side of a diff a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    /// Present in both versions, shown for context.
    Context,
    /// Present only in the new version.
    Added,
    /// Present only in the old version.
    Removed,
}

impl LineKind {
    /// The character unified diff format uses to prefix this kind of line.
    pub fn prefix(self) -> char {
        match self {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        }
    }
}

/// One line within a hunk, without its unified-diff prefix character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// Which side the line came from.
    pub kind: LineKind,
    /// The line's text, without a trailing newline.
    ///
    /// Lossily decoded: a file that is *mostly* text but holds a stray invalid
    /// byte is still worth showing, and the alternative is refusing to display a
    /// diff over one bad byte.
    pub text: String,
}

/// A run of changed lines with its surrounding context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// 1-based first line on the old side.
    pub old_start: u32,
    /// Lines covered on the old side.
    pub old_lines: u32,
    /// 1-based first line on the new side.
    pub new_start: u32,
    /// Lines covered on the new side.
    pub new_lines: u32,
    /// The hunk's lines, in order.
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The `@@ -a,b +c,d @@` header line for this hunk.
    pub fn header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_lines, self.new_start, self.new_lines
        )
    }
}

/// One file's worth of difference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// Path relative to the worktree root, as it is now.
    pub path: String,
    /// Where the file came from, when [`FileDiff::kind`] is
    /// [`ChangeKind::Renamed`]. `None` otherwise.
    pub old_path: Option<String>,
    /// How the file differs.
    pub kind: ChangeKind,
    /// Whether either side looked like binary content.
    ///
    /// Binary files carry no hunks: a byte-level diff of a PNG helps nobody, and
    /// git declines to produce one for the same reason.
    pub binary: bool,
    /// The changes, or empty for a binary file, a rename with no edits, or one
    /// whose hunks were dropped to stay inside the byte budget.
    pub hunks: Vec<Hunk>,
    /// Whether this file's hunks were dropped because it did not fit the budget.
    ///
    /// Either it was over [`MAX_FILE_BYTES`] and so never read, or its hunks
    /// were over what was left of [`MAX_DIFF_BYTES`] and [`MAX_FILE_HUNK_BYTES`]
    /// when its turn came.
    pub truncated: bool,
}

/// Everything a worktree changed relative to its base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDiff {
    /// The revision compared against, as it was named.
    pub base: String,
    /// Changed files, sorted by path, at most [`MAX_DIFF_FILES`] of them.
    pub files: Vec<FileDiff>,
    /// How many files differed in total, including any not kept.
    pub total_files: usize,
    /// Whether [`WorktreeDiff::files`] is a subset.
    pub truncated: bool,
}

impl WorktreeDiff {
    /// Whether anything differs from the base at all.
    pub fn is_empty(&self) -> bool {
        self.total_files == 0
    }

    /// Renders the whole diff in unified format, as `git diff` would print it.
    pub fn to_unified(&self) -> String {
        let mut out = String::new();
        for file in &self.files {
            out.push_str(&file.to_unified());
        }
        out
    }
}

impl FileDiff {
    /// Lines added and removed, in that order.
    ///
    /// Context lines count as neither, so these are the numbers `git diff
    /// --numstat` reports rather than the size of the hunks.
    pub fn line_counts(&self) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for line in self.hunks.iter().flat_map(|hunk| hunk.lines.iter()) {
            match line.kind {
                LineKind::Added => added += 1,
                LineKind::Removed => removed += 1,
                LineKind::Context => {}
            }
        }
        (added, removed)
    }

    /// Renders this file's diff in unified format.
    pub fn to_unified(&self) -> String {
        let mut out = String::new();
        let old_path = self.old_path.as_deref().unwrap_or(&self.path);

        out.push_str(&format!("diff --git a/{} b/{}\n", old_path, self.path));

        if let Some(from) = &self.old_path {
            // Only exact renames are ever claimed, so the similarity is not a
            // number that needs computing.
            out.push_str("similarity index 100%\n");
            out.push_str(&format!("rename from {from}\n"));
            out.push_str(&format!("rename to {}\n", self.path));
        }

        let (old_label, new_label) = match self.kind {
            ChangeKind::Added | ChangeKind::Untracked => {
                ("/dev/null".to_owned(), format!("b/{}", self.path))
            }
            ChangeKind::Deleted => (format!("a/{}", self.path), "/dev/null".to_owned()),
            _ => (format!("a/{old_path}"), format!("b/{}", self.path)),
        };

        if self.binary {
            out.push_str(&format!(
                "Binary files {old_label} and {new_label} differ\n"
            ));
            return out;
        }

        if self.truncated {
            out.push_str(&format!(
                "--- {old_label}\n+++ {new_label}\n@@ diff omitted: too large for the {} MiB budget @@\n",
                MAX_DIFF_BYTES / (1024 * 1024)
            ));
            return out;
        }

        if self.hunks.is_empty() {
            return out;
        }

        out.push_str(&format!("--- {old_label}\n+++ {new_label}\n"));

        for hunk in &self.hunks {
            out.push_str(&hunk.header());
            out.push('\n');
            for line in &hunk.lines {
                out.push(line.kind.prefix());
                out.push_str(&line.text);
                out.push('\n');
            }
        }

        out
    }
}

/// What sort of thing a path holds on one side of the comparison.
///
/// A path can hold different sorts on the two sides — that is exactly what
/// [`ChangeKind::TypeChange`] means — so it is tracked per side rather than per
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    /// A regular file, executable or not.
    File,
    /// A symbolic link, whose content is the target path.
    Symlink,
    /// A gitlink: a commit id belonging to another repository.
    Submodule,
}

/// One side's content, or the reason there is none to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    /// The bytes, in the form git would store them.
    Bytes(Vec<u8>),
    /// Over [`MAX_FILE_BYTES`], so deliberately not read.
    TooLarge,
}

/// What a path holds on one side of the comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Side {
    kind: EntryKind,
    content: Content,
}

impl Side {
    /// The content, or `None` if it was too large to read.
    fn bytes(&self) -> Option<&[u8]> {
        match &self.content {
            Content::Bytes(bytes) => Some(bytes),
            Content::TooLarge => None,
        }
    }

    /// Whether this side was over [`MAX_FILE_BYTES`].
    fn too_large(&self) -> bool {
        matches!(self.content, Content::TooLarge)
    }

    /// Whether the two sides are *provably* the same thing.
    ///
    /// A side that was too large to read is never equal to anything, so a file
    /// whose content nobody looked at is reported as changed rather than
    /// silently dropped. The candidate set only contains paths git already
    /// decided differ — by object id on the committed half, by content hash on
    /// the dirty half — so taking it at its word here is close to free of false
    /// positives.
    fn same_as(&self, other: &Side) -> bool {
        self.kind == other.kind
            && matches!(
                (&self.content, &other.content),
                (Content::Bytes(a), Content::Bytes(b)) if a == b
            )
    }
}

/// Collects hunks as the unified-diff walker produces them, within a budget.
struct HunkCollector {
    hunks: Vec<Hunk>,
    budget: usize,
    spent: usize,
    over_budget: bool,
}

impl ConsumeHunk for HunkCollector {
    /// The hunks, the bytes they cost, and whether the budget ran out.
    type Out = (Vec<Hunk>, usize, bool);

    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(DiffLineKind, &[u8])],
    ) -> std::io::Result<()> {
        if self.over_budget {
            return Ok(());
        }

        let cost: usize = lines.iter().map(|(_, text)| text.len()).sum();
        if self.spent + cost > self.budget {
            // All or nothing per file: half a diff is worse than a diff that
            // says it was omitted, and dropping the lot means the file spends
            // nothing and the files after it keep their share.
            self.over_budget = true;
            self.hunks.clear();
            self.spent = 0;
            return Ok(());
        }
        self.spent += cost;

        self.hunks.push(Hunk {
            old_start: header.before_hunk_start,
            old_lines: header.before_hunk_len,
            new_start: header.after_hunk_start,
            new_lines: header.after_hunk_len,
            lines: lines
                .iter()
                .map(|(kind, text)| DiffLine {
                    kind: match kind {
                        DiffLineKind::Context => LineKind::Context,
                        DiffLineKind::Add => LineKind::Added,
                        DiffLineKind::Remove => LineKind::Removed,
                    },
                    text: String::from_utf8_lossy(strip_newline(text)).into_owned(),
                })
                .collect(),
        });

        Ok(())
    }

    fn finish(self) -> Self::Out {
        (self.hunks, self.spent, self.over_budget)
    }
}

/// Drops one trailing newline, and a preceding carriage return, if present.
fn strip_newline(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Whether content looks like something a byte-level diff would not help with.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|&byte| byte == 0)
}

/// The line git writes for a submodule pointer, with its newline.
///
/// Producing the same text git does is what makes a submodule diff a one-line
/// change on both sides instead of a special case every consumer has to know
/// about, and it is why `--numstat` agrees.
fn subproject_line(id: gix::ObjectId) -> Vec<u8> {
    format!("Subproject commit {id}\n").into_bytes()
}

/// The object id `bytes` would have if committed as a blob.
fn blob_id(repo: &gix::Repository, bytes: &[u8]) -> Option<gix::ObjectId> {
    gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Blob, bytes).ok()
}

/// The repository's worktree-to-git filters, if they could be assembled.
///
/// Without this, `core.autocrlf` or a `.gitattributes` `eol=` setting makes
/// every line of an untouched file look changed: git stores LF and checks out
/// CRLF, and comparing the stored blob against the bytes on disk compares the
/// two halves of a conversion git is doing on purpose.
struct Filters<'repo> {
    pipeline: gix::filter::Pipeline<'repo>,
    index: gix::index::File,
}

impl<'repo> Filters<'repo> {
    /// Builds the pipeline, or `None` if the repository cannot supply one.
    fn open(repo: &'repo gix::Repository) -> Option<Self> {
        let (pipeline, index) = repo.filter_pipeline(None).ok()?;
        Some(Filters {
            pipeline,
            index: index.into_owned(),
        })
    }

    /// Converts worktree bytes at `rel` into the form git would store.
    ///
    /// `None` when the conversion failed, which the caller treats as "use the
    /// bytes as they are": a filter ket could not run is a reason to show a
    /// possibly noisy diff, not to show no diff.
    fn convert_to_git(&mut self, rel: &str, bytes: &[u8]) -> Option<Vec<u8>> {
        let mut outcome = self
            .pipeline
            .convert_to_git(std::io::Cursor::new(bytes), Path::new(rel), &self.index)
            .ok()?;

        let mut out = Vec::with_capacity(bytes.len());
        outcome.read_to_end(&mut out).ok()?;
        Some(out)
    }
}

/// Diffs the worktree at `path` against `base`.
///
/// `base` is a revision name — see [`crate::worktree::Worktree::base_rev`],
/// which decides whether that is a moving branch or a frozen commit. Content on
/// the new side comes from the **working tree**, not from `HEAD`, so uncommitted
/// work by an agent is included.
///
/// Ignored files are excluded, matching [`crate::status::of_worktree`]: a
/// provisioned worktree holds a `node_modules` of 80,000 files, and diffing it
/// would make this proportional to the dependency tree rather than to the change.
pub fn of_worktree(path: &Path, base: &str) -> Result<WorktreeDiff> {
    let repo = gix::open(path).map_err(|e| KetError::gix("open", e))?;

    let base_tree = repo
        .rev_parse_single(base)
        .map_err(|e| KetError::gix("resolve base", e))?
        .object()
        .map_err(|e| KetError::gix("read base object", e))?
        .peel_to_tree()
        .map_err(|e| KetError::gix("peel base to tree", e))?;

    let mut candidates: BTreeSet<String> = BTreeSet::new();

    // Committed work: what the worktree's HEAD holds that the base does not.
    if let Ok(head_tree) = repo.head_tree() {
        let mut changes = base_tree
            .changes()
            .map_err(|e| KetError::gix("diff platform", e))?;

        // gix, like `git diff`, pairs a deletion with a similar addition by
        // default, and reports the pair at the destination path only. Left on,
        // that quietly drops the source path from the diff — and it would only
        // ever apply to the half of the change the agent happened to commit, so
        // a move would be a rename or a delete-plus-add depending on whether the
        // worktree was left dirty. Renames are paired once, over both halves, in
        // `collapse_renames`; the similarity metric this turns off is also the
        // most expensive thing the tree walk could be doing.
        changes.options(|options| {
            options.track_rewrites(None);
        });

        changes
            .for_each_to_obtain_tree(&head_tree, |change| {
                // Directories change whenever anything under them does, so the
                // walk reports them alongside the files. Only blobs, links and
                // gitlinks carry content worth diffing.
                if !change.entry_mode().is_tree() {
                    candidates.insert(change.location().to_str_lossy().into_owned());
                }
                Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
            })
            .map_err(|e| KetError::gix("diff base to head", e))?;
    }

    // Uncommitted work: what the working tree holds that HEAD does not.
    let status = status::of_worktree(path, None)?;
    for file in &status.files {
        match file.path.strip_suffix('/') {
            // Status collapses an untracked directory to a single entry, which
            // is right for a file list and useless for a diff: the agent's new
            // module would show up as one nameless directory and none of its
            // contents. Expand it back out.
            Some(dir) => collect_files_under(path, dir, &mut candidates),
            None => {
                candidates.insert(file.path.clone());
            }
        }
    }

    let mut filters = Filters::open(&repo);

    let total_candidates = candidates.len();
    let mut files = Vec::new();
    let mut contents: Vec<Option<gix::ObjectId>> = Vec::new();
    let mut remaining_bytes = MAX_DIFF_BYTES;
    let mut total_files = 0usize;

    for rel in candidates {
        let old = read_from_tree(&base_tree, &rel);
        let new = read_from_worktree(path, &rel, filters.as_mut());

        // A submodule that is registered but not checked out is a directory we
        // cannot see the HEAD of. git compares the recorded gitlink against
        // itself and reports nothing; reporting a deletion would be worse than
        // useless, because "the agent removed the submodule" is a different
        // change from "the submodule was never initialised".
        if new.is_none()
            && old.as_ref().is_some_and(|s| s.kind == EntryKind::Submodule)
            && path.join(&rel).is_dir()
        {
            continue;
        }

        let changed = match (&old, &new) {
            (None, None) => false,
            (Some(old), Some(new)) => !old.same_as(new),
            _ => true,
        };
        if !changed {
            continue;
        }
        total_files += 1;

        if files.len() >= MAX_DIFF_FILES {
            continue;
        }

        let kind = classify(old.as_ref(), new.as_ref(), &status, &rel);
        let old_bytes = old.as_ref().and_then(Side::bytes);
        let new_bytes = new.as_ref().and_then(Side::bytes);

        // Kept so `collapse_renames` can pair the two halves of a move. Only
        // the halves that could be one are hashed.
        contents.push(match kind {
            ChangeKind::Deleted => old_bytes.and_then(|bytes| blob_id(&repo, bytes)),
            ChangeKind::Added | ChangeKind::Untracked => {
                new_bytes.and_then(|bytes| blob_id(&repo, bytes))
            }
            _ => None,
        });

        let (file, spent) = compare(rel, kind, old.as_ref(), new.as_ref(), remaining_bytes)?;
        remaining_bytes = remaining_bytes.saturating_sub(spent);
        files.push(file);
    }

    total_files -= collapse_renames(&mut files, &contents);

    Ok(WorktreeDiff {
        base: base.to_owned(),
        files,
        total_files,
        truncated: total_files > MAX_DIFF_FILES || total_candidates > MAX_DIFF_FILES,
    })
}

/// One file's difference between `base` and the working tree, on its own.
///
/// What a view showing a single file wants. [`of_worktree`] shares its byte
/// budget across every file in path order, so a file late in a large change can
/// come back with its hunks dropped; asked for alone, a file has the whole of
/// [`MAX_FILE_HUNK_BYTES`] to itself.
///
/// `rel` is slash-separated from the worktree root. `None` when the file does
/// not differ. A rename is not paired here — that takes both halves, and only
/// the whole-worktree walk sees both — so the new path of a moved file reads
/// as an addition.
pub fn file_of_worktree(path: &Path, base: &str, rel: &str) -> Result<Option<FileDiff>> {
    let repo = gix::open(path).map_err(|e| KetError::gix("open", e))?;
    let base_tree = repo
        .rev_parse_single(base)
        .map_err(|e| KetError::gix("resolve base", e))?
        .object()
        .map_err(|e| KetError::gix("read base object", e))?
        .peel_to_tree()
        .map_err(|e| KetError::gix("peel base to tree", e))?;

    let old = read_from_tree(&base_tree, rel);
    let mut filters = Filters::open(&repo);
    let new = read_from_worktree(path, rel, filters.as_mut());
    let changed = match (&old, &new) {
        (None, None) => false,
        (Some(old), Some(new)) => !old.same_as(new),
        _ => true,
    };
    if !changed {
        return Ok(None);
    }

    let status = status::of_worktree(path, None)?;
    let kind = classify(old.as_ref(), new.as_ref(), &status, rel);
    let (file, _) = compare(
        rel.to_owned(),
        kind,
        old.as_ref(),
        new.as_ref(),
        MAX_FILE_HUNK_BYTES,
    )?;
    Ok(Some(file))
}

/// Diffs one path whose two sides are already read, spending at most
/// `remaining` bytes of hunk text (and never more than [`MAX_FILE_HUNK_BYTES`]).
///
/// Returns the file and how many bytes it spent.
fn compare(
    rel: String,
    kind: ChangeKind,
    old: Option<&Side>,
    new: Option<&Side>,
    remaining: usize,
) -> Result<(FileDiff, usize)> {
    if old.is_some_and(Side::too_large) || new.is_some_and(Side::too_large) {
        let file = FileDiff {
            path: rel,
            old_path: None,
            kind,
            binary: false,
            hunks: Vec::new(),
            truncated: true,
        };
        return Ok((file, 0));
    }

    let old_bytes = old.and_then(Side::bytes);
    let new_bytes = new.and_then(Side::bytes);
    if old_bytes.is_some_and(looks_binary) || new_bytes.is_some_and(looks_binary) {
        let file = FileDiff {
            path: rel,
            old_path: None,
            kind,
            binary: true,
            hunks: Vec::new(),
            truncated: false,
        };
        return Ok((file, 0));
    }

    let input = InternedInput::new(old_bytes.unwrap_or_default(), new_bytes.unwrap_or_default());
    let diff = gix::diff::blob::Diff::compute(Algorithm::Histogram, &input);

    let collector = HunkCollector {
        hunks: Vec::new(),
        budget: remaining.min(MAX_FILE_HUNK_BYTES),
        spent: 0,
        over_budget: false,
    };
    let (hunks, spent, over_budget) = UnifiedDiff::new(
        &diff,
        &input,
        collector,
        ContextSize::symmetrical(CONTEXT_LINES),
    )
    .consume()
    .map_err(|e| KetError::io("render diff", e))?;

    let file = FileDiff {
        path: rel,
        old_path: None,
        kind,
        binary: false,
        hunks,
        truncated: over_budget,
    };
    Ok((file, spent))
}

/// What one line of a file does, against the revision it is compared with.
///
/// New-side coordinates throughout: a mark names a line that is there now,
/// which is what a rail beside the text can point at. Lines that were taken
/// out have no line of their own to mark, so they are recorded on the line
/// they were taken out beside — see [`LineMark::RemovedAbove`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMark {
    /// Not in the old version at all.
    Added,
    /// Standing where a different line stood.
    Modified,
    /// Unchanged itself; lines were taken out immediately above it.
    RemovedAbove,
    /// Unchanged itself; lines were taken out after it. Only ever the last
    /// line of the file, where there is nothing below to hang the mark on.
    RemovedBelow,
}

/// Which lines of one file differ, indexed by where each line is now.
///
/// Empty is the honest answer to most questions here — a file outside a
/// repository, a binary one, one too large to read — so this is what
/// [`line_marks`] returns for all of them rather than an error the caller
/// would have to tell apart from a clean file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineMarks {
    marks: Vec<Option<LineMark>>,
}

impl LineMarks {
    /// The mark for a zero-based line, or `None` where it is unchanged.
    pub fn at(&self, line: usize) -> Option<LineMark> {
        self.marks.get(line).copied().flatten()
    }

    /// Whether nothing in the file differs.
    pub fn is_empty(&self) -> bool {
        self.marks.iter().all(Option::is_none)
    }
}

/// Which lines of `text` — what an editor is showing for the file at `path` —
/// differ from the version `HEAD` holds.
///
/// `HEAD`, where the rest of this module compares against a worktree's base.
/// The two answer different questions and both are wanted: the base is what a
/// reviewer reads a whole worktree against, while a rail beside the line
/// numbers is asking the `git status` question — what is uncommitted in the
/// file in front of me — and against a base it would mark every line of an
/// agent's committed work as changed for as long as the worktree lived.
///
/// `text` rather than the file on disk, so the marks stay true while somebody
/// is typing: an insertion at the top of the file moves every mark below it,
/// and marks read off disk would sit a line out until the next save.
///
/// No marks at all — rather than an error — for everything there is honestly
/// nothing to say about: a path outside any repository's working tree, a
/// repository with no commit yet, binary content on either side, and a file
/// over [`MAX_FILE_BYTES`]. The only error raised is a repository that cannot
/// be opened at all.
pub fn line_marks(path: &Path, text: &str) -> Result<LineMarks> {
    let repo = gix::discover(path.parent().unwrap_or(path))
        .map_err(|e| KetError::gix("discover repository", e))?;
    let Some(rel) = worktree_relative(&repo, path) else {
        return Ok(LineMarks::default());
    };

    // Absent when `HEAD` is unborn — a repository whose first commit has not
    // been made — which is the same answer as a file git has never seen: no
    // old side, so every line is an addition.
    let old = repo
        .head_tree()
        .ok()
        .and_then(|tree| read_from_tree(&tree, &rel));
    if old.as_ref().is_some_and(Side::too_large) || text.len() as u64 > MAX_FILE_BYTES {
        return Ok(LineMarks::default());
    }

    // Through the same filters `of_worktree` reads with: a repository that
    // stores LF and checks out CRLF would otherwise report every line of
    // every file as modified.
    let mut filters = Filters::open(&repo);
    let new = filters
        .as_mut()
        .and_then(|filters| filters.convert_to_git(&rel, text.as_bytes()))
        .unwrap_or_else(|| text.as_bytes().to_vec());

    let old = old.as_ref().and_then(Side::bytes).unwrap_or_default();
    if looks_binary(old) || looks_binary(&new) {
        return Ok(LineMarks::default());
    }

    let input = InternedInput::new(old, new.as_slice());
    let diff = gix::diff::blob::Diff::compute(Algorithm::Histogram, &input);

    let mut marks: Vec<Option<LineMark>> = vec![None; input.after.len()];
    for hunk in diff.hunks() {
        let (start, end) = (hunk.after.start as usize, hunk.after.end as usize);

        if start == end {
            // A deletion covers no line on this side, so it is hung on the
            // line that closed over it — the one now at that index, or the
            // last line of the file when the deletion ran off the end.
            let (at, mark) = match start < marks.len() {
                true => (start, LineMark::RemovedAbove),
                false => (marks.len().saturating_sub(1), LineMark::RemovedBelow),
            };
            // Never over an addition or a modification: those say more about
            // the line than "something used to be next to it".
            if let Some(slot @ None) = marks.get_mut(at) {
                *slot = Some(mark);
            }
            continue;
        }

        let mark = match hunk.before.is_empty() {
            true => LineMark::Added,
            false => LineMark::Modified,
        };
        let end = end.min(marks.len());
        for slot in &mut marks[start..end] {
            *slot = Some(mark);
        }
    }

    Ok(LineMarks { marks })
}

/// Where `path` sits inside `repo`'s working tree, as git spells it.
///
/// `None` for a bare repository, a path outside the working tree, and a name
/// that is not valid UTF-8 — none of which is an error worth raising to a
/// rail that simply has nothing to draw.
fn worktree_relative(repo: &gix::Repository, path: &Path) -> Option<String> {
    let workdir = repo.workdir()?;
    let rel = match path.strip_prefix(workdir) {
        Ok(rel) => rel.to_path_buf(),
        // `/tmp` is a symlink to `/private/tmp` on macOS, so a worktree can be
        // reached by one name and discovered under the other.
        Err(_) => path
            .canonicalize()
            .ok()?
            .strip_prefix(workdir.canonicalize().ok()?)
            .ok()?
            .to_path_buf(),
    };
    Some(rel.to_str()?.to_owned())
}

/// Pairs each deletion with an addition of byte-identical content.
///
/// The paired entry becomes a [`ChangeKind::Renamed`] carrying
/// [`FileDiff::old_path`], and the deletion is dropped. Returns how many pairs
/// were collapsed, so the file count stays the one a reviewer would arrive at by
/// hand.
///
/// **Only exact content matches count.** `git diff` also pairs a deletion with
/// an addition it scores at least 50% similar, and ket deliberately does not,
/// for two reasons:
///
/// - Similarity scoring is quadratic in the size of the change — every deletion
///   against every addition — which is exactly the cost this module is bounded
///   against everywhere else. git caps it with `diff.renameLimit` and silently
///   stops detecting renames past 1000 candidates, so the *category* a change is
///   reported under starts depending on how big the rest of the change was.
/// - The score would have to track `diffcore-rename`'s own heuristic to keep
///   agreeing with git, and a rename ket got wrong is worse than a rename it
///   never claimed: a move plus edits still reads correctly as a deletion and an
///   addition, whereas a wrong pairing hides real content behind "renamed".
///
/// Exact matches need no heuristic and no threshold: the content hashes are
/// equal or they are not, and one pass over a map settles it.
fn collapse_renames(files: &mut Vec<FileDiff>, contents: &[Option<gix::ObjectId>]) -> usize {
    let mut deletions: BTreeMap<gix::ObjectId, usize> = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        if file.kind == ChangeKind::Deleted
            && let Some(id) = contents[index]
        {
            deletions.entry(id).or_insert(index);
        }
    }
    if deletions.is_empty() {
        return 0;
    }

    let mut consumed = vec![false; files.len()];
    let mut pairs = 0;

    for index in 0..files.len() {
        if !matches!(files[index].kind, ChangeKind::Added | ChangeKind::Untracked) {
            continue;
        }
        let Some(id) = contents[index] else { continue };
        // A second addition with the same content is a copy, and git does not
        // report copies unless asked. First one wins, the rest stay additions.
        let Some(&from) = deletions.get(&id).filter(|&&from| !consumed[from]) else {
            continue;
        };

        consumed[from] = true;
        files[index].kind = ChangeKind::Renamed;
        files[index].old_path = Some(files[from].path.clone());
        // Identical content: there is nothing to show, and git prints no hunks
        // for a 100% rename either.
        files[index].hunks = Vec::new();
        pairs += 1;
    }

    let mut index = 0;
    files.retain(|_| {
        let keep = !consumed[index];
        index += 1;
        keep
    });

    pairs
}

/// Decides how a file differs, preferring what `status` already worked out.
fn classify(
    old: Option<&Side>,
    new: Option<&Side>,
    status: &status::WorktreeStatus,
    rel: &str,
) -> ChangeKind {
    match (old, new) {
        (None, Some(_)) => {
            // An untracked file and a committed addition are both "new here",
            // but only the former is reported by status, so trust status when it
            // knows.
            status
                .files
                .iter()
                .find(|f| f.path.trim_end_matches('/') == rel)
                .map(|f| f.kind)
                .filter(|k| matches!(k, ChangeKind::Untracked))
                .unwrap_or(ChangeKind::Added)
        }
        (Some(_), None) => ChangeKind::Deleted,
        // A file that became a symlink, a symlink that became a submodule
        // directory, and so on. git renders this as a deletion plus an addition
        // under one path; one entry is what a file list wants.
        (Some(old), Some(new)) if old.kind != new.kind => ChangeKind::TypeChange,
        _ => ChangeKind::Modified,
    }
}

/// Reads a path out of a tree, or `None` if it is not there or is a directory.
///
/// A gitlink yields the `Subproject commit <id>` line git shows for it rather
/// than the commit object, which lives in another repository and is usually not
/// present in this one at all.
fn read_from_tree(tree: &gix::Tree<'_>, rel: &str) -> Option<Side> {
    let entry = tree.lookup_entry_by_path(rel).ok().flatten()?;

    let kind = match entry.mode().kind() {
        gix::objs::tree::EntryKind::Tree => return None,
        gix::objs::tree::EntryKind::Commit => {
            return Some(Side {
                kind: EntryKind::Submodule,
                content: Content::Bytes(subproject_line(entry.id().detach())),
            });
        }
        gix::objs::tree::EntryKind::Link => EntryKind::Symlink,
        gix::objs::tree::EntryKind::Blob | gix::objs::tree::EntryKind::BlobExecutable => {
            EntryKind::File
        }
    };

    // The header is a few bytes out of the pack or the loose object's prefix;
    // asking it first is what keeps an oversized blob from being inflated. A
    // header that will not read is no reason to give up on the file — the read
    // below fails for the same reason if the object really is missing.
    let oversize = entry
        .id()
        .header()
        .is_ok_and(|header| header.size() > MAX_FILE_BYTES);
    if oversize {
        return Some(Side {
            kind,
            content: Content::TooLarge,
        });
    }

    let object = entry.object().ok()?;
    Some(Side {
        kind,
        content: Content::Bytes(object.data.clone()),
    })
}

/// Reads a path out of the working tree, or `None` if there is nothing there.
///
/// Symbolic links are read as links — the target path, which is what git stores
/// — rather than followed, so a link to a file does not masquerade as a copy of
/// it and a dangling link is not mistaken for a deletion.
fn read_from_worktree(root: &Path, rel: &str, filters: Option<&mut Filters<'_>>) -> Option<Side> {
    let full = root.join(rel);
    let meta = std::fs::symlink_metadata(&full).ok()?;

    if meta.is_symlink() {
        let target = std::fs::read_link(&full).ok()?;
        return Some(Side {
            kind: EntryKind::Symlink,
            content: Content::Bytes(gix::path::into_bstr(target).into_owned().into()),
        });
    }

    if meta.is_dir() {
        // The only kind of directory that is content rather than structure.
        let head = gix::open(&full).ok()?.head_id().ok()?.detach();
        return Some(Side {
            kind: EntryKind::Submodule,
            content: Content::Bytes(subproject_line(head)),
        });
    }

    if !meta.is_file() {
        return None;
    }

    if meta.len() > MAX_FILE_BYTES {
        return Some(Side {
            kind: EntryKind::File,
            content: Content::TooLarge,
        });
    }

    let raw = std::fs::read(&full).ok()?;
    let converted = match filters {
        Some(filters) => filters.convert_to_git(rel, &raw),
        None => None,
    };
    Some(Side {
        kind: EntryKind::File,
        content: Content::Bytes(converted.unwrap_or(raw)),
    })
}

/// Adds every file beneath `dir` to `out`, as worktree-relative paths.
///
/// Bounded by [`MAX_DIFF_FILES`]: an untracked directory is usually a new
/// module, but it could equally be a dependency tree someone forgot to ignore,
/// and walking that would make the diff proportional to it.
///
/// A nested repository is added as itself and not descended into. An agent that
/// cloned something into the worktree made one change a reviewer can act on —
/// "there is a repository here, at this commit" — and expanding it would fill
/// the diff with that repository's files and its `.git`.
fn collect_files_under(root: &Path, dir: &str, out: &mut BTreeSet<String>) {
    let mut stack = vec![dir.to_owned()];

    while let Some(rel) = stack.pop() {
        if out.len() >= MAX_DIFF_FILES {
            return;
        }

        if root.join(&rel).join(".git").exists() {
            out.insert(rel);
            continue;
        }

        let Ok(entries) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };

        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let child = format!("{rel}/{name}");

            // `file_type` here does not follow links, so a symlink is a symlink
            // whatever it points at — including nothing.
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(child),
                Ok(kind) if kind.is_file() || kind.is_symlink() => {
                    out.insert(child);
                }
                _ => {}
            }
        }
    }
}
