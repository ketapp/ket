//! The tweak editor pane: cursor, selection, find, save — and
//! nothing past that. It deliberately does not grow into more (no LSP, no
//! completions, no multi-cursor, no project-wide search). "A little bit of an editor" is the
//! whole design; the thing that makes the fence tolerable rather than merely
//! restrictive is that [`Shell::reveal_externally`] hands a real editor the
//! exact cursor position one keystroke away, via [`ket_core::surface`], which
//! already does the hard part of that handoff.
//!
//! # Where the buffer lives
//!
//! A [`crate::tabs::Tab`] must stay `Clone + PartialEq + Eq` for the tab
//! strip's own bookkeeping, and a
//! [`ket_core::buffer::Buffer`] is neither cheap to clone nor comparable. So
//! [`crate::tabs::TabKind::Editor`] carries only a cheap stable key. Saved
//! files use their absolute path; an Untitled buffer uses an in-process
//! identity. The buffer plus scroll, wrap and find state lives in
//! [`EditorState`], keyed by that identity in [`Shell::editors`].
//!
//! # Keeping up with other writers
//!
//! ket is never the only thing writing the files it shows. The agent in the
//! terminal beside this pane edits them, git rewrites them when a branch
//! moves under a worktree, and the person may have the same file open in a
//! real editor — none of which tells the shell anything. Because
//! [`EditorState`] is keyed by path and outlives its tab, a buffer read once
//! would otherwise stay the version it was read as for the life of the
//! process, closing and reopening the tab included.
//!
//! So each state remembers the [`FileStamp`] of the version it holds, and
//! [`EditorState::refresh_from_disk`] re-reads when disk has moved past it:
//! on opening a tab, and on the poll in `main.rs` for tabs already on
//! screen. What it will not do — clobber unsaved edits, reload a write that
//! changed nothing, or show an error for a file that is mid-write — is on
//! that method.
//!
//! # On soft wrap
//!
//! [`uniform_list`] measures one row and assumes every other row is exactly that
//! tall, which is what makes scrolling a large file cheap. So a line that
//! wraps is not one row grown taller: each wrapped segment is a row of its
//! own, still exactly one line tall.
//!
//! Where a segment breaks is a character column — [`Geometry::columns`],
//! the pane's measured text width divided by the advance of one character.
//! That division is exact rather than approximate because the code face is
//! monospace by construction (`fonts.rs` picks it from a list of monospace
//! families and bundles one as the floor). Lines break after the last space
//! that fits, and mid-word only when a single word is wider than the pane.
//! [`LineIndex::ensure`] documents what a rebuild costs and when it is paid.
//!
//! The quick prompt's draft is the one buffer that is not on that grid: it is
//! set in the prose face, large, because it is somebody writing a sentence
//! rather than reading code. Dividing by one character's advance would wrap
//! proportional text at the width of a line of `m`s, so that buffer wraps
//! against the sum of its characters' real advances instead — see
//! [`WrapWidth`].
//!
//! # How a row is drawn
//!
//! Not as a row of styled `div`s, one per highlighted span, but as a single
//! shaped line — `RowText` in `row.rs`, a `gpui` element of its own — with the
//! selection and the find matches carried as colours on that shaping's runs.
//! Composing a line out of boxes was tried first and is wrong twice over.
//! Each box shapes and rounds its own piece, so the glyphs shifted every time
//! a selection grew across them; and the x a click had to be measured against
//! was then an estimate made separately from the one the text was drawn with,
//! so the two drifted apart across a line and the selection landed beside the
//! pointer rather than under it. One [`ShapedLine`] answers both questions,
//! and answers them with the same numbers.
//!
//! # The rail beside the numbers
//!
//! The air between the line numbers and the code carries one more thing: a
//! bar against every line that differs from the version `HEAD` holds, green
//! for a line that is new, amber for one that stands where another did, and a
//! red stub on the boundary a run of deleted lines was taken from.
//!
//! Against `HEAD` rather than against the worktree's base, which is what the
//! diff pane and the sidebar's `+/−` read against — see
//! [`ket_core::diff::line_marks`] for why the two questions differ — and
//! against the buffer rather than the file on disk, so an insertion at the top
//! of a file moves every mark below it as it is typed rather than at the next
//! save.
//!
//! The read is off the window's thread and debounced, and the ten-second tick
//! that keeps every other view current re-runs it: a commit made in the
//! terminal beside this pane changes every mark without touching a file.
//!
//! # Known simplifications
//!
//! Kept deliberately simple rather than gold-plated, each for a reason:
//!
//! - **A drag does not auto-scroll.** Extending a selection past the top or
//!   bottom of the pane stops at the last row the pointer is actually over.
//!   `Shift` with the movement keys extends a selection as far as wanted,
//!   and does scroll, so nothing is out of reach.
//! - **Find is case-insensitive with no toggle.** One search mode is enough
//!   for a tweak; [`ket_core::buffer::Buffer`] supports case-sensitive find
//!   already, so adding a toggle later costs a checkbox, not a redesign.
//! - **Save is not conflict-checked.** [`ket_core::buffer::Buffer::save`] is
//!   a plain write with no stamp to compare against
//!   [`ket_core::surface::write_if_unchanged`]'s; that is `Buffer`'s own
//!   design (it is not this file's to change), and adding a parallel
//!   conflict-detection path here would duplicate rather than reuse it.
//!
//! [`Shell::reveal_externally`]: crate::Shell::reveal_externally
//! [`Shell::editors`]: crate::Shell::editors
//! [`FileStamp`]: ket_core::surface::FileStamp
//! [`uniform_list`]: gpui::uniform_list
//! [`Geometry::columns`]: geometry::Geometry::columns
//! [`LineIndex::ensure`]: line_index::LineIndex::ensure
//! [`WrapWidth`]: geometry::WrapWidth
//! [`ShapedLine`]: gpui::ShapedLine

mod composer;
mod find;
mod geometry;
mod highlight;
mod keys;
mod line_index;
mod open;
mod pane;
mod row;
mod save;
mod selection;
mod state;
mod syntax;

use gpui::{Pixels, px};

pub(crate) use self::geometry::code_cell_width;
pub(crate) use self::state::EditorState;

/// Width of the gutter where it carries line numbers: room for four digits
/// right-aligned, plus [`GUTTER_GAP`] of air before the code starts.
const GUTTER_WIDTH: Pixels = px(64.0);

/// How far the code sits from the last digit of its line number. Wide enough
/// that the numbers read as a column beside the text rather than as the first
/// token of it.
const GUTTER_GAP: Pixels = px(16.0);

/// Width of the gutter where it does not: enough that the caret at column
/// zero is not flush against the pane's edge, and no more.
const PLAIN_MARGIN: Pixels = px(12.0);

pub(crate) const QUICK_PROMPT_EDITOR: &str = "quick-prompt:draft";

/// The size a composer's draft is set at: a prompt is the one thing in its
/// dialog, and it is written in sentences, so it gets a reading size rather
/// than the code face's.
pub(crate) const COMPOSER_TEXT: Pixels = px(20.0);

/// How wide the caret is drawn.
const CARET_WIDTH: Pixels = px(2.0);
