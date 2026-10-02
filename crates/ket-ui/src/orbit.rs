//! The picture over a clean working tree in the Git view: a planet with a
//! moon on a stable orbit round it, drawn in glyphs.
//!
//! Painted, not laid out: one shaped line per row, each with a run per ink,
//! on a fixed field of [`COLS`] by [`ROWS`] cells centred in whatever width it
//! is given. Only the moon moves, so a frame differs from the last by a few
//! cells, but every row is shaped again — a few dozen short lines of mono.
//!
//! Its motion runs off [`crate::motion::phase`], so it redraws at the
//! module's tick, slower while the window is not key, and not at all under
//! `KET_STILL` or while the view is not on screen.

use std::f32::consts::TAU;
use std::time::Duration;

use gpui::{
    AnyElement, Font, FontFeatures, FontStyle, FontWeight, Pixels, Rgba, SharedString, TextRun,
    canvas, point, prelude::*, px,
};

use crate::paint::alpha;

/// One lap of the moon.
const CYCLE: Duration = Duration::from_secs(18);

/// The glyphs' size and the rows' pitch.
const GLYPH: Pixels = px(11.0);
const ROW: f32 = 13.0;

/// The field, in cells.
const COLS: usize = 40;
const ROWS: usize = 19;

/// The planet's radius, in rows.
const PLANET: f32 = 3.4;

/// The orbit's half-width in columns and half-height in rows: a ring seen
/// from just above its plane.
const ORBIT_X: f32 = 16.0;
const ORBIT_Y: f32 = 4.6;

/// The planet's surface, darkest first.
const SHADE: [char; 9] = ['.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// Which of the five inks a glyph takes: faint, dim, soft, text, moon.
type Ink = u8;

/// A cell: its glyph and ink, or `None` for a space.
type Cell = Option<(char, Ink)>;

/// The planet and its moon, `ROWS` rows tall and as wide as it is given.
/// `text` lights the planet, `dim` the ring and the stars, `moon` the moon.
pub(crate) fn orbit(text: Rgba, dim: Rgba, moon: Rgba) -> AnyElement {
    let angle = crate::motion::phase(CYCLE) * TAU;
    let family = crate::fonts::chrome();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let cell = crate::editor::code_cell_width(window, &family, GLYPH);
            // A cell is about half as wide as it is tall; circles are drawn
            // against the real ratio so the planet stays round.
            let aspect = ROW / f32::from(cell);
            let inks = [alpha(dim, 0.3), alpha(dim, 0.65), dim, text, moon];
            let font = Font {
                family: family.clone(),
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            };
            let left = bounds.left() + (bounds.size.width - cell * COLS as f32) / 2.0;
            for (row, cells) in field(angle, aspect).iter().enumerate() {
                let Some((line, runs)) = shape(cells, &font, &inks) else {
                    continue;
                };
                let line = window
                    .text_system()
                    .shape_line(line, GLYPH, &runs, Some(cell));
                let origin = point(left, bounds.top() + px(row as f32 * ROW));
                line.paint(origin, px(ROW), window, cx).ok();
            }
        },
    )
    .w_full()
    .h(px(ROWS as f32 * ROW))
    .into_any_element()
}

/// One frame: the moon `angle` round its orbit.
fn field(angle: f32, aspect: f32) -> Vec<[Cell; COLS]> {
    let mut grid = vec![[None; COLS]; ROWS];
    let cx = (COLS as f32 - 1.0) / 2.0;
    let cy = (ROWS as f32 - 1.0) / 2.0;
    let on_planet = |x: f32, y: f32| (x - cx).hypot((y - cy) * aspect) < PLANET * aspect;
    let on_orbit = |th: f32| (cx + ORBIT_X * th.cos(), cy + ORBIT_Y * th.sin());
    // Positive sine is the near half of the ring, drawn over the planet.
    let near = |th: f32| th.sin() >= 0.0;

    for (row, cells) in grid.iter_mut().enumerate() {
        for (col, cell) in cells.iter_mut().enumerate() {
            if hash(col as i32, row as i32, 5) < 0.035 {
                *cell = Some(('.', 0));
            }
        }
    }
    for step in (0..360).step_by(3) {
        let th = (step as f32).to_radians();
        if !near(th) {
            let (x, y) = on_orbit(th);
            put(&mut grid, x, y, '.', 1);
        }
    }
    for (row, cells) in grid.iter_mut().enumerate() {
        for (col, cell) in cells.iter_mut().enumerate() {
            let dx = (col as f32 - cx) / (PLANET * aspect);
            let dy = (row as f32 - cy) / PLANET;
            let d = dx.hypot(dy);
            if d >= 1.0 {
                continue;
            }
            // Lit from the upper left.
            let light = (-dx * 0.8 - dy * 0.35 + (1.0 - d * d).sqrt() * 0.6).max(0.0);
            let k = ((light * SHADE.len() as f32) as usize).min(SHADE.len() - 1);
            *cell = Some((SHADE[k], if k > 5 { 3 } else { 2 }));
        }
    }
    for step in (0..360).step_by(3) {
        let th = (step as f32).to_radians();
        let (x, y) = on_orbit(th);
        if near(th) && !on_planet(x, y) {
            put(&mut grid, x, y, '.', 1);
        }
    }
    // The moon, and a short wake behind it.
    for back in (1..=5).rev() {
        let th = angle - back as f32 * 0.09;
        let (x, y) = on_orbit(th);
        if near(th) || !on_planet(x, y) {
            let (glyph, ink) = if back < 3 { ('-', 2) } else { ('.', 1) };
            put(&mut grid, x, y, glyph, ink);
        }
    }
    let (x, y) = on_orbit(angle);
    if near(angle) || !on_planet(x, y) {
        put(&mut grid, x, y, 'o', 4);
    }
    grid
}

/// Sets the cell nearest `(x, y)`, if it is on the field.
fn put(grid: &mut [[Cell; COLS]], x: f32, y: f32, glyph: char, ink: Ink) {
    let (col, row) = (x.round(), y.round());
    if (0.0..COLS as f32).contains(&col) && (0.0..ROWS as f32).contains(&row) {
        grid[row as usize][col as usize] = Some((glyph, ink));
    }
}

/// A row's text and a run per stretch of one ink, or nothing for a row with
/// no glyph in it. Spaces ride on the run before them.
fn shape(
    cells: &[Cell; COLS],
    font: &Font,
    inks: &[Rgba; 5],
) -> Option<(SharedString, Vec<TextRun>)> {
    if cells.iter().all(Option::is_none) {
        return None;
    }
    let mut text = String::with_capacity(COLS);
    // (ink, bytes) per stretch of one ink.
    let mut stretches: Vec<(Ink, usize)> = Vec::new();
    for cell in cells {
        let (glyph, ink) = match cell {
            Some((glyph, ink)) => (*glyph, *ink),
            None => (' ', stretches.last().map_or(0, |&(ink, _)| ink)),
        };
        text.push(glyph);
        match stretches.last_mut() {
            Some((last, bytes)) if *last == ink => *bytes += glyph.len_utf8(),
            _ => stretches.push((ink, glyph.len_utf8())),
        }
    }
    let runs = stretches
        .into_iter()
        .map(|(ink, len)| TextRun {
            len,
            font: font.clone(),
            color: inks[ink as usize].into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        })
        .collect();
    Some((text.into(), runs))
}

/// A stable pseudo-random number in `0..1` for a cell and a seed, so the
/// stars keep their places from frame to frame.
fn hash(x: i32, y: i32, t: i32) -> f32 {
    let mut n = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(t.wrapping_mul(1_274_126_177));
    n = (n ^ ((n as u32 >> 13) as i32)).wrapping_mul(1_103_515_245);
    n ^= (n as u32 >> 16) as i32;
    (n as u32 % 10_000) as f32 / 10_000.0
}
