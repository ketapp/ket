//! The line index: where soft wrap breaks every line in the document,
//! kept current one edit at a time. See the parent module's "On soft
//! wrap" section.

use std::ops::Range;

use ket_core::buffer::{Buffer, LineCol, LineEdit};

use super::geometry::WrapWidth;

/// A whole-document summary of where soft wrap breaks each line, kept up to
/// date one edit at a time. See the `editor` module doc's "On soft wrap"
/// section.
#[derive(Default)]
pub(super) struct LineIndex {
    /// The line count this was built for.
    line_count: usize,
    /// The [`Buffer::revision`] this was built for; `None` before the first
    /// build, which is what makes that first `ensure` build.
    revision: Option<u64>,
    /// The wrap width this was built for — see [`WrapWidth::key`].
    width: (usize, u32),
    /// `(buffer_line, start_column)` for every soft-wrapped display row, in
    /// document order. Used only while wrap is on.
    pub(super) wrap_rows: Vec<(u32, u32)>,
}

/// Grows `dirty` — lines to redo, in the line numbers of the text before
/// `edit` — to cover what `edit` touched, in the line numbers after it.
///
/// What lets several edits since the last frame be redone as one run: each
/// moves the lines already waiting below it along with everything else, and
/// adds its own.
pub(super) fn grow_dirty(dirty: Option<Range<usize>>, edit: &LineEdit) -> Range<usize> {
    let replaced_end = edit.start + edit.removed;
    let inserted_end = edit.start + edit.inserted;
    // Where a line boundary before the edit sits after it: untouched above,
    // moved along below, and pulled to the end of the new lines when it fell
    // inside the ones the edit replaced.
    let moved = |line: usize| {
        if line >= replaced_end {
            line - edit.removed + edit.inserted
        } else if line > edit.start {
            inserted_end
        } else {
            line
        }
    };
    match dirty {
        None => edit.start..inserted_end,
        Some(dirty) => moved(dirty.start).min(edit.start)..moved(dirty.end).max(inserted_end),
    }
}

/// Where the segment starting at `start` ends: just after the last space
/// before `limit`, or at `limit` when a single word is wider than the pane
/// and has to be cut. Always past `start`, which is what makes the wrapping
/// loop terminate.
fn break_after(chars: &[char], start: usize, limit: usize) -> usize {
    (start..limit)
        .rev()
        .find(|&i| chars[i].is_whitespace())
        .map(|i| i + 1)
        .unwrap_or(limit)
}

/// The character length of `line`, without copying its text — two rope
/// position lookups rather than [`Buffer::line_text`], since [`LineIndex`]
/// scans every line in the document and a `String` per line would be
/// exactly the "pulling the whole file" cost `Buffer::line_text` exists to
/// let a renderer avoid.
pub(super) fn line_char_count(buffer: &Buffer, line: usize) -> usize {
    let line_no = (line + 1) as u32;
    let start = buffer.char_at_line_col(LineCol {
        line: line_no,
        column: 1,
    });
    let end = buffer.char_at_line_col(LineCol {
        line: line_no,
        column: u32::MAX,
    });
    end - start
}

/// Appends `line`'s display rows — one per soft-wrapped segment — to `out`.
///
/// A line that already fits needs only its length, and its length is two
/// rope lookups rather than a copy of its text. Only a line long enough to
/// actually wrap is read. A proportional width has no length to compare, so
/// every line is read and measured; that is only ever a prompt's draft — see
/// the `editor` module doc.
fn push_segments(buffer: &Buffer, line: usize, width: WrapWidth<'_>, out: &mut Vec<(u32, u32)>) {
    if let WrapWidth::Columns(columns) = width
        && line_char_count(buffer, line) <= columns
    {
        out.push((line as u32, 0));
        return;
    }
    let text: Vec<char> = buffer.line_text(line).chars().collect();
    let mut col = 0usize;
    loop {
        out.push((line as u32, col as u32));
        let limit = width.limit(&text, col);
        if limit >= text.len() {
            break;
        }
        col = break_after(&text, col, limit);
    }
}

impl LineIndex {
    /// Brings the index up to date with the buffer and the wrap width, and
    /// does nothing at all when both are where they were — the usual case,
    /// since this is asked on every frame and most frames change neither.
    ///
    /// An edit redoes only the lines it touched, and moves the rows below it
    /// along. A new width changes every line, so that, the first build, and a
    /// buffer whose edits [`Buffer::line_edits_since`] cannot account for are
    /// a pass over the whole document.
    pub(super) fn ensure(&mut self, buffer: &Buffer, width: WrapWidth<'_>) {
        let key = width.key();
        let revision = buffer.revision();
        if self.revision == Some(revision) && self.width == key {
            return;
        }
        let edits = self
            .revision
            .filter(|_| self.width == key)
            .and_then(|built| buffer.line_edits_since(built));
        self.revision = Some(revision);
        self.width = key;
        self.line_count = buffer.line_count();

        if !edits.is_some_and(|edits| self.patch(buffer, width, &edits)) {
            self.wrap_rows = Vec::with_capacity(self.line_count);
            for line in 0..self.line_count {
                push_segments(buffer, line, width, &mut self.wrap_rows);
            }
        }
    }

    /// Redoes the rows of the lines `edits` touched. Answers `false` when the
    /// result does not end on the buffer's last line, which leaves the index
    /// for [`LineIndex::ensure`] to rebuild whole.
    fn patch(&mut self, buffer: &Buffer, width: WrapWidth<'_>, edits: &[LineEdit]) -> bool {
        let mut dirty = None;
        for edit in edits {
            let from = self.wrap_row_for_line(edit.start);
            let to = self.wrap_row_for_line(edit.start + edit.removed);
            if edit.inserted != edit.removed {
                for row in &mut self.wrap_rows[to..] {
                    row.0 = (row.0 as usize - edit.removed + edit.inserted) as u32;
                }
            }
            // One placeholder row per new line; each is in `dirty`, and
            // redone below.
            self.wrap_rows.splice(
                from..to,
                (edit.start..edit.start + edit.inserted).map(|line| (line as u32, 0)),
            );
            dirty = Some(grow_dirty(dirty, edit));
        }
        if let Some(dirty) = dirty {
            let end = dirty.end.min(self.line_count);
            let mut fresh = Vec::new();
            for line in dirty.start..end {
                push_segments(buffer, line, width, &mut fresh);
            }
            let from = self.wrap_row_for_line(dirty.start);
            let to = self.wrap_row_for_line(end);
            self.wrap_rows.splice(from..to, fresh);
        }
        self.wrap_rows
            .last()
            .is_some_and(|&(line, _)| line as usize + 1 == self.line_count)
    }

    /// The display row showing `buffer_line`'s first soft-wrapped segment.
    fn wrap_row_for_line(&self, buffer_line: usize) -> usize {
        self.wrap_rows
            .partition_point(|(line, _)| (*line as usize) < buffer_line)
    }

    /// The display row containing `column` (characters from the line
    /// start) of `buffer_line` — the exact wrapped segment, not just the
    /// line's first one, so scrolling a cursor into view lands on the row
    /// that actually shows it.
    pub(super) fn wrap_row_for(&self, buffer_line: usize, column: usize) -> usize {
        let start = self.wrap_row_for_line(buffer_line);
        let mut best = start;
        for (offset, &(line, col)) in self.wrap_rows[start..].iter().enumerate() {
            if line as usize != buffer_line {
                break;
            }
            if col as usize <= column {
                best = start + offset;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ket_core::buffer::Buffer;

    #[test]
    fn line_index_rebuilds_when_an_edit_changes_no_line_count() {
        let buffer = Buffer::from_text("one\ntwo\nthree\n");
        let mut index = LineIndex::default();
        index.ensure(&buffer, WrapWidth::Columns(10));
        assert_eq!(index.line_count, 4); // trailing newline makes a 4th, empty line
        assert_eq!(index.wrap_rows, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);

        // The line count has not moved, but line 0 now wraps: rebuilding on
        // that alone would leave the pane showing a line off its right edge.
        let mut edited = buffer.clone();
        edited.set_cursor(0);
        edited.insert("a much longer first line");
        index.ensure(&edited, WrapWidth::Columns(10));
        assert!(
            index
                .wrap_rows
                .iter()
                .filter(|(line, _)| *line == 0)
                .count()
                > 1
        );
    }

    #[test]
    fn line_index_breaks_a_long_line_after_the_last_space_that_fits() {
        let buffer = Buffer::from_text("the quick brown fox jumps over");
        let mut index = LineIndex::default();
        index.ensure(&buffer, WrapWidth::Columns(12));
        // "the quick ", "brown fox ", "jumps over" — no word cut in half.
        assert_eq!(index.wrap_rows, vec![(0, 0), (0, 10), (0, 20)]);
    }

    #[test]
    fn wrap_row_for_finds_the_segment_containing_a_column() {
        // One word wider than the pane, so it is cut at the wrap width.
        let buffer = Buffer::from_text(&"x".repeat(25));
        let mut index = LineIndex::default();
        index.ensure(&buffer, WrapWidth::Columns(10));
        assert_eq!(index.wrap_rows, vec![(0, 0), (0, 10), (0, 20)]);

        assert_eq!(index.wrap_row_for(0, 0), 0);
        assert_eq!(index.wrap_row_for(0, 10), 1);
        assert_eq!(index.wrap_row_for(0, 13), 1);
        assert_eq!(index.wrap_row_for(0, 21), 2);
    }

    #[test]
    fn wrap_row_for_line_finds_a_later_lines_first_segment() {
        let buffer = Buffer::from_text("a\nb\nc\n");
        let mut index = LineIndex::default();
        index.ensure(&buffer, WrapWidth::Columns(40));
        assert_eq!(index.wrap_row_for_line(2), 2);
    }
}
