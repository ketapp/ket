//! A small hover label anchored to one side of its trigger, fading in once
//! shown.
//!
//! Positioning is plain flexbox, not measurement: a zero-sized strip pinned
//! to the trigger's edge, `justify_end`/`justify_start` pushing the label's
//! overflow away from that edge, and `items_center` centring it on the cross
//! axis. That is enough geometry for a label a couple of words long, and it
//! costs nothing to compute — no measuring pass like `ui::popup`'s, which
//! exists for panels big enough to need one.
//!
//! Nothing here holds state: the caller decides `shown`, typically by
//! chaining `.on_hover(...)` onto the element this returns. Mounting the
//! label fresh each time it reappears is what restarts the fade — `gpui`'s
//! `AnimationElement` keys its clock to the element's id across frames, so an
//! id that drops out of the tree and comes back gets a new clock for free.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, Div, ElementId, Pixels, SharedString, Stateful, deferred,
    div, prelude::*, px,
};
use ket_core::theme::Theme;

use super::{CAPTION, RADIUS_MD};
use crate::fonts::Prose;
use crate::paint::paint;

/// Which side of the trigger the label appears on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // The component API is intentionally complete before every side has a caller.
pub(crate) enum Side {
    Top,
    Right,
    Bottom,
    Left,
    /// Above, with the label's right edge on the trigger's rather than
    /// centred on it — for a trigger near the window's right edge, where a
    /// centred label would run off the window. Nothing here measures the
    /// window, so the caller that knows where its trigger sits says so.
    TopEnd,
    /// Below, right edges aligned, for the same reason as [`Side::TopEnd`].
    BottomEnd,
}

/// Space between the label and its trigger.
const GAP: Pixels = px(6.0);

/// How long the label takes to fade in once shown.
const FADE_IN: Duration = Duration::from_millis(120);

/// Wraps `trigger` with a themed label that appears on `side` while `shown`,
/// fading in over [`FADE_IN`].
///
/// `id` must be unique per call site — it both makes the wrapper hoverable
/// and, combined with `side`, seeds the fade's clock.
pub(crate) fn tooltip(
    id: impl Into<ElementId>,
    trigger: AnyElement,
    label: impl Into<SharedString>,
    side: Side,
    shown: bool,
    t: &Theme,
) -> Stateful<Div> {
    let id = id.into();

    let bubble = shown.then(|| {
        let surface: Div = div()
            .flex_none()
            // A label is one line, always. Without this it is laid out
            // against whatever width the pin below happens to offer — and the
            // pin is deliberately zero-sized on one axis, so on the two sides
            // where that axis is the horizontal one the text had nothing to
            // measure against and came out a character per line. Saying it
            // here rather than arranging the flexbox around it means no side
            // can reintroduce that, and the bubble simply overflows the pin,
            // which is what it was always meant to do.
            .whitespace_nowrap()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(RADIUS_MD)
            .bg(paint(t.elevated))
            .border_1()
            .border_color(paint(t.border))
            // A label says what a control does, in words: the prose face.
            .prose()
            .text_size(CAPTION)
            .text_color(paint(t.text.primary))
            .shadow_lg()
            .child(label.into());

        // The gap sits as padding on whichever edge of this wrapper touches
        // the pin below, so the wrapper's own edge — not the surface's — is
        // what the pin's `justify_end`/`justify_start` aligns to.
        let spacer = match side {
            Side::Top | Side::TopEnd => div().pb(GAP),
            Side::Bottom | Side::BottomEnd => div().pt(GAP),
            Side::Left => div().pr(GAP),
            Side::Right => div().pl(GAP),
        }
        .child(surface)
        .flex_shrink_0()
        .with_animation(id.clone(), Animation::new(FADE_IN), |el, delta| {
            el.opacity(delta)
        });

        // A zero-sized strip pinned to the trigger's edge on `side`. Its main
        // axis spans the full width or height of the relative wrapper below
        // — the trigger's own box — so `justify_center` centres the label
        // against the trigger; `flex_shrink_0` on the spacer keeps that
        // centering from shrinking the label to fit, which is what wrapped
        // it down to one character per line when this lived on the cross
        // axis instead (cross-axis alignment measures text against the
        // container's own size, while the main axis respects the item's
        // intrinsic size and simply overflows it). The strip's cross axis is
        // zero, so `items_end`/`items_start` puts the spacer's near edge
        // exactly on the pin and lets the rest of it overflow away from the
        // trigger.
        let pin = div().absolute().flex();
        match side {
            Side::Top => pin
                .top_0()
                .left_0()
                .right_0()
                .h(px(0.0))
                .flex_row()
                .justify_center()
                .items_end(),
            Side::Bottom => pin
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(0.0))
                .flex_row()
                .justify_center()
                .items_start(),
            // As above, but `justify_end`: the label's right edge sits on the
            // trigger's, and what does not fit overflows to the left, away
            // from the window's edge.
            Side::TopEnd => pin
                .top_0()
                .left_0()
                .right_0()
                .h(px(0.0))
                .flex_row()
                .justify_end()
                .items_end(),
            Side::BottomEnd => pin
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(0.0))
                .flex_row()
                .justify_end()
                .items_start(),
            Side::Left => pin
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(0.0))
                .flex_col()
                .justify_center()
                .items_end(),
            Side::Right => pin
                .right_0()
                .top_0()
                .bottom_0()
                .w(px(0.0))
                .flex_col()
                .justify_center()
                .items_start(),
        }
        .child(spacer)
    });

    div()
        .id(id)
        .relative()
        .flex_none()
        .child(trigger)
        // Deferred, like `ui::menu`'s panel and for the same reason: the
        // sidebar this is usually hovered inside is a scroll container, and a
        // scroll container clips. Laid out in place, a label centred on a mark
        // near the panel's edge has the overhanging half cut off — which is
        // what centring it did, and why it spent a while anchored to the
        // trigger's edge instead. Painted in the deferred pass it is outside
        // that clip, so it can be centred like a tooltip should be.
        .children(bubble.map(|pin| deferred(pin).with_priority(1)))
}
