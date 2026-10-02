//! The editor pane: the rows `uniform_list` asks for, and everything
//! around them.

use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    AnyElement, Context, Font, FontFeatures, FontStyle, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseUpEvent, SharedString, Window, canvas, div, prelude::*, px, relative,
    uniform_list,
};

use ket_core::buffer::LineCol;
use ket_core::diff::LineMark;

use crate::Shell;
use crate::paint::paint;
use crate::tabs::PaneId;

use super::CARET_WIDTH;
use super::geometry::{Geometry, Measure, code_cell_width};
use super::row::{RowView, render_editor_row};
use super::selection::{Drag, Granularity};
use super::state::EditorState;

/// A composer's line height, as a multiple of [`COMPOSER_TEXT`]. Looser
/// than code's, since a paragraph of prose is read line after line.
///
/// [`COMPOSER_TEXT`]: super::COMPOSER_TEXT
const COMPOSER_LINE_HEIGHT: f32 = 1.55;

/// The narrowest wrap the pane will use, so dragging a panel until the
/// editor is a sliver does not turn every line into a column of single
/// characters.
const MIN_COLUMNS: usize = 16;

impl Shell {
    /// The rows `uniform_list` asks for: only the visible range ever runs
    /// through [`Buffer::line_text`], never the whole document.
    ///
    /// [`Buffer::line_text`]: ket_core::buffer::Buffer::line_text
    fn editor_rows(
        &mut self,
        key: &str,
        pane: Option<PaneId>,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = self.theme;
        // Read before the mutable borrow: the query lives in an entity, which
        // is the app's rather than the editor state's.
        let searching = self
            .editors
            .get(key)
            .is_some_and(|state| state.find.open && !state.find.text(cx).is_empty());
        // The blink is the shell's, shared with every other caret in the
        // window; see `Shell::editor_active` for what keeps it running here.
        let caret_on = self.caret.visible;
        let Some(state) = self.editors.get_mut(key) else {
            return Vec::new();
        };
        let caret_on = caret_on && state.caret;
        state.visible_rows = range.clone();
        // Before the rows are built, because a row is coloured from what the
        // rows above it left open and this is the only thing that knows.
        state.syntax.ensure(&state.buffer, state.language);
        let language = state.language;

        let wrap = state.wrap;
        let geometry = state.geometry;
        // Cloned once per batch of rows rather than read per row: the marks
        // are shared, so this is a refcount either way.
        let git = state.git.clone();
        let numbered = state.numbered();
        let composer = state.composer;
        let gutter_width = state.gutter_width();
        let cursor = state.buffer.cursor();
        let cursor_line = state.buffer.line_col_at(cursor).line as usize - 1;
        let line_count = state.buffer.line_count();
        let selection = state
            .buffer
            .has_selection()
            .then(|| state.buffer.selection());
        let query_active = searching;

        let shell = cx.entity().downgrade();
        let row_key: SharedString = key.to_owned().into();

        let mut rows = Vec::with_capacity(range.len());
        for display_row in range {
            let (buffer_line, start_col) = if wrap {
                match state.line_index.wrap_rows.get(display_row) {
                    Some(&(line, col)) => (line as usize, col as usize),
                    None => continue,
                }
            } else {
                (display_row, 0)
            };

            let text = state.buffer.line_text(buffer_line);
            let chars: Vec<char> = text.chars().collect();
            // Segments are as long as word wrap made them, so where this one
            // ends is where the next one of the same line begins.
            let seg_end = match state.line_index.wrap_rows.get(display_row + 1) {
                Some(&(next_line, next_col)) if wrap && next_line as usize == buffer_line => {
                    (next_col as usize).min(chars.len())
                }
                _ => chars.len(),
            };
            let last_segment = seg_end >= chars.len();
            let seg_len = seg_end.saturating_sub(start_col);
            let seg_text: String = chars[start_col..seg_end].iter().collect();

            let line_start = state.buffer.char_at_line_col(LineCol {
                line: (buffer_line + 1) as u32,
                column: 1,
            });
            let seg_start = line_start + start_col;
            let seg_stop = seg_start + seg_len;

            let local_selection = selection.clone().and_then(|sel| {
                (sel.start < seg_stop && sel.end > seg_start).then(|| {
                    sel.start.max(seg_start) - seg_start..sel.end.min(seg_stop) - seg_start
                })
            });

            // The whole line was lexed once, up front, in `SyntaxIndex::ensure`
            // — clipped to this segment here the same way the selection and
            // the find matches are, since a wrapped line is one line to the
            // highlighter and several rows to the reader.
            let local_syntax: Vec<(Range<usize>, ket_core::syntax::Token)> = if language.is_some() {
                state
                    .syntax
                    .at(buffer_line)
                    .iter()
                    .filter(|(range, _)| range.start < seg_end && range.end > start_col)
                    .map(|(range, token)| {
                        (
                            range.start.max(start_col) - start_col
                                ..range.end.min(seg_end) - start_col,
                            *token,
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };

            let local_matches: Vec<(Range<usize>, bool)> = if query_active {
                state
                    .find
                    .matches
                    .ranges
                    .iter()
                    .filter(|m| m.start < seg_stop && m.end > seg_start)
                    .map(|m| {
                        let local =
                            m.start.max(seg_start) - seg_start..m.end.min(seg_stop) - seg_start;
                        let is_current = local_selection.as_ref() == Some(&local);
                        (local, is_current)
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // A cursor sitting exactly on a wrap point belongs to one row,
            // not both: the segment that starts there, unless there is none.
            let on_this_row = buffer_line == cursor_line
                && cursor >= seg_start
                && (cursor < seg_stop || (last_segment && cursor == seg_stop));
            let local_cursor = (on_this_row && caret_on).then(|| cursor - seg_start);

            let selection_runs_on = last_segment
                && buffer_line + 1 < line_count
                && selection
                    .as_ref()
                    .is_some_and(|sel| sel.start <= seg_stop && sel.end > seg_stop);

            rows.push(render_editor_row(
                RowView {
                    theme: t,
                    geometry,
                    gutter_width,
                    display_row,
                    buffer_line,
                    start: seg_start,
                    text: &seg_text,
                    selection: local_selection,
                    selection_runs_on,
                    matches: &local_matches,
                    syntax: &local_syntax,
                    cursor: local_cursor,
                    current_line: !composer && buffer_line == cursor_line,
                    // Only a line's first segment is numbered; a wrapped
                    // continuation is the same line still.
                    show_number: numbered && start_col == 0,
                    // A changed line is marked down every row it wraps over,
                    // so the bar is as tall as the line looks. A removal is
                    // not: it marks one boundary, and there is only one.
                    git: match (git.at(buffer_line), start_col == 0) {
                        (Some(LineMark::RemovedAbove | LineMark::RemovedBelow), false) => None,
                        (mark, _) => mark,
                    },
                },
                &shell,
                &row_key,
                pane,
            ));
        }
        rows
    }

    /// The editor pane for `key` — the render hook `main.rs` calls when the
    /// active tab is [`TabKind::Editor`]. `pane` is the tab's pane, which a
    /// press on a row focuses; the quick prompt and the snippet body have
    /// none.
    ///
    /// [`TabKind::Editor`]: crate::tabs::TabKind::Editor
    pub(crate) fn editor_pane(
        &mut self,
        key: &str,
        pane: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        if !self.editors.contains_key(key) && !key.starts_with("untitled:") {
            let path = PathBuf::from(key);
            self.editors
                .insert(key.to_owned().into(), EditorState::open(&path));
        }
        let Some(state) = self.editors.get(key) else {
            return div().into_any_element();
        };

        if let Some(err) = state.load_error.clone() {
            return div()
                .flex()
                .size_full()
                .justify_center()
                .items_center()
                .text_color(paint(t.text.dim))
                .child(err)
                .into_any_element();
        }

        let composer = state.composer;
        let (font_family, font_size, line_height) = if composer {
            (
                crate::fonts::prose(),
                state.composer_size,
                COMPOSER_LINE_HEIGHT,
            )
        } else {
            (
                self.font_family.clone(),
                px(f32::from(self.editor_type.font_size)),
                self.editor_type.line_height,
            )
        };
        let cell_width = code_cell_width(window, &font_family, font_size);
        let measure = composer.then(|| {
            let text_system = window.text_system().clone();
            let font = text_system.resolve_font(&Font {
                family: font_family.clone(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            });
            Measure {
                text_system,
                font,
                size: font_size,
            }
        });
        let Some(state) = self.editors.get_mut(key) else {
            return div().into_any_element();
        };
        state.geometry.cell_width = cell_width;
        state.measure = measure;
        let gutter_width = state.gutter_width();
        state.ensure_lines();
        let wrap = state.wrap;
        let row_count = if wrap {
            state.line_index.wrap_rows.len()
        } else {
            state.buffer.line_count()
        };
        let scroll = state.scroll.clone();

        let editor_key: SharedString = key.to_owned().into();
        let list_key = editor_key.clone();
        let list = uniform_list(
            "editor-lines",
            row_count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                this.editor_rows(list_key.as_ref(), pane, range, cx)
            }),
        )
        .track_scroll(scroll)
        .font_family(font_family)
        // The same code face as the terminal, and the same size as it until
        // somebody says otherwise in Settings › Editor: a file and a shell
        // looking like one machine is the default, not a rule.
        .text_size(font_size)
        // Set on the list rather than on the rows so that the gutter, the
        // shaped text and `uniform_list`'s own row measurement all read the
        // same number out of the window's text style.
        .line_height(relative(line_height))
        .flex_grow()
        .size_full();

        // Measuring how wide the rows were painted, so the next frame wraps
        // at the pane's real width rather than at the fallback.
        let measured_key = editor_key.clone();
        let shell = cx.entity().downgrade();
        let probe = canvas(
            move |bounds, _, cx| {
                let _ = shell.update(cx, |this, cx| {
                    let text_width = f32::from(bounds.size.width - gutter_width).max(0.0);
                    let columns = (text_width / f32::from(cell_width).max(1.0)).floor() as usize;
                    let next = Geometry {
                        cell_width,
                        columns: columns.max(MIN_COLUMNS),
                        // Less the caret, so a caret parked after a full
                        // row's last character is still drawn inside it.
                        text_width: px(text_width) - CARET_WIDTH,
                    };
                    if let Some(state) = this.editors.get_mut(measured_key.as_ref())
                        && state.geometry != next
                    {
                        state.geometry = next;
                        // The row count this frame was built with is now
                        // stale, so ask for one more frame — and only when
                        // the measurement actually moved, or this would
                        // repaint forever.
                        cx.notify();
                    }
                });
            },
            |_, _: (), _, _| {},
        )
        .absolute()
        .size_full();

        let empty_key = editor_key.clone();
        let release_key = editor_key.clone();
        let leave_key = editor_key.clone();
        let text_area = div()
            .relative()
            .flex()
            .flex_grow()
            .overflow_hidden()
            .cursor_text()
            .child(list)
            .child(probe)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    // Only ever reached past the last row — a row stops the
                    // event itself. Clicking the empty space below a document
                    // puts the cursor at its end, as every editor does.
                    window.focus(&this.focus);
                    if let Some(state) = this.editors.get_mut(empty_key.as_ref()) {
                        let end = state.buffer.len_chars();
                        state.buffer.set_cursor(end);
                        state.drag = Some(Drag {
                            granularity: Granularity::Character,
                            anchor: end..end,
                        });
                    }
                    this.caret_wake(cx);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    if this.end_editor_drag(release_key.as_ref()) {
                        cx.notify();
                    }
                }),
            )
            // A drag that ends with the pointer outside the pane still ends.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, _| {
                    this.end_editor_drag(leave_key.as_ref());
                }),
            );

        let find = self.find_bar(editor_key.as_ref(), window, cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .children(find)
            .child(text_area)
            .into_any_element()
    }
}
