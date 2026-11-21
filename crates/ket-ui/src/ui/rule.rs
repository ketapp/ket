//! The hairline between two parts of a surface.
//!
//! Each surface that wanted one drew its own 1 px line, so the line's
//! colour and whether it could be squeezed away were decided again each
//! time. These two draw it in `border`, which the theme keeps for parting
//! two different things: two sections, two columns, two groups of
//! controls. A line between entries of one list is `rule`'s job, the
//! theme's quieter hairline, and stays with the list that draws it.

use gpui::{Div, Pixels, div, prelude::*, px};
use ket_core::theme::Theme;

use crate::paint::paint;

/// A rule's weight.
const WEIGHT: Pixels = px(1.0);

/// A line across its parent: between two sections, or the parts of a panel.
pub(crate) fn rule(t: &Theme) -> Div {
    div().flex_none().h(WEIGHT).bg(paint(t.border))
}

/// A line standing upright: between two columns, or two groups in a row of
/// controls. As tall as its row unless the caller gives it a height.
pub(crate) fn vertical_rule(t: &Theme) -> Div {
    div().flex_none().w(WEIGHT).bg(paint(t.border))
}
