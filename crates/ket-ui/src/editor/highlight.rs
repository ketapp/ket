//! Highlighting a line: splitting a row's text into runs for the
//! selection, the find matches and the syntax colours it carries.

use std::collections::BTreeSet;
use std::ops::Range;

use gpui::{Font, FontStyle, FontWeight, Hsla};

use ket_core::theme::Theme;

use crate::paint::{alpha, paint};

/// How a slice of a rendered line is highlighted, ranked so the strongest
/// wins where spans from different sources — a selection, a find match —
/// overlap. Ord is derived from declaration order, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Mark {
    /// No highlight.
    Plain,
    /// One of the current find query's matches, but not the one navigation
    /// last landed on.
    Match,
    /// Inside the buffer's selection.
    Selection,
    /// The find match navigation last landed on.
    CurrentMatch,
    /// What the highlighter made of this run.
    ///
    /// The lowest precedence of the four: a selection over a keyword is a
    /// selection, because what somebody has *chosen* outranks what the text
    /// happens to be. Plain outranks it only in the sense that a run with no
    /// token is plain.
    Syntax(ket_core::syntax::Token),
}

impl Mark {
    /// The text and background colours this mark paints with, against the
    /// pane's inherited text colour.
    pub(super) fn colors(self, t: &Theme, plain: Hsla) -> (Hsla, Option<Hsla>) {
        match self {
            Mark::Plain => (plain, None),
            // An accent text colour, not a background — the same convention
            // `palette.rs` uses for a fuzzy match's lit characters.
            Mark::Match => (paint(t.diff.hunk_header).into(), None),
            // Not `t.selection`: that is the wash a chosen row takes, and
            // a row is a whole bar of colour where this is a handful of
            // characters. Selected text has to be obvious at a glance and
            // still legible through the wash, which is what this alpha buys.
            Mark::Selection => (plain, Some(alpha(paint(t.accent), 0.20).into())),
            Mark::CurrentMatch => (paint(t.on_accent).into(), Some(paint(t.accent).into())),
            Mark::Syntax(token) => {
                use ket_core::syntax::Token;
                let colour = match token {
                    Token::Keyword => t.syntax.keyword,
                    Token::Str => t.syntax.string,
                    Token::Comment => t.syntax.comment,
                    Token::Number => t.syntax.number,
                    Token::Kind => t.syntax.kind,
                    Token::Punctuation => t.syntax.punctuation,
                };
                (paint(colour).into(), None)
            }
        }
    }
}

impl Mark {
    /// The face this mark is set in, over the pane's own.
    ///
    /// This is the half of the syntax scheme that is not colour. `keyword` is
    /// the window's own ink — the same value as ordinary text — so what tells
    /// `pub fn` from an identifier beside it is that it is heavier, and a
    /// comment is the one run in a file that slants. Three hues and two
    /// weights, rather than six hues.
    ///
    /// Safe for the column arithmetic because the family is monospaced:
    /// Monaspace Neon's semibold and italic faces advance exactly as far as its
    /// regular one, so a heavier keyword does not move the caret.
    pub(super) fn font(self, base: &Font) -> Font {
        use ket_core::syntax::Token;
        match self {
            Mark::Syntax(Token::Keyword) => Font {
                weight: FontWeight::SEMIBOLD,
                ..base.clone()
            },
            Mark::Syntax(Token::Comment) => Font {
                style: FontStyle::Italic,
                ..base.clone()
            },
            _ => base.clone(),
        }
    }
}

/// Splits `len` characters — already in this row's *local* coordinates —
/// into adjacent `(range, mark)` slices from the selection and the find
/// matches that fall on this line or wrap segment. Pure and `gpui`-free so
/// it can be unit tested directly, per the `editor` module doc's emphasis on
/// keeping the cheap, whole-document-adjacent logic separate from rendering.
pub(super) fn mark_line(
    len: usize,
    selection: Option<Range<usize>>,
    matches: &[(Range<usize>, bool)],
    syntax: &[(Range<usize>, ket_core::syntax::Token)],
) -> Vec<(Range<usize>, Mark)> {
    let mut cuts = BTreeSet::new();
    cuts.insert(0);
    cuts.insert(len);
    if let Some(r) = &selection {
        cuts.insert(r.start.min(len));
        cuts.insert(r.end.min(len));
    }
    for (r, _) in matches {
        cuts.insert(r.start.min(len));
        cuts.insert(r.end.min(len));
    }
    for (r, _) in syntax {
        cuts.insert(r.start.min(len));
        cuts.insert(r.end.min(len));
    }
    let cuts: Vec<usize> = cuts.into_iter().collect();

    cuts.windows(2)
        .filter(|w| w[0] < w[1])
        .map(|w| {
            let seg = w[0]..w[1];
            let covers = |r: &Range<usize>| r.start <= seg.start && seg.end <= r.end;
            let current = matches
                .iter()
                .any(|(r, is_current)| *is_current && covers(r));
            let is_match = matches.iter().any(|(r, _)| covers(r));
            let selected = selection.as_ref().is_some_and(covers);
            let mark = if current {
                Mark::CurrentMatch
            } else if selected {
                Mark::Selection
            } else if is_match {
                Mark::Match
            } else {
                // Last, and only where nothing a person did covers the run.
                syntax
                    .iter()
                    .find(|(r, _)| covers(r))
                    .map(|(_, token)| Mark::Syntax(*token))
                    .unwrap_or(Mark::Plain)
            };
            (seg, mark)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_line_marks_a_selection_over_plain_text() {
        let pieces = mark_line(10, Some(2..5), &[], &[]);
        assert_eq!(
            pieces,
            vec![
                (0..2, Mark::Plain),
                (2..5, Mark::Selection),
                (5..10, Mark::Plain)
            ]
        );
    }

    #[test]
    fn mark_line_ranks_the_current_match_above_a_selection_covering_it() {
        let pieces = mark_line(10, Some(2..5), &[(2..5, true)], &[]);
        assert_eq!(
            pieces,
            vec![
                (0..2, Mark::Plain),
                (2..5, Mark::CurrentMatch),
                (5..10, Mark::Plain)
            ]
        );
    }

    #[test]
    fn mark_line_keeps_a_non_current_match_distinct_from_a_disjoint_selection() {
        let pieces = mark_line(10, Some(6..8), &[(1..3, false)], &[]);
        assert_eq!(
            pieces,
            vec![
                (0..1, Mark::Plain),
                (1..3, Mark::Match),
                (3..6, Mark::Plain),
                (6..8, Mark::Selection),
                (8..10, Mark::Plain),
            ]
        );
    }

    #[test]
    fn mark_line_on_an_empty_line_has_nothing_to_split() {
        assert_eq!(mark_line(0, None, &[], &[]), Vec::new());
    }
}
