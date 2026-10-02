//! The modal scaffold.
//!
//! Four dialogs built this by hand, in two houses that contradicted each
//! other: two were `rounded_lg` with a `selection` border on `surface`, two
//! were `rounded_md` with a `focus_ring` border on `panel`. All four also
//! re-declared the full-window absolute wrapper that `Shell::render` already
//! puts them inside, which is why the scrim was drawn twice.

use gpui::{
    AnyElement, Div, ElementId, FontWeight, Pixels, SharedString, Stateful, div, prelude::*, px,
};
use ket_core::theme::Theme;

use super::{CARD_PAD, RADIUS_LG, TITLE};
use crate::fonts::Prose;
use crate::paint::paint;

/// Centres a dialog in the window.
///
/// The scrim is not here: `Shell::render` washes everything behind whichever
/// modal is up, once, so a dialog never dims the window a second time.
pub(crate) fn centered(child: impl IntoElement) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(child)
}

/// The dialog's slab. Add the header, the body and the footer to it.
///
/// Set in the prose face, which everything inside inherits: a dialog is
/// sentences and choices to read, not a column of identifiers to compare. A
/// part that must stay monospace asks for it itself.
pub(crate) fn card(id: impl Into<ElementId>, width: Pixels, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .prose()
        .occlude()
        .flex()
        .flex_col()
        .w(width)
        .p(CARD_PAD)
        .gap(px(15.0))
        .rounded(RADIUS_LG)
        .border_1()
        .border_color(paint(t.border))
        .bg(paint(t.elevated))
        .shadow_lg()
}

/// A dialog's title row. Append the close button to it.
pub(crate) fn header(title: impl Into<SharedString>, t: &Theme) -> Div {
    header_with(title, div(), t)
}

/// [`header`] with something set straight after the title rather than out at
/// the far end — the project a worktree is being created in, so the title
/// reads as one sentence. It takes the title's size and weight.
pub(crate) fn header_with(
    title: impl Into<SharedString>,
    after: impl IntoElement,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(TITLE)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(paint(t.text.primary))
                .child(title.into())
                .child(after),
        )
        .child(div().flex_grow())
}

/// The row a dialog's buttons sit in: trailing-aligned, with the cancel first
/// and the action last, which is the order a Mac reads them in.
pub(crate) fn footer() -> Div {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(10.0))
        .pt(px(6.0))
}

/// A quiet block explaining what the dialog is about to do.
pub(crate) fn body(text: impl Into<SharedString>, t: &Theme) -> AnyElement {
    div()
        .text_size(super::LABEL)
        .text_color(paint(t.text.dim))
        .child(text.into())
        .into_any_element()
}
