//! The phone a device tab draws while it waits: a handset of glyphs turning
//! in the pane, its screen showing what the tab is waiting on — dark while it
//! looks for one, a bar filling while it boots, its icons coming in while ket
//! connects to its screen.
//!
//! Raymarched, not modelled: each cell casts one ray at a rounded slab and
//! shades what it hits, lit from a lamp above and to the left, as `orbit.rs`
//! shades its planet. A frame is a few thousand distance sums and [`ROWS`]
//! shaped lines of mono.
//!
//! How far it has turned is carried from frame to frame in a [`Turn`], so a
//! change of stage — looking, then starting, then connecting — swings the
//! phone round to its new pose rather than snapping it there. Its clock is
//! [`crate::motion::phase`], so it redraws at the module's tick, slower while
//! the window is not key, and not at all under `KET_STILL`.

use std::cell::Cell;
use std::f32::consts::{PI, TAU};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Font, FontFeatures, FontStyle, FontWeight, Pixels, Rgba, SharedString, TextRun,
    canvas, div, point, prelude::*, px,
};
use ket_core::theme::Theme;

use crate::fonts::Prose as _;
use crate::paint::{alpha, paint};

/// The clock the motion reads, which wraps once a minute. Every cycle below
/// fits a whole number of times into it, so the wrap does not show.
const CLOCK: Duration = Duration::from_secs(60);

/// One whole turn while it looks for a phone.
const LAP: f32 = 8.0;

/// The sway while it starts, and while it connects: seconds, and radians
/// either side of facing the viewer.
const BOOT_SWAY: (f32, f32) = (7.5, 0.62);
const CONNECT_SWAY: (f32, f32) = (10.0, 0.2);

/// How fast the phone swings onto a new pose: the share of the way it closes
/// in a second, as a rate.
const SETTLE: f32 = 3.0;

/// How long the icons take to come in, and how often they come in again.
const WIPE: (f32, f32) = (3.2, 5.0);

/// The boot bar fills towards its end and never reaches it: two thirds of
/// the way at this, and most of the rest by three times it.
const BOOT: f32 = 15.0;

/// The glyphs' size and the rows' pitch: `orbit.rs`'s.
const GLYPH: Pixels = px(11.0);
const ROW: f32 = 13.0;

/// The field, in cells.
const COLS: usize = 58;
const ROWS: usize = 27;

/// How much of the scene the field's height spans: the phone is two tall.
const SPAN: f32 = 2.3;

/// The eye's distance from the phone, and the lamp, both in the viewer's
/// space: x right, y up, z away.
const EYE: f32 = 3.4;
const LAMP: [f32; 3] = [-1.6, 2.2, -2.6];

/// How far the phone leans back.
const PITCH: f32 = -0.12;

/// The body, darkest first.
const SHADE: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// Which of the six inks a glyph takes: faint, low, dim, text, accent,
/// accent low.
type Ink = u8;

/// A cell: its glyph and ink, or `None` for a space.
type Glyph = Option<(char, Ink)>;

/// What the tab is waiting on, which sets the phone's pose and its screen.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Stage {
    /// Looking for one running: turning round, its screen dark.
    Looking,
    /// Booting one: swaying, a bar filling across its screen.
    Starting,
    /// Reaching its screen: facing the viewer, icons coming in.
    Connecting,
}

/// Which phone to draw.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Model {
    /// Large corners, a Dynamic Island, three lenses in a square.
    Iphone,
    /// Tighter corners, a punch-hole camera, a camera bar across the back.
    Android,
}

/// How far the phone has turned, kept by the tab between frames.
#[derive(Clone, Copy)]
pub(crate) struct Turn {
    /// Its turn about the vertical, from facing the viewer.
    yaw: f32,
    /// Where the clock was at the last frame, in seconds.
    clock: Option<f32>,
    /// When the stage became [`Stage::Starting`], for the bar and the time.
    starting: Option<Instant>,
}

impl Default for Turn {
    /// Three-quarters on, the screen to the viewer.
    fn default() -> Self {
        Self {
            yaw: -0.6,
            clock: None,
            starting: None,
        }
    }
}

impl Turn {
    /// Moves on to now, in `stage`: the clock, in seconds round [`CLOCK`].
    fn advance(&mut self, stage: Stage) -> f32 {
        let now = crate::motion::phase(CLOCK) * CLOCK.as_secs_f32();
        // A tab back on screen after a while picks up where it was.
        let step = self
            .clock
            .map_or(0.0, |last| (now - last).rem_euclid(CLOCK.as_secs_f32()))
            .min(0.25);
        self.clock = Some(now);
        let sway = |(cycle, reach): (f32, f32)| reach * (now * TAU / cycle).sin();
        match stage {
            Stage::Looking => self.yaw = (self.yaw + step * TAU / LAP).rem_euclid(TAU),
            Stage::Starting => self.settle(sway(BOOT_SWAY), step),
            Stage::Connecting => self.settle(sway(CONNECT_SWAY), step),
        }
        if stage == Stage::Starting {
            self.starting.get_or_insert_with(Instant::now);
        } else {
            self.starting = None;
        }
        now
    }

    /// Swings towards `yaw` the short way round.
    fn settle(&mut self, yaw: f32, step: f32) {
        let gap = (yaw - self.yaw + PI).rem_euclid(TAU) - PI;
        self.yaw += gap * (1.0 - (-step * SETTLE).exp());
    }

    /// How long it has been starting.
    fn booting(&self) -> Duration {
        self.starting.map_or(Duration::ZERO, |at| at.elapsed())
    }
}

/// The line under a starting phone's name: how long it has taken so far.
pub(crate) fn booting(turn: &Cell<Turn>) -> SharedString {
    let secs = turn.get().booting().as_secs();
    format!("{}:{:02} · longer the first time", secs / 60, secs % 60).into()
}

/// The tab's picture while it waits: `title` over `note` over the phone,
/// centred in the pane.
pub(crate) fn waiting(
    turn: &Cell<Turn>,
    model: Model,
    stage: Stage,
    title: impl Into<SharedString>,
    note: impl Into<SharedString>,
    theme: &Theme,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .child(
            div()
                .prose()
                .text_size(px(15.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(theme.text.primary))
                .child(title.into()),
        )
        .child(
            div()
                .mt(px(6.0))
                .text_size(px(10.0))
                .text_color(paint(theme.text.dim))
                .child(note.into()),
        )
        .child(div().w_full().mt(px(28.0)).child(handset(
            turn,
            model,
            stage,
            paint(theme.text.primary),
            paint(theme.text.dim),
            paint(theme.status.running),
        )))
        .into_any_element()
}

/// The phone alone, `ROWS` rows tall and as wide as it is given. `text`
/// lights its edges, `dim` its faces, `accent` what is on its screen.
fn handset(
    turn: &Cell<Turn>,
    model: Model,
    stage: Stage,
    text: Rgba,
    dim: Rgba,
    accent: Rgba,
) -> AnyElement {
    let mut pose = turn.get();
    let now = pose.advance(stage);
    turn.set(pose);
    let scene = Scene {
        model,
        stage,
        yaw: pose.yaw,
        now,
        booted: 1.0 - (-pose.booting().as_secs_f32() / BOOT).exp(),
    };
    let family = crate::fonts::chrome();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let cell = crate::editor::code_cell_width(window, &family, GLYPH);
            let inks = [
                alpha(dim, 0.26),
                alpha(dim, 0.55),
                dim,
                text,
                accent,
                alpha(accent, 0.5),
            ];
            let font = Font {
                family: family.clone(),
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            };
            let left = bounds.left() + (bounds.size.width - cell * COLS as f32) / 2.0;
            for (row, cells) in scene.field(ROW / f32::from(cell)).iter().enumerate() {
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

/// One frame's worth of what is drawn.
#[derive(Clone, Copy)]
struct Scene {
    model: Model,
    stage: Stage,
    yaw: f32,
    /// The clock, in seconds round [`CLOCK`].
    now: f32,
    /// How full the boot bar is, `0..1`.
    booted: f32,
}

/// The phone's size, half of each: width, height, depth; and the radius of
/// its corners and of its edges, and the bezel round its screen.
struct Body {
    half: [f32; 3],
    corner: f32,
    edge: f32,
    bezel: f32,
}

impl Body {
    fn of(model: Model) -> Self {
        match model {
            Model::Iphone => Self {
                half: [0.485, 1.0, 0.06],
                corner: 0.2,
                edge: 0.035,
                bezel: 0.05,
            },
            Model::Android => Self {
                half: [0.47, 1.0, 0.06],
                corner: 0.13,
                edge: 0.035,
                bezel: 0.05,
            },
        }
    }

    /// How far `p` is from the surface, negative inside: a rounded rectangle
    /// pushed out to its depth, its edges rounded too.
    fn distance(&self, p: [f32; 3]) -> f32 {
        let [w, h, d] = self.half;
        let face = rounded(
            p[0],
            p[1],
            w - self.edge,
            h - self.edge,
            self.corner - self.edge,
        );
        let depth = p[2].abs() - (d - self.edge);
        face.max(0.0).hypot(depth.max(0.0)) + face.max(depth).min(0.0) - self.edge
    }

    /// The surface's outward direction at `p`.
    fn normal(&self, p: [f32; 3]) -> [f32; 3] {
        const E: f32 = 0.002;
        let along = |axis: usize| {
            let (mut a, mut b) = (p, p);
            a[axis] += E;
            b[axis] -= E;
            self.distance(a) - self.distance(b)
        };
        unit([along(0), along(1), along(2)])
    }
}

/// How far `(x, y)` is from a rectangle of half-size `(w, h)` with corners of
/// radius `r`, negative inside.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = x.abs() - (w - r);
    let qy = y.abs() - (h - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Scene {
    /// The frame, cells about `aspect` times as tall as they are wide.
    fn field(&self, aspect: f32) -> Vec<[Glyph; COLS]> {
        let mut grid = vec![[None; COLS]; ROWS];
        let body = Body::of(self.model);
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = PITCH.sin_cos();
        // The viewer's space into the phone's, and back.
        let into = |[x, y, z]: [f32; 3]| {
            let (y1, z1) = (y * cp + z * sp, -y * sp + z * cp);
            [x * cy - z1 * sy, y1, x * sy + z1 * cy]
        };
        let out = |[x, y, z]: [f32; 3]| {
            let (x1, z1) = (x * cy + z * sy, -x * sy + z * cy);
            [x1, y * cp - z1 * sp, y * sp + z1 * cp]
        };
        let row_size = SPAN / ROWS as f32;
        let col_size = row_size / aspect;
        let eye = into([0.0, 0.0, -EYE]);
        for (row, cells) in grid.iter_mut().enumerate() {
            for (col, cell) in cells.iter_mut().enumerate() {
                let u = (col as f32 - (COLS as f32 - 1.0) / 2.0) * col_size;
                let v = ((ROWS as f32 - 1.0) / 2.0 - row as f32) * row_size;
                if u * u + v * v > 1.35 * 1.35 {
                    continue;
                }
                let ray = into(unit([u, v, EYE]));
                let Some(hit) = march(&body, eye, ray) else {
                    continue;
                };
                *cell = self.shade(&body, hit, out, (col_size, row_size));
            }
        }
        grid
    }

    /// The glyph for the point `p` on the phone, in its own space.
    fn shade(
        &self,
        body: &Body,
        p: [f32; 3],
        out: impl Fn([f32; 3]) -> [f32; 3],
        (col_size, row_size): (f32, f32),
    ) -> Glyph {
        let [w, h, _] = body.half;
        let normal = body.normal(p);
        let seen = out(normal);
        let at = out(p);
        let lamp = unit([LAMP[0] - at[0], LAMP[1] - at[1], LAMP[2] - at[2]]);
        let eye = unit([-at[0], -at[1], -EYE - at[2]]);
        let half = unit([lamp[0] + eye[0], lamp[1] + eye[1], lamp[2] + eye[2]]);
        let lit = dot(seen, lamp).max(0.0);
        let towards = dot(seen, half).max(0.0);

        let (sw, sh) = (w - body.bezel, h - body.bezel);
        if normal[2] < -0.7 && rounded(p[0], p[1], sw, sh, body.corner - body.bezel) < 0.0 {
            return self.screen(p, (sw, sh), towards.powi(40), (col_size, row_size));
        }

        let light = 0.06 + 0.62 * lit + 0.6 * towards.powi(14);
        let mut glyph = SHADE[((light * 10.0) as usize).clamp(1, 9)];
        let mut ink = match light {
            l if l < 0.3 => 0,
            l if l < 0.52 => 1,
            l if l < 0.78 => 2,
            _ => 3,
        };
        // The cameras, on the back.
        if normal[2] > 0.7 {
            let lens = match self.model {
                Model::Android if (p[1] - (h - 0.3)).abs() < 0.075 && p[0].abs() < w - 0.07 => {
                    Some((p[0] - 0.12).abs() < 0.07 || (p[0] - 0.3).abs() < 0.07)
                }
                Model::Iphone
                    if p[0] > w - 0.44 && p[1] > h - 0.44 && p[0] < w - 0.05 && p[1] < h - 0.05 =>
                {
                    let lenses = [
                        (w - 0.15, h - 0.15),
                        (w - 0.15, h - 0.34),
                        (w - 0.34, h - 0.245),
                    ];
                    Some(
                        lenses
                            .iter()
                            .any(|&(x, y)| ((p[0] - x) * 0.8).hypot(p[1] - y) < 0.07),
                    )
                }
                _ => None,
            };
            match lens {
                Some(true) => (glyph, ink) = ('O', 3),
                Some(false) => {
                    glyph = if self.model == Model::Iphone {
                        '%'
                    } else {
                        '='
                    };
                    ink = ink.max(2);
                }
                None => {}
            }
        }
        Some((glyph, ink))
    }

    /// The screen at `p`: glass with a streak of light across it, and over it
    /// whatever the stage shows. `(sw, sh)` is the screen's half-size.
    fn screen(
        &self,
        p: [f32; 3],
        (sw, sh): (f32, f32),
        gloss: f32,
        (col_size, row_size): (f32, f32),
    ) -> Glyph {
        let (x, y) = (p[0] / sw, p[1] / sh);
        let cutout = match self.model {
            Model::Iphone => p[0].abs() < 0.12 && (p[1] - (sh - 0.085)).abs() < 0.045,
            Model::Android => (p[0] / col_size).hypot((p[1] - (sh - 0.08)) / row_size) < 0.75,
        };
        if cutout {
            return None;
        }
        match self.stage {
            Stage::Looking => {}
            Stage::Starting => {
                if (p[1] + 0.02).abs() < row_size * 0.45 && x.abs() < 0.5 {
                    return Some(if x + 0.5 < self.booted {
                        ('=', 4)
                    } else {
                        ('-', 0)
                    });
                }
            }
            Stage::Connecting => {
                let (reveal, cycle) = WIPE;
                let line = 1.15 - 2.5 * ((self.now % cycle) / reveal).min(1.0);
                if (y - line).abs() < 0.045 {
                    return Some(('-', 3));
                }
                if y > line {
                    let icon = |cx: f32, cy: f32| (x - cx).abs() < 0.13 && (y - cy).abs() < 0.055;
                    let columns = [-0.6, -0.2, 0.2, 0.6];
                    let on = columns.iter().any(|&cx| {
                        [0.66, 0.42, 0.18, -0.06, -0.84]
                            .iter()
                            .any(|&cy| icon(cx, cy))
                    });
                    if on {
                        return Some(('#', 4));
                    }
                    if (y + 0.84).abs() < 0.1 {
                        return Some(('.', 5));
                    }
                }
            }
        }
        // Two bands of light that slide across the glass as it turns.
        let streak = x * 0.55 + y * 0.4 + 2.2 * self.yaw.sin();
        Some(if streak.abs() < 0.05 {
            ('/', 2)
        } else if (streak - 0.14).abs() < 0.025 {
            ('/', 1)
        } else if gloss > 0.55 {
            (':', 1)
        } else if gloss > 0.2 {
            ('.', 1)
        } else {
            ('.', 0)
        })
    }
}

/// Where a ray from `from` along `along` first meets the phone, if it does.
fn march(body: &Body, from: [f32; 3], along: [f32; 3]) -> Option<[f32; 3]> {
    let mut travelled = 0.0;
    for _ in 0..64 {
        let p = [
            from[0] + along[0] * travelled,
            from[1] + along[1] * travelled,
            from[2] + along[2] * travelled,
        ];
        let distance = body.distance(p);
        if distance < 0.002 {
            return Some(p);
        }
        travelled += distance;
        if travelled > EYE + 2.0 {
            break;
        }
    }
    None
}

/// A row's text and a run per stretch of one ink, or nothing for a row with
/// no glyph in it. Spaces ride on the run before them.
fn shape(
    cells: &[Glyph; COLS],
    font: &Font,
    inks: &[Rgba; 6],
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
