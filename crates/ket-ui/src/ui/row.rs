//! A selectable row in a list.
//!
//! The sidebar, the explorer tree, the tab strip and both pickers each had
//! their own. The differences were not deliberate — one had a hover state,
//! three did not, and the two that showed selection did it at different
//! heights.

use gpui::{Div, ElementId, Pixels, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::{ICON_GAP, LABEL, PAD_X, ROW_H};
use crate::paint::paint;

/// A row at the standard height. Attach `on_click` to what comes back.
pub(crate) fn row(id: impl Into<ElementId>, selected: bool, t: &Theme) -> Stateful<Div> {
    sized_row(id, selected, ROW_H, t)
}

/// A row that heads a group rather than belonging to one.
///
/// The sidebar's project rows: clickable, never selected, and deliberately
/// without a hover fill — a heading that grows a slab behind it whenever the
/// pointer crosses the list is the sort of restlessness a sidebar is read
/// past, and the actions on its right carry their own hover wells already.
///
/// A separate constructor rather than a flag, because `gpui` panics outright
/// on a second `hover` — a caller cannot take the fill back off, only decline
/// to ask for it.
///
/// Its horizontal padding is the caller's to set. A row's padding exists to
/// hold its content off the edge of the fill behind it, and with no fill
/// there is nothing to hold it off; what matters instead is that the badge
/// lines up with the label above it and the buttons at the far end line up
/// with the buttons above *them*, and both of those columns belong to the
/// sidebar, not to this row. The default [`PAD_X`] stays only so a caller
/// that sets nothing gets a row and not a clipping error.
pub(crate) fn heading_row(id: impl Into<ElementId>, t: &Theme) -> Stateful<Div> {
    bare_row(id, false, ROW_H, t)
}

/// A row at a height of the caller's choosing, for the denser lists.
///
/// A rounded fill with no hairline, the way every list in the shell reads
/// now: rows are told apart by the air and the hover, not by a rule under
/// each one — see `design/STYLE-GUIDE.md`. A selected row is not washed
/// again under the pointer.
pub(crate) fn sized_row(
    id: impl Into<ElementId>,
    selected: bool,
    height: Pixels,
    t: &Theme,
) -> Stateful<Div> {
    let el = bare_row(id, selected, height, t).rounded(super::RADIUS_SM);
    if selected {
        el
    } else {
        el.hover(|style| style.bg(paint(t.hover)))
    }
}

/// Everything a row is apart from its hover fill — see [`heading_row`].
fn bare_row(id: impl Into<ElementId>, selected: bool, height: Pixels, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(ICON_GAP)
        .h(height)
        .px(PAD_X)
        .text_size(LABEL)
        .text_color(paint(t.text.primary))
        .cursor_pointer()
        .when(selected, |el| el.bg(paint(t.selection)))
}

/// [`block_row`]'s leading padding.
///
/// Named because something else has to know it: the subagent tree under a
/// worktree hangs its guide from the centre of the row's mark, and that centre
/// is this, plus half the mark.
pub(crate) const BLOCK_LEAD: Pixels = px(12.0);

/// [`block_row`]'s vertical padding, for the same reason as [`BLOCK_LEAD`]:
/// the tree's guide leaves the row through it.
pub(crate) const BLOCK_PAD_Y: Pixels = px(9.0);

/// A row that holds more than one line — a worktree's branch above its
/// detail — as a rounded fill with air around it rather than a cell split
/// from its neighbours by a hairline.
///
/// Height comes from the content rather than being fixed: a two-line row
/// forced to [`ROW_H`] either clips or leaves the second line hanging out.
///
/// Selected is the hover step's fill plus the amber rail
/// ([`super::chip::marker`]), which the row draws itself so every caller
/// gets the same one. Hover is a lighter wash than that, so pointing at a
/// row never looks like choosing it.
pub(crate) fn block_row(id: impl Into<ElementId>, selected: bool, t: &Theme) -> Stateful<Div> {
    let el = div()
        .id(id)
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .pl(BLOCK_LEAD)
        .pr(px(10.0))
        .py(BLOCK_PAD_Y)
        .rounded(ROW_RADIUS)
        .text_size(LABEL)
        .text_color(paint(t.text.primary))
        .cursor_pointer()
        .when(selected, |el| {
            el.bg(paint(t.hover)).child(super::chip::marker(t))
        });
    if selected {
        el
    } else {
        el.hover(|style| style.bg(crate::paint::alpha(paint(t.hover), 0.55)))
    }
}

/// A list row's corner: a step rounder than a control, a step squarer than a
/// float.
pub(crate) const ROW_RADIUS: Pixels = px(8.0);
