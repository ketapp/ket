//! Pixels to cells, and cells to the mouse reports a program asked for.
//!
//! [`Geometry`] is the one fact the element publishes about where the grid
//! landed on screen; everything that turns a pointer position into a cell —
//! selection, link hit-testing, mouse reporting — goes through it, so there
//! is exactly one place that knows the grid's origin and cell size.
//!
//! The reports follow xterm: the legacy three-byte `CSI M` form by default,
//! the unambiguous `CSI <` SGR form when the program has enabled it, with
//! the button, modifier, motion and wheel bits laid out the same way in
//! both. Which events a program gets at all depends on the mode it asked
//! for — clicks only, clicks plus drags, or every motion — and that
//! gating happens here too, so the caller can report every event it sees
//! and let this module decide what the program wanted.

use gpui::{Bounds, Modifiers, MouseButton, Pixels, Point, point, px, size};
use ket_core::terminal::alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use ket_core::terminal::alacritty_terminal::term::TermMode;

/// Where the grid was painted: enough to map a pointer back to a cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Geometry {
    /// Top-left corner of cell (0, 0), inside the pane's padding.
    pub(crate) origin: Point<Pixels>,
    /// Every column's width.
    pub(crate) cell_width: Pixels,
    /// Every row's height.
    pub(crate) line_height: Pixels,
    /// Columns on screen.
    pub(crate) cols: usize,
    /// Rows on screen.
    pub(crate) rows: usize,
}

/// A cell in viewport coordinates: row 0 is the top row *on screen*, whatever
/// the scrollback offset. See [`Cell::to_grid`] for the grid's own view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    /// Row on screen, from the top.
    pub(crate) row: usize,
    /// Column, from the left.
    pub(crate) col: usize,
}

impl Cell {
    /// This cell in the grid's coordinates, where line 0 is the top of the
    /// *live* screen and scrollback lines are negative.
    pub(crate) fn to_grid(self, display_offset: usize) -> GridPoint {
        GridPoint::new(
            Line(self.row as i32 - display_offset as i32),
            Column(self.col),
        )
    }
}

impl Geometry {
    /// The cell under `position`, clamped to the grid, and which half of it
    /// the pointer is in — a selection that starts in the right half of a
    /// cell should not include that cell.
    pub(crate) fn cell_at(&self, position: Point<Pixels>) -> (Cell, Side) {
        let x = f32::from(position.x - self.origin.x);
        let y = f32::from(position.y - self.origin.y);
        let cell_width = f32::from(self.cell_width);
        let line_height = f32::from(self.line_height);

        let col_f = (x / cell_width).max(0.0);
        let col = (col_f.floor() as usize).min(self.cols.saturating_sub(1));
        let row_f = (y / line_height).max(0.0);
        let row = (row_f.floor() as usize).min(self.rows.saturating_sub(1));

        let side = if col_f.fract() >= 0.5 && col_f.floor() as usize == col {
            Side::Right
        } else {
            Side::Left
        };
        (Cell { row, col }, side)
    }

    /// The row under `position` *without* clamping: negative above the grid,
    /// `rows` or more below it. What a drag past the edge uses to decide
    /// which way to scroll.
    pub(crate) fn row_at(&self, position: Point<Pixels>) -> i32 {
        let y = f32::from(position.y - self.origin.y);
        (y / f32::from(self.line_height)).floor() as i32
    }

    /// Pixel bounds of `cells` consecutive cells starting at `cell`.
    pub(crate) fn cell_bounds(&self, cell: Cell, cells: usize) -> Bounds<Pixels> {
        Bounds {
            origin: point(
                self.origin.x + self.cell_width * cell.col as f32,
                self.origin.y + self.line_height * cell.row as f32,
            ),
            size: size(self.cell_width * cells as f32, self.line_height),
        }
    }

    /// Whether the geometry has room for at least one cell.
    pub(crate) fn is_valid(&self) -> bool {
        self.cols > 0 && self.rows > 0 && self.cell_width > px(0.) && self.line_height > px(0.)
    }
}

/// Whether the program wants mouse events at all — and `shift` is the
/// user's escape hatch to select text regardless.
pub(crate) fn wants_mouse(mode: TermMode, modifiers: &Modifiers) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !modifiers.shift
}

/// A button press or release.
pub(crate) fn button_report(
    cell: Cell,
    button: MouseButton,
    modifiers: &Modifiers,
    pressed: bool,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let code = button_code(button)? | modifier_bits(modifiers);
    encode(cell, code, pressed, mode)
}

/// Pointer motion, with `button` held if any. Reported only in the modes
/// that asked for motion: every move under `MOUSE_MOTION`, moves with a
/// button held under `MOUSE_DRAG`, never under click-only reporting.
pub(crate) fn motion_report(
    cell: Cell,
    button: Option<MouseButton>,
    modifiers: &Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let wanted = mode.contains(TermMode::MOUSE_MOTION)
        || (mode.contains(TermMode::MOUSE_DRAG) && button.is_some());
    if !wanted {
        return None;
    }
    // xterm reports motion with no button held as button 3 plus the motion
    // bit — the same code a release would use.
    let base = button.and_then(button_code).unwrap_or(3);
    encode(cell, base | 32 | modifier_bits(modifiers), true, mode)
}

/// Wheel movement as one press event per line: button 64 scrolling up, 65
/// scrolling down.
pub(crate) fn wheel_reports(
    cell: Cell,
    lines: i32,
    modifiers: &Modifiers,
    mode: TermMode,
) -> Vec<Vec<u8>> {
    let code = if lines > 0 { 64 } else { 65 } | modifier_bits(modifiers);
    let Some(report) = encode(cell, code, true, mode) else {
        return Vec::new();
    };
    std::iter::repeat_n(report, lines.unsigned_abs() as usize).collect()
}

/// Wheel movement translated to arrow keys, for a full-screen program that
/// asked for that (`ALTERNATE_SCROLL`) rather than for mouse events —
/// `less` and `man` scroll this way. One arrow per line.
pub(crate) fn alternate_scroll(lines: i32, mode: TermMode) -> Vec<u8> {
    let letter = if lines > 0 { b'A' } else { b'B' };
    let prefix: &[u8] = if mode.contains(TermMode::APP_CURSOR) {
        b"\x1bO"
    } else {
        b"\x1b["
    };
    let mut out = Vec::with_capacity(3 * lines.unsigned_abs() as usize);
    for _ in 0..lines.unsigned_abs() {
        out.extend_from_slice(prefix);
        out.push(letter);
    }
    out
}

/// The button bits of a report: left 0, middle 1, right 2. The navigation
/// buttons have no xterm code.
fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

/// The modifier bits of a report: shift 4, alt 8, control 16.
fn modifier_bits(modifiers: &Modifiers) -> u8 {
    (u8::from(modifiers.shift) << 2)
        | (u8::from(modifiers.alt) << 3)
        | (u8::from(modifiers.control) << 4)
}

/// One report, in whichever encoding the program enabled.
///
/// SGR (`CSI < code ; x ; y M`, `m` for release) carries any coordinate and
/// says which button was released. The legacy form (`CSI M` then three bytes
/// offset by 32) tops out at column 223, or 2015 when the program enabled
/// UTF-8 coordinates, and reports every release as button 3.
fn encode(cell: Cell, code: u8, pressed: bool, mode: TermMode) -> Option<Vec<u8>> {
    let x = cell.col + 1;
    let y = cell.row + 1;

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }

    // Legacy: a release keeps the modifier bits but names no button.
    let code = if pressed { code } else { (code & !0b11) | 3 };
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + code);
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    for coordinate in [x, y] {
        let value = 32 + coordinate;
        if utf8 {
            if value > 2015 {
                return None;
            }
            let ch = char::from_u32(value as u32)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        } else {
            if value > 255 {
                return None;
            }
            out.push(value as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Geometry {
        Geometry {
            origin: point(px(10.), px(20.)),
            cell_width: px(8.),
            line_height: px(16.),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn the_pointer_maps_to_the_cell_under_it_and_the_half_it_is_in() {
        let g = geometry();
        let (cell, side) = g.cell_at(point(
            px(10.) + px(8.) * 3. + px(1.),
            px(20.) + px(16.) * 2.,
        ));
        assert_eq!(cell, Cell { row: 2, col: 3 });
        assert_eq!(side, Side::Left);
        let (cell, side) = g.cell_at(point(px(10.) + px(8.) * 3. + px(6.), px(20.)));
        assert_eq!(cell, Cell { row: 0, col: 3 });
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn positions_outside_the_grid_clamp_to_its_edges() {
        let g = geometry();
        let (cell, _) = g.cell_at(point(px(-100.), px(-100.)));
        assert_eq!(cell, Cell { row: 0, col: 0 });
        let (cell, _) = g.cell_at(point(px(10_000.), px(10_000.)));
        assert_eq!(cell, Cell { row: 23, col: 79 });
        assert!(g.row_at(point(px(0.), px(0.))) < 0);
        assert!(g.row_at(point(px(0.), px(10_000.))) >= 24);
    }

    #[test]
    fn a_viewport_row_maps_into_scrollback_when_scrolled_up() {
        let cell = Cell { row: 2, col: 5 };
        assert_eq!(cell.to_grid(0), GridPoint::new(Line(2), Column(5)));
        assert_eq!(cell.to_grid(10), GridPoint::new(Line(-8), Column(5)));
    }

    #[test]
    fn sgr_reports_name_the_button_on_release() {
        let cell = Cell { row: 4, col: 9 };
        let mode = TermMode::SGR_MOUSE | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            button_report(cell, MouseButton::Left, &Modifiers::default(), true, mode),
            Some(b"\x1b[<0;10;5M".to_vec())
        );
        assert_eq!(
            button_report(cell, MouseButton::Right, &Modifiers::default(), false, mode),
            Some(b"\x1b[<2;10;5m".to_vec())
        );
    }

    #[test]
    fn legacy_reports_offset_everything_by_32_and_release_is_button_3() {
        let cell = Cell { row: 0, col: 0 };
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            button_report(cell, MouseButton::Left, &Modifiers::default(), true, mode),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            button_report(cell, MouseButton::Left, &Modifiers::default(), false, mode),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33])
        );
    }

    #[test]
    fn legacy_reports_refuse_coordinates_they_cannot_encode() {
        let cell = Cell { row: 0, col: 300 };
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            button_report(cell, MouseButton::Left, &Modifiers::default(), true, mode),
            None
        );
        let utf8 = mode | TermMode::UTF8_MOUSE;
        assert!(
            button_report(cell, MouseButton::Left, &Modifiers::default(), true, utf8).is_some()
        );
    }

    #[test]
    fn motion_is_reported_only_in_the_modes_that_asked_for_it() {
        let cell = Cell { row: 1, col: 1 };
        let none = Modifiers::default();
        assert_eq!(
            motion_report(cell, None, &none, TermMode::MOUSE_REPORT_CLICK),
            None
        );
        assert_eq!(motion_report(cell, None, &none, TermMode::MOUSE_DRAG), None);
        assert!(
            motion_report(cell, Some(MouseButton::Left), &none, TermMode::MOUSE_DRAG).is_some()
        );
        assert_eq!(
            motion_report(
                cell,
                None,
                &none,
                TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE
            ),
            Some(b"\x1b[<35;2;2M".to_vec())
        );
    }

    #[test]
    fn modifiers_set_their_bits() {
        let cell = Cell { row: 0, col: 0 };
        let modifiers = Modifiers {
            shift: true,
            control: true,
            ..Default::default()
        };
        assert_eq!(
            button_report(
                cell,
                MouseButton::Left,
                &modifiers,
                true,
                TermMode::SGR_MOUSE
            ),
            Some(b"\x1b[<20;1;1M".to_vec())
        );
    }

    #[test]
    fn wheel_reports_one_press_per_line() {
        let cell = Cell { row: 0, col: 0 };
        let reports = wheel_reports(cell, -2, &Modifiers::default(), TermMode::SGR_MOUSE);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0], b"\x1b[<65;1;1M".to_vec());
        assert_eq!(
            wheel_reports(cell, 3, &Modifiers::default(), TermMode::SGR_MOUSE)[0],
            b"\x1b[<64;1;1M".to_vec()
        );
    }

    #[test]
    fn alternate_scroll_sends_arrows_in_the_cursor_mode_the_program_set() {
        assert_eq!(
            alternate_scroll(2, TermMode::NONE),
            b"\x1b[A\x1b[A".to_vec()
        );
        assert_eq!(
            alternate_scroll(-1, TermMode::APP_CURSOR),
            b"\x1bOB".to_vec()
        );
    }
}
