//! A QR code, drawn as modules on a light ground.
//!
//! For the pairing screen, which is its first caller: a phone's camera reads
//! the code off the window. `qrcode` does the encoding; nothing here draws
//! through an image or an SVG, because a module is a filled square and `gpui`
//! paints filled squares directly.
//!
//! Two things matter more here than anywhere else in the chrome, and both are
//! about the camera rather than the eye:
//!
//! - **Dark modules on a light ground**, whatever the theme. Scanners are
//!   built for that polarity and many will not read the inverse, so the
//!   code carries its own light quiet zone rather than sitting on a dark
//!   window. The two inks are the theme's own text and surface tokens,
//!   assigned by which of the pair is lighter, so a light theme and a dark
//!   one both come out right way round.
//! - **Whole pixels.** Every module is the same integer size and the code is
//!   placed on a whole pixel, so no module is drawn a pixel wider than its
//!   neighbour or smeared across two by antialiasing.

use std::sync::Arc;

use gpui::{AnyElement, Bounds, Pixels, canvas, div, fill, point, prelude::*, px, size};
use ket_core::theme::{Color, Theme};

use super::RADIUS_MD;
use crate::paint::paint;

/// The light margin around the code, in modules. Four is what the standard
/// asks for, and what scanners are tuned to find the finder patterns inside.
const QUIET_ZONE: usize = 4;

/// The smallest module drawn, in pixels. Below this a phone held at arm's
/// length stops resolving them.
const MIN_MODULE: f32 = 2.0;

/// An encoded code: which modules are dark, row by row.
///
/// Built once per code rather than per frame — encoding runs Reed-Solomon
/// and tries eight masks — and shared into the paint closure.
#[derive(Debug)]
pub(crate) struct QrModules {
    width: usize,
    dark: Vec<bool>,
}

impl QrModules {
    /// Encodes `text`, or `None` if it is too long for any QR version.
    ///
    /// Error correction at M, which recovers from about 15% damage: a
    /// screen has glare and moiré but no torn corners, and the pairing code
    /// is long enough that H would push it several versions denser.
    pub(crate) fn encode(text: &str) -> Option<Arc<Self>> {
        let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
        let width = code.width();
        let dark = code
            .into_colors()
            .into_iter()
            .map(|colour| colour == qrcode::Color::Dark)
            .collect();
        Some(Arc::new(Self { width, dark }))
    }

    /// Modules per side, quiet zone included.
    fn span(&self) -> usize {
        self.width + 2 * QUIET_ZONE
    }
}

/// A code drawn at the largest whole-pixel module size that fits in `fit`.
///
/// The element is exactly the code's size, quiet zone included, so it can
/// be centred or aligned like any other fixed box.
pub(crate) fn qr_code(modules: Arc<QrModules>, fit: Pixels, t: &Theme) -> AnyElement {
    let span = modules.span();
    let module = (f32::from(fit) / span as f32).floor().max(MIN_MODULE);
    let side = px(module * span as f32);
    let (light, dark) = inks(t);

    div()
        .flex_none()
        .size(side)
        .rounded(RADIUS_MD)
        .bg(paint(light))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    paint_modules(&modules, bounds, module, paint(dark), window);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// The quiet zone's ink and the modules', lighter first.
fn inks(t: &Theme) -> (Color, Color) {
    let (a, b) = (t.text.primary, t.surface);
    if lightness(a) >= lightness(b) {
        (a, b)
    } else {
        (b, a)
    }
}

/// A rough perceived lightness, enough to tell which of two inks is the
/// light one.
fn lightness(colour: Color) -> u32 {
    299 * u32::from(colour.r) + 587 * u32::from(colour.g) + 114 * u32::from(colour.b)
}

/// Paints the dark modules, a horizontal run at a time.
///
/// One quad per run rather than per module: a pairing code is around
/// 57×57, and merging each row's runs roughly halves the quads without
/// changing a pixel.
fn paint_modules(
    modules: &QrModules,
    bounds: Bounds<Pixels>,
    module: f32,
    ink: gpui::Rgba,
    window: &mut gpui::Window,
) {
    let origin = point(bounds.origin.x.round(), bounds.origin.y.round());
    let inset = QUIET_ZONE as f32 * module;
    let width = modules.width;
    for (y, row) in modules.dark.chunks(width).enumerate() {
        let mut x = 0;
        while x < width {
            if !row[x] {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && row[x] {
                x += 1;
            }
            let at = point(
                origin.x + px(inset + start as f32 * module),
                origin.y + px(inset + y as f32 * module),
            );
            let extent = size(px((x - start) as f32 * module), px(module));
            window.paint_quad(fill(Bounds::new(at, extent), ink));
        }
    }
}
