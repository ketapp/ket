//! Where a dragged tab lands in a pane, and how the landing is drawn: the
//! body's edge bands, the wash over the half a split would take, and the rule
//! in a strip's gap. Moved out of `tabs.rs`, which is over the line count.

use gpui::{AnyElement, Axis, Bounds, Pixels, Point, div, point, prelude::*, px, relative, size};
use ket_core::theme::Theme;

use super::{MIN_PANE_SIZE, PaneId, TabDrop};
use crate::paint::{alpha, paint};

/// How much of a pane's width or height each of its four edge bands takes.
const EDGE_FRACTION: f32 = 0.22;

/// The narrowest an edge band gets, so a small pane can still be aimed at.
const EDGE_MIN: Pixels = px(48.0);

/// The widest, so a large pane keeps a middle worth aiming at.
const EDGE_MAX: Pixels = px(220.0);

/// A pane's content area: everything it occupies but its tab strip.
///
/// The strip is a drop target in its own right — it names a gap between two
/// tabs — so the body's edge bands are measured against what is left once it
/// is taken off the top, and the wash is drawn there too. Otherwise the top
/// band would sit under the strip, and the pointer would be offered a division
/// while it hovered a gap.
pub(super) fn body_area(pane: Bounds<Pixels>) -> Bounds<Pixels> {
    let strip = crate::tree::TOP_STRIP.min(pane.size.height);
    Bounds {
        origin: point(pane.origin.x, pane.origin.y + strip),
        size: size(pane.size.width, pane.size.height - strip),
    }
}

/// Which drop a pointer inside a pane's body is asking for.
///
/// Every edge offers its distance as a fraction of its own band, and the
/// smallest fraction under one wins. Comparing raw distances instead would
/// make a wide, short pane resolve both of its corners to a horizontal
/// divider, because in such a pane the pointer is nearer the top or bottom
/// edge almost everywhere.
pub(super) fn body_drop(pane: PaneId, bounds: Bounds<Pixels>, at: Point<Pixels>) -> TabDrop {
    let band = |extent: Pixels| {
        let wanted = extent * EDGE_FRACTION;
        if wanted < EDGE_MIN {
            EDGE_MIN
        } else if wanted > EDGE_MAX {
            EDGE_MAX
        } else {
            wanted
        }
    };
    let across = band(bounds.size.width);
    let down = band(bounds.size.height);

    let edges = [
        (at.x - bounds.left(), across, Axis::Horizontal, true),
        (bounds.right() - at.x, across, Axis::Horizontal, false),
        (at.y - bounds.top(), down, Axis::Vertical, true),
        (bounds.bottom() - at.y, down, Axis::Vertical, false),
    ];

    // What this pane has left to give along each axis. A pane divided below
    // `MIN_PANE_SIZE` has no reliably clickable tab strip left, and a pane
    // that cannot be clicked is one that cannot be closed — so an edge with
    // no room behind it stops offering a division and offers the pane itself
    // instead. Height counts the strip the body does not include, because the
    // minimum is about a whole pane rather than about its content area.
    let divisible = |extent: Pixels| extent >= MIN_PANE_SIZE * 2.0;
    let across_fits = divisible(bounds.size.width);
    // `bounds` is the body, so the strip both panes would get goes back on.
    let down_fits = divisible(bounds.size.height + crate::tree::TOP_STRIP);

    let mut nearest: Option<(f32, Axis, bool)> = None;
    for (distance, band, axis, before) in edges {
        let fits = match axis {
            Axis::Horizontal => across_fits,
            Axis::Vertical => down_fits,
        };
        if !fits || band <= px(0.0) || distance >= band {
            continue;
        }
        let share = distance / band;
        if nearest.is_none_or(|(smallest, _, _)| share < smallest) {
            nearest = Some((share, axis, before));
        }
    }

    match nearest {
        Some((_, axis, before)) => TabDrop::Split { pane, axis, before },
        None => TabDrop::Into { pane },
    }
}

/// The wash showing what a dragged tab would take if it landed here.
///
/// `accent` rather than `selection`, so the preview does not read as the
/// focused-pane border it is drawn inside. A split shows the half it would
/// take rather than the share it would really get: in the flattening case that
/// share depends on how many children the *parent* has, which a pane cannot
/// see from inside itself, and half is what every editor with this gesture
/// draws.
pub(super) fn drop_wash(drop: TabDrop, t: &Theme) -> AnyElement {
    // Inset from the top by the strip's height, because the wash is a child of
    // the whole pane and belongs only over its body — see [`body_area`].
    let wash = div()
        .absolute()
        .top(crate::tree::TOP_STRIP)
        .left_0()
        .right_0()
        .bottom_0()
        .bg(alpha(paint(t.accent), 0.16))
        .border_1()
        .border_color(paint(t.accent));

    match drop {
        TabDrop::Split {
            axis: Axis::Horizontal,
            before,
            ..
        } => wash
            .when(before, |el| el.right(relative(0.5)))
            .when(!before, |el| el.left(relative(0.5))),
        TabDrop::Split {
            axis: Axis::Vertical,
            before,
            ..
        } => wash
            // Half of the *body*, so the inset above is doubled back out of the
            // half being taken — otherwise the top half would be short by a
            // strip and the bottom half long by one.
            .when(before, |el| {
                el.bottom(relative(0.5)).mb(crate::tree::TOP_STRIP / 2.0)
            })
            .when(!before, |el| {
                el.top(relative(0.5)).mt(crate::tree::TOP_STRIP / 2.0)
            }),
        TabDrop::Into { .. } | TabDrop::Strip { .. } => wash,
    }
    .into_any_element()
}

/// The rule drawn in the gap along a strip that a dragged tab would drop into.
///
/// Absolutely positioned, and that is the point: a rule taking part in the
/// layout would push every label along the strip aside as the pointer crossed
/// it, so the gap would move away from the pointer that was aiming at it.
pub(super) fn tab_rule(t: &Theme, trailing: bool) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(2.0))
        .bg(paint(t.accent))
        .when(trailing, |el| el.right(px(-1.0)))
        .when(!trailing, |el| el.left(px(-1.0)))
}
