//! A figure: a label over its value — "Next start" over "Tonight, 2:00 AM",
//! "Changes" over `+128 −14`. The header's read-outs, the Savings popover's
//! and an automation's are all this.

use gpui::{Div, FontWeight, IntoElement, ParentElement, Pixels, SharedString, Styled, div, px};
use ket_core::theme::Theme;

use super::LABEL;
use crate::fonts::Prose;
use crate::paint::paint;

/// A value's size: bigger than the chrome around it, since the numbers are
/// what a figure is for.
const VALUE: Pixels = px(14.0);

/// A label in the prose face over its value.
pub(crate) fn figure(label: impl Into<SharedString>, value: impl IntoElement, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .prose()
                .text_size(LABEL)
                .text_color(paint(t.text.dim))
                .child(label.into()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(VALUE)
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.primary))
                .child(value),
        )
}
