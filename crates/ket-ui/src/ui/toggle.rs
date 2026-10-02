//! Switches and checkboxes: a yes or no, thrown with one press.
//!
//! A switch is for a setting that takes effect as it is thrown; a checkbox is
//! for a choice inside a form that is applied with the form. Both come back
//! without a handler, so a read-only view — a config that failed to load —
//! draws the same control with no way to write through it.

use gpui::{Div, ElementId, SharedString, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::icon::{Icon, sized_icon};
use super::{LABEL, RADIUS_SM};
use crate::fonts::Prose;
use crate::paint::paint;

/// A switch's track.
const TRACK_W: gpui::Pixels = px(32.0);
const TRACK_H: gpui::Pixels = px(18.0);

/// Its knob, and how far it stands in from the track's edge.
const KNOB: gpui::Pixels = px(14.0);
const KNOB_INSET: gpui::Pixels = px(1.0);

/// A checkbox's box.
const BOX: gpui::Pixels = px(16.0);

/// A switch: on is the accent track with a dark knob at the end, off is a
/// well with a dim knob at the start.
pub(crate) fn switch(id: impl Into<ElementId>, on: bool, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .w(TRACK_W)
        .h(TRACK_H)
        .px(KNOB_INSET)
        .rounded_full()
        .cursor_pointer()
        .bg(paint(if on { t.accent } else { t.sunken }))
        .border_1()
        .border_color(paint(if on { t.accent } else { t.border }))
        .when(on, |el| el.justify_end())
        .child(
            div()
                .size(KNOB)
                .rounded_full()
                .bg(paint(if on { t.on_accent } else { t.text.dim })),
        )
}

/// The box alone: accent with a check when on, a well with an edge when off.
pub(crate) fn check_box(on: bool, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(BOX)
        .rounded(RADIUS_SM)
        .bg(paint(if on { t.accent } else { t.sunken }))
        .border_1()
        .border_color(paint(if on { t.accent } else { t.border }))
        .children(on.then(|| sized_icon(Icon::Check, px(12.0), paint(t.on_accent))))
}

/// A checkbox with its label: the whole line is the target, and the label is
/// a sentence, so it takes the prose face.
pub(crate) fn checkbox(
    id: impl Into<ElementId>,
    on: bool,
    label: impl Into<SharedString>,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .prose()
        .flex()
        .items_center()
        .gap(px(9.0))
        .px(px(6.0))
        .py(px(4.0))
        .rounded(super::RADIUS_MD)
        .cursor_pointer()
        .text_size(LABEL)
        .text_color(paint(t.text.primary))
        .hover(|style| style.bg(paint(t.hover)))
        .child(check_box(on, t))
        .child(label.into())
}
