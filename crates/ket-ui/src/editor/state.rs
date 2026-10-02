//! Everything about one open file beyond what [`Buffer`] itself tracks.
//! See the parent module's "Where the buffer lives" section for why
//! this is keyed rather than held by the tab.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Pixels, SharedString, UniformListScrollHandle, px};

use ket_core::buffer::{Buffer, FindMatches, LineCol};
use ket_core::diff::LineMarks;
use ket_core::surface::FileStamp;

use super::find::Find;
use super::geometry::{Geometry, Measure, WrapWidth};
use super::line_index::{LineIndex, line_char_count};
use super::selection::Drag;
use super::syntax::SyntaxIndex;
use super::{COMPOSER_TEXT, GUTTER_WIDTH, PLAIN_MARGIN};

/// Extensions the pane reads as prose rather than code.
///
/// A denylist rather than a list of the languages that *are* code, because
/// the pane opens whatever a worktree holds and an unknown extension is far
/// more often a language nobody listed than it is a document. Files with no
/// extension at all — `Makefile`, `Dockerfile`, `.gitignore` — fall on the
/// code side for the same reason.
const PROSE_EXTENSIONS: [&str; 3] = ["txt", "md", "markdown"];

/// Whether `path` names one of [`PROSE_EXTENSIONS`].
fn is_prose(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PROSE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// Everything about one open file beyond what [`Buffer`] itself tracks. See
/// the `editor` module doc's "Where the buffer lives" section for why this is
/// keyed by stable identity on [`Shell::editors`] rather than living on the
/// tab.
///
/// [`Shell::editors`]: crate::Shell::editors
pub(crate) struct EditorState {
    /// The file's contents, cursor, selection and undo history.
    pub(super) buffer: Buffer,
    /// The destination after the first successful Save As; absent for Untitled.
    pub(super) path: Option<PathBuf>,
    /// Initial directory for an Untitled buffer's native Save panel.
    pub(super) save_directory: PathBuf,
    /// The version of the file the buffer was read from or last written to,
    /// for [`Self::refresh_from_disk`] to compare disk against. Absent for an
    /// Untitled buffer, and for a file that could not be read.
    pub(super) stamp: Option<FileStamp>,
    /// Set instead of loading a buffer when the file could not be read, so
    /// the pane says why rather than pretending an empty scratch buffer is
    /// the file — which would silently truncate it on the first save.
    pub(super) load_error: Option<SharedString>,
    /// Scroll position, persisted across tab switches.
    pub(super) scroll: UniformListScrollHandle,
    /// Whether long lines are soft-wrapped. See the `editor` module doc.
    pub(super) wrap: bool,
    /// Whether this is a prompt being written rather than a file: set in the
    /// prose face at [`COMPOSER_TEXT`], wrapped by real advances, and with no
    /// wash behind the caret's line, which is a code-reading aid.
    pub(super) composer: bool,
    /// Whether the caret is drawn. Off for a composer that sits beside a
    /// field holding the keyboard: the shell's focus feeds it keys, so gpui
    /// cannot say it is not the one being typed in — see
    /// [`Shell::set_composer_caret`].
    ///
    /// [`Shell::set_composer_caret`]: crate::Shell::set_composer_caret
    pub(super) caret: bool,
    /// A composer's text size: [`COMPOSER_TEXT`] unless its surface sets
    /// another with [`Shell::set_composer_size`].
    ///
    /// [`Shell::set_composer_size`]: crate::Shell::set_composer_size
    pub(super) composer_size: Pixels,
    /// The composer's face, measured for wrapping. Refreshed each frame the
    /// pane is drawn; `None` for every code buffer.
    pub(super) measure: Option<Measure>,
    pub(super) find: Find,
    pub(super) line_index: LineIndex,
    /// What language the file is in, if ket colours it. Resolved from the path
    /// once rather than per row.
    pub(super) language: Option<&'static ket_core::syntax::Language>,
    /// Where every line starts, for the highlighter.
    pub(super) syntax: SyntaxIndex,
    /// The display rows last handed to `uniform_list`'s render callback, so
    /// a cursor move can tell whether it already scrolled into view without
    /// asking `gpui` for pixel bounds.
    pub(super) visible_rows: Range<usize>,
    /// Where the text was last painted and how wide a character is there.
    pub(super) geometry: Geometry,
    /// The mouse selection in progress, if the button is still down.
    pub(super) drag: Option<Drag>,
    /// Which of this buffer's lines differ from the version `HEAD` holds, for
    /// the rail beside the numbers. Read off the window's thread and shared
    /// rather than copied per frame — see [`Shell::refresh_editor_marks`].
    ///
    /// [`Shell::refresh_editor_marks`]: crate::Shell::refresh_editor_marks
    pub(super) git: Arc<LineMarks>,
}

impl EditorState {
    /// Whether this buffer has edits that are not on disk.
    ///
    /// The one thing the tab strip asks of an editor, so it is a question
    /// rather than a borrow of the buffer.
    pub(crate) fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// The real file represented by this state; absent while it is Untitled.
    pub(crate) fn saved_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether this file's lines are numbered.
    ///
    /// Numbers are a code affordance: they are there to be quoted back — in
    /// a stack trace, a review comment, a `path:line` handoff to another
    /// editor — and a note or a README is not read that way. An Untitled
    /// buffer has no name to judge by and is a scratch pad until it is
    /// saved, so it starts without them and gains them the moment it is
    /// saved as something that is code.
    pub(super) fn numbered(&self) -> bool {
        self.path.as_deref().is_some_and(|path| !is_prose(path))
    }

    /// How far the text sits from the pane's left edge, which is the width
    /// of the numbers where there are numbers and a plain margin where there
    /// are not.
    pub(super) fn gutter_width(&self) -> Pixels {
        if self.numbered() {
            GUTTER_WIDTH
        } else {
            PLAIN_MARGIN
        }
    }

    /// Brings the wrap index up to date with the buffer and the pane's width.
    pub(super) fn ensure_lines(&mut self) {
        let width = match &self.measure {
            Some(measure) if self.geometry.text_width > px(0.0) => {
                WrapWidth::Pixels(f32::from(self.geometry.text_width), measure)
            }
            _ => WrapWidth::Columns(self.geometry.columns),
        };
        self.line_index.ensure(&self.buffer, width);
    }

    /// Cmd-Delete in a Mac text field: everything from the caret back to the
    /// start of the row it is on — the wrapped row, not the whole line — or
    /// on to the row's end with the forward key. A selection goes instead,
    /// and a caret already at that edge takes one character the way
    /// Backspace would, so holding the chord keeps walking back through the
    /// text rather than stopping at the first line break.
    pub(super) fn delete_to_row_edge(&mut self, forward: bool) {
        if self.buffer.has_selection() {
            self.buffer.replace_selection("");
            return;
        }
        self.ensure_lines();

        let cursor = self.buffer.cursor();
        let LineCol { line, column } = self.buffer.line_col_at(cursor);
        let (line_idx, column_idx) = (line as usize - 1, column as usize - 1);
        let line_start = cursor - column_idx;
        let line_len = line_char_count(&self.buffer, line_idx);
        let (row_start, row_end) = if self.wrap {
            let row = self.line_index.wrap_row_for(line_idx, column_idx);
            let start = self
                .line_index
                .wrap_rows
                .get(row)
                .map_or(0, |&(_, col)| col as usize);
            let end = match self.line_index.wrap_rows.get(row + 1) {
                Some(&(next, col)) if next as usize == line_idx => col as usize,
                _ => line_len,
            };
            (start, end)
        } else {
            (0, line_len)
        };

        let target = line_start + if forward { row_end } else { row_start };
        if target == cursor {
            if forward {
                self.buffer.delete();
            } else {
                self.buffer.backspace();
            }
            return;
        }
        self.buffer.set_selection(target, cursor);
        // Not coalesced with the typing before it, so one undo brings the
        // row back and nothing else with it.
        self.buffer.replace_selection("");
    }

    /// Where the cursor is, 1-based, for a handoff to another editor.
    ///
    /// A question rather than a borrow of the buffer, for the same reason
    /// [`Self::is_dirty`] is one: what the tab strip and its menu need to know
    /// about an open buffer is a fact, not the buffer.
    pub(crate) fn cursor_line_col(&self) -> (u32, u32) {
        let LineCol { line, column } = self.buffer.line_col_at(self.buffer.cursor());
        (line, column)
    }

    /// Re-reads the file when someone else has written it, answering whether
    /// the buffer changed.
    ///
    /// See the `editor` module doc's "Keeping up with other writers" for why
    /// this is needed at all. Three things it deliberately will not do:
    ///
    /// - **It never touches a dirty buffer.** Unsaved edits are the one thing
    ///   in this pane that exists nowhere else, and silently replacing them
    ///   with somebody else's version is the failure worth going out of the
    ///   way to avoid. A file that moved under unsaved edits is left to the
    ///   save to resolve, on the terms "Known simplifications" describes.
    /// - **It does not reload a write that changed nothing.** `touch`, a
    ///   formatter that reproduced the same bytes, `git checkout` of the
    ///   branch already there: the digest says so, and not reloading keeps
    ///   the cursor, the undo history and the scroll exactly where they were.
    /// - **It leaves the buffer alone when the read fails.** A file being
    ///   deleted, or replaced by something that does not write atomically, is
    ///   unreadable for an instant; swapping the pane for an error in that
    ///   instant would be a worse lie than the last contents that were true.
    ///   The stamp is left alone too, so the next look tries again.
    pub(super) fn refresh_from_disk(&mut self) -> bool {
        let (Some(path), Some(stamp)) = (self.path.clone(), self.stamp.clone()) else {
            return false;
        };
        if self.buffer.is_dirty() || stamp.metadata_matches(&path) {
            return false;
        }
        let Ok((mut fresh, fresh_stamp)) = Buffer::load_stamped(&path) else {
            return false;
        };
        if fresh_stamp.len == stamp.len && fresh_stamp.digest == stamp.digest {
            // New metadata over identical bytes. Recording it keeps the next
            // poll a `stat` rather than another read of the whole file.
            self.stamp = Some(fresh_stamp);
            return false;
        }

        // Where the cursor was, rather than which character index it was at:
        // a line and column survive an insertion higher up the file, and
        // `char_at_line_col` clamps whatever no longer exists.
        let at = self.buffer.line_col_at(self.buffer.cursor());
        let cursor = fresh.char_at_line_col(at);
        fresh.set_cursor(cursor);
        self.buffer = fresh;
        self.stamp = Some(fresh_stamp);
        self.load_error = None;
        self.drag = None;

        // The find matches index text that has just been replaced: ranges
        // that would otherwise paint over whatever now sits at those offsets.
        // The wrap and syntax indexes need nothing — a fresh buffer has a
        // revision they have never seen and no edits to replay, so both
        // rebuild. Re-running an open search needs a context this has no
        // business taking, so `Shell::refresh_editors` does that part.
        self.find.matches = FindMatches {
            ranges: Vec::new(),
            total: 0,
            truncated: false,
        };
        true
    }

    /// Starts a plain-text buffer that has no file until its first save.
    pub(super) fn empty(save_directory: PathBuf) -> Self {
        Self {
            buffer: Buffer::new(),
            path: None,
            save_directory,
            stamp: None,
            load_error: None,
            scroll: UniformListScrollHandle::new(),
            wrap: true,
            composer: false,
            caret: true,
            composer_size: COMPOSER_TEXT,
            measure: None,
            find: Find::default(),
            line_index: LineIndex::default(),
            // An Untitled buffer has no name to read a language off. It gets
            // one at its first Save As, where the name is chosen.
            language: None,
            syntax: SyntaxIndex::default(),
            visible_rows: 0..0,
            geometry: Geometry::default(),
            drag: None,
            git: Arc::default(),
        }
    }

    /// Loads `path` into a fresh editor state, or records why it could not
    /// be — see [`EditorState::load_error`].
    pub(super) fn open(path: &Path) -> Self {
        let (buffer, stamp, load_error) = match Buffer::load_stamped(path) {
            Ok((buffer, stamp)) => (buffer, Some(stamp), None),
            Err(e) => (
                Buffer::new(),
                None,
                Some(SharedString::from(format!(
                    "could not open {}: {e}",
                    path.display()
                ))),
            ),
        };
        Self {
            buffer,
            path: Some(path.to_path_buf()),
            save_directory: path.parent().unwrap_or(path).to_path_buf(),
            stamp,
            load_error,
            scroll: UniformListScrollHandle::new(),
            wrap: true,
            composer: false,
            caret: true,
            composer_size: COMPOSER_TEXT,
            measure: None,
            find: Find::default(),
            line_index: LineIndex::default(),
            language: ket_core::syntax::language_for(&path.display().to_string()),
            syntax: SyntaxIndex::default(),
            visible_rows: 0..0,
            geometry: Geometry::default(),
            drag: None,
            git: Arc::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_editor_does_not_create_its_file_before_save() {
        let state = EditorState::empty(PathBuf::from("/tmp"));
        assert_eq!(state.buffer.text(), "");
        assert!(state.buffer.path().is_none());
        assert!(!state.buffer.is_dirty());
    }
}
