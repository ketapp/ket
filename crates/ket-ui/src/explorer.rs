//! The file explorer, the sidebar rail it lives on, and the file finder.
//!
//! **The panel.** The tree lives in a panel on the right of the window,
//! beside the content rather than in the projects sidebar: the two are looked
//! at together — which worktree, then what is in it — and a rail switching one
//! sidebar between them made that a toggle. The panel is the home for more
//! than files later on, so what is here is the panel's frame and its first
//! occupant.
//!
//! **The tree.** One directory is read per expansion, never the whole
//! worktree, so opening a project costs its top level and nothing more. What
//! has been read is kept, so collapsing and re-expanding is free; what is
//! expanded is remembered by path, so the shape survives a reload.
//!
//! The rows are flattened outside the render loop and drawn through
//! `uniform_list`: an element per row per frame is a cost that grows with the
//! tree, and this one can be twenty thousand rows deep.
//!
//! **The finder.** `Cmd-P`, matching every editor. It ranks the worktree's
//! files with `crate::palette`'s scorer — the same subsequence match with the
//! same bonuses, because a person who has learned how the command palette
//! responds to their typing should not have to learn a second set of rules for
//! files.
//!
//! The panel's own search box is the same index put to a different use: it
//! narrows the tree in place rather than opening a list beside it. A file five
//! directories down is drawn where it lives, under the directories above it,
//! with those opened for you — so what the box shows is still a tree, and
//! still says where each answer came from. The two differ in what they match:
//! the finder scores whole paths, because `uisrc/pal` is how people describe
//! *where* a file is, while the box scores file names, because narrowing a
//! tree by name is what a box sitting on top of one means. Scoring decides
//! which files survive [`MAX_MATCHES`]; the tree decides the order they are
//! drawn in.
//!
//! The index behind all of it is built on demand and cached, not built when a
//! worktree is selected: walking a large repository is worth a moment after
//! `Cmd-P`, and is not worth a stutter on every click in the sidebar.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, ClipboardItem, Context, CursorStyle, DragMoveEvent, KeyDownEvent, MouseButton,
    MouseDownEvent, Pixels, Point, Render, ScrollStrategy, SharedString, UniformListScrollHandle,
    Window, div, prelude::*, px, uniform_list,
};
use ket_core::files::{Entry, EntryKind, FileIndex};
use ket_core::id::WorktreeId;
use ket_core::status::ChangeKind;
use ket_core::theme::Theme;

use crate::Shell;
use crate::input::{Style, is_text, text_field, text_line};
use crate::paint::paint;
use crate::palette::score;
use crate::ui::chip::caption;
use crate::ui::filetype::{folder_mark, tree_mark};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu};
use crate::ui::picker::{hints, list_height, lit, note, overlay, panel, query_line, row};
use crate::ui::toast::Tone;

/// Ranked files shown at once, in the panel and in the finder.
///
/// Past this nobody scrolls; the list is ranked, so what is cut is what
/// matched worst.
const MAX_MATCHES: usize = 50;

/// Indent per level of the tree.
const INDENT: Pixels = px(12.0);

/// Height of one tree row. Fixed because `uniform_list` requires it, and tall
/// enough to hold a file's mark without the rows closing up around it.
const ROW_HEIGHT: Pixels = px(25.0);

/// Narrowest panel that leaves file names readable.
const MIN_PANEL_SIZE: Pixels = px(200.0);

/// Widest panel worth keeping before it is the content that suffers.
const MAX_PANEL_SIZE: Pixels = px(600.0);

/// A path's right-click menu's width: the worktree menu's, so the context
/// menus agree.
const MENU_WIDTH: Pixels = px(220.0);

/// How the path menu names itself.
const MENU_ORIGIN: &str = "path-context";

/// What picking a row in a file's or folder's right-click menu does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathAction {
    /// Open the file itself in an editor tab, rather than its diff. A changed
    /// file only.
    OpenFile,
    /// Put the absolute path on the clipboard.
    CopyPath,
    /// Put the path from the worktree's root on the clipboard — what goes in a
    /// commit message or a review, where this machine's home directory is
    /// noise.
    CopyRelativePath,
    /// Show it in the platform's file manager.
    OpenInFileManager,
    /// Throw away its uncommitted changes, after asking. A changed file only;
    /// see `crate::discard_changes`.
    DiscardChanges,
}

/// A row of the Git view's change list, for the rows only a changed file's
/// menu offers.
pub(crate) struct PathChange {
    /// The worktree it changed in.
    pub(crate) root: PathBuf,
    /// How it changed.
    pub(crate) kind: ChangeKind,
    /// Where a renamed file came from, slash-separated from the root.
    pub(crate) old_path: Option<SharedString>,
    /// Whether the list compares against `HEAD`, which is the only time a
    /// row is work that Discard can throw away.
    pub(crate) uncommitted: bool,
}

/// A file's or folder's right-click menu, and the path it was opened on.
///
/// The path is carried rather than the row's index, for the reason
/// `WorktreeTarget` carries its worktree: the rows are rebuilt on every
/// refresh, and an index taken when the menu opened can name a different row
/// by the time one is picked.
pub(crate) struct PathMenu {
    menu: OpenMenu<PathAction>,
    /// Absolute.
    path: PathBuf,
    /// Slash-separated from the worktree root.
    rela: SharedString,
    /// Set when the menu is on a row of the Git view.
    change: Option<PathChange>,
}

/// The panel's edge, matching the sidebar's: a gutter of desk.
const PANEL_DIVIDER_SIZE: Pixels = crate::header::GUTTER;

/// The drag that resizes the right panel. See `SidebarResize` in `tree.rs`.
#[derive(Clone, Copy)]
pub(crate) struct PanelResize;

impl Render for PanelResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn clamped_panel_width(wanted: Pixels, extent: Pixels) -> Option<Pixels> {
    let available = extent - crate::tree::MIN_CONTENT_SIZE - PANEL_DIVIDER_SIZE;
    let maximum = if available < MAX_PANEL_SIZE {
        available
    } else {
        MAX_PANEL_SIZE
    };
    if maximum < MIN_PANEL_SIZE {
        return None;
    }
    Some(if wanted < MIN_PANEL_SIZE {
        MIN_PANEL_SIZE
    } else if wanted > maximum {
        maximum
    } else {
        wanted
    })
}

/// One visible row of the tree.
pub(crate) struct Row {
    /// How deep, for the indent.
    pub(crate) depth: usize,
    /// The entry itself.
    pub(crate) entry: Entry,
    /// Whether this directory is open. Meaningless for files.
    pub(crate) expanded: bool,
}

/// One file the query matched.
pub(crate) struct FileMatch {
    /// Slash-separated path from the worktree root.
    pub(crate) rela: SharedString,
    /// Absolute path, for opening it.
    pub(crate) path: PathBuf,
    /// Character indices in `rela` that the query matched, for lighting them.
    pub(crate) hits: Vec<usize>,
}

/// The file tree's state, for one worktree at a time.
#[derive(Default)]
pub(crate) struct Explorer {
    /// The worktree the tree is showing, so a changed selection is detectable.
    pub(crate) worktree: Option<WorktreeId>,
    /// Its root on disk.
    pub(crate) root: Option<PathBuf>,
    /// Directories the user has opened, by slash-separated path.
    pub(crate) expanded: HashSet<String>,
    /// What each read directory contained, so collapsing is not destructive.
    pub(crate) listings: HashMap<String, Vec<Entry>>,
    /// The flattened visible rows.
    pub(crate) rows: Vec<Row>,
    /// The rows a query narrowed the tree to.
    pub(crate) filtered: Vec<Row>,
    /// Whether a query is in force.
    ///
    /// Asked rather than inferred from `filtered` being empty: a query nothing
    /// matched is "no files match", and showing the whole tree again would
    /// look like the box had been ignored.
    pub(crate) filtering: bool,
    /// How many files the query matched before [`MAX_MATCHES`] cut it down.
    pub(crate) matched: usize,
    /// Every file under the worktree, built on first use. See the module doc.
    pub(crate) index: Option<FileIndex>,
    /// Why the tree is empty, when it is.
    pub(crate) error: Option<SharedString>,
    /// Whether `root` was chosen by a click rather than by the selection.
    ///
    /// Set when the reader opens a directory that is not a ket-managed
    /// worktree — the repository's own checkout, or one git knows about that
    /// ket did not create. Those have no [`WorktreeId`], so there is nothing
    /// for the selection to point at and nothing to sync against.
    pub(crate) pinned: bool,
}

impl Explorer {
    /// The rows on screen: the narrowed set while a query is in force, the
    /// whole tree otherwise.
    pub(crate) fn shown(&self) -> &[Row] {
        if self.filtering {
            &self.filtered
        } else {
            &self.rows
        }
    }

    /// How many of the narrowed rows are files, which is what the match count
    /// is comparable against — the rest are the directories drawn above them.
    fn filtered_files(&self) -> usize {
        self.filtered
            .iter()
            .filter(|row| row.entry.kind != EntryKind::Directory)
            .count()
    }
}

/// The `Cmd-P` overlay's state.
#[derive(Default)]
pub(crate) struct Finder {
    /// Whether it is showing.
    pub(crate) open: bool,
    /// Index into `matches`.
    pub(crate) selected: usize,
    /// The ranked files.
    pub(crate) matches: Vec<FileMatch>,
    /// Where the list is scrolled to, so the arrow keys can pull the selected
    /// row back into view.
    pub(crate) scroll: UniformListScrollHandle,
}

/// Ranks `paths` against `query`, best first.
///
/// Scored against the whole relative path rather than the file name, so
/// `uisrc/pal` finds `crates/ket-ui/src/palette.rs`: a path is how people
/// describe where a file is, and the scorer already rewards the characters
/// that land on segment boundaries.
fn rank(query: &str, root: &std::path::Path, paths: &[String]) -> Vec<FileMatch> {
    let mut scored: Vec<(i32, FileMatch)> = paths
        .iter()
        .filter_map(|rela| {
            let (points, hits) = score(query, rela)?;
            Some((
                points,
                FileMatch {
                    path: root.join(rela.replace('/', std::path::MAIN_SEPARATOR_STR)),
                    rela: rela.clone().into(),
                    hits,
                },
            ))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.rela.cmp(&b.1.rela)));
    scored.truncate(MAX_MATCHES);
    scored.into_iter().map(|(_, entry)| entry).collect()
}

/// Orders two relative paths the way the tree draws them: depth first, a
/// directory before a file beside it, then by name the way [`read_dir`] sorts
/// one directory — case-insensitively, with an exact comparison to break the
/// tie so two spellings of one name keep a stable order.
///
/// [`read_dir`]: ket_core::files::read_dir
fn tree_order(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mine: Vec<&str> = a.split('/').collect();
    let theirs: Vec<&str> = b.split('/').collect();

    for (depth, (left, right)) in mine.iter().zip(&theirs).enumerate() {
        // A segment with anything after it is a directory at this depth; the
        // last one is the file itself.
        let left_dir = depth + 1 < mine.len();
        let right_dir = depth + 1 < theirs.len();
        let ordered = left_dir
            .cmp(&right_dir)
            .reverse()
            .then_with(|| left.to_lowercase().cmp(&right.to_lowercase()))
            .then_with(|| left.cmp(right));
        if ordered != Ordering::Equal {
            return ordered;
        }
    }

    mine.len().cmp(&theirs.len())
}

/// The git mark at the end of a tree row: the letter `git status` would print
/// for a changed file, or a dot on a directory with changes somewhere under
/// it. An untracked directory is a change in its own right and takes the
/// letter rather than the dot.
fn change_mark(
    marks: &crate::tree::ChangeMarks,
    rela: &str,
    directory: bool,
    t: &Theme,
) -> Option<gpui::Div> {
    use ket_core::status::ChangeKind;
    let (text, colour) = match marks.file(rela) {
        Some(ChangeKind::Added) => ("A", t.diff.added),
        Some(ChangeKind::Untracked) => ("U", t.diff.added),
        Some(ChangeKind::Modified | ChangeKind::TypeChange) => ("M", t.diff.modified),
        Some(ChangeKind::Renamed) => ("R", t.diff.modified),
        Some(ChangeKind::Deleted) => ("D", t.diff.removed),
        Some(ChangeKind::Conflicted) => ("!", t.status.failed),
        None if directory && marks.dir(rela) => ("•", t.diff.modified),
        None => return None,
    };
    Some(
        div()
            .flex_none()
            .pl(px(6.0))
            .text_size(px(10.5))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(paint(colour))
            .child(text),
    )
}

/// The ink an ignored entry is drawn in.
///
/// `text.dim` thinned again rather than a token of its own, the same move a
/// placeholder makes in `ui::field`: ignored is a step *below* the quietest
/// ink the shell has, and there is no fourth level of text colour to reach
/// for. It has to stay legible — the point is that the file is there.
fn ignored_ink(t: &Theme) -> gpui::Rgba {
    let mut colour = paint(t.text.dim);
    colour.a = 0.45;
    colour
}

/// One row of the narrowed tree.
fn narrowed_row(root: &Path, rela: &str, kind: EntryKind, depth: usize) -> Row {
    Row {
        depth,
        entry: Entry {
            name: rela.rsplit('/').next().unwrap_or(rela).to_owned(),
            rela: rela.to_owned(),
            path: root.join(rela.replace('/', std::path::MAIN_SEPARATOR_STR)),
            kind,
            // Never true here: the index the narrowing reads holds no ignored
            // file, so nothing an ignored directory contains can reach this.
            ignored: false,
        },
        // Every directory the narrowing keeps is one it is showing through.
        expanded: kind == EntryKind::Directory,
    }
}

/// Narrows `paths` to the files whose *name* matches `query`, as tree rows
/// with the directories above each one kept for context.
///
/// Returns the rows and how many files matched, which is not the number of
/// rows: [`MAX_MATCHES`] cuts the worst-scoring files, and the directories
/// that survive are drawn on top of what is left.
///
/// The two passes are doing different jobs and cannot be collapsed into one.
/// The first ranks, because the cap has to fall on the files that matched
/// worst rather than on whatever the walk reached last. The second re-sorts
/// what survived into tree order, because a tree that listed its rows by score
/// would put a file above the directory it is inside.
fn narrow(query: &str, root: &Path, paths: &[String]) -> (Vec<Row>, usize) {
    let mut scored: Vec<(i32, &str)> = paths
        .iter()
        .filter_map(|rela| {
            let name = rela.rsplit('/').next().unwrap_or(rela);
            let (points, _) = score(query, name)?;
            Some((points, rela.as_str()))
        })
        .collect();
    let matched = scored.len();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored.truncate(MAX_MATCHES);

    let mut kept: Vec<&str> = scored.into_iter().map(|(_, rela)| rela).collect();
    kept.sort_by(|a, b| tree_order(a, b));

    let mut rows = Vec::new();
    // The directories already emitted, outermost first — which is exactly the
    // path down to the file being drawn, so it doubles as the walk's state.
    let mut open: Vec<&str> = Vec::new();

    for rela in kept {
        let mut segments: Vec<&str> = rela.split('/').collect();
        // Never empty: `split` yields at least one segment.
        segments.pop();

        let shared = open
            .iter()
            .zip(&segments)
            .take_while(|(open, wanted)| open == wanted)
            .count();
        open.truncate(shared);
        for segment in &segments[shared..] {
            open.push(segment);
            rows.push(narrowed_row(
                root,
                &open.join("/"),
                EntryKind::Directory,
                open.len() - 1,
            ));
        }

        rows.push(narrowed_row(root, rela, EntryKind::File, segments.len()));
    }

    (rows, matched)
}

impl Shell {
    /// Points the tree at the selected worktree, reading its top level.
    ///
    /// Cheap enough to call from a render: one directory, and only when the
    /// selection has actually moved.
    pub(crate) fn sync_explorer(&mut self, cx: &mut App) {
        // A directory the reader pointed the panel at themselves outranks the
        // selection. The repository's own checkout is not a ket worktree and
        // has no id to select, so following the selection here would snap the
        // panel back off it on the very next frame.
        if self.explorer.pinned {
            return;
        }

        let selected = self.selected_id();
        if selected == self.explorer.worktree {
            return;
        }

        // What the tree going away had open, filed under the worktree it was
        // of. Without it, leaving a worktree and coming back is coming back to
        // a collapsed root — and the same memory is what `crate::view` stores,
        // so it survives the run as well as the switch.
        self.remember_tree();

        let expanded = selected
            .as_ref()
            .and_then(|id| self.explorer_memory.get(id).cloned())
            .unwrap_or_default();

        self.explorer = Explorer {
            worktree: selected.clone(),
            expanded,
            ..Default::default()
        };
        // The box is the shell's, not the panel's, so pointing the panel
        // somewhere else has to empty it by hand — a query left over from the
        // last worktree would show that worktree's files ranked against the
        // new one's root.
        self.searches.explorer.update(cx, |input, _| input.clear());
        let Some(id) = selected else { return };
        let Some(root) = self
            .selected_path()
            .or_else(|| crate::terminal::worktree_path(&id))
        else {
            self.explorer.error = Some("that worktree is gone".into());
            return;
        };
        self.explorer.root = Some(root);
        self.read_into_tree("");
        self.read_expanded();
        self.flatten_tree();
    }

    /// Files away what the tree on screen has open, under the worktree it is
    /// of. A tree with nothing open is remembered as nothing rather than as an
    /// empty entry, so the stored shape stays the size of what has been done.
    fn remember_tree(&mut self) {
        let Some(id) = self.explorer.worktree.clone() else {
            return;
        };
        match self.explorer.expanded.is_empty() {
            true => self.explorer_memory.remove(&id),
            false => self
                .explorer_memory
                .insert(id, self.explorer.expanded.clone()),
        };
    }

    /// Reads the directories a restored tree is already showing as open.
    ///
    /// [`Self::read_into_tree`] reads one directory, and a tree that came back
    /// with five of them open needs all five or its chevrons are open over
    /// nothing. One call rather than five: reading a directory opens the
    /// repository and reads its index, which is the cost
    /// [`Self::refresh_file_tree`] exists to pay once.
    fn read_expanded(&mut self) {
        let Some(root) = self.explorer.root.clone() else {
            return;
        };
        let wanted: Vec<String> = self
            .explorer
            .expanded
            .iter()
            .filter(|rela| !self.explorer.listings.contains_key(*rela))
            .cloned()
            .collect();
        if wanted.is_empty() {
            return;
        }

        for (rela, listing) in ket_core::files::read_dirs(&root, &wanted) {
            match listing {
                Ok(entries) => {
                    self.explorer.listings.insert(rela, entries);
                }
                // A directory that has been deleted since is not open any
                // more either, which is what `refresh_file_tree` says about
                // the same case.
                Err(_) => {
                    self.explorer.expanded.remove(&rela);
                }
            }
        }
    }

    /// Points the panel at a directory the reader picked.
    ///
    /// The fallback for a discovered worktree ket cannot adopt — a detached
    /// checkout has no branch to register, and so nothing ket can run a session
    /// against. Its files are still there to read, which is better than a row
    /// that refuses and then does nothing.
    pub(crate) fn open_files_at(&mut self, root: PathBuf, cx: &mut App) {
        self.remember_tree();
        self.explorer = Explorer {
            root: Some(root),
            pinned: true,
            ..Default::default()
        };
        self.searches.explorer.update(cx, |input, _| input.clear());
        self.read_into_tree("");
        self.flatten_tree();
    }

    /// Reads one directory into the cache, recording why if it will not.
    fn read_into_tree(&mut self, rela: &str) {
        let Some(root) = self.explorer.root.clone() else {
            return;
        };
        match ket_core::files::read_dir(&root, rela) {
            Ok(entries) => {
                self.explorer.listings.insert(rela.to_owned(), entries);
                self.explorer.error = None;
            }
            // One unreadable directory is a row that will not open, not an
            // empty sidebar: the rest of the tree is still true.
            Err(e) => self.explorer.error = Some(e.to_string().into()),
        }
    }

    /// Re-reads the directories the tree is showing.
    ///
    /// [`Self::read_into_tree`] caches a listing the first time a directory is
    /// opened and nothing invalidated it, so a file written by an agent, an
    /// external editor, or a `git checkout` never appeared until the panel was
    /// pointed somewhere else and back. This is what keeps it current.
    ///
    /// On the shell's existing tick rather than a filesystem watcher. A watcher
    /// is the better answer and is an epic of its own — `notify` over a whole
    /// worktree has its own costs, and `ket_core::surface::FileWatcher` already
    /// exists for the editor's purposes.
    ///
    /// Three things keep the cost of polling honest:
    ///
    /// - **Nothing runs while the panel is hidden.** There is no tree on screen
    ///   to be stale, and the sweep resumes — immediately, not on the next tick
    ///   — when the panel comes back. See [`Self::show_files_panel`].
    /// - **Only the root and what is expanded.** A collapsed directory's cached
    ///   listing is not on screen, and re-reading it would be paying for rows
    ///   nobody can see. It is re-read when it is opened again.
    /// - **One set of ignore rules for the sweep.** This is the part that
    ///   actually costs: [`ket_core::files::read_dir`] opens the repository and
    ///   reads its index every call, which scales with the size of the
    ///   repository rather than the directory, so per-directory it dominated
    ///   everything else here. [`ket_core::files::read_dirs`] pays it once.
    pub(crate) fn refresh_file_tree(&mut self) {
        if !self.panel_open {
            return;
        }
        let Some(root) = self.explorer.root.clone() else {
            return;
        };

        // Never read for the first time here. A directory that is expanded but
        // unread is one `toggle_directory` is about to read anyway, and reading
        // it on a tick would race that.
        let mut wanted: Vec<String> = Vec::new();
        for rela in std::iter::once(String::new()).chain(self.explorer.expanded.iter().cloned()) {
            if self.explorer.listings.contains_key(&rela) {
                wanted.push(rela);
            }
        }
        if wanted.is_empty() {
            return;
        }

        let mut changed = false;
        for (rela, listing) in ket_core::files::read_dirs(&root, &wanted) {
            match listing {
                Ok(entries) => {
                    if self.explorer.listings.get(&rela) != Some(&entries) {
                        self.explorer.listings.insert(rela, entries);
                        changed = true;
                    }
                }
                // Gone since it was read. Dropped rather than reported: the
                // panel's error says why the tree is *empty*, and one
                // subdirectory that has been deleted is not that — the rest of
                // the tree is still true, which is what `read_into_tree` says
                // about the same case.
                Err(_) => {
                    self.explorer.listings.remove(&rela);
                    self.explorer.expanded.remove(&rela);
                    changed = true;
                }
            }
        }

        if changed {
            self.flatten_tree();
            // A narrowed tree is built from the index, not the listings, so it
            // is left alone here — see `filter_tree`.
        }
    }

    /// Shows the file panel, with a tree that is current rather than whatever
    /// was on screen when it was last hidden.
    ///
    /// The sweep stops while the panel is hidden, so the listings are as old as
    /// the moment it went away. Reading them here rather than waiting for the
    /// next tick is the difference between a panel that opens correct and one
    /// that opens wrong and fixes itself a few seconds later, in front of you.
    pub(crate) fn show_files_panel(&mut self) {
        self.panel_open = true;
        self.refresh_file_tree();
        self.persist_view();
    }

    /// Rebuilds the visible rows from what has been read and what is expanded.
    fn flatten_tree(&mut self) {
        self.explorer.rows = flatten(&self.explorer.listings, &self.explorer.expanded);
    }

    /// Opens or closes a directory, reading it the first time.
    pub(crate) fn toggle_directory(&mut self, rela: &str) {
        // Both arms end in a save. The tick would get there within five
        // seconds, and a directory opened and then quit on is exactly the
        // change somebody would notice missing.
        if self.explorer.expanded.remove(rela) {
            self.flatten_tree();
            self.persist_view();
            return;
        }

        self.explorer.expanded.insert(rela.to_owned());
        if !self.explorer.listings.contains_key(rela) {
            self.read_into_tree(rela);
        }
        self.flatten_tree();
        self.persist_view();
    }

    /// The worktree's file list, built on first use and kept.
    fn file_index(&mut self) -> Option<&FileIndex> {
        if self.explorer.index.is_none() {
            let root = self.explorer.root.clone()?;
            // A walk that fails leaves an empty index rather than retrying on
            // every keystroke.
            self.explorer.index = Some(ket_core::files::index(&root).unwrap_or_default());
        }
        self.explorer.index.as_ref()
    }

    /// Re-narrows the tree to what the panel's search box matches.
    fn filter_tree(&mut self, cx: &App) {
        if self.searches.explorer.read(cx).is_empty() {
            self.clear_filter();
            return;
        }
        let query = self.searches.explorer.read(cx).text();
        let Some(root) = self.explorer.root.clone() else {
            return;
        };
        let Some(index) = self.file_index() else {
            return;
        };
        let (rows, matched) = narrow(&query, &root, index.paths());
        self.explorer.filtered = rows;
        self.explorer.matched = matched;
        self.explorer.filtering = true;
    }

    /// Puts the whole tree back.
    fn clear_filter(&mut self) {
        self.explorer.filtered.clear();
        self.explorer.matched = 0;
        self.explorer.filtering = false;
    }

    /// Opens the finder, empty and offering everything.
    pub(crate) fn open_finder(&mut self, cx: &mut Context<Self>) {
        self.sync_explorer(cx);
        // Built once and kept, so a file written since the last time the finder
        // was opened was not in it — the same staleness the tree had, and worse
        // here because the finder is what you reach for precisely when you know
        // a file exists and cannot remember where. Dropped rather than swept on
        // the tick: this is a walk of the whole worktree, which is far too much
        // to pay on every tick, and opening the finder is both the moment
        // it matters and a moment somebody is already waiting.
        self.explorer.index = None;
        self.popup = None;
        self.finder.open = true;
        self.finder.selected = 0;
        // Asked for here and granted on the first frame — see
        // `TextInput::request_focus`. The overlay has to hold the keyboard
        // itself, or the window's handle keeps it and a terminal pane behind
        // the finder takes every character typed into it.
        self.searches.finder.update(cx, |input, cx| {
            input.clear();
            input.request_focus();
            cx.notify();
        });
        self.rank_finder(cx);
    }

    /// Closes it without opening anything.
    pub(crate) fn close_finder(&mut self, cx: &mut Context<Self>) {
        self.finder.open = false;
        self.finder.matches.clear();
        self.finder.selected = 0;
        self.searches.finder.update(cx, |input, _| input.clear());
    }

    /// Re-ranks the finder against its query.
    pub(crate) fn rank_finder(&mut self, cx: &App) {
        let query = self.searches.finder.read(cx).text();
        let Some(root) = self.explorer.root.clone() else {
            self.finder.matches.clear();
            return;
        };
        let Some(index) = self.file_index() else {
            return;
        };
        self.finder.matches = rank(&query, &root, index.paths());
        self.finder.selected = self
            .finder
            .selected
            .min(self.finder.matches.len().saturating_sub(1));
    }

    /// Opens the highlighted file and closes the finder.
    fn open_selected_match(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.finder.matches.get(self.finder.selected) else {
            return;
        };
        let path = found.path.clone();
        self.close_finder(cx);
        self.open_editor_tab(path, cx);
    }

    /// Handles a key while the finder is open. Returns whether it consumed one.
    ///
    /// Mirrors `Shell::palette_key`, including reporting every printable key
    /// as consumed: an overlay that let some characters through to the shell
    /// underneath would run commands while you were typing a filename.
    pub(crate) fn finder_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if !self.finder.open {
            return false;
        }

        // The query line first, then what it declines — the same order the
        // palette uses, and for the same reason: text is never taken, because
        // it has to reach macOS's input context to become a character at all.
        let query = self.searches.finder.clone();
        if is_text(&event.keystroke) {
            return true;
        }
        if query.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => self.close_finder(cx),
            "enter" => self.open_selected_match(cx),
            // `Top` here is "the least scrolling that brings it into view":
            // the handle does nothing at all when the row is already showing.
            "down" => {
                let last = self.finder.matches.len().saturating_sub(1);
                self.finder.selected = (self.finder.selected + 1).min(last);
                self.finder
                    .scroll
                    .scroll_to_item(self.finder.selected, ScrollStrategy::Top);
            }
            "up" => {
                self.finder.selected = self.finder.selected.saturating_sub(1);
                self.finder
                    .scroll
                    .scroll_to_item(self.finder.selected, ScrollStrategy::Top);
            }
            _ => {}
        }

        true
    }

    /// The finder overlay, or nothing when it is closed.
    pub(crate) fn finder_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.finder.open {
            return None;
        }

        let t = &self.theme;
        let truncated = self
            .explorer
            .index
            .as_ref()
            .is_some_and(FileIndex::truncated);

        let query = query_line(
            text_line(
                &self.searches.finder,
                "finder-query",
                Style::new(t, self.caret.visible),
                window,
                cx,
            ),
            t,
        );

        // Virtualised like the tree: the rows are capped at
        // [`MAX_MATCHES`], but each one is a file mark plus a lit name, and
        // building fifty of those every frame is what made the list stutter as
        // it scrolled. Only what is on screen is built now.
        let count = self.finder.matches.len();
        let list = uniform_list(
            "finder-rows",
            count,
            cx.processor(|this, range: Range<usize>, _, cx| this.finder_rows(range, cx)),
        )
        .track_scroll(self.finder.scroll.clone())
        .h(list_height(count));

        Some(
            overlay(
                panel(t)
                    .child(query)
                    .child(div().flex_none().p(px(6.0)).child(list))
                    // Said rather than swallowed: a finder that cannot see
                    // the whole repository should admit it.
                    .when(truncated, |el| {
                        el.child(note("showing the first 20,000 files", t))
                    })
                    .child(hints(
                        &[
                            ("Enter", "Open"),
                            ("Esc", "Close"),
                            ("\u{2191}\u{2193}", "Move"),
                        ],
                        self.font_family.clone(),
                        t,
                    )),
            )
            // The wash is part of the picker now, so a click on it is a click
            // outside — which dismisses, the way clicking off any overlay does.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_finder(cx);
                    cx.notify();
                }),
            )
            .into_any_element(),
        )
    }

    /// The result rows `uniform_list` asked for, and only those.
    fn finder_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = self.theme;
        let mono = self.font_family.clone();
        let selected = self.finder.selected;

        range
            .filter_map(|index| {
                let found = self.finder.matches.get(index)?;
                let path = found.path.clone();
                let rela = found.rela.clone();

                // The name leads and the directory follows it, quietly. A row
                // that opens with `crates/ket-ui/src/` puts the answer at the
                // end of a line whose beginning is the same as the row above
                // it — the file name is what was being looked for, and the
                // directory is only how two files sharing one are told apart.
                let cut = rela.rfind('/').map(|at| at + 1).unwrap_or(0);
                let parent: SharedString = rela[..cut].to_owned().into();
                let name = rela[cut..].to_owned();
                // `hits` indexes characters of the whole path, so the name's
                // own lighting is measured from where the name starts. A hit
                // that landed in the directory simply drops out.
                let offset = rela[..cut].chars().count();
                let hits: Vec<usize> = found
                    .hits
                    .iter()
                    .filter_map(|hit| hit.checked_sub(offset))
                    .collect();

                Some(
                    row(("finder-row", index), index == selected, &t)
                        .child(tree_mark(&name, false, mono.clone(), &t))
                        .child(lit(&name, &hits, &t))
                        .when(!parent.is_empty(), |el| {
                            el.child(caption(parent.clone(), &t))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_finder(cx);
                            this.open_editor_tab(path.clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The right panel: the explorer today, more later.
    ///
    /// The width lives here rather than on the explorer so that whatever
    /// joins it shares one edge and one drag.
    pub(crate) fn right_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match self.panel_view {
            crate::panel::PanelView::Files => self.explorer_panel(window, cx),
            crate::panel::PanelView::Search => self.workspace_search_panel(window, cx),
            crate::panel::PanelView::Git => self.git_panel(cx),
        };

        crate::header::card(&self.theme)
            .id("right-panel")
            .flex()
            .flex_col()
            .flex_none()
            .w(self.panel_width)
            .h_full()
            .child(content)
            .into_any_element()
    }

    /// The panel's edge, on its left; the mirror of the sidebar's.
    pub(crate) fn panel_divider(&self) -> AnyElement {
        div()
            .id("panel-divider")
            .relative()
            .w(PANEL_DIVIDER_SIZE)
            .h_full()
            .flex_none()
            .child(
                div()
                    .id("panel-divider-grab")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(-crate::tree::DIVIDER_GRAB)
                    .right(-crate::tree::DIVIDER_GRAB)
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_drag(PanelResize, |drag, _, _, cx| cx.new(|_| *drag)),
            )
            .into_any_element()
    }

    /// Clamped on both sides, so a drag can hide neither the panel nor the
    /// content it sits beside.
    pub(crate) fn resize_panel(&mut self, event: &DragMoveEvent<PanelResize>) -> bool {
        let wanted = event.bounds.right() - event.event.position.x;
        let Some(width) = clamped_panel_width(wanted, event.bounds.size.width) else {
            return false;
        };
        if width == self.panel_width {
            return false;
        }
        self.panel_width = width;
        true
    }

    /// The file panel: the view strip, a search box, and the tree. The
    /// worktree's name is the header's to say — see `crate::header`.
    pub(crate) fn explorer_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.sync_explorer(cx);

        let t = self.theme;
        let header = self.panel_strip(cx);

        // A real field: clicking it takes the keyboard, and the caret, the
        // selection and paste all come with it. It used to be a picture of
        // one, filled in by the window's key handler, which is why it could
        // not be corrected in the middle or pasted into.
        let search = text_field(
            &self.searches.explorer,
            "explorer-search",
            false,
            Style::new(&t, self.caret.visible).leading(Icon::Search),
            window,
            cx,
        )
        .mx_2()
        .mb(px(6.0))
        // Fixed, and held fixed. Without this the box is a shrinkable item in
        // a column whose list asks for the full height, so it was squeezed
        // while the tree was showing and released the moment a query replaced
        // it — which read as the field jumping as you typed into it.
        .flex_none()
        .h(crate::ui::FIELD_H);

        self.filter_tree(cx);
        let empty = self.explorer.filtering && self.explorer.filtered.is_empty();
        let truncated = self.explorer.matched > self.explorer.filtered_files();
        let body = self.tree_list(cx);

        div()
            .flex()
            .flex_col()
            .flex_grow()
            .h_full()
            .overflow_hidden()
            // No fill: the card paints it, rounded — see `Shell::sidebar`.
            .child(header)
            .child(search)
            .children(self.explorer.error.clone().map(|message| {
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_size(px(10.0))
                    .text_color(paint(t.text.dim))
                    .child(message)
            }))
            .child(body)
            // Said rather than left to look like an empty worktree.
            .when(empty, |el| el.child(note("no file names match", &t)))
            // A narrowed tree that is not the whole answer should admit it,
            // the same way the finder does.
            .when(truncated, |el| {
                el.child(note(
                    format!(
                        "showing the best {MAX_MATCHES} of {} matches",
                        self.explorer.matched
                    ),
                    &t,
                ))
            })
            .into_any_element()
    }

    /// The tree itself, virtualised — narrowed when a query is in force, whole
    /// when it is not.
    fn tree_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        uniform_list(
            "explorer-tree",
            self.explorer.shown().len(),
            cx.processor(|this, range: Range<usize>, _, cx| this.tree_rows(range, cx)),
        )
        .flex_grow()
        .size_full()
        .into_any_element()
    }

    /// The rows `uniform_list` asked for, and only those.
    fn tree_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let _span = crate::frametrace::span("explorer:rows");
        let t = self.theme;
        let mono = self.font_family.clone();
        // A directory in the narrowed tree is context, not a control: it is
        // there to say where the file under it lives, and collapsing it would
        // hide the very match that put it on screen. So it takes no click, and
        // wears none of the marks of something that would answer one.
        let filtering = self.explorer.filtering;
        // What git says changed in the worktree the tree is showing, from the
        // sweep's last status read — see `crate::git_watch`.
        let marks = self
            .explorer
            .worktree
            .as_ref()
            .and_then(|id| self.worktree_node(id))
            .map(|node| node.marks.clone());

        range
            .filter_map(|index| {
                let row = self.explorer.shown().get(index)?;
                let directory = row.entry.kind == EntryKind::Directory;
                let inert = directory && filtering;
                // Shown, and shown to be ignored: it opens and expands like
                // anything else, it just says what git thinks of it.
                let ignored = row.entry.ignored;
                let rela = row.entry.rela.clone();
                let path = row.entry.path.clone();
                let name = row.entry.name.clone();
                let expanded = row.expanded;
                let symlink = row.entry.kind == EntryKind::Symlink;
                let indent = INDENT * (row.depth as f32 + 1.0);
                let change = marks
                    .as_ref()
                    .and_then(|marks| change_mark(marks, &row.entry.rela, directory, &t));

                Some(
                    div()
                        .id(("explorer-row", index))
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(ROW_HEIGHT)
                        // The row is the panel's full width, not the label's:
                        // a hover or a click target that stops where the name
                        // does leaves most of the row inert, and a short name
                        // in a wide panel is most of it.
                        .w_full()
                        .pl(indent)
                        .pr_2()
                        // No hairline under a tree row, unlike the sidebar's:
                        // a tree is read by its indents, and a rule under
                        // every one of a few hundred short rows is a grid.
                        // The same size as a sidebar row's branch name: a
                        // file name is a file name wherever it is listed, and
                        // the sidebar came down to this on request. Stated in
                        // pixels, not `text_xs`, which is a fraction of the
                        // window's rem size and so moved with
                        // `theme.ui_font_size` while the row height and the
                        // file marks beside it did not.
                        .text_size(px(11.0))
                        .text_color(if ignored {
                            ignored_ink(&t)
                        } else if directory {
                            paint(t.text.primary)
                        } else {
                            paint(t.text.dim)
                        })
                        .when(!inert, |el| {
                            el.cursor_pointer().hover(|style| style.bg(paint(t.hover)))
                        })
                        .child(div().flex().flex_none().w(px(12.0)).children(
                            (directory && !inert).then(|| {
                                sized_icon(
                                    if expanded {
                                        Icon::ChevronDown
                                    } else {
                                        Icon::ChevronRight
                                    },
                                    px(12.0),
                                    if ignored {
                                        ignored_ink(&t)
                                    } else {
                                        paint(t.text.dim)
                                    },
                                )
                            }),
                        ))
                        // The same mark the tab strip gives this file, so one
                        // file looks like itself wherever it is drawn.
                        .child(if directory {
                            folder_mark(
                                expanded,
                                if ignored {
                                    ignored_ink(&t)
                                } else {
                                    paint(t.text.dim)
                                },
                            )
                        } else {
                            tree_mark(&name, ignored, mono.clone(), &t)
                        })
                        // The name takes the row's slack, which is what puts
                        // the git mark at the far end. Not `ml_auto` on the
                        // mark: taffy drops the row's `gap` once any child has
                        // an auto margin, so a changed row lost the space
                        // between its chevron, folder and name.
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(div().min_w_0().truncate().child(name))
                                // A symlink says so rather than pretending to
                                // be the thing it points at, which the tree
                                // will not open.
                                .when(symlink, |el| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .text_color(paint(t.text.dim))
                                            .child(SharedString::from("↗")),
                                    )
                                }),
                        )
                        .children(change)
                        // Every folder, the narrowed tree's inert ones too:
                        // what makes those take no click is that collapsing
                        // one would hide its match, and copying its path hides
                        // nothing. `on_mouse_down`, for the reason the
                        // sidebar's rows give.
                        .when(directory, |el| {
                            let path = path.clone();
                            let rela = SharedString::from(rela.clone());
                            el.on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.open_path_menu(
                                        path.clone(),
                                        rela.clone(),
                                        None,
                                        event.position,
                                    );
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                        })
                        .when(!inert, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                if directory {
                                    this.toggle_directory(&rela);
                                } else {
                                    this.open_editor_tab(path.clone(), cx);
                                }
                                cx.notify();
                            }))
                        })
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// Handles a key while the panel's search box has the keyboard.
    ///
    /// Returns whether it consumed one. Escape leaves the box rather than
    /// closing the sidebar: every state is escapable, and the state being left
    /// is the innermost one.
    ///
    /// Whether the box is live is asked of the field rather than remembered.
    /// It used to be a `searching` flag beside the text, which had to be
    /// cleared by hand everywhere focus could move and was wrong the moment
    /// one of those places was missed.
    pub(crate) fn explorer_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let search = self.searches.explorer.clone();
        if !self.panel_open || !search.read(cx).is_focused(window) {
            return false;
        }

        if is_text(&event.keystroke) {
            return true;
        }
        if search.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                search.update(cx, |input, _| input.clear());
                self.clear_filter();
                // Blurred to nothing rather than to some fallback: the shell
                // takes the window's handle back on the next frame, which is
                // what puts its own shortcuts back in reach.
                window.blur();
            }
            // The first *file*, not the first row: the rows above it are the
            // directories it is drawn under, and none of them opens.
            "enter" => {
                let first = self
                    .explorer
                    .filtered
                    .iter()
                    .find(|row| row.entry.kind != EntryKind::Directory)
                    .map(|row| row.entry.path.clone());
                if let Some(path) = first {
                    self.open_editor_tab(path, cx);
                }
            }
            // Anything else falls through to the shell: the box is one
            // control in a panel, not a modal, so a chord typed in it still
            // reaches the window.
            _ => return false,
        }

        true
    }

    /// Opens the right-click menu for the file or folder at `path`, whose
    /// path from its worktree's root is `rela`. The file tree's folders and the
    /// Git view's changed files both open this one; `change` is set for the
    /// second, and adds Open File and Discard Changes.
    ///
    /// Open in Finder is dropped rather than disabled where there is no file
    /// manager to open it in: that is a fact about the platform, not about
    /// this path, and it will not change while the menu is up. What depends on
    /// the row — a deleted file has nothing to open or show, and Discard
    /// touches uncommitted work and not a conflict — is disabled rather than
    /// dropped, so a changed file's menu keeps one shape, the tab menu's rule.
    pub(crate) fn open_path_menu(
        &mut self,
        path: PathBuf,
        rela: SharedString,
        change: Option<PathChange>,
        at: Point<Pixels>,
    ) {
        // Every other menu closes: two menus open at once is two things
        // claiming the keyboard.
        self.menu = None;
        self.worktree_menu = None;
        self.project_menu = None;
        self.close_tab_menus();
        self.popup = None;

        let exists = path.exists();
        let enabled = |item: MenuItem<PathAction>, on: bool| {
            MenuEntry::Item(if on { item } else { item.disabled() })
        };

        let mut entries = Vec::new();
        if change.is_some() {
            entries.push(enabled(
                MenuItem::new(PathAction::OpenFile, "Open File").icon(Icon::File),
                exists,
            ));
        }
        if let Some(name) = ket_core::surface::FILE_MANAGER {
            entries.push(enabled(
                MenuItem::new(PathAction::OpenInFileManager, format!("Open in {name}"))
                    .icon(Icon::ExternalLink),
                exists,
            ));
        }
        if !entries.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        entries.extend([
            MenuEntry::Item(MenuItem::new(PathAction::CopyPath, "Copy Path").icon(Icon::Copy)),
            MenuEntry::Item(
                MenuItem::new(PathAction::CopyRelativePath, "Copy Relative Path").icon(Icon::Copy),
            ),
        ]);
        if let Some(change) = &change {
            entries.push(MenuEntry::Separator);
            entries.push(enabled(
                MenuItem::new(PathAction::DiscardChanges, "Discard Changes")
                    .icon(Icon::RotateCcw)
                    .danger(),
                change.uncommitted && change.kind != ChangeKind::Conflicted,
            ));
        }

        self.path_menu = Some(PathMenu {
            menu: OpenMenu::new(MENU_ORIGIN.into(), None, MENU_WIDTH, entries).at(at),
            path,
            rela,
            change,
        });
    }

    /// Runs one pick from a path's menu.
    fn run_path_action(&mut self, action: PathAction, cx: &mut Context<Self>) {
        let Some(open) = self.path_menu.take() else {
            return;
        };

        match action {
            PathAction::OpenFile => self.open_editor_tab(open.path, cx),
            PathAction::CopyPath => {
                let path = open.path.display().to_string();
                cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                self.toast_detail(Tone::Info, "Copied path", path, cx);
            }
            PathAction::CopyRelativePath => {
                cx.write_to_clipboard(ClipboardItem::new_string(open.rela.to_string()));
                self.toast_detail(Tone::Info, "Copied relative path", open.rela, cx);
            }
            PathAction::OpenInFileManager => {
                if let Err(error) = ket_core::surface::reveal_in_file_manager(&open.path) {
                    let name = ket_core::surface::FILE_MANAGER.unwrap_or("the file manager");
                    self.toast_detail(
                        Tone::Error,
                        format!("Could not open in {name}"),
                        error.to_string(),
                        cx,
                    );
                }
            }
            PathAction::DiscardChanges => {
                if let Some(change) = open.change {
                    self.request_discard(change.root, open.rela, change.old_path, change.kind);
                }
            }
        }
    }

    /// Handles a key while a path's menu is open.
    pub(crate) fn path_menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.path_menu.as_mut() else {
            return false;
        };
        match open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.path_menu = None,
            MenuKey::Run(action) => self.run_path_action(action, cx),
        }
        true
    }

    /// A path's menu, or nothing when none is open.
    pub(crate) fn path_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.path_menu.as_ref()?;
        Some(open.menu.view(
            &self.theme,
            cx,
            |shell, action, cx| shell.run_path_action(*action, cx),
            |shell, cx| {
                shell.path_menu = None;
                cx.notify();
            },
        ))
    }
}

/// Flattens the cached listings into the rows the tree draws.
///
/// A free function rather than a method so it can be tested without a window:
/// it is the part of the explorer with an answer that can be wrong, and the
/// part around it is all gpui.
fn flatten(listings: &HashMap<String, Vec<Entry>>, expanded: &HashSet<String>) -> Vec<Row> {
    let mut rows = Vec::new();
    // Depth-first, and iterative rather than recursive: a deep tree must not
    // be able to take the shell down with a stack overflow.
    //
    // Each entry is pushed with the depth it draws at, and a directory's
    // children go on immediately after its own row is emitted — which is what
    // puts them directly beneath it. Groups go on in reverse so the stack pops
    // them in the order they sorted.
    let Some(root) = listings.get("") else {
        return rows;
    };
    let mut stack: Vec<(&Entry, usize)> = root.iter().rev().map(|entry| (entry, 0)).collect();

    while let Some((entry, depth)) = stack.pop() {
        let open = expanded.contains(&entry.rela);
        rows.push(Row {
            depth,
            entry: entry.clone(),
            expanded: open,
        });

        if entry.kind != EntryKind::Directory || !open {
            continue;
        }
        // Expanded but never read — the state between the click and the
        // listing, and the one it stays in when the directory will not open.
        if let Some(children) = listings.get(&entry.rela) {
            stack.extend(children.iter().rev().map(|child| (child, depth + 1)));
        }
    }

    rows
}

/// Renders a path with its matched characters lit.
///
/// One element per character, which is affordable for the same reason it is in
/// `crate::palette`: the list is capped at [`MAX_MATCHES`] and a path is one
/// short line.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_resizing_keeps_both_the_panel_and_content_reachable() {
        assert_eq!(
            clamped_panel_width(px(10.0), px(1200.0)),
            Some(MIN_PANEL_SIZE)
        );
        // Past the widest the window allows: all of it but the content's
        // least and the divider.
        let widest = px(700.0) - crate::tree::MIN_CONTENT_SIZE - PANEL_DIVIDER_SIZE;
        assert_eq!(clamped_panel_width(px(900.0), px(700.0)), Some(widest));
        assert_eq!(clamped_panel_width(px(200.0), px(400.0)), None);
    }

    fn entry(rela: &str, kind: EntryKind) -> Entry {
        Entry {
            name: rela.rsplit('/').next().unwrap_or(rela).to_owned(),
            rela: rela.to_owned(),
            path: PathBuf::from("/w").join(rela),
            kind,
            ignored: false,
        }
    }

    /// A worktree with `src/` holding one file, and a file at the root.
    fn listings() -> HashMap<String, Vec<Entry>> {
        HashMap::from([
            (
                String::new(),
                vec![
                    entry("src", EntryKind::Directory),
                    entry("README.md", EntryKind::File),
                ],
            ),
            (
                "src".to_owned(),
                vec![entry("src/main.rs", EntryKind::File)],
            ),
        ])
    }

    fn shown(rows: &[Row]) -> Vec<(usize, &str)> {
        rows.iter()
            .map(|row| (row.depth, row.entry.rela.as_str()))
            .collect()
    }

    #[test]
    fn a_collapsed_directory_hides_what_is_inside_it() {
        // The listing is cached either way — collapsing must not throw away
        // the read, only stop drawing it.
        let rows = flatten(&listings(), &HashSet::new());
        assert_eq!(shown(&rows), [(0, "src"), (0, "README.md")]);
    }

    #[test]
    fn children_are_drawn_under_their_parent_and_one_level_in() {
        let expanded = HashSet::from(["src".to_owned()]);
        let rows = flatten(&listings(), &expanded);
        assert_eq!(
            shown(&rows),
            [(0, "src"), (1, "src/main.rs"), (0, "README.md")]
        );
    }

    #[test]
    fn a_directory_expanded_before_it_was_read_draws_only_itself() {
        // The state the tree is in for the instant between the click and the
        // listing, and the state it stays in when the directory will not open.
        let mut listings = listings();
        listings.remove("src");
        let expanded = HashSet::from(["src".to_owned()]);

        let rows = flatten(&listings, &expanded);
        assert_eq!(shown(&rows), [(0, "src"), (0, "README.md")]);
        assert!(rows[0].expanded, "the chevron should still read as open");
    }

    #[test]
    fn the_finder_ranks_a_path_you_half_remember_above_one_you_did_not_mean() {
        let root = std::path::Path::new("/w");
        let paths = [
            "crates/ket-ui/src/palette.rs".to_owned(),
            "crates/ket-core/src/paths.rs".to_owned(),
            "docs/particulars.md".to_owned(),
        ];

        let ranked = rank("uipal", root, &paths);
        assert_eq!(ranked[0].rela, "crates/ket-ui/src/palette.rs");

        // And the match positions come back, because the list lights them.
        assert!(!ranked[0].hits.is_empty());
    }

    #[test]
    fn a_query_nothing_contains_matches_nothing() {
        let paths = ["src/main.rs".to_owned()];
        assert!(rank("zzz", std::path::Path::new("/w"), &paths).is_empty());
    }

    #[test]
    fn an_empty_query_offers_every_file() {
        // What `Cmd-P` shows before anything is typed.
        let paths = ["a.rs".to_owned(), "b.rs".to_owned()];
        assert_eq!(rank("", std::path::Path::new("/w"), &paths).len(), 2);
    }

    #[test]
    fn a_matched_path_carries_the_absolute_path_the_editor_needs() {
        let paths = ["src/main.rs".to_owned()];
        let ranked = rank("main", std::path::Path::new("/w"), &paths);
        assert_eq!(ranked[0].path, PathBuf::from("/w/src/main.rs"));
    }

    #[test]
    fn a_pinned_panel_is_not_following_any_worktree() {
        // What clicking the repository's own checkout leaves behind: a root
        // with no worktree to sync against. `sync_explorer` reads exactly
        // these two fields to decide whether to leave the panel alone.
        let explorer = Explorer {
            root: Some(PathBuf::from("/repo")),
            pinned: true,
            ..Default::default()
        };

        assert!(explorer.pinned);
        assert!(explorer.worktree.is_none());
    }

    #[test]
    fn a_fresh_panel_is_not_pinned() {
        // So a panel that has never been pointed anywhere still follows the
        // selection, which is the ordinary case.
        assert!(!Explorer::default().pinned);
    }
}
