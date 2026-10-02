//! A transient report: what just happened, said once, then gone.
//!
//! The shell had `Shell::note` for this and it does not work. A note is drawn
//! only by `projects::landing_view` — the empty state — so every note set from
//! a worktree menu, a terminal, a tab or the editor is written to a field that
//! nothing renders whenever a project exists, which is always. Half a dozen
//! call sites report their outcome into a channel with no outlet.
//!
//! A toast is the outlet. It floats above everything including a modal,
//! because the thing it is usually reporting is the result of what that modal
//! just did, and it takes itself away again so nobody has to dismiss the news
//! that something worked.
//!
//! **Tone is carried by hue, by a shape, and by the words, never by hue
//! alone.** Four colours with no other difference is a signal a colour-blind
//! reader does not get, so each tone also has a mark — tick, triangle, cross —
//! and the message is written to say it too: "Deleted x", "Could not delete
//! x". The icon set grew the four it was missing rather than the toast going
//! without; see [`Icon::TriangleAlert`] and its neighbours.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, BoxShadow, Div, ElementId, FontWeight, MouseButton,
    MouseDownEvent, Pixels, Rgba, SharedString, Window, div, ease_out_quint, point, prelude::*, px,
};
use ket_core::theme::Theme;

use super::icon::{Icon, sized_icon};
use super::{CAPTION, LABEL, RADIUS_WELL};
use crate::fonts::Prose;
use crate::paint::{alpha, paint};

/// How wide a toast is allowed to get.
///
/// Sized against the sidebar rather than the window: a report about a row is
/// read next to that row, and a banner running the width of a 1200px window to
/// say four words is a different, louder thing than this is meant to be.
const TOAST_W: f32 = 380.0;

/// The card's corner radius.
const RADIUS: f32 = 6.0;

/// How tall the card is at least. Two lines of text fill it.
const MIN_H: f32 = 56.0;

/// The tinted tile behind the tone's mark.
const WELL: f32 = 32.0;

/// How long a toast takes to slide in from the edge of the window.
const SLIDE: Duration = Duration::from_millis(380);

/// How far off its resting place a toast starts: its own width, the margin,
/// and room for its shadow, so the first frame shows none of it.
const SLIDE_FROM: f32 = TOAST_W + 12.0 + 36.0;

/// How long the room a new toast needs takes to open up, pushing the ones
/// already there along instead of making them jump.
const GROW: Duration = Duration::from_millis(420);

/// How long the exit takes, start to finish: the fade, then the room
/// closing behind it.
pub(crate) const EXIT: Duration = Duration::from_millis(420);

/// Where in the exit the card has faded out completely.
const FADE_TO: f32 = 0.55;

/// Where in the exit the card's slot starts closing up — as the card is
/// nearly gone, so the stack settles into a gap that is already emptying.
const CLOSE_FROM: f32 = 0.45;

/// Space between the stack and the window edges.
const MARGIN: gpui::Pixels = px(12.0);

/// Space between two stacked toasts.
const STACK_GAP_PX: f32 = 8.0;
const STACK_GAP: gpui::Pixels = px(STACK_GAP_PX);

/// What a toast is saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // The component API is intentionally complete before its second caller.
pub(crate) enum Tone {
    /// Something happened that is worth knowing and needed nothing.
    Info,
    /// Something the reader asked for worked.
    Success,
    /// It worked, but not the whole way, and the remainder is theirs to know
    /// about — a worktree deleted whose branch had to be kept.
    Warning,
    /// It did not work.
    Error,
}

impl Tone {
    /// The hue that carries this tone.
    ///
    /// Every one of them is an existing theme token. A toast palette of its
    /// own would be a fifth opinion about what "bad" looks like in a window
    /// that already has four.
    pub(crate) fn colour(self, t: &Theme) -> Rgba {
        match self {
            Self::Info => paint(t.accent),
            Self::Success => paint(t.status.running),
            // Yellow, the quota meter's first step — not `attention`, whose
            // blue means an agent is waiting on you, which a caveat is not.
            Self::Warning => paint(t.quota.warm),
            Self::Error => paint(t.status.failed),
        }
    }

    /// The mark that carries this tone where hue cannot.
    pub(crate) fn icon(self) -> Icon {
        match self {
            Self::Info => Icon::Info,
            Self::Success => Icon::CircleCheck,
            Self::Warning => Icon::TriangleAlert,
            Self::Error => Icon::CircleX,
        }
    }
}

/// Which corner the stack grows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)] // The component API is intentionally complete before its second caller.
pub(crate) enum Corner {
    /// The default: out of the way of the sidebar, and out of the way of the
    /// status bar along the bottom.
    #[default]
    TopRight,
    /// Above the sidebar, which is where a report about a project row is
    /// closest to the row it is about.
    TopLeft,
    /// Nearest the status bar.
    BottomRight,
    /// Nearest the status bar, on the sidebar's side.
    BottomLeft,
}

impl Corner {
    /// Whether the newest toast belongs at the top of the stack.
    ///
    /// Newest is always the one nearest the corner, so a stack that grows
    /// downward from the top puts it first and one that grows upward from the
    /// bottom puts it last. Otherwise a second toast arriving would shove the
    /// first one under the reader's eye instead of queueing behind it.
    pub(crate) fn newest_first(self) -> bool {
        matches!(self, Self::TopRight | Self::TopLeft)
    }
}

/// The positioned container the toasts stack inside.
///
/// Absolute, so it has to be a child of a `relative` parent — the shell's root
/// is one. The caller fills it; this owns only where it sits.
pub(crate) fn stack(corner: Corner) -> Div {
    // Full-bleed and aligned, the way `dialog::centered` places a card, rather
    // than a small box pinned with `top`/`right`. Deliberately *not* occluding:
    // this covers the whole window, and only the toasts within it may take the
    // pointer — see `toast`, which occludes itself.
    let base = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .gap(STACK_GAP)
        .p(MARGIN);

    match corner {
        Corner::TopRight => base.items_end().justify_start(),
        Corner::TopLeft => base.items_start().justify_start(),
        Corner::BottomRight => base.items_end().justify_end(),
        Corner::BottomLeft => base.items_start().justify_end(),
    }
}

/// What one toast says and how it should arrive.
pub(crate) struct Spec {
    /// How it reads.
    pub(crate) tone: Tone,
    /// The headline.
    pub(crate) message: SharedString,
    /// A quieter second line — what it happened to.
    pub(crate) detail: Option<SharedString>,
    /// The corner the stack grows from, which is the edge it slides in off.
    pub(crate) corner: Corner,
    /// How far through its exit it is, 0 to 1, once it has started leaving.
    pub(crate) exit: Option<f32>,
    /// Its height as last laid out, which the caller keeps across frames.
    ///
    /// The slot a toast sits in opens and closes by animating its height, and
    /// a card's height depends on how its words wrap, so it is measured rather
    /// than guessed. Owned by the caller because an element keeps nothing
    /// between frames.
    pub(crate) height: Rc<Cell<Pixels>>,
}

/// One toast.
///
/// `on_press` is the caller's dismissal — clicking a toast should take it
/// away, and which shell state that touches is not this module's business.
///
/// It slides in off the nearest edge of the window while its room opens, and
/// leaves by fading where it is, after which its room closes so the rest of
/// the stack glides into the gap.
///
/// **The element tree is the same shape whether or not it is leaving.** An
/// animation's clock is keyed by the ids of everything above it, so wrapping a
/// leaving toast in a different animation restarts every one inside it. The
/// exit is driven by `Spec::exit` instead, from the caller's clock.
pub(crate) fn toast(
    id: impl Into<ElementId>,
    spec: Spec,
    t: &Theme,
    on_press: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id: ElementId = id.into();
    let colour = spec.tone.colour(t);
    let inner_w = TOAST_W - 2.0;
    let leaving = spec.exit.is_some();
    let exit = spec.exit.map_or(0.0, |p| p.clamp(0.0, 1.0));
    let shown = 1.0 - (exit / FADE_TO).clamp(0.0, 1.0);
    let closed = ease_in_out_cubic(((exit - CLOSE_FROM) / (1.0 - CLOSE_FROM)).clamp(0.0, 1.0));
    // Off the right edge for the right-hand corners, the left for the left.
    let side = match spec.corner {
        Corner::TopRight | Corner::BottomRight => 1.0,
        Corner::TopLeft | Corner::BottomLeft => -1.0,
    };

    let copy = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(LABEL)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(paint(t.text.primary))
                .child(spec.message),
        )
        .children(spec.detail.map(|detail| {
            div()
                .text_size(CAPTION)
                .text_color(paint(t.text.dim))
                .child(detail)
        }));

    let card = div()
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(13.0))
        .w(px(inner_w))
        .min_h(px(MIN_H - 2.0))
        .px(px(15.0))
        .py(px(12.0))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(WELL))
                .rounded(RADIUS_WELL)
                .bg(alpha(colour, 0.16))
                .child(sized_icon(spec.tone.icon(), px(19.0), colour)),
        )
        .child(copy);

    let shell = div()
        .id(id.clone())
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .w(px(TOAST_W))
        .overflow_hidden()
        .prose()
        .rounded(px(RADIUS))
        .bg(paint(t.elevated))
        .border_1()
        .border_color(alpha(paint(t.text.primary), 0.08))
        .shadow(vec![
            BoxShadow {
                color: crate::paint::shadow(0.5, t),
                offset: point(px(0.0), px(14.0)),
                blur_radius: px(34.0),
                spread_radius: px(0.0),
            },
            BoxShadow {
                color: crate::paint::shadow(0.35, t),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(6.0),
                spread_radius: px(0.0),
            },
        ])
        // The stack above it does not take the pointer, so the toast has to
        // take its own — this is what makes clicking one dismiss it rather
        // than falling through to whatever it is floating over. Not once it is
        // leaving: a card that is fading should not swallow a click meant for
        // whatever is showing through it.
        .when(!leaving, |el| {
            el.occlude()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, on_press)
        })
        .child(card)
        // Offset rather than moved in the layout, so the stack's alignment
        // and the room's measurement are the resting card's from the first
        // frame. The fade rides on the same animation: past its end gpui
        // keeps calling it at 1, and the exit's progress comes in from the
        // caller rather than from this clock.
        .with_animation(
            (id.clone(), "enter"),
            Animation::new(SLIDE).with_easing(ease_out_quint()),
            move |el, delta| {
                el.left(px(side * SLIDE_FROM * (1.0 - delta)))
                    .opacity(shown)
            },
        );

    // The room the card takes in the stack. Its height is what opens and
    // closes; the card inside keeps its own and is never clipped, so its
    // shadow is not cut off while the room around it changes.
    let measure = spec.height.clone();
    let height = spec.height;
    div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .on_children_prepainted(move |bounds, _, _| {
            if let Some(card) = bounds.first() {
                measure.set(card.size.height);
            }
        })
        .child(shell)
        .with_animation(
            (id, "room"),
            Animation::new(GROW).with_easing(ease_out_quint()),
            move |el, delta| {
                let open = delta * (1.0 - closed);
                if open >= 1.0 {
                    return el;
                }
                // The gap to the next toast belongs to the stack, not the
                // slot, so it is eaten with a negative margin as the room
                // closes; otherwise the neighbours would settle 8px short
                // and then jump the rest when the toast is removed.
                el.h(height.get() * open)
                    .mb(px(-STACK_GAP_PX * (1.0 - open)))
            },
        )
        .into_any_element()
}

/// Gentle at both ends, for the stack settling into a gap.
fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}
