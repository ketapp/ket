//! The planet under the empty content view: a sphere of glyphs rising from
//! the bottom of the pane, lit at its rim, with a grid turning slowly across
//! it and a scatter of stars above.
//!
//! Painted, not laid out: a character field is one shaped line per row, each
//! with a run per ink, and building it as elements would be thousands of
//! them. Only its three inks change between frames — the grid moves and the
//! glyphs flicker — so the cost is the rows' shaping, a few dozen lines of
//! mono at a caption's size.
//!
//! Its motion runs off [`crate::motion::phase`], so it redraws at the
//! module's tick, slower while the window is not key, and not at all under
//! `KET_STILL` or while nothing reads it — the view is only drawn with no
//! worktree open, when there is no terminal on screen to compete with.

use std::time::Duration;

use gpui::{
    AnyElement, Bounds, Font, FontFeatures, FontStyle, FontWeight, Pixels, Rgba, SharedString,
    TextRun, canvas, point, prelude::*, px,
};

use crate::paint::alpha;

/// How long one full turn of the grid takes, and so how often the clock the
/// planet reads wraps. The grid turns a whole number of cells in it, so the
/// wrap does not show.
const CYCLE: Duration = Duration::from_secs(120);

/// Grid turns per [`CYCLE`].
const TURNS: f32 = 9.0;

/// The glyphs' size and the rows' pitch.
const GLYPH: Pixels = px(10.0);
const ROW: f32 = 12.0;

/// Where the planet's top sits, as a share of the pane's height from the top.
/// What the pane says sits above it.
pub(crate) const HORIZON: f32 = 0.62;

/// The sphere's radius as a share of the pane's width: wide enough that its
/// arc reads as a horizon rather than a ball.
const RADIUS: f32 = 0.865;

/// How far the glow reaches off the sphere's edge.
const HALO: f32 = 70.0;

/// Glyphs, brightest first within each ink.
const RIM: [char; 4] = ['@', '#', '%', '*'];
const MID: [char; 4] = ['*', '+', '=', 'o'];
const LOW: [char; 4] = ['.', ':', '-', '~'];

/// The planet, filling whatever it is given, in `ink`.
pub(crate) fn horizon(ink: Rgba) -> AnyElement {
    let seconds = crate::motion::phase(CYCLE) * CYCLE.as_secs_f32();
    let family = crate::fonts::chrome();
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, cx| {
            let cell = crate::editor::code_cell_width(window, &family, GLYPH);
            let field = Field::new(bounds, f32::from(cell), seconds);
            let inks = [ink, alpha(ink, 0.62), alpha(ink, 0.3)];
            let font = Font {
                family: family.clone(),
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            };
            for row in 0..field.rows {
                let Some((text, runs)) = field.row(row, &font, &inks) else {
                    continue;
                };
                let line = window
                    .text_system()
                    .shape_line(text, GLYPH, &runs, Some(cell));
                let origin = point(bounds.left(), bounds.top() + px(row as f32 * ROW));
                line.paint(origin, px(ROW), window, cx).ok();
            }
        },
    )
    .size_full()
    .into_any_element()
}

/// One frame of the field: where the sphere is, and when it is.
struct Field {
    cols: usize,
    rows: usize,
    cell: f32,
    centre_x: f32,
    centre_y: f32,
    radius: f32,
    /// How far the grid has turned.
    turn: f32,
    /// Steps of the glyphs' flicker and the halo's shimmer.
    flicker: i32,
    shimmer: i32,
}

impl Field {
    fn new(bounds: Bounds<Pixels>, cell: f32, seconds: f32) -> Self {
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let radius = (width * RADIUS).max(480.0);
        Self {
            cols: (width / cell).ceil() as usize,
            rows: (height / ROW).ceil() as usize,
            cell,
            centre_x: width / 2.0,
            centre_y: height * HORIZON + radius,
            radius,
            turn: seconds * TURNS * std::f32::consts::TAU / CYCLE.as_secs_f32(),
            flicker: (seconds / 0.44) as i32,
            shimmer: (seconds / 0.22) as i32 + 11,
        }
    }

    /// Row `row`'s glyphs and a run per stretch of one ink, or nothing for a
    /// row with no glyph in it. Spaces ride on the run before them, so a row
    /// is as few runs as it has changes of ink.
    fn row(
        &self,
        row: usize,
        font: &Font,
        inks: &[Rgba; 3],
    ) -> Option<(SharedString, Vec<TextRun>)> {
        let mut text = String::with_capacity(self.cols);
        // (ink, bytes) per stretch of one ink.
        let mut stretches: Vec<(usize, usize)> = Vec::new();
        let mut any = false;
        for col in 0..self.cols {
            let (glyph, ink) = self.glyph(col as i32, row as i32);
            text.push(glyph);
            let len = glyph.len_utf8();
            any |= ink.is_some();
            let ink = ink.unwrap_or_else(|| stretches.last().map_or(2, |&(ink, _)| ink));
            match stretches.last_mut() {
                Some((last, bytes)) if *last == ink => *bytes += len,
                _ => stretches.push((ink, len)),
            }
        }
        let runs = stretches
            .into_iter()
            .map(|(ink, len)| TextRun {
                len,
                font: font.clone(),
                color: inks[ink].into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        any.then(|| (text.into(), runs))
    }

    /// The glyph at a cell and which ink it takes — 0 brightest — or a space.
    fn glyph(&self, col: i32, row: i32) -> (char, Option<usize>) {
        let x = col as f32 * self.cell + self.cell / 2.0 - self.centre_x;
        let y = row as f32 * ROW + ROW / 2.0 - self.centre_y;
        let distance = (x * x + y * y).sqrt();
        let pick = |set: &[char; 4], roll: f32| set[((roll * 4.0) as usize).min(3)];

        if distance <= self.radius {
            let nx = x / self.radius;
            let ny = y / self.radius;
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            // Bright where the surface turns away: the rim of a lit horizon.
            let rim = (1.0 - nz).powf(2.4);
            let longitude = nx.atan2(nz) * 14.0 + self.turn;
            let latitude = ny.asin() * 30.0;
            let grid = if longitude.sin().abs() > 0.97 || latitude.sin().abs() > 0.985 {
                0.42
            } else {
                0.0
            };
            let light = rim * 1.15 + grid * (0.35 + rim) + if nx < -0.1 { 0.06 } else { 0.0 };
            let roll = hash(col, row, self.flicker);
            return if light > 0.72 {
                (pick(&RIM, roll), Some(0))
            } else if light > 0.36 {
                (pick(&MID, roll), Some(1))
            } else if light > 0.1 || roll < 0.28 {
                (pick(&LOW, roll), Some(2))
            } else {
                (' ', None)
            };
        }
        if distance < self.radius + HALO {
            let reach = 1.0 - (distance - self.radius) / HALO;
            if hash(col, row, self.shimmer) < reach * reach * 0.55 {
                return if distance < self.radius + 18.0 {
                    ('=', Some(1))
                } else {
                    ('.', Some(2))
                };
            }
            return (' ', None);
        }
        // Stars: fixed places, each twinkling on its own beat.
        if hash(col, row, 5) < 0.012 {
            let twinkle = hash(col, row, self.flicker);
            return if twinkle > 0.85 {
                ('+', Some(1))
            } else if twinkle > 0.4 {
                ('.', Some(2))
            } else {
                ('·', Some(2))
            };
        }
        (' ', None)
    }
}

/// A stable pseudo-random number in `0..1` for a cell at a step — the same
/// cell at the same step always rolls the same, so the field is steady
/// between the steps of its flicker.
fn hash(x: i32, y: i32, t: i32) -> f32 {
    let mut n = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(t.wrapping_mul(1_274_126_177));
    n = (n ^ ((n as u32 >> 13) as i32)).wrapping_mul(1_103_515_245);
    n ^= (n as u32 >> 16) as i32;
    (n as u32 % 10_000) as f32 / 10_000.0
}
