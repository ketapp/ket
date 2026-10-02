//! The single adapter between `ket-core`'s colours and `gpui`'s.
//!
//! Isolated deliberately. Every other module names a semantic token —
//! `theme.diff.added` — and this is the only code that knows how a token turns
//! into something paintable. When the renderer changes, one function moves.

use gpui::{Rgba, rgb};
use ket_core::theme::{Appearance, Color, Theme};

/// Turns a core colour token into one `gpui` can paint.
///
/// The only place in the shell that knows how a `Theme` colour is represented.
pub fn paint(colour: Color) -> Rgba {
    rgb((u32::from(colour.r) << 16) | (u32::from(colour.g) << 8) | u32::from(colour.b))
}

/// A translucent wash for a modal's backdrop.
///
/// Alpha, which no theme token carries: a scrim is not a colour a palette
/// chooses, it is the same dimming over every theme. Built here rather than at
/// a call site because this module is the only one that knows how a paintable
/// colour is made — see the module docs.
pub fn scrim() -> Rgba {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.55,
    }
}

/// `colour` at an alpha of the caller's choosing.
///
/// A token carries no alpha — a palette picks colours, not transparencies — so
/// every translucent thing in the shell is a token plus a number stated where
/// it is used. This is that, named, so a call site says what it means.
pub fn alpha(colour: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..colour }
}

/// `amount` of the way from `from` to `to`.
///
/// For the state washes: a worktree that is working is its selected ground
/// with a little of the running colour stirred in, rather than a second fill
/// that has to be kept in step with the first by hand. Straight linear
/// interpolation in sRGB — these are small steps between near neighbours,
/// where the difference from doing it properly is not visible.
pub fn mix(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    let amount = amount.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| a + (b - a) * amount;
    Rgba {
        r: lerp(from.r, to.r),
        g: lerp(from.g, to.g),
        b: lerp(from.b, to.b),
        a: lerp(from.a, to.a),
    }
}

/// A fixed identity colour — a file type's, a provider's mark — made to read
/// on the theme's ground.
///
/// Those colours are not tokens: they name a thing that does not change
/// meaning with the theme. But they were chosen against dark cards, and on a
/// light one the paler of them fall under 2:1. So a light theme takes each a
/// fixed step towards black, which keeps the hue someone recognises and
/// brings it back over 3:1; a dark theme uses it as it is.
pub fn on_ground(colour: Rgba, t: &Theme) -> Rgba {
    match t.appearance {
        Appearance::Dark => colour,
        Appearance::Light => mix(
            colour,
            Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: colour.a,
            },
            0.4,
        ),
    }
}

/// The colour of a drop shadow at `opacity`.
///
/// Black either way, but a shadow tuned to be seen on a near-black desk is a
/// smudge on a pale one: a light theme draws it at two fifths the strength.
pub fn shadow(opacity: f32, t: &Theme) -> gpui::Hsla {
    let opacity = match t.appearance {
        Appearance::Dark => opacity,
        Appearance::Light => opacity * 0.4,
    };
    gpui::Hsla::from(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: opacity,
    })
}
