//! A button group: one setting, every answer to it, side by side in a well.
//!
//! The control a settings pane reaches for when a choice has two or three
//! answers and all of them are worth showing. Below that it is a checkbox;
//! above it, a menu — a group of eight segments is a dropdown that has been
//! unrolled and now owns half the row.
//!
//! ## Why this exists rather than more loose buttons
//!
//! The Agents pane had free-standing bordered buttons doing this job, and they
//! could not say what they were. Each one was drawn as its own control, so
//! "Yolo" and "Manual" beside each other read as two independent things to
//! press rather than as two answers to one question — and the pair had no
//! shared container to say where the question ended. The Enabled control was
//! worse: a single button whose *label* changed between "Enabled" and
//! "Disabled", so the word under the pointer was sometimes the current state
//! and sometimes the thing a click would do, with nothing to distinguish the
//! two. A group shows both answers and highlights the one in force, which can
//! only be read one way.
//!
//! ## The shape
//!
//! A sunken track holds the segments; the selected one is a raised slab on top
//! of it. That direction is the whole idea — the track recedes, the answer in
//! force sits proud of it — and it is why the selected segment uses `selection`
//! over `sunken` rather than a border or an accent tint. An accent fill was
//! tried first and made every group in a pane compete with the pane's actual
//! primary action.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Pixels, Rgba, SharedString, Stateful,
    Window, div, prelude::*, px,
};
use ket_core::theme::Theme;

use super::icon::{Icon, sized_icon};
use super::{CAPTION, RADIUS_MD, RADIUS_SM};
use crate::fonts::Prose;
use crate::paint::paint;

/// How tall the track is, and so the control.
///
/// Matches the bordered buttons this replaced closely enough that swapping one
/// for the other does not move the rows around it.
const TRACK_H: Pixels = px(34.0);

/// The compact track, beside a popup's title.
const SMALL_TRACK_H: Pixels = px(22.0);

/// The gap between the track's edge and a segment.
///
/// Three pixels: enough that the raised segment reads as sitting *in* the
/// track, little enough that the track does not become a frame around it.
const INSET: Pixels = px(3.0);

/// A segment's horizontal padding.
const SEGMENT_PAD: Pixels = px(11.0);

/// A segment's padding in a [`ButtonGroup::dense`] group.
const DENSE_PAD: Pixels = px(8.0);

/// The padding of a segment with no word in it — a glyph, or a dot and a
/// figure — which needs only enough room for the pointer to find it.
const GLYPH_PAD: Pixels = px(6.0);

/// A status dot's diameter.
const DOT: Pixels = px(7.0);

/// What a segment is handed to once drawn — see [`Segment::wrap`].
type Wrap<'a> = Box<dyn FnOnce(Stateful<Div>) -> AnyElement + 'a>;

/// What a click on a segment does. Boxed because each segment carries its own,
/// and a call site builds them from `cx.listener`.
type Press = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One answer within a [`ButtonGroup`].
pub(crate) struct Segment<'a> {
    id: ElementId,
    label: SharedString,
    selected: bool,
    icon: Option<(Icon, Rgba)>,
    glyph: Option<Icon>,
    dot: Option<Rgba>,
    count: Option<SharedString>,
    press: Option<Press>,
    wrap: Option<Wrap<'a>>,
}

/// One answer, not selected and doing nothing until told otherwise.
///
/// An empty label draws no word: for a segment that is a [`glyph`] or a
/// [`dot`] and its count, where the name belongs in a tooltip instead.
///
/// [`glyph`]: Segment::glyph
/// [`dot`]: Segment::dot
pub(crate) fn segment<'a>(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Segment<'a> {
    Segment {
        id: id.into(),
        label: label.into(),
        selected: false,
        icon: None,
        glyph: None,
        dot: None,
        count: None,
        press: None,
        wrap: None,
    }
}

impl<'a> Segment<'a> {
    /// Whether this is the answer currently in force.
    ///
    /// Exactly one segment in a group should be selected. Two is a group
    /// describing two settings, and none is a group describing a setting that
    /// has no value — both are worth noticing at the call site rather than
    /// being quietly drawn.
    pub(crate) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// A mark before the label — a provider's logo, most often.
    pub(crate) fn leading(mut self, which: Icon, colour: Rgba) -> Self {
        self.icon = Some((which, colour));
        self
    }

    /// A chrome glyph at full icon size, in the segment's own ink — for a
    /// segment that is only a glyph, like a view switch's two layouts.
    pub(crate) fn glyph(mut self, which: Icon) -> Self {
        self.glyph = Some(which);
        self
    }

    /// A status dot before the count, in its signal's colour: a filter
    /// answer that says which state it holds without spending a word on it.
    pub(crate) fn dot(mut self, colour: Rgba) -> Self {
        self.dot = Some(colour);
        self
    }

    /// Hands the drawn segment to `wrap` before it goes in the track — to
    /// hang a tooltip on it, the same seam `icon_cluster`'s cells have. A
    /// segment with no word in it owes the reader one.
    pub(crate) fn wrap(mut self, wrap: impl FnOnce(Stateful<Div>) -> AnyElement + 'a) -> Self {
        self.wrap = Some(Box::new(wrap));
        self
    }

    /// What pressing it does.
    ///
    /// A segment with no handler is drawn but inert, and deliberately takes no
    /// pointer cursor: that is the shape a group needs when one of its answers
    /// is a state the user cannot select directly, only arrive at.
    /// A figure after the label — how many worktrees a filter holds, how
    /// many files changed — in the chrome's mono, a step quieter than the
    /// label and brightening with it when chosen.
    pub(crate) fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    pub(crate) fn on_click(
        mut self,
        press: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.press = Some(Box::new(press));
        self
    }

    /// Draws the segment. Private: a segment outside a track is just a button
    /// drawn wrong, and the two states only mean anything against each other.
    fn render(self, fill: bool, small: bool, dense: bool, t: &Theme) -> AnyElement {
        let text = paint(if self.selected {
            t.text.primary
        } else {
            t.text.dim
        });
        let pressable = self.press.is_some();
        let worded = !self.label.is_empty();
        let pad = match (small, worded, dense) {
            (true, _, _) => px(8.0),
            (false, false, _) => GLYPH_PAD,
            (false, true, true) => DENSE_PAD,
            (false, true, false) => SEGMENT_PAD,
        };

        let drawn = div()
            .id(self.id)
            // A segment is a button: its label is a word.
            .prose()
            .flex()
            .when(fill, |el| el.flex_1().min_w_0())
            .when(!fill, |el| el.flex_none())
            .items_center()
            .justify_center()
            .gap(px(6.0))
            // A fixed height rather than `h_full`: a segment handed to
            // `wrap` sits inside the tooltip's wrapper, whose height is its
            // content's, so `h_full` there drew a shorter slab than the
            // unwrapped segments beside it.
            .h(if small {
                SMALL_TRACK_H - px(4.0)
            } else {
                TRACK_H - INSET * 2.0
            })
            .px(pad)
            .rounded(if small { px(4.0) } else { RADIUS_SM })
            .text_size(if small { px(10.5) } else { CAPTION })
            .text_color(text)
            // One weight whichever is chosen: a medium label is wider, and
            // selecting a worded segment nudged every segment after it.
            .font_weight(FontWeight::NORMAL)
            .when(self.selected, |el| el.bg(paint(t.selection)))
            // Hover moves the *unselected* segments only. Lighting the one
            // already in force says a click would change something, and it
            // would not.
            .when(pressable && !self.selected, |el| {
                el.cursor_pointer()
                    .hover(|style| style.bg(paint(t.hover)).text_color(paint(t.text.primary)))
            })
            .children(
                self.icon
                    .map(|(which, colour)| sized_icon(which, px(13.0), colour)),
            )
            .children(self.glyph.map(|which| sized_icon(which, super::ICON, text)))
            .children(
                self.dot
                    .map(|colour| div().flex_none().size(DOT).rounded_full().bg(colour)),
            )
            // Cut rather than wrapped when a filled track is narrower than
            // its labels: a segment that wraps is two rows tall.
            .when(worded, |el| {
                el.child(div().min_w_0().truncate().child(self.label))
            })
            .children(self.count.map(|count| {
                div()
                    .font_family(crate::fonts::chrome())
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(10.5))
                    .text_color(if self.selected {
                        paint(t.text.dim)
                    } else {
                        crate::paint::alpha(paint(t.text.dim), 0.7)
                    })
                    .child(count)
            }))
            .when_some(self.press, |el, press| el.on_click(press));

        match self.wrap {
            Some(wrap) => wrap(drawn),
            None => drawn.into_any_element(),
        }
    }
}

/// One setting's answers, in a shared track.
pub(crate) struct ButtonGroup<'a> {
    id: ElementId,
    segments: Vec<Segment<'a>>,
    fill: bool,
    small: bool,
    dense: bool,
}

/// An empty group. Add [`segment`]s to it.
pub(crate) fn button_group<'a>(id: impl Into<ElementId>) -> ButtonGroup<'a> {
    ButtonGroup {
        id: id.into(),
        segments: Vec::new(),
        fill: false,
        small: false,
        dense: false,
    }
}

impl<'a> ButtonGroup<'a> {
    /// Adds one answer.
    pub(crate) fn child(mut self, segment: Segment<'a>) -> Self {
        self.segments.push(segment);
        self
    }

    /// Adds several, for answers built from a list.
    pub(crate) fn children(mut self, segments: impl IntoIterator<Item = Segment<'a>>) -> Self {
        self.segments.extend(segments);
        self
    }

    /// Draws the group.
    ///
    /// Returns an element rather than a builder: unlike a button, nothing is
    /// left for the call site to attach — every handler is already on a
    /// segment, and a click on the track between them should do nothing.
    /// Stretches the track across its parent and shares the width out
    /// evenly, for a group that is a view switch rather than a setting.
    pub(crate) fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// The compact size, for a switch that sits beside a popup's title
    /// rather than occupying a band of its own.
    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Tighter padding round worded segments, full height kept — for a
    /// group that shares its row with other controls rather than owning it.
    pub(crate) fn dense(mut self) -> Self {
        self.dense = true;
        self
    }

    pub(crate) fn render(self, t: &Theme) -> AnyElement {
        let fill = self.fill;
        let small = self.small;
        let dense = self.dense;
        div()
            .id(self.id)
            .flex()
            .when(fill, |el| el.flex_1())
            .when(!fill, |el| el.flex_none())
            .items_center()
            .h(if small { SMALL_TRACK_H } else { TRACK_H })
            .p(if small { px(2.0) } else { INSET })
            .gap(px(2.0))
            .rounded(RADIUS_MD)
            .bg(paint(t.sunken))
            .children(
                self.segments
                    .into_iter()
                    .map(|s| s.render(fill, small, dense, t)),
            )
            .into_any_element()
    }
}
