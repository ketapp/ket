//! A message that stays put inside a surface until what it reports changes.
//!
//! The in-place sibling of [`super::toast`]: a toast floats over the window
//! and leaves on its own, which suits news about something that already
//! happened. A banner sits in the dialog or panel it is about, in the flow,
//! for as long as the problem it names is still true — a refused permission,
//! a send that could not go anywhere. The caption-sized red line under a
//! field (`field::error`) was the only thing doing this, and a sentence that
//! explains what to change in System Settings is too easy to miss in it.
//!
//! Tone is the toast's, so the four tones mean the same hue and the same mark
//! in both places — see [`Tone`] for why hue never carries it alone.

use gpui::{AnyElement, Div, ElementId, SharedString, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::chip::kbd;
use super::icon::icon;
use super::toast::Tone;
use super::{ICON_GAP, LABEL, RADIUS_MD};
use crate::fonts::Prose;
use crate::paint::{alpha, paint};

/// One banner, as wide as the surface it sits in.
///
/// Append a dismiss button as a child when the reader should be able to put
/// it away before the problem clears; it lands at the trailing edge.
pub(crate) fn banner(
    id: impl Into<ElementId>,
    tone: Tone,
    message: impl Into<SharedString>,
    t: &Theme,
) -> Stateful<Div> {
    let colour = tone.colour(t);

    div()
        .id(id)
        .flex()
        .items_start()
        .gap(ICON_GAP)
        .w_full()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(RADIUS_MD)
        // A wash of the tone's own hue behind the words, and a border of it,
        // rather than a coloured sentence: the words stay at full ink, where
        // they are readable, and the box is what the eye catches.
        .bg(alpha(colour, 0.1))
        .border_1()
        .border_color(alpha(colour, 0.45))
        .child(
            // Nudged onto the first line's centre, as the toast's is.
            div()
                .flex_none()
                .mt(px(1.0))
                .child(icon(tone.icon(), colour)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .prose()
                .text_size(LABEL)
                .text_color(paint(t.text.primary))
                .child(words(message.into(), t)),
        )
}

/// The message, with anything it quotes in backticks — a branch, a path —
/// drawn as a [`kbd`] chip rather than as literal backticks.
///
/// Without a backtick it stays one run of text and wraps as text does. With
/// one, it is laid out a word at a time so a chip can sit inline: gpui has no
/// inline boxes, and a chip beside a single text run would be a column, not
/// part of the sentence.
fn words(message: SharedString, t: &Theme) -> AnyElement {
    if !message.contains('`') {
        return message.into_any_element();
    }

    // Runs that must not break apart: a word, or a chip with whatever is
    // written hard against it — `main`. keeps its full stop on its line.
    // Split on backticks first, so a quote with a space inside it is still
    // one chip.
    let mono = crate::fonts::chrome();
    let mut runs: Vec<Vec<AnyElement>> = Vec::new();
    let mut glued = false;
    for (index, part) in message.split('`').enumerate() {
        if index % 2 == 1 {
            let chip = kbd(part.to_owned(), mono.clone(), t).into_any_element();
            match runs.last_mut() {
                Some(run) if glued => run.push(chip),
                _ => runs.push(vec![chip]),
            }
            glued = true;
            continue;
        }
        let starts_apart = part.starts_with(char::is_whitespace);
        for (at, word) in part.split_whitespace().enumerate() {
            let word = word.to_owned().into_any_element();
            match runs.last_mut() {
                Some(run) if at == 0 && glued && !starts_apart => run.push(word),
                _ => runs.push(vec![word]),
            }
        }
        glued = !part.is_empty() && !part.ends_with(char::is_whitespace);
    }

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(3.5))
        .gap_y(px(3.0))
        .children(
            runs.into_iter()
                .map(|run| div().flex().items_center().children(run)),
        )
        .into_any_element()
}
