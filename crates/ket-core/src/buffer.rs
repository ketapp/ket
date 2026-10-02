//! A text buffer for the tweak editor: a rope, a cursor, and undo history.
//!
//! This is the thing a person edits when a diff is worth a one-line fix rather
//! than a trip to VS Code or Zed (see [`crate::surface`], which is where that
//! larger trip is handed off). It is deliberately small: a rope-backed buffer,
//! the cursor and selection movements typing has trained into people's fingers,
//! five editing operations, undo/redo, literal find, and the position math a
//! renderer and a `code --goto` handoff both need. Nothing here knows how to
//! draw a glyph — that is the shell's job — and
//! nothing here parses syntax, offers a completion, or tracks more than one
//! cursor. Those are a different, much bigger tool.
//!
//! # Why a rope
//!
//! A `String` makes every edit to a large file an `O(n)` copy — insert one
//! character into a ten-megabyte file and the whole ten megabytes moves.
//! [`ropey::Rope`] keeps the text in a balanced tree of small chunks, so an
//! edit near the middle of a large file touches a handful of chunks instead of
//! the whole document. Positions into it are **character indices**, not byte
//! offsets: [`Buffer::insert`] at char index 5 in a string full of
//! multi-byte emoji means the fifth character, not the fifth byte, which is
//! exactly the distinction that keeps a cursor from landing inside a UTF-8
//! sequence and corrupting it.
//!
//! # Line endings
//!
//! A loaded file's line ending style is detected once — [`LineEnding::Crlf`]
//! if any `"\r\n"` occurs, [`LineEnding::Lf`] otherwise — and every `"\r\n"`
//! is folded to `"\n"` before the text ever reaches the rope. Every movement,
//! edit, and position calculation in this module therefore only ever has to
//! reason about a single line-break character. [`Buffer::save`] converts back
//! on the way out, so a CRLF file round-trips as CRLF.
//!
//! This is a deliberate simplification for a file with **mixed** endings: the
//! whole file is written back in whichever style was detected, not per line.
//! Preserving each line's own ending would mean carrying a style tag through
//! every edit, movement, and undo entry in this module for a case that, in
//! practice, is a file already in a state most editors normalize on save
//! anyway.
//!
//! # Undo coalescing
//!
//! Typing ten characters and pressing undo once should undo all ten, not
//! peel them off one at a time — but a paste, a find-and-replace, or pressing
//! Enter should never quietly merge into a run of typing it has nothing to do
//! with. The rule:
//!
//! An edit extends the top of the undo stack only when **all** of the
//! following hold:
//!
//! - It is a single-character insertion, backward deletion (Backspace), or
//!   forward deletion (the Delete key) — never a multi-character insertion,
//!   never [`Buffer::replace_selection`], never [`Buffer::newline`], and never
//!   an edit that replaced a selection.
//! - The top entry is the same one of those three kinds.
//! - It lands exactly where the top entry's effect ends, extending it
//!   contiguously (typing forward, backspacing backward, or pressing Delete
//!   repeatedly at the same spot as text shifts left underneath it).
//! - The top entry's affected text does not already contain a newline —
//!   crossing a line break always closes the current step.
//! - Nothing has moved the cursor, changed the selection, or called
//!   [`Buffer::undo`] or [`Buffer::redo`] since the top entry was created.
//!
//! Concretely: typing `"hello"` is one undo step; pressing Backspace three
//! times afterward is a second step (deletion never merges with insertion);
//! pressing Enter, or pasting a block of text, always starts a fresh step and
//! never merges with what came before or after it; moving the cursor between
//! two keystrokes splits them into two steps even though they'd otherwise be
//! contiguous.
//!
//! The undo stack is capped at [`MAX_UNDO_ENTRIES`] — see its doc for why an
//! unbounded history is the wrong default for a long editing session.
//!
//! # Dirty tracking
//!
//! [`Buffer::is_dirty`] is a plain flag, set on every edit and cleared by
//! [`Buffer::load`] and [`Buffer::save`] — not a comparison against the file
//! on disk. Undoing back to the exact text that was last saved still reports
//! dirty. Recomputing "does this match the saved snapshot" would mean hashing
//! or diffing the whole buffer on every keystroke and every undo, which is
//! precisely the per-edit cost proportional to file size that the rope exists
//! to avoid; a boolean set by the operations that actually change the buffer
//! costs nothing to maintain and is never wrong in the direction that matters
//! ("do I need to save before I close this").
//!
//! # Revisions
//!
//! A renderer keeps things per line — a wrap index, the highlighter's spans —
//! that go stale on every edit, and rebuilding them from the whole document
//! on every keystroke makes typing cost more the longer the file is.
//! [`Buffer::revision`] names the text as it is now, and
//! [`Buffer::line_edits_since`] says which lines changed since a revision the
//! caller last saw, so it can redo those and keep the rest. The log is short
//! ([`EDIT_LOG_LEN`]); a caller that fell further behind than that, or holds a
//! revision from a different buffer, is told to rebuild instead.
//!
//! # Out of scope
//!
//! No LSP, no completions, no syntax highlighting (`tree-sitter` belongs to
//! the shell, later), no multiple cursors, no folding, no project-wide search,
//! no formatting. A rename or a multi-file refactor is exactly the trip to a
//! real editor that [`crate::surface`] hands off to.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;

use crate::surface::{self, FileStamp};
use crate::{KetError, Result};

/// Most entries kept on the undo stack.
///
/// Past this, the oldest entry is dropped to make room for a new one. An
/// editing session that runs for hours accumulates undo history forever if
/// nothing bounds it, and the entries nobody will ever reach for again are the
/// ones from an hour ago, not the ones from a second ago — so eviction is
/// FIFO from the old end, exactly like [`crate::status::MAX_CHANGED_FILES`]
/// bounds a file list rather than trying to guess which files matter.
pub const MAX_UNDO_ENTRIES: usize = 1000;

/// How many edits [`Buffer::line_edits_since`] can replay. A caller more than
/// this far behind rebuilds from the whole text, which is what it would have
/// done anyway before there was a log.
pub const EDIT_LOG_LEN: usize = 64;

/// How many lines [`Buffer::indent_unit`] reads to find a file's indent: the
/// top of a file says how it is written, and a huge one is not read whole for
/// a keystroke.
const INDENT_SCAN_LINES: usize = 2000;

/// The indent a file with none to go by gets, and the width a tab stands for
/// when spaces are taken out a tab's worth at a time.
const DEFAULT_INDENT: usize = 4;

/// Where revision numbers come from. Shared by every buffer, so a revision
/// names one text in one buffer: a buffer swapped for a freshly loaded one, or
/// a clone edited on its own, never reports a revision a caller has already
/// seen for different text.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// One edit, in whole lines: lines `start..start + removed` as they were
/// became lines `start..start + inserted`. Everything before `start` is
/// untouched, and everything after moved by `inserted - removed` lines
/// without changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineEdit {
    /// The first line the edit touched.
    pub start: usize,
    /// How many lines the edit replaced, counting the one it started on.
    pub removed: usize,
    /// How many lines stand in their place, counting the one it started on.
    pub inserted: usize,
}

/// One entry in a buffer's edit log.
#[derive(Debug, Clone, Copy)]
struct LoggedEdit {
    /// The revision the edit was made to.
    before: u64,
    edit: LineEdit,
}

/// Most match ranges retained by [`Buffer::find_all`].
///
/// [`FindMatches::total`] still counts every match; only the kept ranges are
/// capped. Searching a large file for a single common character can produce
/// tens of thousands of hits, and holding one [`Range`] per hit to render a
/// list nobody scrolls that far into is the same unbounded buffer
/// [`crate::diff`] and [`crate::status`] are capped against.
pub const MAX_FIND_MATCHES: usize = 10_000;

// ---------------------------------------------------------------------------
// Line endings
// ---------------------------------------------------------------------------

/// How a buffer's line breaks are written back to disk.
///
/// See the module doc's "Line endings" section: the rope only ever holds
/// `"\n"`, and this is what [`Buffer::save`] converts back from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// `"\n"` only.
    Lf,
    /// `"\r\n"`.
    Crlf,
}

/// Detects which style a loaded file used, by whether `"\r\n"` occurs at all.
///
/// First occurrence wins for a file that mixes styles — see the module doc.
fn detect_line_ending(text: &str) -> LineEnding {
    if text.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

/// Folds every `"\r\n"` to `"\n"`, leaving a lone `"\r"` untouched.
///
/// A lone `"\r"` is content, not a line ending, in the model this module
/// uses — see the module doc. Untouched here means it round-trips through
/// load and save exactly as it was, which matters for a file that happens to
/// carry one in the middle of a line.
fn normalize_line_endings(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains("\r\n") {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

/// A place in the buffer named by line and column, both 1-based.
///
/// 1-based to match every editor [`crate::surface`] hands off to, and the
/// unified diff hunk headers in [`crate::diff`] that positions in ket
/// typically come from. The column counts **characters**, not bytes — see the
/// module doc — so it agrees with what a person editing the line would count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, in characters.
    pub column: u32,
}

// ---------------------------------------------------------------------------
// Find
// ---------------------------------------------------------------------------

/// Whether a search treats letters of different case as equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    /// `"A" != "a"`.
    Sensitive,
    /// `"A" == "a"`, using Unicode case folding rather than an ASCII-only
    /// comparison, so an accented letter's case still matches.
    Insensitive,
}

/// Whether two characters are equal under `case`.
fn chars_equal(a: char, b: char, case: Case) -> bool {
    match case {
        Case::Sensitive => a == b,
        Case::Insensitive => a.to_lowercase().eq(b.to_lowercase()),
    }
}

/// Every literal match of a search term, bounded per [`MAX_FIND_MATCHES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindMatches {
    /// Char-index ranges of the matches that were kept, in document order.
    pub ranges: Vec<Range<usize>>,
    /// How many matches there were in total, including any not kept.
    pub total: usize,
    /// Whether [`FindMatches::ranges`] is a subset of all matches.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Undo
// ---------------------------------------------------------------------------

/// Which of the three coalescible edit shapes an entry is, or [`EditKind::Other`]
/// for anything that must always stand alone. See the module doc's "Undo
/// coalescing" section — the kinds named here are exactly the three it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// A single character typed at the cursor.
    Insert,
    /// A single character removed by Backspace.
    Backspace,
    /// A single character removed by the Delete key.
    ForwardDelete,
    /// Everything else: multi-character inserts, replacing a selection,
    /// [`Buffer::newline`]. Never merges with anything, in either direction.
    Other,
}

/// One undoable change, stored as the minimal edit that produced it.
///
/// Applying `(removed, inserted)` as a replacement at `at` is the edit;
/// applying `(inserted, removed)` at the same `at` is its exact inverse. That
/// symmetry is what lets [`Buffer::undo`] and [`Buffer::redo`] share one
/// splice operation instead of needing separate logic per edit shape —
/// insert, delete, and replace are all just the case where one side of the
/// pair happens to be empty.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UndoEntry {
    /// Char index where the change starts.
    at: usize,
    /// The text that was there before, now gone.
    removed: String,
    /// The text that replaced it.
    inserted: String,
    /// Cursor and selection to restore on undo.
    anchor_before: usize,
    cursor_before: usize,
    /// Cursor and selection to restore on redo.
    anchor_after: usize,
    cursor_after: usize,
    /// What kind of edit this is, for the coalescing check.
    kind: EditKind,
}

/// Extends the top of `stack` with a new coalescible edit, if it qualifies.
///
/// Only inspects and mutates the stack; the caller is responsible for
/// splicing the rope itself either way. See the module doc's "Undo
/// coalescing" section for the rule this implements.
fn try_merge(
    stack: &mut VecDeque<UndoEntry>,
    range: &Range<usize>,
    text: &str,
    removed: &str,
    kind: EditKind,
) -> bool {
    let Some(top) = stack.back_mut() else {
        return false;
    };
    if top.kind != kind {
        return false;
    }

    let contiguous = match kind {
        EditKind::Insert => {
            !top.inserted.contains('\n') && range.start == top.at + top.inserted.chars().count()
        }
        EditKind::Backspace => !top.removed.contains('\n') && range.end == top.at,
        EditKind::ForwardDelete => !top.removed.contains('\n') && range.start == top.at,
        EditKind::Other => false,
    };
    if !contiguous {
        return false;
    }

    match kind {
        EditKind::Insert => top.inserted.push_str(text),
        EditKind::Backspace => {
            top.removed = format!("{removed}{}", top.removed);
            top.at = range.start;
        }
        EditKind::ForwardDelete => top.removed.push_str(removed),
        EditKind::Other => unreachable!("Other never reaches this match; filtered out above"),
    }
    true
}

// ---------------------------------------------------------------------------
// Word classification, for word-wise movement
// ---------------------------------------------------------------------------

/// The three classes a character falls into for word-wise cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    /// Whitespace.
    Space,
    /// A letter, digit, or underscore.
    Word,
    /// Everything else: punctuation, symbols.
    Punct,
}

/// Classifies `ch` for word-wise movement.
fn char_class(ch: char) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Space
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

// ---------------------------------------------------------------------------
// The buffer
// ---------------------------------------------------------------------------

/// A rope-backed text buffer with a cursor, a selection, and undo history.
///
/// All positions on the public API — [`Buffer::cursor`], [`Buffer::selection`],
/// [`Buffer::find_all`], and so on — are **character indices**, counted from
/// the start of the document. See the module doc for why, and for how those
/// relate to the byte offsets [`Buffer::char_to_byte`] and
/// [`Buffer::byte_to_char`] convert to and from.
#[derive(Debug, Clone)]
pub struct Buffer {
    rope: Rope,
    /// The selection's moving end. Equal to `anchor` when there is no
    /// selection.
    cursor: usize,
    /// The selection's fixed end, set when a selection begins.
    anchor: usize,
    /// The character column [`Buffer::move_up`] and [`Buffer::move_down`]
    /// aim for, remembered across a run of vertical moves so that passing
    /// through a short line and back onto a long one restores the original
    /// column instead of leaving the cursor wherever the short line ended.
    /// Cleared by every other movement and every edit.
    goal_column: Option<usize>,
    path: Option<PathBuf>,
    line_ending: LineEnding,
    dirty: bool,
    undo_stack: VecDeque<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    /// Whether the next coalescible edit may extend the top of the undo
    /// stack. Cleared by any cursor or selection movement, by undo, by redo,
    /// and by any non-coalescible edit — see the module doc.
    coalescing: bool,
    /// Names the text as it is now — see the module doc's "Revisions".
    revision: u64,
    /// The last [`EDIT_LOG_LEN`] edits, oldest first.
    edits: VecDeque<LoggedEdit>,
}

impl Default for Buffer {
    /// An empty buffer, equivalent to [`Buffer::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    /// An empty, unsaved buffer with no file behind it.
    pub fn new() -> Self {
        Self::from_text("")
    }

    /// A buffer over `text`, detached from any file.
    ///
    /// For a new scratch buffer, and the base every other constructor in this
    /// module builds on.
    pub fn from_text(text: &str) -> Self {
        let line_ending = detect_line_ending(text);
        let normalized = normalize_line_endings(text);
        Buffer {
            rope: Rope::from_str(&normalized),
            cursor: 0,
            anchor: 0,
            goal_column: None,
            path: None,
            line_ending,
            dirty: false,
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            coalescing: false,
            revision: next_revision(),
            edits: VecDeque::new(),
        }
    }

    /// Reads `path` into a new buffer.
    ///
    /// The file must be valid UTF-8. This is a text buffer, not a hex editor:
    /// a byte that cannot be decoded is reported as an I/O error rather than
    /// silently replaced with a placeholder that would then get written back
    /// as something the file never contained.
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self::load_stamped(path)?.0)
    }

    /// Reads `path` into a new buffer together with the [`FileStamp`] of the
    /// version it read.
    ///
    /// Both from a single read on purpose. A caller that wants to notice
    /// later writes by anyone else — the agent working in the same worktree,
    /// the person's other editor — needs a stamp that describes exactly the
    /// bytes in this buffer, and stamping with a second read cannot promise
    /// that: a write landing between the two reads would be recorded as
    /// already loaded, and so would never be shown.
    pub fn load_stamped(path: &Path) -> Result<(Self, FileStamp)> {
        let (bytes, stamp) = surface::read(path)?;
        let text = String::from_utf8(bytes).map_err(|e| {
            let utf8_error = e.utf8_error();
            KetError::io(
                path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, utf8_error),
            )
        })?;
        let mut buffer = Self::from_text(&text);
        buffer.path = Some(path.to_path_buf());
        Ok((buffer, stamp))
    }

    /// Writes the buffer to the file it was loaded from or last saved to.
    ///
    /// Fails with [`KetError::Path`] if the buffer has never had a file —
    /// use [`Buffer::save_as`] for a buffer created with [`Buffer::new`] or
    /// [`Buffer::from_text`].
    pub fn save(&mut self) -> Result<FileStamp> {
        let path = self.path.clone().ok_or_else(|| KetError::Path {
            what: "buffer save path",
            why: "buffer has no associated file; call save_as instead".to_owned(),
        })?;
        self.write_to(&path)
    }

    /// Writes the buffer to `path` and adopts it as the buffer's file, so a
    /// later plain [`Buffer::save`] writes back to the same place.
    pub fn save_as(&mut self, path: &Path) -> Result<FileStamp> {
        let stamp = self.write_to(path)?;
        self.path = Some(path.to_path_buf());
        Ok(stamp)
    }

    /// The write behind both [`Buffer::save`] and [`Buffer::save_as`].
    ///
    /// Returns the [`FileStamp`] of what it wrote, stamped from the bytes in
    /// hand rather than by reading the file back — for the reason
    /// [`Buffer::load_stamped`] takes its stamp from the bytes it read. A
    /// caller watching the file for other people's writes would otherwise
    /// record whichever version won a race with its own save, and then treat
    /// that version as the one it is already showing.
    fn write_to(&mut self, path: &Path) -> Result<FileStamp> {
        let text = self.rope.to_string();
        let text = match self.line_ending {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        };
        let bytes = text.as_bytes();
        std::fs::write(path, bytes).map_err(|e| KetError::io(path, e))?;
        self.dirty = false;
        Ok(FileStamp::of_bytes(bytes, surface::modified_ms(path)))
    }

    /// The file this buffer was loaded from or last saved to, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The line ending style detected on load, or [`LineEnding::Lf`] for a
    /// buffer that never came from a file.
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Whether the buffer has changed since it was loaded or last saved.
    ///
    /// See the module doc's "Dirty tracking" section for why this is a flag
    /// rather than a comparison against the file on disk.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The whole buffer's text, with line endings as they are held
    /// internally (always `"\n"` — see the module doc). Converted to the
    /// original style only on save.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Number of characters in the buffer.
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Number of lines in the buffer. A buffer with no trailing newline still
    /// counts its last, unterminated line; an empty buffer has exactly one
    /// (empty) line, matching how a text file with zero bytes is still one
    /// blank line to open in an editor.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// The text of one line, without its trailing line break.
    ///
    /// Returns an empty string for an out-of-range index rather than
    /// panicking: the shell asks for lines to fill a viewport, and a
    /// viewport can run past the end of a buffer that just got smaller.
    pub fn line_text(&self, line_idx: usize) -> String {
        if line_idx >= self.rope.len_lines() {
            return String::new();
        }
        let start = self.rope.line_to_char(line_idx);
        let end = self.line_end_char(line_idx);
        self.rope.slice(start..end).to_string()
    }

    /// The char index just past the last character of `line`, i.e. excluding
    /// its trailing `"\n"` if it has one.
    fn line_end_char(&self, line: usize) -> usize {
        let start = self.rope.line_to_char(line);
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len -= 1;
        }
        start + len
    }

    // -- Position conversion --------------------------------------------

    /// The 1-based line and column of `char_idx`, clamped to the buffer's
    /// length.
    pub fn line_col_at(&self, char_idx: usize) -> LineCol {
        let char_idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(char_idx);
        let column = char_idx - self.rope.line_to_char(line);
        LineCol {
            line: line as u32 + 1,
            column: column as u32 + 1,
        }
    }

    /// The char index of `at`, clamping a line past the end to the last line
    /// and a column past a line's end to that line's end — the same
    /// forgiving behaviour editors give a `path:line:col` handoff whose
    /// numbers no longer quite match after the file changed underneath it.
    pub fn char_at_line_col(&self, at: LineCol) -> usize {
        let last_line = self.rope.len_lines().saturating_sub(1);
        let line = (at.line.saturating_sub(1) as usize).min(last_line);
        let start = self.rope.line_to_char(line);
        let end = self.line_end_char(line);
        let column = at.column.saturating_sub(1) as usize;
        (start + column).min(end)
    }

    /// The byte offset of char index `char_idx`.
    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        self.rope.char_to_byte(char_idx.min(self.rope.len_chars()))
    }

    /// The char index containing byte offset `byte_idx`.
    pub fn byte_to_char(&self, byte_idx: usize) -> usize {
        self.rope.byte_to_char(byte_idx.min(self.rope.len_bytes()))
    }

    /// The 1-based line and column at byte offset `byte_idx`.
    pub fn line_col_at_byte(&self, byte_idx: usize) -> LineCol {
        self.line_col_at(self.byte_to_char(byte_idx))
    }

    /// The byte offset of `at`, with the same clamping as
    /// [`Buffer::char_at_line_col`].
    pub fn byte_at_line_col(&self, at: LineCol) -> usize {
        self.char_to_byte(self.char_at_line_col(at))
    }

    // -- Cursor and selection --------------------------------------------

    /// The selection's moving end (where the cursor is drawn).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selection's fixed end. Equal to [`Buffer::cursor`] when there is
    /// no selection.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// Whether any text is selected.
    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    /// The selection as a char range, ordered low to high regardless of
    /// which end the cursor is at. Empty (`cursor..cursor`) when there is no
    /// selection.
    pub fn selection(&self) -> Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor..self.cursor
        } else {
            self.cursor..self.anchor
        }
    }

    /// The selected text, or an empty string when there is none.
    pub fn selected_text(&self) -> String {
        self.rope.slice(self.selection()).to_string()
    }

    /// Moves the cursor to `char_idx`, clamped to the buffer, collapsing any
    /// selection.
    pub fn set_cursor(&mut self, char_idx: usize) {
        self.place_cursor(char_idx, false);
    }

    /// Sets the selection directly, e.g. from a mouse drag. Both ends are
    /// clamped to the buffer.
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        let len = self.rope.len_chars();
        self.anchor = anchor.min(len);
        self.cursor = cursor.min(len);
        self.goal_column = None;
        self.coalescing = false;
    }

    /// Selects the whole buffer.
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.rope.len_chars();
        self.goal_column = None;
        self.coalescing = false;
    }

    /// Places the cursor at `pos`, clamped to the buffer. Moves only the
    /// cursor when `extend` is set (growing or shrinking the selection);
    /// otherwise moves the anchor along with it, collapsing the selection.
    ///
    /// The shared tail of every movement method: clamps, ends the current
    /// undo-coalescing run (see the module doc), and clears the vertical
    /// "goal column" (callers doing vertical movement set it back
    /// afterward).
    fn place_cursor(&mut self, pos: usize, extend: bool) {
        let pos = pos.min(self.rope.len_chars());
        self.cursor = pos;
        if !extend {
            self.anchor = pos;
        }
        self.goal_column = None;
        self.coalescing = false;
    }

    /// One character left, or to the selection's start if one is active —
    /// matching every text field's convention that an unmodified Left arrow
    /// with a selection collapses it rather than moving further.
    pub fn move_left(&mut self) {
        let target = if self.has_selection() {
            self.selection().start
        } else {
            self.cursor.saturating_sub(1)
        };
        self.place_cursor(target, false);
    }

    /// One character right, or to the selection's end if one is active.
    pub fn move_right(&mut self) {
        let target = if self.has_selection() {
            self.selection().end
        } else {
            (self.cursor + 1).min(self.rope.len_chars())
        };
        self.place_cursor(target, false);
    }

    /// Extends the selection one character left.
    pub fn extend_left(&mut self) {
        let target = self.cursor.saturating_sub(1);
        self.place_cursor(target, true);
    }

    /// Extends the selection one character right.
    pub fn extend_right(&mut self) {
        let target = (self.cursor + 1).min(self.rope.len_chars());
        self.place_cursor(target, true);
    }

    /// The char index one word left of `pos`: past any whitespace, then past
    /// a run of characters in the same class (word or punctuation).
    fn word_left_of(&self, mut pos: usize) -> usize {
        while pos > 0 && char_class(self.rope.char(pos - 1)) == CharClass::Space {
            pos -= 1;
        }
        if pos > 0 {
            let class = char_class(self.rope.char(pos - 1));
            while pos > 0 && char_class(self.rope.char(pos - 1)) == class {
                pos -= 1;
            }
        }
        pos
    }

    /// The char index one word right of `pos`, mirroring [`Buffer::word_left_of`].
    fn word_right_of(&self, mut pos: usize) -> usize {
        let len = self.rope.len_chars();
        while pos < len && char_class(self.rope.char(pos)) == CharClass::Space {
            pos += 1;
        }
        if pos < len {
            let class = char_class(self.rope.char(pos));
            while pos < len && char_class(self.rope.char(pos)) == class {
                pos += 1;
            }
        }
        pos
    }

    /// Moves left to the start of the previous word.
    pub fn move_word_left(&mut self) {
        let target = self.word_left_of(self.cursor);
        self.place_cursor(target, false);
    }

    /// Moves right to the start of the next word.
    pub fn move_word_right(&mut self) {
        let target = self.word_right_of(self.cursor);
        self.place_cursor(target, false);
    }

    /// Extends the selection left by one word.
    pub fn extend_word_left(&mut self) {
        let target = self.word_left_of(self.cursor);
        self.place_cursor(target, true);
    }

    /// Extends the selection right by one word.
    pub fn extend_word_right(&mut self) {
        let target = self.word_right_of(self.cursor);
        self.place_cursor(target, true);
    }

    /// Moves to the start of the current line.
    pub fn move_home(&mut self) {
        let line = self.rope.char_to_line(self.cursor);
        let target = self.rope.line_to_char(line);
        self.place_cursor(target, false);
    }

    /// Extends the selection to the start of the current line.
    pub fn extend_home(&mut self) {
        let line = self.rope.char_to_line(self.cursor);
        let target = self.rope.line_to_char(line);
        self.place_cursor(target, true);
    }

    /// Moves to the end of the current line, before its line break if any.
    pub fn move_end(&mut self) {
        let line = self.rope.char_to_line(self.cursor);
        let target = self.line_end_char(line);
        self.place_cursor(target, false);
    }

    /// Extends the selection to the end of the current line.
    pub fn extend_end(&mut self) {
        let line = self.rope.char_to_line(self.cursor);
        let target = self.line_end_char(line);
        self.place_cursor(target, true);
    }

    /// Moves to the very start of the buffer.
    pub fn move_document_start(&mut self) {
        self.place_cursor(0, false);
    }

    /// Extends the selection to the very start of the buffer.
    pub fn extend_document_start(&mut self) {
        self.place_cursor(0, true);
    }

    /// Moves to the very end of the buffer.
    pub fn move_document_end(&mut self) {
        let target = self.rope.len_chars();
        self.place_cursor(target, false);
    }

    /// Extends the selection to the very end of the buffer.
    pub fn extend_document_end(&mut self) {
        let target = self.rope.len_chars();
        self.place_cursor(target, true);
    }

    /// The char index of `column` (characters from the line's start) on
    /// `line`, clamped to that line's actual length — used so moving through
    /// a shorter line and back doesn't lose the intended column.
    fn char_at_column(&self, line: usize, column: usize) -> usize {
        let start = self.rope.line_to_char(line);
        let end = self.line_end_char(line);
        (start + column).min(end)
    }

    /// The shared logic behind [`Buffer::move_up`], [`Buffer::move_down`],
    /// [`Buffer::extend_up`] and [`Buffer::extend_down`]. No-op at the first
    /// or last line rather than jumping to document start/end — a person
    /// holding Up expects to stop, not to suddenly land at column 1 of line 1.
    fn move_vertical(&mut self, delta: i32, extend: bool) {
        let current_line = self.rope.char_to_line(self.cursor);
        let goal = self
            .goal_column
            .unwrap_or(self.cursor - self.rope.line_to_char(current_line));

        let target_line = if delta < 0 {
            match current_line.checked_sub(1) {
                Some(line) => line,
                None => return,
            }
        } else {
            let line = current_line + 1;
            if line >= self.rope.len_lines() {
                return;
            }
            line
        };

        let pos = self.char_at_column(target_line, goal);
        self.cursor = pos;
        if !extend {
            self.anchor = pos;
        }
        self.goal_column = Some(goal);
        self.coalescing = false;
    }

    /// Moves up one line, keeping the goal column (the field doc on
    /// `Buffer::goal_column` explains why that matters).
    pub fn move_up(&mut self) {
        self.move_vertical(-1, false);
    }

    /// Moves down one line, keeping the goal column.
    pub fn move_down(&mut self) {
        self.move_vertical(1, false);
    }

    /// Extends the selection up one line.
    pub fn extend_up(&mut self) {
        self.move_vertical(-1, true);
    }

    /// Extends the selection down one line.
    pub fn extend_down(&mut self) {
        self.move_vertical(1, true);
    }

    // -- Editing -----------------------------------------------------------

    /// Splices `text` into `range`, without touching undo history, the
    /// cursor, or the dirty flag — the one place both edits and undo/redo
    /// touch the rope, and so the one place the edit log is written.
    fn splice(&mut self, range: Range<usize>, text: &str) {
        let start = self.rope.char_to_line(range.start);
        let removed = self.rope.char_to_line(range.end) - start + 1;
        if range.end > range.start {
            self.rope.remove(range.clone());
        }
        if !text.is_empty() {
            self.rope.insert(range.start, text);
        }
        let end = range.start + text.chars().count();
        let inserted = self.rope.char_to_line(end) - start + 1;

        if self.edits.len() >= EDIT_LOG_LEN {
            self.edits.pop_front();
        }
        self.edits.push_back(LoggedEdit {
            before: self.revision,
            edit: LineEdit {
                start,
                removed,
                inserted,
            },
        });
        self.revision = next_revision();
    }

    /// Names the text as it is now. Any edit changes it, and no other text —
    /// in this buffer or any other — ever has the same one.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The edits that turned the text at `revision` into the text now, oldest
    /// first — empty when nothing has changed. `None` when the log no longer
    /// reaches back that far, or `revision` was never this buffer's, which
    /// means the caller has to rebuild from the whole text.
    pub fn line_edits_since(&self, revision: u64) -> Option<Vec<LineEdit>> {
        if revision == self.revision {
            return Some(Vec::new());
        }
        let first = self.edits.iter().position(|e| e.before == revision)?;
        Some(self.edits.iter().skip(first).map(|e| e.edit).collect())
    }

    /// Pushes a new undo entry, evicting the oldest one past
    /// [`MAX_UNDO_ENTRIES`].
    fn push_undo(&mut self, entry: UndoEntry) {
        if self.undo_stack.len() >= MAX_UNDO_ENTRIES {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(entry);
    }

    /// Replaces `range` with `text`: the one path every editing method in
    /// this module funnels through, so undo, coalescing, cursor placement
    /// and the dirty flag are each handled in exactly one place.
    fn replace_range(&mut self, range: Range<usize>, text: &str, kind: EditKind) {
        if range.is_empty() && text.is_empty() {
            return;
        }

        let anchor_before = self.anchor;
        let cursor_before = self.cursor;
        let removed = self.rope.slice(range.clone()).to_string();

        self.redo_stack.clear();

        let merged = kind != EditKind::Other
            && self.coalescing
            && try_merge(&mut self.undo_stack, &range, text, &removed, kind);

        self.splice(range.clone(), text);

        let cursor_after = range.start + text.chars().count();

        if merged {
            if let Some(top) = self.undo_stack.back_mut() {
                top.cursor_after = cursor_after;
                top.anchor_after = cursor_after;
            }
        } else {
            self.push_undo(UndoEntry {
                at: range.start,
                removed,
                inserted: text.to_owned(),
                anchor_before,
                cursor_before,
                anchor_after: cursor_after,
                cursor_after,
                kind,
            });
        }

        self.cursor = cursor_after;
        self.anchor = cursor_after;
        self.goal_column = None;
        self.coalescing = kind != EditKind::Other;
        self.dirty = true;
    }

    /// Inserts `text` at the cursor, first replacing the selection if one is
    /// active — exactly what typing into any text field does.
    ///
    /// A single character with no active selection joins a coalescing run for
    /// undo (see the module doc); anything else — multiple characters, or
    /// typing over a selection — is always its own undo step.
    pub fn insert(&mut self, text: &str) {
        let has_selection = self.has_selection();
        let range = self.selection();
        let coalescible = !has_selection && text.chars().count() == 1 && text != "\n";
        self.replace_range(
            range,
            text,
            if coalescible {
                EditKind::Insert
            } else {
                EditKind::Other
            },
        );
    }

    /// Inserts a line break at the cursor, first replacing the selection if
    /// one is active.
    ///
    /// Always its own undo step: pressing Enter closes off whatever typing
    /// run came before it, so undo reverts one line's worth of typing at a
    /// time instead of an entire paragraph.
    pub fn newline(&mut self) {
        let range = self.selection();
        self.replace_range(range, "\n", EditKind::Other);
    }

    /// Deletes the selection, or the character after the cursor if there is
    /// none (the Delete key).
    pub fn delete(&mut self) {
        let has_selection = self.has_selection();
        let range = if has_selection {
            self.selection()
        } else {
            let end = (self.cursor + 1).min(self.rope.len_chars());
            self.cursor..end
        };
        self.replace_range(
            range,
            "",
            if has_selection {
                EditKind::Other
            } else {
                EditKind::ForwardDelete
            },
        );
    }

    /// Deletes the selection, or the character before the cursor if there is
    /// none (Backspace).
    pub fn backspace(&mut self) {
        let has_selection = self.has_selection();
        let range = if has_selection {
            self.selection()
        } else {
            let start = self.cursor.saturating_sub(1);
            start..self.cursor
        };
        self.replace_range(
            range,
            "",
            if has_selection {
                EditKind::Other
            } else {
                EditKind::Backspace
            },
        );
    }

    /// Replaces the current selection with `text`, or inserts at the cursor
    /// if nothing is selected.
    ///
    /// Distinct from [`Buffer::insert`]: this is for a programmatic
    /// replacement — find-and-replace, or a completion accepted from a menu
    /// — and, unlike typing, never coalesces with a neighboring edit even
    /// when `text` is one character. Accepting a one-letter suggestion should
    /// not make undo also erase whatever was typed just before it.
    pub fn replace_selection(&mut self, text: &str) {
        let range = self.selection();
        self.replace_range(range, text, EditKind::Other);
    }

    /// What one level of indentation is in this buffer, read off the file
    /// itself: a tab when its indented lines lead with tabs, else the step
    /// its space-indented lines most often go in by, else four spaces.
    ///
    /// A step, not the narrowest indent: a block comment's ` * ` lines are
    /// one space in and say nothing about how the code around them nests.
    pub fn indent_unit(&self) -> String {
        let (mut tabbed, mut spaced) = (0usize, 0usize);
        let mut steps = [0usize; 9];
        let mut previous = 0usize;
        for line in self.rope.lines().take(INDENT_SCAN_LINES) {
            let text: String = line.chars().take(200).collect();
            let body = text.trim_end_matches(['\n', '\r']);
            if body.trim().is_empty() {
                continue;
            }
            if body.starts_with('\t') {
                tabbed += 1;
                continue;
            }
            let width = body.chars().take_while(|c| *c == ' ').count();
            if width > 0 {
                spaced += 1;
            }
            if width > previous && width - previous < steps.len() {
                steps[width - previous] += 1;
            }
            previous = width;
        }
        if tabbed > spaced {
            return "\t".to_owned();
        }
        // The commonest step of two or more; a lone one-space step only when
        // the file has nothing else.
        let step = (2..steps.len())
            .max_by_key(|&step| (steps[step], std::cmp::Reverse(step)))
            .filter(|&step| steps[step] > 0)
            .or_else(|| (steps[1] > 0).then_some(1))
            .unwrap_or(DEFAULT_INDENT);
        " ".repeat(step)
    }

    /// ⌘]: one `unit` more at the start of every line the selection touches,
    /// or of the cursor's line. Blank lines among several are left alone, so
    /// indenting a block leaves no trailing spaces behind. One undo step; the
    /// selection keeps hold of the same text.
    pub fn indent_lines(&mut self, unit: &str) {
        let added = isize::try_from(unit.chars().count()).unwrap_or(0);
        self.reindent(|line, several| {
            if several && line.trim().is_empty() {
                (line.to_owned(), 0)
            } else {
                (format!("{unit}{line}"), added)
            }
        });
    }

    /// ⌘[: one level less at the start of every line the selection touches:
    /// `unit` where the line starts with it, else a tab, else as many spaces
    /// as there are up to `unit`'s width. A line with none is left as it is.
    /// One undo step.
    pub fn outdent_lines(&mut self, unit: &str) {
        let width = if unit == "\t" {
            DEFAULT_INDENT
        } else {
            unit.chars().count()
        };
        self.reindent(|line, _| {
            let taken = if !unit.is_empty() && line.starts_with(unit) {
                unit.chars().count()
            } else if line.starts_with('\t') {
                1
            } else {
                line.chars().take(width).take_while(|c| *c == ' ').count()
            };
            (
                line.chars().skip(taken).collect(),
                -isize::try_from(taken).unwrap_or(0),
            )
        });
    }

    /// Rewrites each line the selection touches with `change`, which is
    /// handed the line without its break and whether several are being
    /// changed, and answers the new line and how many characters it gained at
    /// its start (negative for lost).
    fn reindent(&mut self, change: impl Fn(&str, bool) -> (String, isize)) {
        let range = self.selection();
        let first = self.rope.char_to_line(range.start);
        let mut last = self.rope.char_to_line(range.end);
        // A selection that ends at the start of a line does not take it in.
        if range.end > range.start && last > first && range.end == self.rope.line_to_char(last) {
            last -= 1;
        }
        let several = last > first;
        let start = self.rope.line_to_char(first);
        let end = self.line_end_char(last);

        let mut lines = Vec::new();
        let mut shifts = Vec::new();
        for line in first..=last {
            let (text, shift) = change(&self.line_text(line), several);
            lines.push(text);
            shifts.push(shift);
        }
        if shifts.iter().all(|shift| *shift == 0) {
            return;
        }

        // Where each end of the selection lands once its line has moved.
        let selecting = range.end > range.start;
        let place = |pos: usize, keep_line_start: bool| -> usize {
            let line = self.rope.char_to_line(pos).clamp(first, last);
            let line_start = self.rope.line_to_char(line);
            let column = isize::try_from(pos.saturating_sub(line_start)).unwrap_or(0);
            let before: isize = shifts[..line - first].iter().sum();
            let shift = shifts[line - first];
            // A selection from the start of a line keeps the new indent in
            // it, so pressing again goes on moving the whole line.
            let column = if keep_line_start && column == 0 {
                0
            } else {
                (column + shift).max(0)
            };
            let moved = isize::try_from(line_start).unwrap_or(0) + before + column;
            usize::try_from(moved).unwrap_or(0)
        };
        let anchor = place(self.anchor, selecting && self.anchor == range.start);
        let cursor = place(self.cursor, selecting && self.cursor == range.start);

        self.replace_range(start..end, &lines.join("\n"), EditKind::Other);
        self.set_selection(anchor, cursor);
        if let Some(top) = self.undo_stack.back_mut() {
            top.anchor_after = self.anchor;
            top.cursor_after = self.cursor;
        }
    }

    /// Replaces `range` with `text` as a revision of the edit on top of the
    /// undo stack, when that edit is what put exactly `range` there.
    ///
    /// For text that is rewritten as it arrives — dictation, whose words firm
    /// up as more is heard. The top entry is amended rather than a new one
    /// pushed, so however many revisions there were, one undo takes the text
    /// back to what was there before the first. When the top entry is not the
    /// one that wrote `range` — something else was edited in between — this is
    /// an ordinary replacement and its own undo step.
    pub fn revise(&mut self, range: Range<usize>, text: &str) {
        let current = self.rope.slice(range.clone()).to_string();
        // Never an empty range: a deletion's entry also inserted nothing at
        // its position, and amending that would fold the revision into it.
        let amends = !current.is_empty()
            && self.redo_stack.is_empty()
            && self
                .undo_stack
                .back()
                .is_some_and(|top| top.at == range.start && top.inserted == current);
        if !amends {
            self.replace_range(range, text, EditKind::Other);
            return;
        }

        self.splice(range.clone(), text);
        let cursor_after = range.start + text.chars().count();
        if let Some(top) = self.undo_stack.back_mut() {
            top.inserted = text.to_owned();
            top.cursor_after = cursor_after;
            top.anchor_after = cursor_after;
        }
        self.cursor = cursor_after;
        self.anchor = cursor_after;
        self.goal_column = None;
        self.coalescing = false;
        self.dirty = true;
    }

    // -- Undo/redo -----------------------------------------------------------

    /// Reverts the most recent undo entry, restoring the cursor and selection
    /// to what they were before it made its first (or only, if coalesced)
    /// change. Returns `false` and does nothing if there is no undo history.
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop_back() else {
            return false;
        };
        let inserted_len = entry.inserted.chars().count();
        self.splice(entry.at..entry.at + inserted_len, &entry.removed);
        self.cursor = entry.cursor_before;
        self.anchor = entry.anchor_before;
        self.goal_column = None;
        self.coalescing = false;
        self.dirty = true;
        self.redo_stack.push(entry);
        true
    }

    /// Re-applies the most recently undone entry. Returns `false` and does
    /// nothing if there is nothing to redo, including when a new edit was
    /// made since the last undo (which clears the redo stack, as usual).
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        let removed_len = entry.removed.chars().count();
        self.splice(entry.at..entry.at + removed_len, &entry.inserted);
        self.cursor = entry.cursor_after;
        self.anchor = entry.anchor_after;
        self.goal_column = None;
        self.coalescing = false;
        self.dirty = true;
        self.undo_stack.push_back(entry);
        true
    }

    /// Whether [`Buffer::undo`] would do anything.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether [`Buffer::redo`] would do anything.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    // -- Find ----------------------------------------------------------------

    /// Whether `needle` matches the buffer starting at `start`.
    fn matches_at(&self, start: usize, needle: &[char], case: Case) -> bool {
        if start + needle.len() > self.rope.len_chars() {
            return false;
        }
        let mut haystack = self.rope.chars_at(start);
        needle
            .iter()
            .all(|&nc| haystack.next().is_some_and(|hc| chars_equal(hc, nc, case)))
    }

    /// Every literal match of `needle`, capped at [`MAX_FIND_MATCHES`] kept
    /// ranges. Empty for an empty `needle`.
    pub fn find_all(&self, needle: &str, case: Case) -> FindMatches {
        let needle: Vec<char> = needle.chars().collect();
        let len = self.rope.len_chars();
        if needle.is_empty() || needle.len() > len {
            return FindMatches {
                ranges: Vec::new(),
                total: 0,
                truncated: false,
            };
        }

        let mut ranges = Vec::new();
        let mut total = 0usize;
        for start in 0..=(len - needle.len()) {
            if self.matches_at(start, &needle, case) {
                total += 1;
                if ranges.len() < MAX_FIND_MATCHES {
                    ranges.push(start..start + needle.len());
                }
            }
        }

        FindMatches {
            truncated: total > ranges.len(),
            ranges,
            total,
        }
    }

    /// The next match at or after `from`, wrapping to the start of the
    /// buffer if none is found before the end. `None` if `needle` does not
    /// occur at all, or is empty.
    ///
    /// `from` is inclusive: pass the *end* of the previous match, not its
    /// start, to advance past it rather than finding it again.
    pub fn find_next(&self, needle: &str, case: Case, from: usize) -> Option<Range<usize>> {
        let needle: Vec<char> = needle.chars().collect();
        let len = self.rope.len_chars();
        if needle.is_empty() || needle.len() > len {
            return None;
        }
        let max_start = len - needle.len();
        let from = from.min(max_start);

        (from..=max_start)
            .chain(0..from)
            .find(|&start| self.matches_at(start, &needle, case))
            .map(|start| start..start + needle.len())
    }

    /// The previous match at or before `from`, wrapping to the end of the
    /// buffer if none is found before the start. `None` if `needle` does not
    /// occur at all, or is empty.
    ///
    /// `from` is inclusive: pass the *start* of the previous match, not its
    /// end, to move further back rather than finding it again.
    pub fn find_previous(&self, needle: &str, case: Case, from: usize) -> Option<Range<usize>> {
        let needle: Vec<char> = needle.chars().collect();
        let len = self.rope.len_chars();
        if needle.is_empty() || needle.len() > len {
            return None;
        }
        let max_start = len - needle.len();
        let from = from.min(max_start);

        (0..=from)
            .rev()
            .chain((from + 1..=max_start).rev())
            .find(|&start| self.matches_at(start, &needle, case))
            .map(|start| start..start + needle.len())
    }
}
