//! Every line's syntax highlighting, kept current one edit at a time.

use std::ops::Range;

use ket_core::buffer::{Buffer, LineEdit};

use super::line_index::grow_dirty;

/// Every buffer line's highlighted spans, in character offsets.
///
/// A block comment is open at the end of one line and still open at the start
/// of the next, and the editor draws rows out of order — so a row cannot be
/// coloured without knowing what the rows above it left open. That is what
/// keeping every line's opening state buys: a row somebody scrolled to is a
/// lookup here, not a re-lex plus a byte-to-character conversion redone on
/// every frame it happens to be on screen.
///
/// Kept current one edit at a time rather than re-lexed whole: an edit
/// re-lexes the lines it touched and carries on down only while what it left
/// open differs from what the next line was lexed with — which for ordinary
/// typing is one line, however long the file. The whole document is lexed
/// only for the first build, or when [`Buffer::line_edits_since`] cannot say
/// what changed.
#[derive(Debug, Default)]
pub(super) struct SyntaxIndex {
    /// The [`Buffer::revision`] this was built for; `None` before the first
    /// build.
    revision: Option<u64>,
    /// The language this was built for.
    language: Option<&'static ket_core::syntax::Language>,
    /// Each line's opening state — what the lines above it left open.
    states: Vec<ket_core::syntax::State>,
    /// Each line's highlighted spans, in document order.
    spans: Vec<ket_core::syntax::Spans>,
}

impl SyntaxIndex {
    /// Brings the spans up to date with `buffer`, and does nothing at all
    /// when they already are — the usual case, since this is asked on every
    /// frame and most frames edit nothing.
    pub(super) fn ensure(
        &mut self,
        buffer: &Buffer,
        language: Option<&'static ket_core::syntax::Language>,
    ) {
        let Some(language) = language else {
            self.revision = None;
            self.states.clear();
            self.spans.clear();
            return;
        };
        let same_language = self
            .language
            .is_some_and(|built| std::ptr::eq(built, language));
        let revision = buffer.revision();
        if same_language && self.revision == Some(revision) {
            return;
        }
        let edits = self
            .revision
            .filter(|_| same_language)
            .and_then(|built| buffer.line_edits_since(built));
        self.revision = Some(revision);
        self.language = Some(language);

        if !edits.is_some_and(|edits| self.patch(buffer, language, &edits)) {
            let (states, spans) = ket_core::syntax::highlighted_lines(&buffer.text(), language);
            self.states = states;
            self.spans = spans;
        }
    }

    /// Re-lexes what `edits` touched. Answers `false` when the index and the
    /// buffer disagree about the document's shape, which leaves the index for
    /// [`SyntaxIndex::ensure`] to rebuild whole.
    fn patch(
        &mut self,
        buffer: &Buffer,
        language: &'static ket_core::syntax::Language,
        edits: &[LineEdit],
    ) -> bool {
        use ket_core::syntax::{State, highlighted_line};

        let mut dirty = None;
        for edit in edits {
            let replaced = edit.start..edit.start + edit.removed;
            if replaced.end > self.states.len() {
                return false;
            }
            // Placeholders: every one of these lines is in `dirty`, so each
            // is lexed below before anything reads it.
            self.states.splice(
                replaced.clone(),
                std::iter::repeat_n(State::Code, edit.inserted),
            );
            self.spans.splice(
                replaced,
                std::iter::repeat_with(Vec::new).take(edit.inserted),
            );
            dirty = Some(grow_dirty(dirty, edit));
        }
        let line_count = buffer.line_count();
        if self.states.len() != line_count {
            return false;
        }
        let Some(dirty) = dirty else {
            return true;
        };

        // What the first dirty line opens in is what the untouched line above
        // it closed in, and only a lex of that line says so.
        let mut state = match dirty.start.checked_sub(1) {
            Some(above) => {
                highlighted_line(&buffer.line_text(above), self.states[above], language).1
            }
            None => State::Code,
        };
        for line in dirty.start..line_count {
            // Past the edit, a line opening in what it was last lexed with
            // colours as it did — and so does every line below it.
            if line >= dirty.end && self.states[line] == state {
                break;
            }
            let (spans, next) = highlighted_line(&buffer.line_text(line), state, language);
            self.states[line] = state;
            self.spans[line] = spans;
            state = next;
        }
        true
    }

    /// This line's highlighted spans, in character offsets.
    pub(super) fn at(&self, line: usize) -> &[(Range<usize>, ket_core::syntax::Token)] {
        self.spans.get(line).map_or(&[], Vec::as_slice)
    }
}
