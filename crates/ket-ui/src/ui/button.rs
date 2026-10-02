//! Buttons, and the square icon buttons beside them.
//!
//! Six button recipes lived at six call sites before this. They disagreed on
//! height (34px, `py_1`, `py_2`), on radius, on whether there was a border at
//! all, and on which token carried the primary fill — one of them added a
//! border *on hover*, so the button moved two pixels under the pointer. The
//! icon buttons were worse: of five, two had neither a hover state nor a
//! pointer cursor, so a third of the chrome's controls gave no sign they were
//! controls.
//!
//! Under Subdued a variant is a hairline and an ink, not a slab: the window
//! has one ground and a button is not a second one. Only the primary action
//! fills, and it fills with the one accent the theme has.

use std::time::Duration;

use gpui::{
    Bounds, Div, ElementId, FontWeight, Pixels, Rgba, SharedString, Stateful, canvas, div,
    linear_color_stop, linear_gradient, prelude::*, px, relative, transparent_black,
};
use ket_core::theme::Theme;

use super::icon::{Icon, icon};
use super::{
    CONTROL_H, ICON_GAP, LABEL, PAD_X, RADIUS_MD, RADIUS_SM, RADIUS_WELL, WELL, WELL_SM, WELL_XS,
};
use crate::fonts::Prose;
use crate::paint::paint;

/// What job a button is doing, which is the only thing a call site should have
/// to decide. Everything else — fill, border, hover, disabled — follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    /// The one action the surface exists for. At most one per surface.
    Primary,
    /// Everything else that is a real action: Cancel, Save, Add.
    Secondary,
    /// An action that should not compete: a dismissal beside a primary.
    Ghost,
    /// Destructive. Never also primary — a delete that is styled as the
    /// obvious next step is a trap.
    Danger,
    /// Words that go somewhere, inside a line of text: no box, no height of
    /// its own, ink that brightens under the pointer.
    Link,
}

/// A labelled button.
///
/// Built rather than returned directly so the common cases stay one line and
/// the rare ones (a leading icon, a key hint, a disabled state) do not each
/// need their own constructor.
pub(crate) struct Button {
    id: ElementId,
    label: SharedString,
    variant: Variant,
    leading: Option<Icon>,
    trailing: Option<Icon>,
    detail: Option<SharedString>,
    hint: Option<SharedString>,
    enabled: bool,
    small: bool,
    busy: Option<SharedString>,
}

/// The widest a button's [`Button::detail`] grows before it is cut short.
const DETAIL_MAX_W: Pixels = px(180.0);

/// How long the busy spinner takes to go round once.
const SPIN: Duration = Duration::from_millis(900);

/// How long the danger button's sheen takes to cross it once.
const SHEEN: Duration = Duration::from_millis(1400);

/// A small button's height: for a button inside a row or a banner, where a
/// full control would be taller than the line it acts on.
const SMALL_H: Pixels = px(26.0);

/// A secondary button, which is what most buttons are.
pub(crate) fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button {
        id: id.into(),
        label: label.into(),
        variant: Variant::Secondary,
        leading: None,
        trailing: None,
        detail: None,
        hint: None,
        enabled: true,
        small: false,
        busy: None,
    }
}

impl Button {
    /// Sets the variant.
    pub(crate) fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }

    /// The one action this surface exists for.
    pub(crate) fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }

    /// Quieter than secondary.
    pub(crate) fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }

    /// Destructive.
    pub(crate) fn danger(self) -> Self {
        self.variant(Variant::Danger)
    }

    pub(crate) fn link(self) -> Self {
        self.variant(Variant::Link)
    }

    /// An icon before the label.
    pub(crate) fn leading(mut self, which: Icon) -> Self {
        self.leading = Some(which);
        self
    }

    /// An icon after the label: where the action goes rather than what it
    /// is, as a link that leaves ket carries its arrow. A size smaller than
    /// a leading icon, since it follows the words instead of heading them.
    pub(crate) fn trailing(mut self, which: Icon) -> Self {
        self.trailing = Some(which);
        self
    }

    /// What the action will act on, after the label in the chrome's mono —
    /// the branch a Start will make. Quieter than the label and cut short
    /// when long, so the verb always reads first.
    pub(crate) fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The key that also does this, shown quietly at the trailing edge.
    pub(crate) fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Whether it can be pressed. A disabled button takes no handler at all —
    /// see the note in `menu`: a control that cannot be used should not
    /// swallow the click either.
    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The small size: 26px, 12px type.
    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Shows the button working: a spinner in the leading icon's place and
    /// `label` — "Deleting…" — in the label's. It keeps its variant's
    /// colours, so a delete in progress still reads as the delete, and it is
    /// as wide as the wider of its two labels, so nothing beside it moves
    /// when it starts or stops. Presses land on nothing while it is busy,
    /// whatever handler the caller attached.
    pub(crate) fn loading(mut self, label: impl Into<SharedString>) -> Self {
        self.busy = Some(label.into());
        self
    }

    /// [`Button::loading`] when `busy`, and the button as it was otherwise.
    pub(crate) fn loading_if(self, busy: bool, label: impl Into<SharedString>) -> Self {
        if busy { self.loading(label) } else { self }
    }

    /// Builds the element. Attach `on_click` to what comes back.
    pub(crate) fn render(self, t: &Theme) -> Stateful<Div> {
        // A disabled button is a quieter version of *itself*, never a generic
        // grey slab. Flattening every variant into one disabled look left the
        // create-worktree dialog's primary action identical to the Cancel
        // beside it, so the dialog appeared to offer two ways to back out.
        let dim = |mut colour: Rgba, alpha: f32| {
            colour.a = alpha;
            colour
        };
        let (fill, text, border) = match (self.enabled, self.variant) {
            (true, Variant::Primary) => (paint(t.accent), paint(t.on_accent), None),
            (false, Variant::Primary) => (
                dim(paint(t.accent), 0.4),
                dim(paint(t.on_accent), 0.55),
                None,
            ),
            (true, Variant::Secondary) => (
                transparent_black().into(),
                paint(t.text.primary),
                Some(paint(t.border)),
            ),
            (true, Variant::Ghost) => (transparent_black().into(), paint(t.text.dim), None),
            (true, Variant::Link) => (transparent_black().into(), paint(t.text.dim), None),
            (true, Variant::Danger) => (
                transparent_black().into(),
                paint(t.status.failed),
                Some(dim(paint(t.status.failed), 0.45)),
            ),
            (false, _) => (
                transparent_black().into(),
                dim(paint(t.text.dim), 0.55),
                Some(paint(t.border)),
            ),
        };
        let primary = self.variant == Variant::Primary;
        let hovered = match self.variant {
            Variant::Primary => paint(t.accent_hover),
            Variant::Link => transparent_black().into(),
            _ => paint(t.hover),
        };
        let link = self.variant == Variant::Link;
        let small = self.small;
        let hovered_text = match self.variant {
            Variant::Ghost | Variant::Link => paint(t.text.primary),
            Variant::Danger => paint(t.status.failed),
            _ => text,
        };
        let busy = self.busy.is_some();
        let enabled = self.enabled && !busy;
        let danger = self.variant == Variant::Danger;
        let hint_colour = match self.variant {
            Variant::Primary => paint(t.on_accent),
            _ => paint(t.text.dim),
        };

        div()
            .id(self.id)
            // A button's label is a verb, read as a word: prose, whatever
            // face the surface around it is set in.
            .prose()
            .flex()
            .flex_none()
            .items_center()
            .gap(if small { px(6.0) } else { ICON_GAP })
            .when(!link, |el| {
                el.h(if small { SMALL_H } else { CONTROL_H }).px(if small {
                    px(9.0)
                } else {
                    PAD_X
                })
            })
            .rounded(RADIUS_MD)
            .bg(fill)
            .text_size(if small { px(12.0) } else { LABEL })
            .text_color(text)
            .font_weight(if primary {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .when_some(border, |el, colour| el.border_1().border_color(colour))
            .when(enabled, |el| {
                el.cursor_pointer()
                    .hover(|style| style.bg(hovered).text_color(hovered_text))
            })
            .map(|el| match self.busy {
                None => el
                    .children(self.leading.map(|which| icon(which, text)))
                    .child(self.label)
                    .children(self.trailing.map(|which| {
                        super::icon::sized_icon(
                            which,
                            if small { px(12.0) } else { px(14.0) },
                            text,
                        )
                    }))
                    .children(self.detail.map(|detail| {
                        div()
                            .min_w_0()
                            .max_w(DETAIL_MAX_W)
                            .truncate()
                            .font_family(crate::fonts::chrome())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(hint_colour)
                            .child(detail)
                    })),
                Some(busy) => {
                    let gap = if small { px(6.0) } else { ICON_GAP };
                    let glyph = if small { px(12.0) } else { px(14.0) };
                    el.relative()
                        .overflow_hidden()
                        .when(danger, |el| el.child(sheen(text)))
                        .child(
                            // Both faces, stacked: the busy one drawn, the idle
                            // one laid out at no height and never painted, so
                            // the button is as wide as the wider of the two.
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .gap(gap)
                                        .child(spinner(glyph, text))
                                        .child(busy),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(gap)
                                        .h(px(0.0))
                                        .overflow_hidden()
                                        .invisible()
                                        .children(self.leading.map(|which| icon(which, text)))
                                        .child(self.label),
                                ),
                        )
                        // Over everything, so a press lands here and the
                        // caller's handler never hears it.
                        .child(div().absolute().inset_0().occlude())
                }
            })
            .children(self.hint.map(|keys| {
                div()
                    .flex_none()
                    .text_size(super::CAPTION)
                    .text_color(hint_colour)
                    .child(keys)
            }))
    }
}

/// A ring going round, in `colour`: a faint full track under a quarter arc
/// whose start follows the clock.
fn spinner(size: Pixels, colour: Rgba) -> impl IntoElement {
    let start = crate::motion::phase(SPIN);
    let mut track = colour;
    track.a *= 0.25;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            super::chip::stroke_arc(bounds, 0.0, 1.0, track, px(1.6), window);
            super::chip::stroke_arc(bounds, start, 0.25, colour, px(1.6), window);
        },
    )
    .flex_none()
    .size(size)
}

/// A soft band of `colour` crossing the button on a loop: the danger
/// variant's busy state, since that is the action someone is watching finish.
fn sheen(colour: Rgba) -> impl IntoElement {
    let at = crate::motion::phase(SHEEN);
    let mut clear = colour;
    clear.a = 0.0;
    let mut tint = colour;
    tint.a = 0.14;
    // From a band's width off the leading edge to fully off the trailing one.
    let left = at * 1.4 - 0.4;
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(left))
        .w(relative(0.4))
        .flex()
        .child(div().flex_1().bg(linear_gradient(
            90.0,
            linear_color_stop(clear, 0.0),
            linear_color_stop(tint, 1.0),
        )))
        .child(div().flex_1().bg(linear_gradient(
            90.0,
            linear_color_stop(tint, 0.0),
            linear_color_stop(clear, 1.0),
        )))
}

/// A square button carrying one icon and no label.
/// A close mark that stands in for an unsaved dot: one 16px cell holding
/// both, the dot at rest and the cross while the pointer is over `row_group`.
///
/// One cell rather than two, because two would shift every label along a
/// strip as the pointer crossed it, and hiding the cross behind the dot would
/// make an unsaved tab the one you cannot close. `mark_group` names the
/// cross's own hover, which brightens it.
///
/// `mark` is the glyph in the cell: [`Icon::Close`] for an ordinary tab, and
/// [`Icon::Pin`] for a pinned one, whose cell unpins rather than closes. The
/// swap with the dot is the same either way.
pub(crate) fn close_cell(
    id: impl Into<ElementId>,
    mark: Icon,
    dirty: bool,
    row_group: SharedString,
    mark_group: SharedString,
    on_close: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    t: &Theme,
) -> Div {
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .children(dirty.then(|| {
            div()
                .absolute()
                .size(px(7.0))
                .rounded_full()
                .bg(paint(t.text.dim))
                .group_hover(row_group.clone(), |style| style.bg(transparent_black()))
        }))
        .child(
            div()
                .id(id)
                .group(mark_group.clone())
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.0))
                .rounded(super::RADIUS_SM)
                .cursor_pointer()
                .hover(|style| style.bg(paint(t.selection)))
                .child(
                    // The mark carries its own colour: an svg is tinted by its
                    // own text colour, never an inherited one.
                    super::icon::sized_icon(
                        mark,
                        px(11.0),
                        if dirty {
                            transparent_black().into()
                        } else {
                            paint(t.text.dim)
                        },
                    )
                    .when(dirty, |el| {
                        el.group_hover(row_group, |style| style.text_color(paint(t.text.dim)))
                    })
                    .when(!dirty, |el| {
                        el.group_hover(mark_group, |style| style.text_color(paint(t.text.primary)))
                    }),
                )
                .on_click(on_close),
        )
}

/// A pressable surface around content of the caller's own: the header's
/// figures that open a popup, a size that opens a card. A button whose face
/// is not a label. It brings the hover wash, the corner and the pointer; the
/// caller brings padding and children and attaches `on_click`.
pub(crate) fn pressable(id: impl Into<ElementId>, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .hover(|style| style.bg(paint(t.hover)))
}

pub(crate) struct IconButton {
    id: ElementId,
    which: Icon,
    well: bool,
    size: Pixels,
    radius: Pixels,
    round: bool,
    pressed: bool,
    tint: Option<Rgba>,
    indicator: Option<bool>,
    glyph: Pixels,
    primary: bool,
    overlay: Option<SharedString>,
    count: Option<SharedString>,
}

/// An icon button in a well — a filled, bordered rounded square.
pub(crate) fn icon_button(id: impl Into<ElementId>, which: Icon) -> IconButton {
    IconButton {
        id: id.into(),
        which,
        well: true,
        size: WELL,
        radius: RADIUS_WELL,
        round: false,
        pressed: false,
        tint: None,
        indicator: None,
        glyph: super::ICON,
        primary: false,
        overlay: None,
        count: None,
    }
}

impl IconButton {
    /// Drops the well, leaving only the hover. For icons inside something that
    /// is already a raised surface, where a well inside a well reads as noise.
    pub(crate) fn bare(mut self) -> Self {
        self.well = false;
        self
    }

    /// The dense size, for a tab strip or a sidebar header.
    pub(crate) fn small(mut self) -> Self {
        self.size = WELL_SM;
        self
    }

    /// Denser still, for a run of buttons that has to read as one cluster. The
    /// corner comes down with the square: [`RADIUS_WELL`] on 24px is a circle.
    pub(crate) fn dense(mut self) -> Self {
        self.size = WELL_XS;
        self.radius = RADIUS_SM;
        self
    }

    /// A circle rather than a rounded square — what a dialog's dismissal is in
    /// the design.
    pub(crate) fn circle(mut self) -> Self {
        self.round = true;
        self
    }

    /// Square corners, for a button that is one cell of an
    /// [`icon_cluster`](super::cluster::icon_cluster): the cluster's frame is
    /// what carries the corner, and a rounded fill inside a cell would leave
    /// slivers of ground between it and the hairlines either side.
    pub(crate) fn square(mut self) -> Self {
        self.radius = px(0.0);
        self
    }

    /// A latch light in the top-right corner: lit when `on`, dark otherwise.
    ///
    /// For a toggle whose "on" has to read at a glance from across the
    /// sidebar, where the selection ground alone is a shade apart from the
    /// panel. The light is a 3px square in the running green — the one
    /// colour the chrome already spends on "this is live" — and the unlit one
    /// is drawn in the border tone, so the pair reads as two switches rather
    /// than one with a mark on it.
    pub(crate) fn indicator(mut self, on: bool) -> Self {
        self.indicator = Some(on);
        self
    }

    /// Held down: the button is a toggle and its mode is on. Drawn in the
    /// selection ground with the icon at full ink, so it reads as a chosen
    /// row does — and stays that way under the pointer, where a plain hover
    /// would otherwise make an on button and an off one look the same.
    pub(crate) fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Overrides the ink this button reaches for once it is active — hovered
    /// or [`pressed`](Self::pressed) — in place of `t.text.primary`. For a
    /// button that is one of several states a control can be in (a view
    /// toggle's two icons, say), where the chosen one should read as chosen
    /// through its own colour, not just the shared selection ground the well
    /// already draws under it.
    pub(crate) fn tint(mut self, colour: Rgba) -> Self {
        self.tint = Some(colour);
        self
    }

    /// A size of the caller's choosing, with a glyph to match — for a
    /// transport's controls, which are sized to be hit without looking.
    pub(crate) fn sized(mut self, size: Pixels, glyph: Pixels) -> Self {
        self.size = size;
        self.glyph = glyph;
        self
    }

    /// The one action a surface exists for, as a glyph: an accent fill with
    /// the glyph in `on_accent`. Play, in a transport.
    pub(crate) fn primary(mut self) -> Self {
        self.primary = true;
        self.well = false;
        self
    }

    /// A figure set inside the glyph — the "5" in a skip-by-five arrow.
    pub(crate) fn overlay(mut self, text: impl Into<SharedString>) -> Self {
        self.overlay = Some(text.into());
        self
    }

    /// A figure after the glyph — how many rows a fold is hiding. The button
    /// grows to hold it rather than staying square.
    pub(crate) fn count(mut self, text: impl Into<SharedString>) -> Self {
        self.count = Some(text.into());
        self
    }

    /// Builds the element. Attach `on_click` to what comes back.
    pub(crate) fn render(self, t: &Theme) -> Stateful<Div> {
        if self.primary {
            return div()
                .id(self.id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(self.size)
                .when(self.round, |el| el.rounded_full())
                .when(!self.round, |el| el.rounded(self.radius))
                .cursor_pointer()
                .bg(paint(t.accent))
                .hover(|style| style.bg(paint(t.accent_hover)))
                .child(super::icon::sized_icon(
                    self.which,
                    self.glyph,
                    paint(t.on_accent),
                ));
        }
        let rest = paint(t.text.dim);
        let active = self.tint.unwrap_or_else(|| paint(t.text.primary));
        let hovered = paint(t.hover);
        let well = self.well;
        let border = paint(t.border);
        let pressed = self.pressed;
        let held = paint(t.selection);

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .h(self.size)
            .when(self.count.is_none(), |el| el.w(self.size))
            .when(self.count.is_some(), |el| el.min_w(self.size).px(px(4.0)))
            .when(self.round, |el| el.rounded_full())
            .when(!self.round, |el| el.rounded(self.radius))
            .text_color(rest)
            .cursor_pointer()
            .when(well, |el| el.border_1().border_color(border))
            .when(pressed, |el| el.bg(held))
            .hover(move |style| {
                style
                    .bg(if pressed { held } else { hovered })
                    .text_color(active)
            })
            .children(
                self.count
                    .map(|count| div().text_size(px(10.0)).text_color(rest).child(count)),
            )
            .child({
                let ink = if pressed { active } else { rest };
                let glyph = super::icon::sized_icon(self.which, self.glyph, ink);
                match self.overlay {
                    // The figure sits inside the glyph's own box and centres
                    // by filling it, so the two stay concentric whatever the
                    // glyph's metrics.
                    Some(text) => div()
                        .relative()
                        .size(self.glyph)
                        .child(glyph)
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size(self.glyph)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(8.5))
                                .line_height(px(8.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ink)
                                .child(text),
                        )
                        .into_any_element(),
                    None => glyph.into_any_element(),
                }
            })
            .when_some(self.indicator, |el, on| {
                el.relative().child(
                    div()
                        .absolute()
                        .top(px(4.0))
                        .right(px(4.0))
                        .size(px(3.0))
                        .bg(paint(if on { t.status.running } else { t.border })),
                )
            })
    }
}
