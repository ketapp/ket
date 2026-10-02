//! The small stuff: key hints, project badges, status dots.
//!
//! Three keybinding chips existed with three different treatments, and the
//! menu drew its hints as bare text while the palette drew the same thing in a
//! box. A key hint means one thing wherever it appears, so it looks like one
//! thing wherever it appears.

use std::time::Duration;

use gpui::{
    AnyElement, Bounds, Div, PathBuilder, Pixels, Rgba, SharedString, Window, canvas, div, point,
    prelude::*, px,
};
use ket_core::theme::{Color, Theme, contrast_ratio};

use super::{CAPTION, RADIUS_SM};
use crate::fonts::Prose;
use crate::paint::{alpha, paint};

/// A key or chord, drawn as something you press.
///
/// `mono` is the shell's code face: a keybinding is a literal, and setting it
/// in the proportional face puts `⌘K` at a different width from `⌘N`.
pub(crate) fn kbd(keys: impl Into<SharedString>, mono: SharedString, t: &Theme) -> Div {
    div()
        .flex_none()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(RADIUS_SM)
        .bg(paint(t.surface))
        .border_1()
        .border_color(paint(t.border))
        .font_family(mono)
        .text_size(px(11.0))
        .text_color(paint(t.text.dim))
        .child(keys.into())
}

/// A key cap and the word for what it does — `↵ save`, `esc close` — for the
/// strip of them under a surface that answers to its own keys.
///
/// The key is a [`kbd`], so it reads as the same thing it does on a button or
/// in the palette; the word is a caption, prose, because it is read.
pub(crate) fn key_hint(
    keys: impl Into<SharedString>,
    what: impl Into<SharedString>,
    t: &Theme,
) -> Div {
    div()
        .prose()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .text_size(CAPTION)
        .text_color(paint(t.text.dim))
        .child(kbd(keys, crate::fonts::chrome(), t))
        .child(what.into())
}

/// A project's coloured square, carrying its icon or its initial.
///
/// The label colour is chosen against the badge rather than fixed: the project
/// palette runs from pale to saturated, and one fixed foreground is
/// unreadable on half of it. `contrast_ratio` already exists in core for the
/// theme check, so the badge asks it the same question.
pub(crate) fn badge(colour: Color, label: impl Into<SharedString>, t: &Theme) -> Div {
    let on_badge = if contrast_ratio(t.surface, colour) >= contrast_ratio(t.text.primary, colour) {
        t.surface
    } else {
        t.text.primary
    };

    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.0))
        .rounded(RADIUS_SM)
        .bg(paint(colour))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(paint(on_badge))
        .child(label.into())
}

/// One beat of a moving dot.
const DOT_CYCLE: Duration = Duration::from_millis(1500);

/// What a beating dot keeps at the bottom of its beat. High enough that the
/// dot is always a dot: one that fades to nothing reads as blinking off.
const DOT_FLOOR: f32 = 0.35;

/// What a breathing dot keeps: shallower than a beat, since work going on is
/// news but not an alarm.
const BREATH_FLOOR: f32 = 0.6;

/// How a state dot moves — the half of the grammar that says *running*.
///
/// Hue says how it is going; motion says whether anything is happening at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Motion {
    /// A fact, not an event: a run that already failed. There to be noticed,
    /// not chased.
    Still,
    /// Work in progress: the dot breathes, gently.
    Travel,
    /// Blocked on a person: the dot beats all the way down, which is harder
    /// to ignore than a breath.
    Pulse,
}

/// A row's state dot: `size` across, in `colour`, moving as `motion` says.
///
/// A moving dot is drawn where the shared clock says its cycle is, and it is
/// the clock, not the dot, that books the next redraw — see
/// [`crate::motion`] for why this is not `with_animation`. Every moving dot
/// is at the same point of its cycle as a result, which is what a column of
/// them should look like: one system running, not eight loading spinners.
pub(crate) fn status_dot(colour: Rgba, motion: Motion, size: Pixels) -> Div {
    let ink = match motion {
        Motion::Still => colour,
        Motion::Travel | Motion::Pulse => {
            let delta = crate::motion::phase(DOT_CYCLE);
            // A cosine rather than a triangle: a linear fade in and out reads
            // as a blink with a pause at each end.
            let lift = 0.5 + (delta * std::f32::consts::TAU).cos() * 0.5;
            let floor = match motion {
                Motion::Pulse => DOT_FLOOR,
                _ => BREATH_FLOOR,
            };
            alpha(colour, floor + (1.0 - floor) * lift)
        }
    };
    div().flex_none().size(size).rounded_full().bg(ink)
}

/// The selected row's rail: a short amber bar at the row's leading edge,
/// inset from its top and bottom. Absolutely placed, so it costs the row no
/// width and nothing beside it moves when the selection does. The row must be
/// `relative`.
pub(crate) fn marker(t: &Theme) -> Div {
    div()
        .absolute()
        .left_0()
        .top(MARKER_INSET)
        .bottom(MARKER_INSET)
        .w(MARKER_W)
        .rounded(px(2.0))
        .bg(paint(t.marker))
}

/// How wide the selection rail is.
const MARKER_W: Pixels = px(3.0);

/// How far the selection rail stands off the row's top and bottom.
const MARKER_INSET: Pixels = px(10.0);

/// A colour to pick: a square of it, ringed in ink and checked while chosen.
///
/// The ring stands off the colour by a gap of whatever the swatch sits on,
/// the way the sidebar's override ring does, so a ring in near-white beside a
/// pale swatch still reads as a ring rather than as the swatch growing an
/// edge. The check is for the swatches whose hue sits close to a neighbour's —
/// amber beside yellow, violet beside purple — where a ring alone says which
/// one only to someone comparing the two.
///
/// The outer square is the same size chosen or not, so a row of them never
/// moves when the choice does.
pub(crate) fn swatch(
    id: impl Into<gpui::ElementId>,
    colour: Color,
    chosen: bool,
    t: &Theme,
) -> gpui::Stateful<Div> {
    let tick = if contrast_ratio(t.surface, colour) >= contrast_ratio(t.text.primary, colour) {
        t.surface
    } else {
        t.text.primary
    };
    let el = div()
        .id(id)
        .flex()
        .flex_none()
        .size(SWATCH)
        .rounded(RADIUS_SM)
        .border_2()
        .cursor_pointer();
    if chosen {
        el.border_color(paint(t.text.primary)).p(px(2.0)).child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .bg(paint(colour))
                .child(super::icon::sized_icon(
                    super::icon::Icon::Check,
                    px(11.0),
                    paint(tick),
                )),
        )
    } else {
        el.bg(paint(colour))
            .border_color(gpui::transparent_black())
            .hover(|style| style.border_color(paint(t.border)))
    }
}

/// A swatch's side.
const SWATCH: Pixels = px(24.0);

/// A tag that can be taken away: its words and a small cross. The whole chip
/// is the target — the caller attaches the removal to it.
pub(crate) fn removable(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .prose()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.0))
        .pl(px(8.0))
        .pr(px(5.0))
        .py(px(2.0))
        .rounded(RADIUS_SM)
        .bg(paint(t.selection))
        .text_size(px(10.5))
        .text_color(paint(t.text.primary))
        .child(text.into())
        .child(super::icon::sized_icon(
            super::icon::Icon::Close,
            px(11.0),
            alpha(paint(t.text.dim), 0.8),
        ))
}

/// A [`tinted_tag`] led by a glyph in the same colour — a git operation
/// under way, say.
pub(crate) fn signal_tag(
    which: super::icon::Icon,
    text: impl Into<SharedString>,
    colour: Rgba,
) -> Div {
    div()
        .prose()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.0))
        .px(px(6.0))
        .py(px(1.0))
        .rounded(RADIUS_SM)
        .bg(alpha(colour, 0.12))
        .text_size(px(10.5))
        .text_color(colour)
        .child(super::icon::sized_icon(which, px(10.0), colour))
        .child(text.into())
}

/// A read-out in a line of them: a figure in a small recessed well, usually
/// led by a glyph — the compact header's usage, branch, changes and agents.
/// Set in the chrome's mono, because what it holds is values to line up and
/// quote. The caller adds the glyph and the figure.
pub(crate) fn readout(t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.0))
        .h(READOUT_H)
        .px(px(8.0))
        .rounded(RADIUS_SM)
        .bg(paint(t.sunken))
        .font_family(crate::fonts::chrome())
        .text_size(px(12.0))
        .text_color(paint(t.text.primary))
        .whitespace_nowrap()
}

/// A read-out's height: a step under a small button, so a line of them sits
/// inside a 32px bar with air above and below.
const READOUT_H: Pixels = px(22.0);

/// A caption: secondary text at the size the rest of the chrome uses for it.
pub(crate) fn caption(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .text_size(CAPTION)
        .text_color(paint(t.text.dim))
        .child(text.into())
}

/// A small inline label: which agent a worktree is for, or that it is the
/// primary checkout. One more fact about a row, not a heading.
///
/// Set in the prose face, here rather than at the call sites: every tag the
/// chrome draws is a word — "merged", "missing", "primary", "rebasing" — so
/// there is no call site that would want it otherwise.
pub(crate) fn tag(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .prose()
        .flex_none()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(super::RADIUS_MD)
        .bg(paint(t.hover))
        .text_size(px(10.5))
        .text_color(paint(t.text.dim))
        .child(text.into())
}

/// A [`tag`] that carries a state's own hue instead of the neutral one.
///
/// For the labels that are not just one more fact about a row but a report of
/// what is happening to it — "rebasing" on a tree stopped mid-rebase. The
/// ground is the same colour at low alpha rather than a second flat fill, so
/// the pill reads as tinted rather than as a button someone could press.
pub(crate) fn tinted_tag(text: impl Into<SharedString>, colour: Rgba) -> Div {
    div()
        .prose()
        .flex_none()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(super::RADIUS_MD)
        .bg(alpha(colour, 0.16))
        .text_size(px(10.5))
        .text_color(colour)
        .child(text.into())
}

// ---- token-reduction ring ---------------------------------------------------
//
// The worktree row used to flag its token-reduction level with `tag`, the
// same pill as "primary" and "missing" — but those are words about what a
// row *is*, and this is a number on a five-point scale. A pill sized to fit
// "moderate reduction" is wider than the branch name on a fresh worktree, and
// widens and narrows as the level changes, which shoves the branch name
// sideways for a fact nobody reads word-for-word twice. A ring is a gauge: a
// glance says how deep the level cuts, and the label survives as a tooltip
// for the one time someone wants it spelled out.

/// Big enough to read as a ring rather than a dot at the sidebar's own text
/// size; small enough to sit beside a branch name without competing with it.
const RING_SIZE: Pixels = px(14.0);

/// Stroke width for both the ring's dim track and its coloured arc.
const RING_STROKE: Pixels = px(1.7);

/// Points sampled around the full circle. A partial sweep walks a fraction of
/// these — enough that a stroke this thin never shows a facet at 14px.
const RING_SEGMENTS: usize = 40;

/// The sidebar row's stand-in for the old "light/moderate/heavy/max
/// reduction" pill: a dim full-circle track with a coloured arc on top,
/// starting at twelve o'clock and sweeping clockwise as deep as the level
/// cuts — a quarter turn for light, all the way round for max.
///
/// **Drawn on every row, including the default level**, where it is the track
/// alone with no arc.
///
/// It used to be omitted there, on the reasoning that a row which reasons and
/// explains freely has nothing to flag. That was right while the ring was only
/// a flag. It is now also the handle for the session's usage — see
/// `crate::usage_card` — and a row with nothing to hover is a row whose
/// spending cannot be looked at. An empty ring is not noise either: "no
/// reduction" is a setting somebody chose, and the track says so.
///
/// Carries no hover of its own; the caller anchors to it.
pub(crate) fn token_reduction_ring(level: u8, t: &Theme) -> AnyElement {
    tinted_token_reduction_ring(
        level,
        crate::worktree_dialog::token_reduction_tint(level, t),
        t,
    )
}

/// [`token_reduction_ring`] with its arc in `arc_colour` rather than the
/// level's grey-to-ink: where the ring stands for Economy itself, as in the
/// Settings pane, and wears Economy's green.
pub(crate) fn tinted_token_reduction_ring(level: u8, arc_colour: Rgba, t: &Theme) -> AnyElement {
    let sweep = ring_sweep(level);
    let track_colour = alpha(paint(t.text.dim), 0.22);

    div()
        .flex_none()
        .size(RING_SIZE)
        .child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    stroke_ring(bounds, 1.0, track_colour, window);
                    // `stroke_ring` returns on a zero sweep, so the default
                    // level costs one call and paints nothing.
                    stroke_ring(bounds, sweep, arc_colour, window);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// How far round the ring goes at `level`.
///
/// Stated once: the sidebar's mark and the card's hero read the same dial, and
/// a level that swept a quarter on one and a half on the other would make the
/// card look like it was describing a different row.
fn ring_sweep(level: u8) -> f32 {
    // From the level's *place* in the table rather than its number: the
    // built-in five run 4 down to 0, but a pack may ship three levels or nine,
    // and a hardcoded quarter-turn would then draw the wrong dial. Position 0
    // is the least reduction and draws no arc; the last position closes it.
    match ket_core::worktree::level_position(level) {
        Some((_, 1)) => 1.0,
        Some((index, count)) => index as f32 / (count - 1) as f32,
        // A level the active table does not contain — a stored number from a
        // pack that has since changed. The track alone claims nothing.
        None => 0.0,
    }
}

/// Strokes `sweep` (0 to 1) of a circle inscribed in `bounds`, clockwise from
/// twelve o'clock.
///
/// Walked as a many-point polyline rather than through `PathBuilder`'s
/// SVG-style `arc_to` — at a 14px indicator the two are visually identical,
/// and placing points by angle is the version that cannot get large-arc or
/// sweep flags backwards. A full sweep closes the loop so the stroke has no
/// start/end caps to show as a seam; a partial one leaves them open, which is
/// exactly what a partial ring should look like.
fn stroke_ring(bounds: Bounds<Pixels>, sweep: f32, colour: Rgba, window: &mut Window) {
    stroke_ring_with(bounds, sweep, colour, RING_STROKE, window);
}

/// [`stroke_ring`] at a caller's stroke width.
///
/// The sidebar's 1.7px hairline scaled onto a 38px hero would be a thread; the
/// hero's 3px on a 14px mark would be a blob. Same geometry either way.
fn stroke_ring_with(
    bounds: Bounds<Pixels>,
    sweep: f32,
    colour: Rgba,
    stroke: Pixels,
    window: &mut Window,
) {
    stroke_arc(bounds, 0.0, sweep, colour, stroke, window);
}

/// [`stroke_ring_with`] starting `start` turns (0 to 1) clockwise from twelve
/// o'clock rather than at it: the button's busy spinner, which is this arc
/// with its start moving.
pub(crate) fn stroke_arc(
    bounds: Bounds<Pixels>,
    start: f32,
    sweep: f32,
    colour: Rgba,
    stroke: Pixels,
    window: &mut Window,
) {
    if sweep <= 0.0 {
        return;
    }
    let sweep = sweep.min(1.0);
    let center = bounds.center();
    let radius = f32::from(bounds.size.width) / 2.0 - f32::from(stroke) / 2.0;
    let steps = ((RING_SEGMENTS as f32) * sweep).ceil().max(1.0) as usize;

    let mut path = PathBuilder::stroke(stroke);
    for i in 0..=steps {
        let angle = (start + i as f32 / RING_SEGMENTS as f32) * std::f32::consts::TAU;
        let point = point(
            px(f32::from(center.x) + radius * angle.sin()),
            px(f32::from(center.y) - radius * angle.cos()),
        );
        if i == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    if sweep >= 1.0 {
        path.close();
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, colour);
    }
}
