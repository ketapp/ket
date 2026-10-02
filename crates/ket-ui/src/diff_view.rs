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
//! **Picking lines.** A click picks a line, ⇧-click runs the pick to another,
//! and a click on a hunk's header picks the whole hunk — to copy with ⌘C, or
//! to take into a backlog note with ⇧⌘A (see `crate::backlog`). Escape lets
//! go of it.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, ClipboardItem, Context, FontStyle, FontWeight, HighlightStyle, KeyDownEvent,
    MouseButton, MouseDownEvent, ScrollStrategy, SharedString, StyledText, UniformListScrollHandle,
    Window, div, prelude::*, px, relative, uniform_list,
};
use ket_core::diff::{FileDiff, LineKind};
use ket_core::status::ChangeKind;
use ket_core::syntax::{self, Token};
use ket_core::theme::Theme;

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::tabs::{Tab, TabKind};
use crate::ui::button::{button, icon_button};
use crate::ui::filetype::file_mark;
use crate::ui::icon::Icon;

/// Columns a tab is drawn as. The highlighter and the row both count
/// characters, and a literal tab would be one character drawn as none.
const TAB_SPACES: &str = "    ";

/// Width of each line-number gutter.
const NUMBER_WIDTH: gpui::Pixels = px(44.0);

/// Width of the `+`/`-` column.
const SIGN_WIDTH: gpui::Pixels = px(16.0);

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
#[derive(Default)]
pub(crate) struct DiffState {
    read: Option<DiffRead>,
    error: Option<SharedString>,
    reading: bool,
    scroll: UniformListScrollHandle,
    /// The hunk F7 last moved to.
    cursor: Option<usize>,
    /// The lines picked, as the row a pick started on and the row it runs
    /// to: either way round.
    picked: Option<(usize, usize)>,
}

/// All open diff tabs' states, by the comparison they show.
pub(crate) type Diffs = HashMap<DiffKey, DiffState>;

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
    pub(crate) fn open_diff_tab(&mut self, key: DiffKey, name: String, cx: &mut Context<Self>) {
        let Some(worktree_id) = self.selected_id() else {
            return;
        };
        // A tab opened again reads again: whoever clicked the row wants what
        // the file is now, not what it was when the tab first opened.
        self.load_diff(&key, cx);
        self.spaces.entry(worktree_id).or_default().open_tab(Tab {
            title: format!("{name} (diff)").into(),
            kind: TabKind::Diff(key),
            renamed: false,
            pinned: false,
        });
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
    fn active_diff(&self) -> Option<DiffKey> {
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

    /// F7 and Shift-F7, while a diff is the tab in front; ⌘C and Escape
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
            }
            cx.notify();
            return true;
        }
        if picked && event.keystroke.key == "c" && (keys.platform || keys.control) {
            if let Some((text, _)) = self.diff_selection_of(&key) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            return true;
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

    /// A press on row `index`: picks the line, runs the pick to it with
    /// `extend`, or — on a hunk's header — picks the hunk. A press on the one
    /// line already picked lets go of it.
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
                Some(only) if only == (index, index) => None,
                _ => Some((index, index)),
            }
        };
    }

    /// The lines picked in the diff tab in front, if any — see
    /// [`Shell::diff_selection_of`] — with its comparison.
    pub(crate) fn diff_selection(&self) -> Option<(DiffKey, String, String)> {
        let key = self.active_diff()?;
        let (text, lines) = self.diff_selection_of(&key)?;
        Some((key, text, lines))
    }

    /// The lines picked in `key`'s tab, each with its `+`, `-` or space as a
    /// patch writes them, and which lines they are: `lines 40–44` of the new
    /// file, or of the old one where only removals were picked.
    fn diff_selection_of(&self, key: &DiffKey) -> Option<(String, String)> {
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
        let numbers = if new_lines.is_empty() {
            &old_lines
        } else {
            &new_lines
        };
        let lines = match (numbers.first(), numbers.last()) {
            (Some(first), Some(last)) if first == last => format!("line {first}"),
            (Some(first), Some(last)) => format!("lines {first}\u{2013}{last}"),
            _ => String::new(),
        };
        Some((text.join("\n"), lines))
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
        state
            .scroll
            .scroll_to_item(read.hunks[next], ScrollStrategy::Top);
        cx.notify();
    }

    /// The tab's content.
    pub(crate) fn diff_pane(
        &mut self,
        key: &DiffKey,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        if !self.diffs.contains_key(key) {
            self.load_diff(key, cx);
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
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .h(px(34.0))
            .pl_3()
            .pr_2()
            .border_b_1()
            .border_color(paint(t.rule))
            .text_size(px(11.0))
            .child(file_mark(&name, false, self.font_family.clone(), &t))
            .child(
                div()
                    .flex_none()
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
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(paint(t.text.dim))
                    .child(versus),
            )
            .children(counts)
            .child(
                icon_button("diff-previous-change", Icon::ChevronUp)
                    .bare()
                    .dense()
                    .render(&t)
                    .when(!has_changes, |el| el.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.step_diff(&prev_key, -1, cx);
                    })),
            )
            .child(
                icon_button("diff-next-change", Icon::ChevronDown)
                    .bare()
                    .dense()
                    .render(&t)
                    .when(!has_changes, |el| el.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.step_diff(&next_key, 1, cx);
                    })),
            )
            // A deleted file has nothing on disk to open.
            .when(!deleted, |el| {
                el.child(
                    button("diff-open-file", "Open file")
                        .ghost()
                        .leading(Icon::File)
                        .render(&t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_editor_tab(open_path.clone(), cx);
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

    /// The rows, virtualised.
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
        let font_size = px(f32::from(self.editor_type.font_size));
        let picked = state
            .picked
            .map(|(anchor, head)| anchor.min(head)..=anchor.max(head));
        let owner = key.clone();
        let list = uniform_list(
            SharedString::from(format!("diff-lines-{}", key.rel)),
            rows.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let t = this.theme;
                range
                    .filter_map(|index| {
                        let row = rows.get(index)?;
                        let on = picked
                            .as_ref()
                            .is_some_and(|picked| picked.contains(&index));
                        let key = owner.clone();
                        Some(
                            diff_row(index, row, on, &t)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.pick_diff_row(&key, index, event.modifiers.shift);
                                        cx.notify();
                                    }),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(state.scroll.clone())
        .font_family(self.font_family.clone())
        .text_size(font_size)
        .line_height(relative(self.editor_type.line_height))
        .flex_grow()
        .size_full();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
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

/// One row of the list, washed in the selection's colour while `picked`.
fn diff_row(index: usize, row: &DiffRow, picked: bool, t: &Theme) -> gpui::Stateful<gpui::Div> {
    match row {
        DiffRow::Hunk(header) => div()
            .id(("diff-row", index))
            .flex()
            .w_full()
            .items_center()
            .bg(paint(t.hover))
            .text_color(alpha(paint(t.diff.hunk_header), 0.85))
            .child(div().flex_none().w(NUMBER_WIDTH * 2.0 + SIGN_WIDTH))
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
            let number = |n: &Option<u32>| {
                div()
                    .flex_none()
                    .w(NUMBER_WIDTH)
                    .pr_2()
                    .text_right()
                    .text_color(paint(t.text.dim))
                    .child(n.map(|n| n.to_string()).unwrap_or_default())
            };
            let highlights: Vec<(Range<usize>, HighlightStyle)> = spans
                .iter()
                .map(|(range, token)| (range.clone(), token_style(*token, t)))
                .collect();
            div()
                .id(("diff-row", index))
                .flex()
                .w_full()
                .overflow_hidden()
                .when_some(wash, |el, wash| el.bg(alpha(wash, 0.10)))
                .when(picked, |el| el.bg(paint(t.selection)))
                .child(number(old))
                .child(number(new))
                .child(div().flex_none().w(SIGN_WIDTH).text_color(ink).child(sign))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_color(paint(t.text.primary))
                        .child(StyledText::new(text.clone()).with_highlights(highlights)),
                )
        }
    }
}
