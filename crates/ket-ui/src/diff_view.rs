//! A file's diff, as a tab in the editor area.
//!
//! Opened from the Git view's rows. Inline rather than side by side: the
//! panes this lands in are as often half a window as a whole one, and two
//! columns of code at half a window is two columns nobody can read. Each hunk
//! is shown with [`ket_core::diff::CONTEXT_LINES`] of context, under a header
//! saying where it sits, with the old and new line numbers in two gutters and
//! the code coloured by the same highlighter the editor uses.
//!
//! **Read-only, and read again.** The tab holds the comparison it was opened
//! for — the file, and either `HEAD` or the branch's base — not a copy of the
//! lines. It reads the file's diff off the window's thread when it is first
//! drawn and again on the shell's status tick while it is the tab in front,
//! so an agent still writing the file is followed rather than frozen at the
//! moment of the click. It is not saved with the layout: it is a view of a
//! moment, and reopening one from the Git view is a click.
//!
//! **Moving through it.** F7 and Shift-F7 jump to the next and previous
//! change, as they do in the editors this borrows from; the two arrows in the
//! header do the same for a pointer.
//!
//! **Commenting.** Hovering a line shows a "+" in its gutter. A click in the
//! gutter, a drag down it or a ⇧-click picks lines — past the list's edge,
//! the list scrolls to follow — and a click on a hunk's header picks the
//! hunk; letting go opens the composer under the last line picked, which
//! moves the lines after it down (see `crate::review`). Cancelling lets go
//! of the lines. A comment stays as a card with Edit and Delete until the
//! review is sent, and the lines it is about wear a dot at the gutter's
//! edge. Rows are a list of their own heights for that reason.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, ClipboardItem, Context, DispatchPhase, ElementId, FontStyle, FontWeight,
    HighlightStyle, KeyDownEvent, ListAlignment, ListOffset, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, StyledText, Window, canvas,
    div, list, prelude::*, px, relative,
};
use ket_core::diff::{FileDiff, LineKind};
use ket_core::status::ChangeKind;
use ket_core::syntax::{self, Token};
use ket_core::theme::Theme;

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::tabs::{Tab, TabKind};
use crate::ui::RADIUS_SM;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{Motion, status_dot};
use crate::ui::filetype::file_mark;
use crate::ui::icon::Icon;
use crate::ui::tooltip::{Side, tooltip};

/// Columns a tab is drawn as. The highlighter and the row both count
/// characters, and a literal tab would be one character drawn as none.
const TAB_SPACES: &str = "    ";

/// Width of each line-number gutter.
const NUMBER_WIDTH: gpui::Pixels = px(44.0);

/// Width of the `+`/`-` column.
const SIGN_WIDTH: gpui::Pixels = px(16.0);

/// Width of the gutter: both line numbers and the sign, the part of a row
/// that answers the pointer.
const GUTTER_WIDTH: gpui::Pixels = px(104.0);

/// Which comparison a diff tab shows. Its identity: one tab per file per
/// comparison, and opening the same one again focuses it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffKey {
    /// The worktree's root, absolute.
    pub(crate) root: SharedString,
    /// The file, slash-separated from the root.
    pub(crate) rel: SharedString,
    /// The base branch's name and revision when the tab compares against the
    /// branch's base; `None` when it compares against `HEAD`.
    pub(crate) base: Option<(SharedString, SharedString)>,
}

impl DiffKey {
    /// The file on disk.
    pub(crate) fn path(&self) -> PathBuf {
        PathBuf::from(self.root.as_ref()).join(self.rel.as_ref())
    }
}

/// One drawn row.
#[derive(Clone)]
enum DiffRow {
    /// Where a hunk starts.
    Hunk(SharedString),
    /// A line of it.
    Line {
        kind: LineKind,
        old: Option<u32>,
        new: Option<u32>,
        text: SharedString,
        /// Highlighted runs, in byte offsets into `text`.
        spans: Vec<(Range<usize>, Token)>,
    },
}

/// What was last read for one tab.
struct DiffRead {
    /// The file's kind of change; `None` when it no longer differs.
    kind: Option<ChangeKind>,
    binary: bool,
    truncated: bool,
    added: usize,
    removed: usize,
    rows: Arc<Vec<DiffRow>>,
    /// Row index of each hunk's header, for moving between changes.
    hunks: Vec<usize>,
}

/// One diff tab's state.
pub(crate) struct DiffState {
    read: Option<DiffRead>,
    error: Option<SharedString>,
    reading: bool,
    /// The list's heights and scroll position, kept in step with `items`.
    list: ListState,
    /// What the list draws, in order — see [`Shell::review_layout`].
    items: Vec<DiffItem>,
    /// The rows a pending comment is about, which wear a dot.
    marked: Arc<HashSet<usize>>,
    /// A press on a line not yet let go of: the pick follows the pointer.
    drag: Option<DiffDrag>,
    /// Whether a loop is scrolling the list for a drag at its edge.
    autoscrolling: bool,
    /// Frames left to bring a composer that just opened into view: it is
    /// measured by the layout after the one that inserts it, so the scroll
    /// waits for that.
    reveal_draft: u8,
    /// The hunk F7 last moved to.
    cursor: Option<usize>,
    /// The lines picked, as the row a pick started on and the row it runs
    /// to: either way round.
    picked: Option<(usize, usize)>,
}

impl Default for DiffState {
    fn default() -> Self {
        Self {
            read: None,
            error: None,
            reading: false,
            // Rows out of view are measured as they come into it, plus a
            // screen's worth either side so a fling does not show them late.
            list: ListState::new(0, ListAlignment::Top, px(600.0)),
            items: Vec::new(),
            marked: Arc::default(),
            drag: None,
            autoscrolling: false,
            reveal_draft: 0,
            cursor: None,
            picked: None,
        }
    }
}

/// A pick being dragged out.
#[derive(Clone, Copy, Debug)]
struct DiffDrag {
    /// The row the pick runs from.
    anchor: usize,
    /// Where the pointer was last, in window coordinates, for the scroll at
    /// the list's edge to run the pick on with while it holds still.
    pointer_y: Pixels,
    /// A press on a hunk's header, which picked the hunk: moving does not
    /// change that, and letting go opens the composer on it.
    whole_hunk: bool,
}

/// One thing in a diff tab's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffItem {
    /// A row of the diff, by index.
    Row(usize),
    /// The composer.
    Draft,
    /// A comment in the review, by id.
    Comment(usize),
}

/// All open diff tabs' states, by the comparison they show.
pub(crate) type Diffs = HashMap<DiffKey, DiffState>;

/// The lines picked in a diff tab, as text and as numbers.
#[derive(Clone, Debug)]
pub(crate) struct DiffPick {
    /// Each line with its `+`, `-` or space, as a patch writes them.
    pub(crate) text: String,
    /// Which lines they are, said: `lines 40–44`. Empty when no line was
    /// picked, only a hunk's header.
    pub(crate) lines: String,
    /// The first and last of the new file's line numbers among them.
    pub(crate) new_span: Option<(u32, u32)>,
    /// The same of the old file's: a removed line has only this, and a
    /// pick of additions has none.
    pub(crate) old_span: Option<(u32, u32)>,
}

/// Reads and lays out one file's diff. Runs off the window's thread.
fn read_diff(key: &DiffKey) -> ket_core::Result<DiffRead> {
    let root = PathBuf::from(key.root.as_ref());
    let rev = match &key.base {
        Some((_, base)) => crate::git_panel::since_rev(&root, base),
        None => "HEAD".to_owned(),
    };
    let Some(file) = ket_core::diff::file_of_worktree(&root, &rev, &key.rel)? else {
        return Ok(DiffRead {
            kind: None,
            binary: false,
            truncated: false,
            added: 0,
            removed: 0,
            rows: Arc::new(Vec::new()),
            hunks: Vec::new(),
        });
    };
    let (added, removed) = file.line_counts();
    let (rows, hunks) = layout(&file);
    Ok(DiffRead {
        kind: Some(file.kind),
        binary: file.binary,
        truncated: file.truncated,
        added,
        removed,
        rows: Arc::new(rows),
        hunks,
    })
}

/// Flattens a file's hunks into rows, numbering and highlighting each line.
///
/// Each hunk is highlighted as two short documents — its old side (context
/// and removals) and its new side (context and additions) — so a line is
/// coloured against the lines around it on its own side, which is what a
/// string or a comment spanning several of them needs.
fn layout(file: &FileDiff) -> (Vec<DiffRow>, Vec<usize>) {
    let language = syntax::language_for(&file.path);
    let mut rows = Vec::new();
    let mut hunks = Vec::new();

    for hunk in &file.hunks {
        hunks.push(rows.len());
        rows.push(DiffRow::Hunk(hunk.header().into()));

        let texts: Vec<String> = hunk
            .lines
            .iter()
            .map(|line| line.text.replace('\t', TAB_SPACES))
            .collect();
        let old_spans = side_spans(&hunk.lines, &texts, language, LineKind::Removed);
        let new_spans = side_spans(&hunk.lines, &texts, language, LineKind::Added);

        let mut old = hunk.old_start;
        let mut new = hunk.new_start;
        for (index, (line, text)) in hunk.lines.iter().zip(texts).enumerate() {
            let (old_no, new_no, spans) = match line.kind {
                LineKind::Context => {
                    let numbers = (Some(old), Some(new));
                    old += 1;
                    new += 1;
                    (numbers.0, numbers.1, new_spans.get(&index).cloned())
                }
                LineKind::Removed => {
                    old += 1;
                    (Some(old - 1), None, old_spans.get(&index).cloned())
                }
                LineKind::Added => {
                    new += 1;
                    (None, Some(new - 1), new_spans.get(&index).cloned())
                }
            };
            rows.push(DiffRow::Line {
                kind: line.kind,
                old: old_no,
                new: new_no,
                text: text.into(),
                spans: spans.unwrap_or_default(),
            });
        }
    }
    (rows, hunks)
}

/// Highlights one side of a hunk — context plus `side`'s own lines — and
/// returns each of those lines' spans in byte offsets, by the line's index in
/// the hunk.
fn side_spans(
    lines: &[ket_core::diff::DiffLine],
    texts: &[String],
    language: Option<&'static syntax::Language>,
    side: LineKind,
) -> HashMap<usize, Vec<(Range<usize>, Token)>> {
    let Some(language) = language else {
        return HashMap::new();
    };
    let indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind == LineKind::Context || line.kind == side)
        .map(|(index, _)| index)
        .collect();
    let document = indices
        .iter()
        .map(|&index| texts[index].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let (_, spans) = syntax::highlighted_lines(&document, language);

    indices
        .into_iter()
        .zip(spans)
        .map(|(index, spans)| {
            // The highlighter counts characters; a shaped line counts bytes.
            let text = &texts[index];
            let byte_of: Vec<usize> = text
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(text.len()))
                .collect();
            let last = byte_of.len() - 1;
            let spans = spans
                .into_iter()
                .map(|(range, token)| {
                    (
                        byte_of[range.start.min(last)]..byte_of[range.end.min(last)],
                        token,
                    )
                })
                .filter(|(range, _)| !range.is_empty())
                .collect();
            (index, spans)
        })
        .collect()
}

/// How a highlighted run is drawn, matching the editor's scheme: three hues,
/// with keywords set heavier and comments slanted.
fn token_style(token: Token, t: &Theme) -> HighlightStyle {
    let colour = match token {
        Token::Keyword => t.syntax.keyword,
        Token::Str => t.syntax.string,
        Token::Comment => t.syntax.comment,
        Token::Number => t.syntax.number,
        Token::Kind => t.syntax.kind,
        Token::Punctuation => t.syntax.punctuation,
    };
    HighlightStyle {
        color: Some(paint(colour).into()),
        font_weight: (token == Token::Keyword).then_some(FontWeight::SEMIBOLD),
        font_style: (token == Token::Comment).then_some(FontStyle::Italic),
        ..Default::default()
    }
}

impl Shell {
    /// Opens the diff tab for `key`, or focuses it if it is open.
    pub(crate) fn open_diff_tab(
        &mut self,
        key: DiffKey,
        name: String,
        preview: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree_id) = self.selected_id() else {
            return;
        };
        // A tab opened again reads again: whoever clicked the row wants what
        // the file is now, not what it was when the tab first opened.
        self.load_diff(&key, cx);
        let tab = Tab {
            title: format!("{name} (diff)").into(),
            kind: TabKind::Diff(key),
            renamed: false,
            pinned: false,
            preview,
        };
        let space = self.spaces.entry(worktree_id).or_default();
        if preview {
            space.open_preview_tab(tab);
        } else {
            space.open_tab(tab);
        }
        self.prune_diffs();
        self.persist_layout();
    }

    /// Starts a read of `key`'s diff, unless one is already running.
    fn load_diff(&mut self, key: &DiffKey, cx: &mut Context<Self>) {
        let state = self.diffs.entry(key.clone()).or_default();
        if state.reading {
            return;
        }
        state.reading = true;
        let read_key = key.clone();
        let read = cx
            .background_executor()
            .spawn(async move { read_diff(&read_key) });
        let key = key.clone();
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = read.await;
            let _ = shell.update(cx, |shell, cx| {
                // Closed while reading: nothing to put it in.
                let Some(state) = shell.diffs.get_mut(&key) else {
                    return;
                };
                state.reading = false;
                match result {
                    Ok(read) => {
                        if state
                            .cursor
                            .is_some_and(|cursor| cursor >= read.hunks.len())
                        {
                            state.cursor = None;
                        }
                        state.read = Some(read);
                        state.error = None;
                    }
                    Err(error) => state.error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-reads the diff in front, on the status tick.
    pub(crate) fn refresh_open_diff(&mut self, cx: &mut Context<Self>) {
        // Tabs close by more routes than one — a pane closing, a worktree
        // going — and not all of them come through `close_tab_now`.
        self.prune_diffs();
        if let Some(key) = self.active_diff() {
            self.load_diff(&key, cx);
        }
    }

    /// The comparison the focused pane's active tab shows, if it is a diff.
    pub(crate) fn active_diff(&self) -> Option<DiffKey> {
        match &self.space()?.active_tab()?.kind {
            TabKind::Diff(key) => Some(key.clone()),
            _ => None,
        }
    }

    /// Drops the state of diff tabs that are no longer open anywhere.
    pub(crate) fn prune_diffs(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let open: Vec<DiffKey> = self
            .spaces
            .values()
            .flat_map(|space| space.diff_keys())
            .collect();
        self.diffs.retain(|key, _| open.contains(key));
    }

    /// F7 and Shift-F7, while a diff is the tab in front; ⌘C, ⌘↵ and Escape
    /// while lines are picked in it.
    pub(crate) fn diff_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.active_diff() else {
            return false;
        };
        let keys = event.keystroke.modifiers;
        let picked = self
            .diffs
            .get(&key)
            .is_some_and(|state| state.picked.is_some());
        if picked && event.keystroke.key == "escape" {
            if let Some(state) = self.diffs.get_mut(&key) {
                state.picked = None;
                state.drag = None;
            }
            cx.notify();
            return true;
        }
        if picked && event.keystroke.key == "c" && (keys.platform || keys.control) {
            if let Some(pick) = self.diff_selection_of(&key) {
                cx.write_to_clipboard(ClipboardItem::new_string(pick.text));
            }
            return true;
        }
        if picked && event.keystroke.key == "enter" && (keys.platform || keys.control) {
            return self.open_review_draft(key);
        }
        if event.keystroke.key != "f7" {
            return false;
        }
        let step = if event.keystroke.modifiers.shift {
            -1
        } else {
            1
        };
        self.step_diff(&key, step, cx);
        true
    }

    /// Lets go of the lines picked in `key`'s tab.
    pub(crate) fn clear_diff_pick(&mut self, key: &DiffKey) {
        if let Some(state) = self.diffs.get_mut(key) {
            state.picked = None;
        }
    }

    /// A press on row `index`: picks the line, runs the pick to it with
    /// `extend`, or — on a hunk's header — picks the hunk. A press always
    /// picks, since letting go opens the composer on the pick; Escape is
    /// what lets go of one.
    fn pick_diff_row(&mut self, key: &DiffKey, index: usize, extend: bool) {
        let Some(state) = self.diffs.get_mut(key) else {
            return;
        };
        let Some(read) = &state.read else {
            return;
        };
        let header = matches!(read.rows.get(index), Some(DiffRow::Hunk(_)));
        state.picked = if header {
            let end = read
                .hunks
                .iter()
                .find(|&&start| start > index)
                .map_or(read.rows.len(), |&next| next)
                .saturating_sub(1);
            (end > index).then_some((index + 1, end))
        } else {
            match state.picked {
                Some((anchor, _)) if extend => Some((anchor, index)),
                _ => Some((index, index)),
            }
        };
    }

    /// A press on row `index` that may become a drag: picks as a click does,
    /// then holds the pick's start so moving the pointer runs it from there,
    /// and letting go opens the composer on what was picked — one gesture,
    /// not a pick and then a button. A hunk's header picks its hunk whole,
    /// and moving before letting go leaves it so.
    fn press_diff_row(&mut self, key: &DiffKey, index: usize, extend: bool, y: Pixels) {
        self.pick_diff_row(key, index, extend);
        let Some(state) = self.diffs.get_mut(key) else {
            return;
        };
        let line = state
            .read
            .as_ref()
            .is_some_and(|read| matches!(read.rows.get(index), Some(DiffRow::Line { .. })));
        // Opened on release either way, not here: the press goes on to the
        // shell, whose click-elsewhere would close an empty composer at once.
        state.drag = Some(DiffDrag {
            anchor: state.picked.map_or(index, |(anchor, _)| anchor),
            pointer_y: y,
            whole_hunk: !line,
        });
    }

    /// The button came up on a drag: it ends, and the composer opens on the
    /// lines picked — unless the diff has no worktree to file a comment
    /// under, when the pick is all there is. Says whether there was a drag
    /// to end.
    fn finish_diff_drag(&mut self, key: &DiffKey) -> bool {
        if !self.end_diff_drag(key) {
            return false;
        }
        let picked = self
            .diffs
            .get(key)
            .is_some_and(|state| state.picked.is_some());
        if picked {
            self.open_review_draft(key.clone());
        }
        true
    }

    /// The pointer moved, with the button held, to `y` in the window: the
    /// pick runs to the row there, or to the row at the list's edge when
    /// the pointer is past it — and past it, the list scrolls to follow.
    fn drag_diff_pick(&mut self, key: &DiffKey, y: Pixels, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.diffs.get_mut(key) else {
            return false;
        };
        let Some(drag) = state.drag.as_mut().filter(|drag| !drag.whole_hunk) else {
            return false;
        };
        drag.pointer_y = y;
        let anchor = drag.anchor;
        let before = state.picked;
        if let Some(head) = row_at(state, y) {
            state.picked = Some((anchor, head));
        }
        let changed = state.picked != before;
        if edge_step(state, y) != px(0.0) {
            self.start_diff_autoscroll(key.clone(), cx);
        }
        changed
    }

    /// The button came up, or a move arrived without it: the drag is over
    /// and the pick stays as it is.
    fn end_diff_drag(&mut self, key: &DiffKey) -> bool {
        self.diffs
            .get_mut(key)
            .is_some_and(|state| state.drag.take().is_some())
    }

    /// Whether `x`, in window coordinates, is over the gutter of `key`'s
    /// rows: the line numbers and the sign, the part that picks.
    fn in_diff_gutter(&self, key: &DiffKey, x: Pixels) -> bool {
        self.diffs.get(key).is_some_and(|state| {
            let left = state.list.viewport_bounds().left();
            x >= left && x < left + GUTTER_WIDTH
        })
    }

    /// Scrolls `key`'s list a step each frame while a drag holds the
    /// pointer near or past its top or bottom edge. One loop per tab: a
    /// second call while one runs does nothing.
    fn start_diff_autoscroll(&mut self, key: DiffKey, cx: &mut Context<Self>) {
        let Some(state) = self.diffs.get_mut(&key) else {
            return;
        };
        if std::mem::replace(&mut state.autoscrolling, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL_TICK).await;
                let going = this
                    .update(cx, |this, cx| this.diff_autoscroll_step(&key, cx))
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// One frame of [`Shell::start_diff_autoscroll`]: scrolls, and runs the
    /// pick to the row now at the edge. Says whether to keep going.
    fn diff_autoscroll_step(&mut self, key: &DiffKey, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.diffs.get_mut(key) else {
            return false;
        };
        let step = state
            .drag
            .as_ref()
            .map_or(px(0.0), |drag| edge_step(state, drag.pointer_y));
        let Some(drag) = state.drag.as_ref().filter(|_| step != px(0.0)) else {
            state.autoscrolling = false;
            return false;
        };
        let (anchor, y) = (drag.anchor, drag.pointer_y);
        state.list.scroll_by(step);
        if let Some(head) = row_at(state, y) {
            state.picked = Some((anchor, head));
        }
        cx.notify();
        true
    }

    /// The lines picked in the diff tab in front, if any — see
    /// [`Shell::diff_selection_of`] — with its comparison.
    pub(crate) fn diff_selection(&self) -> Option<(DiffKey, String, String)> {
        let key = self.active_diff()?;
        let pick = self.diff_selection_of(&key)?;
        Some((key, pick.text, pick.lines))
    }

    /// The lines picked in `key`'s tab, each with its `+`, `-` or space as a
    /// patch writes them, and which lines they are: `lines 40–44` of the new
    /// file, or of the old one where only removals were picked.
    pub(crate) fn diff_selection_of(&self, key: &DiffKey) -> Option<DiffPick> {
        let state = self.diffs.get(key)?;
        let read = state.read.as_ref()?;
        let (anchor, head) = state.picked?;
        let (low, high) = (anchor.min(head), anchor.max(head));
        let mut text = Vec::new();
        let mut new_lines = Vec::new();
        let mut old_lines = Vec::new();
        for row in read.rows.get(low..=high)? {
            match row {
                DiffRow::Hunk(header) => text.push(header.to_string()),
                DiffRow::Line {
                    kind,
                    old,
                    new,
                    text: line,
                    ..
                } => {
                    let sign = match kind {
                        LineKind::Added => '+',
                        LineKind::Removed => '-',
                        LineKind::Context => ' ',
                    };
                    text.push(format!("{sign}{line}"));
                    new_lines.extend(*new);
                    old_lines.extend(*old);
                }
            }
        }
        let span_of = |numbers: &[u32]| numbers.first().zip(numbers.last()).map(|(a, b)| (*a, *b));
        let new_span = span_of(&new_lines);
        let old_span = span_of(&old_lines);
        let lines = match new_span.or(old_span) {
            Some((first, last)) if first == last => format!("line {first}"),
            Some((first, last)) => format!("lines {first}\u{2013}{last}"),
            None => String::new(),
        };
        Some(DiffPick {
            text: text.join("\n"),
            lines,
            new_span,
            old_span,
        })
    }

    /// Moves to the next (`1`) or previous (`-1`) change, wrapping at the ends.
    fn step_diff(&mut self, key: &DiffKey, step: isize, cx: &mut Context<Self>) {
        let Some(state) = self.diffs.get_mut(key) else {
            return;
        };
        let Some(read) = &state.read else {
            return;
        };
        let count = read.hunks.len();
        if count == 0 {
            return;
        }
        let next = match state.cursor {
            None if step < 0 => count - 1,
            None => 0,
            Some(at) => (at as isize + step).rem_euclid(count as isize) as usize,
        };
        state.cursor = Some(next);
        let row = read.hunks[next];
        if let Some(item_ix) = state
            .items
            .iter()
            .position(|item| *item == DiffItem::Row(row))
        {
            state.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(0.0),
            });
        }
        cx.notify();
    }

    /// `trigger` with a tooltip saying `label`, shown while the pointer is
    /// on it: the shared label, for the glyph-only controls the diff and
    /// its review put up. `id` and `ix` name the control among its kind —
    /// `ix` is a row's or a comment's, and 0 for a control there is one of.
    pub(crate) fn tipped(
        &self,
        id: &'static str,
        ix: usize,
        trigger: AnyElement,
        label: &'static str,
        side: Side,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        tooltip(
            ElementId::named_usize(format!("{id}-tip"), ix),
            trigger,
            label,
            side,
            self.hovered_diff_control == Some((id, ix)),
            &self.theme,
        )
        .flex_none()
        .on_hover(cx.listener(move |this, is_hovered, _, cx| {
            if *is_hovered {
                this.hovered_diff_control = Some((id, ix));
            } else if this.hovered_diff_control == Some((id, ix)) {
                this.hovered_diff_control = None;
            }
            cx.notify();
        }))
    }

    /// The tab's content.
    pub(crate) fn diff_pane(
        &mut self,
        key: &DiffKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        if !self.diffs.contains_key(key) {
            self.load_diff(key, cx);
        }
        if self.sync_diff_items(key) {
            window.request_animation_frame();
        }
        let Some(state) = self.diffs.get(key) else {
            return div().into_any_element();
        };

        let name = key.rel.rsplit('/').next().unwrap_or(&key.rel).to_owned();
        let dir = key
            .rel
            .rsplit_once('/')
            .map(|(dir, _)| dir.to_owned())
            .unwrap_or_default();
        let read = state.read.as_ref();
        let deleted = read.and_then(|read| read.kind) == Some(ChangeKind::Deleted);
        let has_changes = read.is_some_and(|read| !read.hunks.is_empty());

        let versus: SharedString = match &key.base {
            Some((label, _)) => format!("since {label}").into(),
            None => "uncommitted".into(),
        };
        let counts = read.map(|read| {
            div()
                .flex()
                .flex_none()
                .gap(px(6.0))
                .when(read.added > 0, |el| {
                    el.child(
                        div()
                            .text_color(paint(t.diff.added))
                            .child(format!("+{}", read.added)),
                    )
                })
                .when(read.removed > 0, |el| {
                    el.child(
                        div()
                            .text_color(paint(t.diff.removed))
                            .child(format!("\u{2212}{}", read.removed)),
                    )
                })
        });

        let prev_key = key.clone();
        let next_key = key.clone();
        let open_path = key.path();
        let reviewer = self.review_worktree_of(key);
        // The worktree's whole review, not only this file's: the one message
        // the Git panel's Send sends too.
        let pending = reviewer
            .map(|id| {
                let count = self.reviews.get(&id).map_or(0, Vec::len);
                (id, count)
            })
            .filter(|(_, count)| *count > 0);
        let glyph = |id: &'static str,
                     which: Icon,
                     label: &'static str,
                     side: Side,
                     enabled: bool,
                     cx: &mut Context<Self>| {
            let trigger = icon_button(id, which)
                .bare()
                .dense()
                .render(&t)
                .when(!enabled, |el| el.opacity(0.4))
                .into_any_element();
            self.tipped(id, 0, trigger, label, side, cx)
        };

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(40.0))
            .pl_3()
            .pr_2()
            .border_b_1()
            .border_color(paint(t.rule))
            .text_size(px(11.0))
            .child(file_mark(&name, false, self.font_family.clone(), &t))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(paint(if deleted { t.text.dim } else { t.text.primary }))
                    .when(deleted, |el| el.line_through())
                    .child(name.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(10.0))
                    .text_color(paint(t.text.dim))
                    .child(dir),
            )
            .child(div().flex_1())
            .when_some(pending, |el, (id, count)| {
                el.child(
                    button("diff-send-review", format!("Send review ({count})"))
                        .small()
                        .render(&t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send_review(id.clone(), cx);
                            cx.notify();
                        })),
                )
                .child(div().flex_none().w(px(1.0)).h(px(20.0)).bg(paint(t.rule)))
            })
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(paint(t.text.dim))
                    .child(versus),
            )
            .children(counts)
            .child(
                glyph(
                    "diff-previous-change",
                    Icon::ChevronUp,
                    "Previous change  \u{21e7}F7",
                    Side::Bottom,
                    has_changes,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.step_diff(&prev_key, -1, cx);
                })),
            )
            .child(
                glyph(
                    "diff-next-change",
                    Icon::ChevronDown,
                    "Next change  F7",
                    Side::Bottom,
                    has_changes,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.step_diff(&next_key, 1, cx);
                })),
            )
            // A deleted file has nothing on disk to open.
            .when(!deleted, |el| {
                el.child(
                    glyph(
                        "diff-open-file",
                        Icon::File,
                        "Open file",
                        Side::BottomEnd,
                        true,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_editor_from_diff(open_path.clone(), cx);
                        cx.notify();
                    })),
                )
            });

        let message = |text: String| {
            div()
                .flex()
                .flex_1()
                .justify_center()
                .items_center()
                .text_size(px(11.0))
                .text_color(paint(t.text.dim))
                .child(text)
                .into_any_element()
        };
        let body = match (read, &state.error) {
            (_, Some(error)) => message(error.to_string()),
            (None, None) => message("Reading…".to_owned()),
            (Some(read), None) if read.kind.is_none() => {
                message("No changes in this file any more.".to_owned())
            }
            (Some(read), None) if read.binary => message("Binary file changed.".to_owned()),
            (Some(read), None) if read.truncated && read.rows.is_empty() => {
                message("This file is too large to diff here.".to_owned())
            }
            (Some(read), None) if read.rows.is_empty() => message(match read.kind {
                Some(ChangeKind::Added | ChangeKind::Untracked) => "New empty file.".to_owned(),
                Some(ChangeKind::Deleted) => "Deleted an empty file.".to_owned(),
                _ => "Only the file's mode or type changed.".to_owned(),
            }),
            (Some(read), None) => self.diff_list(key, read, state, cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(paint(t.surface))
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The rows a pending review comment on `key` is about, by index, and
    /// the list as drawn: each row, with the cards of the comments that end
    /// on it, and the composer, under the last line they are about.
    ///
    /// Found by line number rather than kept as row indices, which the next
    /// read of a file still being written would shift. A comment whose
    /// lines are no longer in the diff has no card here; it is still in the
    /// Git panel's Review section. A composer whose lines went the same way
    /// is put at the end rather than lost, since it holds the keyboard.
    fn review_layout(&self, key: &DiffKey, rows: &[DiffRow]) -> (HashSet<usize>, Vec<DiffItem>) {
        let comments: Vec<&crate::review::ReviewComment> = self
            .reviews
            .values()
            .flatten()
            .filter(|comment| &comment.key == key)
            .collect();
        let draft = self.review_draft.as_ref().filter(|draft| &draft.key == key);
        let mut items: Vec<DiffItem> = (0..rows.len()).map(DiffItem::Row).collect();
        let mut marked = HashSet::new();
        if comments.is_empty() && draft.is_none() {
            return (marked, items);
        }
        // Either side: a removed line has only an old number, and a comment
        // on a change takes in both.
        let within = |number: &Option<u32>, span: Option<(u32, u32)>| {
            number
                .zip(span)
                .is_some_and(|(n, (first, last))| (first..=last).contains(&n))
        };
        let last_of = |new_span: Option<(u32, u32)>, old_span: Option<(u32, u32)>| {
            rows.iter().rposition(|row| {
                matches!(row, DiffRow::Line { old, new, .. }
                    if within(new, new_span) || within(old, old_span))
            })
        };
        // What goes under each row, in the order written.
        let mut under: HashMap<usize, Vec<DiffItem>> = HashMap::new();
        let mut draft_placed = draft.is_none();
        for comment in &comments {
            for (index, row) in rows.iter().enumerate() {
                if let DiffRow::Line { old, new, .. } = row
                    && (within(new, comment.new_span) || within(old, comment.old_span))
                {
                    marked.insert(index);
                }
            }
            let Some(last) = last_of(comment.new_span, comment.old_span) else {
                continue;
            };
            let editing = draft.is_some_and(|draft| draft.editing == Some(comment.id));
            draft_placed |= editing;
            under.entry(last).or_default().push(if editing {
                DiffItem::Draft
            } else {
                DiffItem::Comment(comment.id)
            });
        }
        if let Some(draft) = draft.filter(|draft| draft.editing.is_none())
            && let Some(last) = last_of(draft.pick.new_span, draft.pick.old_span)
        {
            under.entry(last).or_default().push(DiffItem::Draft);
            draft_placed = true;
        }
        if under.is_empty() && draft_placed {
            return (marked, items);
        }
        items = Vec::with_capacity(rows.len() + under.len() + 1);
        for index in 0..rows.len() {
            items.push(DiffItem::Row(index));
            if let Some(cards) = under.remove(&index) {
                items.extend(cards);
            }
        }
        if !draft_placed {
            items.push(DiffItem::Draft);
        }
        (marked, items)
    }

    /// Brings `key`'s list up to date with its rows, its comments and the
    /// composer: what changed between the last layout and this one is
    /// spliced into the list's state, so heights already measured and the
    /// scroll position outside it stay put. A composer that just opened is
    /// scrolled into view once it has been measured — opened from the last
    /// line on screen, it would otherwise sit under the fold. Says whether
    /// another frame is wanted for that.
    fn sync_diff_items(&mut self, key: &DiffKey) -> bool {
        let Some(rows) = self
            .diffs
            .get(key)
            .and_then(|state| state.read.as_ref())
            .map(|read| read.rows.clone())
        else {
            return false;
        };
        let (marked, items) = self.review_layout(key, &rows);
        let Some(state) = self.diffs.get_mut(key) else {
            return false;
        };
        if *state.marked != marked {
            state.marked = Arc::new(marked);
        }
        if state.items != items {
            let opened =
                items.contains(&DiffItem::Draft) && !state.items.contains(&DiffItem::Draft);
            splice_items(state, items);
            if opened {
                state.reveal_draft = 3;
            }
        }
        reveal_draft(state)
    }
}

/// Puts `items` in place of `state`'s, splicing only what differs between
/// them into the list's state.
fn splice_items(state: &mut DiffState, items: Vec<DiffItem>) {
    {
        let old = &state.items;
        let head = old.iter().zip(&items).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(items[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        state
            .list
            .splice(head..old.len() - tail, items.len() - head - tail);
    }
    state.items = items;
}

/// One frame of bringing a just-opened composer into view: once the layout
/// has measured it, scrolls just far enough to show all of it. Says whether
/// another frame is wanted — still waiting on the measure, or to draw the
/// scroll.
fn reveal_draft(state: &mut DiffState) -> bool {
    if state.reveal_draft == 0 {
        return false;
    }
    let Some(ix) = state.items.iter().position(|item| *item == DiffItem::Draft) else {
        state.reveal_draft = 0;
        return false;
    };
    let Some(bounds) = state.list.bounds_for_item(ix) else {
        state.reveal_draft -= 1;
        return state.reveal_draft > 0;
    };
    state.reveal_draft = 0;
    let view = state.list.viewport_bounds();
    if bounds.bottom() > view.bottom() || bounds.top() < view.top() {
        state.list.scroll_to_reveal_item(ix);
        return true;
    }
    false
}

impl Shell {
    /// The rows, with the comments' cards and the composer between them.
    /// A list of rows of their own heights rather than a uniform one, since
    /// a card is not a line tall; only the rows in view are drawn either way.
    fn diff_list(
        &self,
        key: &DiffKey,
        read: &DiffRead,
        state: &DiffState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let rows = read.rows.clone();
        let truncated = read.truncated;
        let font = self.font_family.clone();
        let font_size = px(f32::from(self.editor_type.font_size));
        let line_height = relative(self.editor_type.line_height);
        let picked = state
            .picked
            .map(|(anchor, head)| anchor.min(head)..=anchor.max(head));
        let marked = state.marked.clone();
        let items = Arc::new(state.items.clone());
        // While a pick is dragged, the pointer is followed past the list's
        // edges too — the window's own moves, not the rows' — so the pick
        // runs on and the list scrolls when it leaves the top or bottom.
        let follow = state.drag.is_some().then(|| {
            let shell = cx.weak_entity();
            let key = key.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let (moved, moved_key) = (shell.clone(), key.clone());
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let _ = moved.update(cx, |this, cx| {
                            let changed = match event.pressed_button {
                                Some(MouseButton::Left) => {
                                    this.drag_diff_pick(&moved_key, event.position.y, cx)
                                }
                                _ => this.end_diff_drag(&moved_key),
                            };
                            if changed {
                                cx.notify();
                            }
                        });
                    });
                    let (released, released_key) = (shell.clone(), key.clone());
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let _ = released.update(cx, |this, cx| {
                            if this.finish_diff_drag(&released_key) {
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .absolute()
            .size_full()
        });
        let owner = key.clone();
        let list = list(
            state.list.clone(),
            cx.processor(move |this, ix: usize, window, cx| {
                let t = this.theme;
                // A card sits under the code, indented to it.
                let card = |card: AnyElement| {
                    div()
                        .w_full()
                        .pl(GUTTER_WIDTH + px(6.0))
                        .pr(px(12.0))
                        .py(px(6.0))
                        .child(card)
                        .into_any_element()
                };
                match items.get(ix) {
                    Some(DiffItem::Row(index)) => {
                        let index = *index;
                        let Some(row) = rows.get(index) else {
                            return div().into_any_element();
                        };
                        let on = picked
                            .as_ref()
                            .filter(|picked| picked.contains(&index))
                            .map(|picked| (index == *picked.start(), index == *picked.end()));
                        let key = owner.clone();
                        let up_key = owner.clone();
                        let line = matches!(row, DiffRow::Line { .. });
                        // The "+" a reviewer looks for in a diff's gutter;
                        // the press lands on the row, which opens the
                        // composer when it lets go. Not on a picked line,
                        // whose composer is open.
                        let add = (line && on.is_none()).then(|| {
                            let plus = icon_button(("diff-add-comment", index), Icon::Plus)
                                .primary()
                                .sized(px(16.0), px(12.0))
                                .render(&t)
                                .into_any_element();
                            this.tipped(
                                "diff-add-comment",
                                index,
                                plus,
                                "Add a comment (drag for more lines)",
                                Side::Top,
                                cx,
                            )
                            .into_any_element()
                        });
                        diff_row(index, row, on, marked.contains(&index), add, &t)
                            .font_family(font.clone())
                            .text_size(font_size)
                            .line_height(line_height)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    // A line is picked from its gutter, the
                                    // numbers and the sign, as a reviewer
                                    // expects; a press on the code is not a
                                    // comment. It still reaches the shell,
                                    // which closes an empty composer.
                                    if line && !this.in_diff_gutter(&key, event.position.x) {
                                        return;
                                    }
                                    // An empty composer gives way to the new
                                    // pick; one with words in it keeps
                                    // picking off, and the click reaches the
                                    // shell, which takes its keyboard away.
                                    let extend = event.modifiers.shift;
                                    if !this.discard_empty_review_draft(extend) {
                                        return;
                                    }
                                    this.press_diff_row(
                                        &key,
                                        index,
                                        event.modifiers.shift,
                                        event.position.y,
                                    );
                                    cx.notify();
                                }),
                            )
                            // Here as well as on the window: a quick click can
                            // let go before the frame that starts listening
                            // there is drawn.
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                    if this.finish_diff_drag(&up_key) {
                                        cx.notify();
                                    }
                                }),
                            )
                            .into_any_element()
                    }
                    Some(DiffItem::Draft) => card(this.review_draft_card(window, cx)),
                    Some(DiffItem::Comment(id)) => card(this.review_comment_card(*id, cx)),
                    None => div().into_any_element(),
                }
            }),
        )
        .flex_grow()
        .size_full();

        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
            .children(follow)
            // Hunks were dropped at the per-file budget: say so rather than
            // let the last hunk read as the end of the change.
            .when(truncated, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py(px(6.0))
                        .border_t_1()
                        .border_color(paint(t.rule))
                        .text_size(px(10.0))
                        .text_color(paint(t.text.dim))
                        .child("The rest of this diff is too large to show."),
                )
            })
            .into_any_element()
    }
}

/// How close to the list's top or bottom a drag starts it scrolling.
const AUTOSCROLL_EDGE: Pixels = px(28.0);

/// How often the list scrolls a step while a drag holds it at an edge.
const AUTOSCROLL_TICK: std::time::Duration = std::time::Duration::from_millis(16);

/// The diff row under `y`, in window coordinates — or, with `y` above or
/// below the list, the row at that edge. A card under `y` counts as the row
/// it hangs from.
fn row_at(state: &DiffState, y: Pixels) -> Option<usize> {
    let view = state.list.viewport_bounds();
    let y = y.max(view.top() + px(1.0)).min(view.bottom() - px(1.0));
    let mut ix = state.list.logical_scroll_top().item_ix;
    let mut row = None;
    while let Some(bounds) = state.list.bounds_for_item(ix) {
        if let Some(DiffItem::Row(index)) = state.items.get(ix) {
            row = Some(*index);
        }
        if bounds.bottom() > y {
            break;
        }
        ix += 1;
    }
    row
}

/// How far to scroll this frame for a drag whose pointer is at `y`: up when
/// it is near or above the top, down near or below the bottom, faster the
/// further out it is, and nothing between.
fn edge_step(state: &DiffState, y: Pixels) -> Pixels {
    let view = state.list.viewport_bounds();
    let past = if y < view.top() + AUTOSCROLL_EDGE {
        y - (view.top() + AUTOSCROLL_EDGE)
    } else if y > view.bottom() - AUTOSCROLL_EDGE {
        y - (view.bottom() - AUTOSCROLL_EDGE)
    } else {
        return px(0.0);
    };
    let step = past / 3.0;
    if step < px(0.0) {
        step.max(px(-40.0)).min(px(-2.0))
    } else {
        step.min(px(40.0)).max(px(2.0))
    }
}

/// The hover group every row of the list belongs to, so its gutter can
/// answer the pointer anywhere along the line.
const ROW_GROUP: &str = "diff-row";

/// One row of the list.
///
/// The two line numbers and the sign sit in one well, the gutter, which is
/// what a click on the line picks: it lifts under the pointer and shows a
/// comment mark in place of the sign, so the line reads as something to
/// press. `picked` is `(first, last)` while the row is among the lines
/// picked — the well fills and rounds only at the pick's two ends, so a run
/// of lines reads as one block. `marked` puts a dot at the gutter's edge
/// while a pending review comment is about the line; the comment itself is
/// a card under its last line — see [`Shell::review_layout`].
fn diff_row(
    index: usize,
    row: &DiffRow,
    picked: Option<(bool, bool)>,
    marked: bool,
    add: Option<AnyElement>,
    t: &Theme,
) -> gpui::Stateful<gpui::Div> {
    match row {
        DiffRow::Hunk(header) => div()
            .id(("diff-row", index))
            .group(ROW_GROUP)
            .flex()
            .w_full()
            .items_center()
            .bg(paint(t.hover))
            .cursor_pointer()
            .text_color(alpha(paint(t.diff.hunk_header), 0.85))
            .hover(|style| style.text_color(paint(t.diff.hunk_header)))
            .child(div().flex_none().w(GUTTER_WIDTH + px(6.0)))
            .child(header.clone()),
        DiffRow::Line {
            kind,
            old,
            new,
            text,
            spans,
        } => {
            let (sign, ink, wash) = match kind {
                LineKind::Added => ("+", paint(t.diff.added), Some(paint(t.diff.added))),
                LineKind::Removed => (
                    "\u{2212}",
                    paint(t.diff.removed),
                    Some(paint(t.diff.removed)),
                ),
                LineKind::Context => (" ", paint(t.text.dim), None),
            };
            let on = picked.is_some();
            let number = |n: &Option<u32>| {
                div()
                    .flex_none()
                    .w(NUMBER_WIDTH)
                    .pr_2()
                    // Right-aligned by layout, not `text_right`: a hover
                    // refinement replaces the element's whole text style,
                    // so the colour change below would take the alignment
                    // with it and the number would jump left.
                    .flex()
                    .justify_end()
                    .text_color(paint(if on { t.text.primary } else { t.text.dim }))
                    .when(on, |el| el.font_weight(FontWeight::MEDIUM))
                    .when(!on, |el| {
                        el.group_hover(ROW_GROUP, |style| style.text_color(paint(t.text.primary)))
                    })
                    .child(n.map(|n| n.to_string()).unwrap_or_default())
            };
            // The sign, and over it the "+" the pointer brings up, when the
            // caller gives one.
            let sign_cell = div()
                .relative()
                .flex()
                .flex_none()
                .w(SIGN_WIDTH)
                .h_full()
                .items_center()
                .justify_center()
                .text_color(ink)
                .child(
                    div()
                        .when(!on, |el| {
                            el.group_hover(ROW_GROUP, |style| style.opacity(0.0))
                        })
                        .child(sign),
                )
                .when_some(add, |el, add| {
                    el.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.0)
                            .group_hover(ROW_GROUP, |style| style.opacity(1.0))
                            .child(add),
                    )
                });
            let gutter = div()
                .relative()
                .cursor_pointer()
                .flex()
                .flex_none()
                .items_center()
                .w(GUTTER_WIDTH)
                .h_full()
                .map(|el| match picked {
                    Some((first, last)) => {
                        let el = el.bg(paint(t.selection));
                        match (first, last) {
                            (true, true) => el.rounded(RADIUS_SM),
                            (true, false) => el.rounded_t(RADIUS_SM),
                            (false, true) => el.rounded_b(RADIUS_SM),
                            (false, false) => el,
                        }
                    }
                    None => el
                        .rounded(RADIUS_SM)
                        .group_hover(ROW_GROUP, |style| style.bg(paint(t.hover))),
                })
                .when(marked, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(4.0))
                            .top_0()
                            .bottom_0()
                            .flex()
                            .items_center()
                            .child(status_dot(paint(t.accent), Motion::Still, px(6.0))),
                    )
                })
                .child(number(old))
                .child(number(new))
                .child(sign_cell);
            let highlights: Vec<(Range<usize>, HighlightStyle)> = spans
                .iter()
                .map(|(range, token)| (range.clone(), token_style(*token, t)))
                .collect();
            div()
                .id(("diff-row", index))
                .group(ROW_GROUP)
                .flex()
                .w_full()
                .items_center()
                .overflow_hidden()
                .when_some(wash, |el, wash| el.bg(alpha(wash, 0.10)))
                .when(on, |el| el.bg(alpha(paint(t.selection), 0.7)))
                .child(gutter)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl(px(6.0))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(paint(t.text.primary))
                        .child(StyledText::new(text.clone()).with_highlights(highlights)),
                )
        }
    }
}
