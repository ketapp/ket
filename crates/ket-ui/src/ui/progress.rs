//! A progress bar: one length read against its whole.
//!
//! A quota in the header and in the usage popup, a build's share of a
//! worktree's footprint. Each was drawn where it was used, and they disagreed
//! on their ends — round in the header, square in the popup — for no reason
//! beyond who drew which. Round ends everywhere now: at four or six pixels
//! the cap is most of what makes a short fill read as a fill.
//!
//! Not a slider. The audio player's seek track takes a press and carries a
//! thumb; this only reports.

use gpui::{Div, Pixels, Rgba, div, prelude::*, px, relative};
use ket_core::theme::Theme;

use crate::paint::paint;

/// A bar's thickness unless the caller says otherwise: the header's.
const HEIGHT: Pixels = px(4.0);

/// A bar under construction. See [`progress`].
pub(crate) struct Progress {
    fraction: f32,
    width: Option<Pixels>,
    grow: bool,
    height: Pixels,
    fill: Option<Rgba>,
}

/// A bar filled to `fraction` of its length, `0.0..=1.0`. Anything outside
/// that, NaN included, is drawn clamped rather than as a bar of
/// indeterminate width.
pub(crate) fn progress(fraction: f32) -> Progress {
    Progress {
        fraction: if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        },
        width: None,
        grow: false,
        height: HEIGHT,
        fill: None,
    }
}

impl Progress {
    /// A fixed length, for a bar inside a figure. Without it the bar takes
    /// its container's width.
    pub(crate) fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Take whatever a row leaves between its fixed columns.
    pub(crate) fn grow(mut self) -> Self {
        self.grow = true;
        self
    }

    pub(crate) fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    /// The fill's colour; `accent` unless a ramp has something to say.
    pub(crate) fn fill(mut self, fill: Rgba) -> Self {
        self.fill = Some(fill);
        self
    }

    pub(crate) fn render(self, t: &Theme) -> Div {
        let bar = div()
            .flex_none()
            .h(self.height)
            .rounded_full()
            // `border`, so the far end is there to see even at zero.
            .bg(paint(t.border));
        let bar = match (self.width, self.grow) {
            (Some(width), _) => bar.w(width),
            (None, true) => bar.flex_1().min_w_0(),
            (None, false) => bar.w_full(),
        };
        bar.child(
            div()
                .h_full()
                .w(relative(self.fraction))
                // A sliver narrower than the bar is tall stops being a
                // rounded end and becomes a squashed oval.
                .when(self.fraction > 0.0, |fill| fill.min_w(self.height))
                .rounded_full()
                .bg(self.fill.unwrap_or_else(|| paint(t.accent))),
        )
    }
}
