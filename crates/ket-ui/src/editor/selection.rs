//! Selecting with the pointer: a click places the caret, a double-click
//! takes a word, a triple-click a line, and a drag grows any of them.

use std::ops::Range;

use gpui::MouseDownEvent;

use ket_core::buffer::{Buffer, LineCol};

use crate::Shell;

/// How much of the document a mouse drag takes at a time — set by the click
/// that started it, the way a double-click-and-drag selects by whole words
/// everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Granularity {
    Character,
    Word,
    Line,
}

/// A selection currently being dragged out with the mouse.
#[derive(Debug, Clone)]
pub(super) struct Drag {
    pub(super) granularity: Granularity,
    /// What the click that started the drag selected, kept whole rather than
    /// as a single anchor position: dragging back past it has to leave that
    /// first word or line selected in its entirety, which needs both ends.
    pub(super) anchor: Range<usize>,
}

/// How a character counts when a double-click decides what a word is.
/// Identifiers, runs of punctuation and runs of whitespace each select as a
/// unit — the classes every editor splits on.
fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        2
    } else if c.is_whitespace() {
        0
    } else {
        1
    }
}

/// The run of same-class characters around `at`, for a double-click.
fn word_at(buffer: &Buffer, at: usize) -> Range<usize> {
    let line = buffer.line_col_at(at).line;
    let line_start = buffer.char_at_line_col(LineCol { line, column: 1 });
    let chars: Vec<char> = buffer.line_text(line as usize - 1).chars().collect();
    if chars.is_empty() {
        return at..at;
    }
    // A click past the last character takes that character's run rather than
    // nothing, so double-clicking the blank space after a word still selects
    // the word.
    let pivot = at.saturating_sub(line_start).min(chars.len() - 1);
    let class = char_class(chars[pivot]);
    let mut start = pivot;
    while start > 0 && char_class(chars[start - 1]) == class {
        start -= 1;
    }
    let mut end = pivot + 1;
    while end < chars.len() && char_class(chars[end]) == class {
        end += 1;
    }
    line_start + start..line_start + end
}

/// The text of the line containing `at`, without the line break that ends
/// it, for a triple-click: the caret lands at the end of the line it
/// selected rather than at the start of the next one, and the selection
/// stops at the last character instead of running a cell past it.
fn line_text_range(buffer: &Buffer, at: usize) -> Range<usize> {
    let line = buffer.line_col_at(at).line;
    let start = buffer.char_at_line_col(LineCol { line, column: 1 });
    let end = buffer.char_at_line_col(LineCol {
        line,
        column: u32::MAX,
    });
    start..end
}

/// The whole line containing `at`, including the line break that ends it,
/// for deleting a line outright.
pub(super) fn line_range(buffer: &Buffer, at: usize) -> Range<usize> {
    let line = buffer.line_col_at(at).line;
    let start = buffer.char_at_line_col(LineCol { line, column: 1 });
    let next = buffer.char_at_line_col(LineCol {
        line: line + 1,
        column: 1,
    });
    // `char_at_line_col` clamps a line past the end back onto the last one,
    // so a start that did not move means there is no next line to run to.
    let end = if next > start {
        next
    } else {
        buffer.char_at_line_col(LineCol {
            line,
            column: u32::MAX,
        })
    };
    start..end
}

impl Shell {
    /// A press at `at`, the document position `RowText` worked out from
    /// its own shaping: places the cursor there, or starts a selection at
    /// whatever granularity the click count asks for.
    pub(super) fn editor_mouse_down(&mut self, key: &str, at: usize, event: &MouseDownEvent) {
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        match event.click_count {
            1 if event.modifiers.shift => {
                // Shift-click extends what is already selected, so the fixed
                // end of the existing selection stays put.
                let anchor = state.buffer.anchor();
                state.buffer.set_selection(anchor, at);
                state.drag = Some(Drag {
                    granularity: Granularity::Character,
                    anchor: anchor..anchor,
                });
            }
            1 => {
                state.buffer.set_cursor(at);
                state.drag = Some(Drag {
                    granularity: Granularity::Character,
                    anchor: at..at,
                });
            }
            2 => {
                let word = word_at(&state.buffer, at);
                state.buffer.set_selection(word.start, word.end);
                state.drag = Some(Drag {
                    granularity: Granularity::Word,
                    anchor: word,
                });
            }
            _ => {
                let line = line_text_range(&state.buffer, at);
                state.buffer.set_selection(line.start, line.end);
                state.drag = Some(Drag {
                    granularity: Granularity::Line,
                    anchor: line,
                });
            }
        }
    }

    /// Extends a drag-selection to the pointer, answering whether anything
    /// actually moved — a pointer travelling across one character sends many
    /// events, and only the ones that change the selection are worth a
    /// repaint.
    pub(super) fn editor_drag(&mut self, key: &str, at: usize) -> bool {
        let Some(state) = self.editors.get_mut(key) else {
            return false;
        };
        let Some(drag) = state.drag.clone() else {
            return false;
        };
        // Past the anchor in either direction the whole anchor word or line
        // stays in, so the selection grows outward from it rather than
        // collapsing into it.
        let (anchor, cursor) = match drag.granularity {
            Granularity::Character => (drag.anchor.start, at),
            Granularity::Word => {
                let word = word_at(&state.buffer, at);
                if word.start < drag.anchor.start {
                    (drag.anchor.end, word.start)
                } else {
                    (drag.anchor.start, word.end)
                }
            }
            Granularity::Line => {
                let line = line_text_range(&state.buffer, at);
                if line.start < drag.anchor.start {
                    (drag.anchor.end, line.start)
                } else {
                    (drag.anchor.start, line.end)
                }
            }
        };
        if (state.buffer.anchor(), state.buffer.cursor()) == (anchor, cursor) {
            return false;
        }
        state.buffer.set_selection(anchor, cursor);
        true
    }

    /// Ends a drag, wherever the button came up — including outside the
    /// pane, which is why the release is watched for in two places. Answers
    /// whether there was one, so an ordinary click does not cost a repaint.
    pub(super) fn end_editor_drag(&mut self, key: &str) -> bool {
        self.editors
            .get_mut(key)
            .is_some_and(|state| state.drag.take().is_some())
    }
}
