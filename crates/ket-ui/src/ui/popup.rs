//! A non-modal surface anchored to an arbitrary trigger.
//!
//! The caller owns whether the popup is open and everything drawn inside it.
//! This module owns only geometry and the shared floating-surface treatment,
//! so a quota card and a future inspector can agree without sharing state or
//! business actions.

use gpui::{
    AnyElement, App, Bounds, BoxShadow, Display, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, MouseButton, Pixels, Point, Position, Size, Style,
    Window, deferred, div, point, prelude::*, px,
};
use ket_core::theme::Theme;

use crate::paint::paint;

/// The panel's corner radius.
///
/// The one radius every floating panel takes — see `ui::menu`'s
/// `PANEL_RADIUS`, which is where the rest of the floating chrome landed.
/// It was 10 against menus at 8; both are `RADIUS_LG` now, because a popup
/// and a menu opening off the same strip should not be two shapes.
const PANEL_RADIUS: Pixels = super::RADIUS_LG;

/// Space between the visible popup and its trigger.
const TRIGGER_GAP: Pixels = px(8.0);

/// Space between the visible popup and the window edge.
const WINDOW_MARGIN: Pixels = px(8.0);

/// Which edges of the trigger and popup meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // The component API is intentionally complete before its second caller.
pub(crate) enum Placement {
    /// Popup bottom-left to trigger top-left.
    AboveStart,
    /// Popup bottom-right to trigger top-right.
    AboveEnd,
    /// Popup top-left to trigger bottom-left.
    BelowStart,
    /// Popup top-right to trigger bottom-right.
    BelowEnd,
}

impl Placement {
    /// Computes the requested panel bounds from a measured trigger corner.
    fn bounds(self, anchor: Point<Pixels>, size: Size<Pixels>) -> Bounds<Pixels> {
        let x = match self {
            Self::AboveStart | Self::BelowStart => anchor.x,
            Self::AboveEnd | Self::BelowEnd => anchor.x - size.width,
        };
        let y = match self {
            Self::AboveStart | Self::AboveEnd => anchor.y - TRIGGER_GAP - size.height,
            Self::BelowStart | Self::BelowEnd => anchor.y + TRIGGER_GAP,
        };
        Bounds {
            origin: point(x, y),
            size,
        }
    }

    /// The same cross-axis alignment on the other side of the trigger.
    fn flipped(self) -> Self {
        match self {
            Self::AboveStart => Self::BelowStart,
            Self::AboveEnd => Self::BelowEnd,
            Self::BelowStart => Self::AboveStart,
            Self::BelowEnd => Self::AboveEnd,
        }
    }
}

/// Layout state for [`PopupAnchor`].
struct PopupAnchorState {
    child_layout: LayoutId,
}

/// Positions one measured panel at a measured trigger corner.
///
/// GPUI's general anchor swaps horizontal alignment before clamping. Popups
/// need the less surprising rule used by native menus: preserve start/end,
/// clamp the cross axis, and swap only above/below.
struct PopupAnchor {
    child: AnyElement,
    placement: Placement,
}

impl Element for PopupAnchor {
    type RequestLayoutState = PopupAnchorState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout = self.child.request_layout(window, cx);
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        let layout = window.request_layout(style, [child_layout], cx);
        (layout, PopupAnchorState { child_layout })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let size = window.layout_bounds(state.child_layout).size;
        let anchor = bounds.origin;
        let viewport = window.viewport_size();
        let top = WINDOW_MARGIN;
        let right = viewport.width - WINDOW_MARGIN;
        let bottom = viewport.height - WINDOW_MARGIN;
        let mut desired = self.placement.bounds(anchor, size);

        // Flip only when the alternate side has less vertical overflow. If
        // neither side can contain a tall panel, the final clamp below keeps
        // its top reachable instead of oscillating between two bad choices.
        if desired.top() < top || desired.bottom() > bottom {
            let flipped = self.placement.flipped().bounds(anchor, size);
            let overflow = |candidate: &Bounds<Pixels>| {
                (top - candidate.top()).max(Pixels::ZERO)
                    + (candidate.bottom() - bottom).max(Pixels::ZERO)
            };
            if overflow(&flipped) < overflow(&desired) {
                desired = flipped;
            }
        }

        if desired.right() > right {
            desired.origin.x -= desired.right() - right;
        }
        if desired.left() < WINDOW_MARGIN {
            desired.origin.x = WINDOW_MARGIN;
        }
        if desired.bottom() > bottom {
            desired.origin.y -= desired.bottom() - bottom;
        }
        if desired.top() < top {
            desired.origin.y = top;
        }

        let offset = desired.origin - bounds.origin;
        let offset = point(offset.x.round(), offset.y.round());
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl IntoElement for PopupAnchor {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Draws `trigger` and, when supplied, a themed popup anchored to it.
///
/// The panel is measured before positioning, so an above/below placement can
/// flip when needed while start/end overflow is clamped to the window margin.
///
/// For a popup a *click* opens, which is why the trigger swallows the press —
/// see [`hover_anchored`] for one a pointer opens.
pub(crate) fn anchored(
    id: impl Into<ElementId>,
    trigger: AnyElement,
    content: Option<AnyElement>,
    placement: Placement,
    preferred_width: Pixels,
    window: &Window,
    t: &Theme,
) -> AnyElement {
    build(
        id,
        trigger,
        content,
        placement,
        preferred_width,
        true,
        window,
        t,
    )
}

/// [`anchored`] for a popup the pointer opens rather than a click.
///
/// Identical but for the press: `anchored` stops left mouse-down on the
/// trigger, so that reopening a click-popup is not first read as an outside
/// click by the shell root. A hover popup has no such press to protect, and
/// swallowing one costs whatever the trigger is sitting on — the sidebar's
/// ring is inside a row you select by clicking and reorder by dragging, and
/// both of those died on the 14px the ring occupies.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hover_anchored(
    id: impl Into<ElementId>,
    trigger: AnyElement,
    content: Option<AnyElement>,
    placement: Placement,
    preferred_width: Pixels,
    window: &Window,
    t: &Theme,
) -> AnyElement {
    build(
        id,
        trigger,
        content,
        placement,
        preferred_width,
        false,
        window,
        t,
    )
}

/// The shared body of [`anchored`] and [`hover_anchored`].
#[allow(clippy::too_many_arguments)]
fn build(
    id: impl Into<ElementId>,
    trigger: AnyElement,
    content: Option<AnyElement>,
    placement: Placement,
    preferred_width: Pixels,
    swallow_press: bool,
    window: &Window,
    t: &Theme,
) -> AnyElement {
    let available_width = (window.viewport_size().width - WINDOW_MARGIN * 2).max(Pixels::ZERO);
    let width = preferred_width.min(available_width);

    let popup = content.map(|content| {
        let surface = div()
            .occlude()
            .flex()
            .flex_col()
            .min_w_0()
            .w(width)
            .max_w(width)
            .overflow_hidden()
            .rounded(PANEL_RADIUS)
            .border_1()
            .border_color(paint(t.border))
            .bg(paint(t.elevated))
            .text_color(paint(t.text.primary))
            .opacity(1.0)
            // Large and faint rather than `shadow_lg`'s tighter preset: a
            // popup sits over whatever the pane behind it happens to be —
            // the sidebar, the status bar, a terminal's own colours — and a
            // shadow has to read against all of them to say "this is above
            // that" at all. Same two-layer shape as `ui::menu`'s panel, at
            // roughly half its opacity: a menu is chrome nobody else is
            // drawn near, and a popup is answering a much lower bar — being
            // visibly apart from the ground behind it, not looking cut out
            // of it.
            .shadow(vec![
                BoxShadow {
                    color: crate::paint::shadow(0.28, t),
                    offset: point(px(0.0), px(14.0)),
                    blur_radius: px(36.0),
                    spread_radius: px(0.0),
                },
                BoxShadow {
                    color: crate::paint::shadow(0.18, t),
                    offset: point(px(0.0), px(2.0)),
                    blur_radius: px(6.0),
                    spread_radius: px(0.0),
                },
            ])
            .child(content)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        // The anchor div has no area of its own. Its origin is the trigger
        // corner from which `PopupAnchor` measures the panel.
        let anchor = match placement {
            Placement::AboveStart | Placement::BelowStart => div()
                .absolute()
                .when(matches!(placement, Placement::AboveStart), |anchor| {
                    anchor.top_0()
                })
                .when(matches!(placement, Placement::BelowStart), |anchor| {
                    anchor.bottom_0()
                })
                .left_0()
                .size(Pixels::ZERO),
            Placement::AboveEnd | Placement::BelowEnd => div()
                .absolute()
                .when(matches!(placement, Placement::AboveEnd), |anchor| {
                    anchor.top_0()
                })
                .when(matches!(placement, Placement::BelowEnd), |anchor| {
                    anchor.bottom_0()
                })
                .right_0()
                .size(Pixels::ZERO),
        };

        anchor.child(
            deferred(PopupAnchor {
                child: surface.into_any_element(),
                placement,
            })
            .with_priority(9),
        )
    });

    div()
        .id(id)
        .relative()
        .flex_none()
        .child(trigger)
        .children(popup)
        // A second press on the trigger must reach its click handler without
        // first looking like an outside click to the shell root.
        .when(swallow_press, |el| {
            el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        })
        .into_any_element()
}
