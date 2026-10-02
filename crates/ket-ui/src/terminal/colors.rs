//! Colour resolution for the terminal grid.
//!
//! A cell names its colours the way a program named them — a theme slot, an
//! xterm-256 index, or a 24-bit value — and this module turns that into the
//! colour to paint, honouring anything the program redefined with an OSC
//! colour-set sequence along the way.
//!
//! Colour comes from `ket_core::theme::TerminalTokens`: the sixteen ANSI
//! slots plus foreground/background/cursor. A program that never asked for a
//! specific colour on a cell gets the theme's default; a program that named
//! an indexed or 24-bit colour gets exactly that. Indices 16..256 (the xterm
//! colour cube and greyscale ramp) are not part of any theme —
//! `alacritty_terminal` does not compute them either, leaving that to
//! whatever draws the grid — so they are computed here with the standard
//! formula and used only when a program has not overridden that index.

use ket_core::terminal::alacritty_terminal::term::cell::{Cell, Flags};
use ket_core::terminal::alacritty_terminal::term::color::Colors as AlacrittyColors;
use ket_core::terminal::alacritty_terminal::vte::ansi::{
    Color as AlacrittyColor, NamedColor, Rgb as AlacrittyRgb,
};
use ket_core::theme::{Color as ThemeColor, Theme};

/// One cell's effective colours, every attribute folded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellColors {
    /// The glyph.
    pub(crate) fg: ThemeColor,
    /// The cell behind it.
    pub(crate) bg: ThemeColor,
    /// The underline, when the cell has one.
    pub(crate) underline: ThemeColor,
}

/// Resolves one cell's effective foreground, background and underline
/// colour: reverse video, dim and hidden folded in.
pub(crate) fn cell_colors(theme: &Theme, palette: &AlacrittyColors, cell: &Cell) -> CellColors {
    let mut fg = resolve_color(theme, palette, cell.fg);
    let mut bg = resolve_color(theme, palette, cell.bg);
    let underline = cell
        .underline_color()
        .map(|color| resolve_color(theme, palette, color))
        .unwrap_or(fg);

    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(Flags::DIM) {
        fg = blend(fg, bg, 0.4);
    }
    if cell.flags.contains(Flags::HIDDEN) {
        fg = bg;
    }

    CellColors { fg, bg, underline }
}

/// One ANSI colour reference, resolved against the theme and whatever a
/// program has explicitly set with an OSC colour-set sequence.
pub(crate) fn resolve_color(
    theme: &Theme,
    palette: &AlacrittyColors,
    color: AlacrittyColor,
) -> ThemeColor {
    match color {
        AlacrittyColor::Named(named) => resolve_named(theme, palette, named),
        AlacrittyColor::Indexed(index) => palette[index as usize]
            .map(rgb_to_theme)
            .unwrap_or_else(|| indexed_default(theme, index)),
        AlacrittyColor::Spec(rgb) => rgb_to_theme(rgb),
    }
}

/// A named colour slot, honouring a program's own override before falling
/// back to the theme.
fn resolve_named(theme: &Theme, palette: &AlacrittyColors, named: NamedColor) -> ThemeColor {
    if let Some(rgb) = palette[named] {
        return rgb_to_theme(rgb);
    }

    let ansi = &theme.terminal.ansi;
    match named {
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
            theme.terminal.foreground
        }
        NamedColor::Background => theme.terminal.background,
        NamedColor::Cursor => theme.terminal.cursor,
        NamedColor::Black | NamedColor::DimBlack => ansi.black,
        NamedColor::Red | NamedColor::DimRed => ansi.red,
        NamedColor::Green | NamedColor::DimGreen => ansi.green,
        NamedColor::Yellow | NamedColor::DimYellow => ansi.yellow,
        NamedColor::Blue | NamedColor::DimBlue => ansi.blue,
        NamedColor::Magenta | NamedColor::DimMagenta => ansi.magenta,
        NamedColor::Cyan | NamedColor::DimCyan => ansi.cyan,
        NamedColor::White | NamedColor::DimWhite => ansi.white,
        NamedColor::BrightBlack => ansi.bright_black,
        NamedColor::BrightRed => ansi.bright_red,
        NamedColor::BrightGreen => ansi.bright_green,
        NamedColor::BrightYellow => ansi.bright_yellow,
        NamedColor::BrightBlue => ansi.bright_blue,
        NamedColor::BrightMagenta => ansi.bright_magenta,
        NamedColor::BrightCyan => ansi.bright_cyan,
        NamedColor::BrightWhite => ansi.bright_white,
    }
}

/// The xterm-256 colour cube and greyscale ramp, for indices `alacritty_terminal`
/// leaves unset until a program redefines one explicitly.
///
/// `alacritty_terminal` stores only overrides (see [`AlacrittyColors`]); the
/// standard 256-colour palette itself is conventionally the terminal
/// front-end's job, same as the sixteen base colours are the theme's.
fn indexed_default(theme: &Theme, index: u8) -> ThemeColor {
    if index < 16 {
        return theme.terminal.ansi.get(index);
    }
    if index < 232 {
        let index = index - 16;
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let r = LEVELS[(index / 36) as usize];
        let g = LEVELS[((index / 6) % 6) as usize];
        let b = LEVELS[(index % 6) as usize];
        return ThemeColor::new(r, g, b);
    }
    let level = 8 + (index - 232) * 10;
    ThemeColor::new(level, level, level)
}

/// An explicit RGB colour, as a program's own truecolor or OSC-set request.
fn rgb_to_theme(rgb: AlacrittyRgb) -> ThemeColor {
    ThemeColor::new(rgb.r, rgb.g, rgb.b)
}

/// Blends `a` toward `b` by `t` (0 keeps `a`, 1 reaches `b`), for the `DIM`
/// attribute's faint text — a real emulator dims by drawing lighter, which
/// this shell has no font-rendering hook for, so it dims by mixing colour
/// instead.
fn blend(a: ThemeColor, b: ThemeColor, t: f32) -> ThemeColor {
    let mix =
        |from: u8, to: u8| (f32::from(from) + (f32::from(to) - f32::from(from)) * t).round() as u8;
    ThemeColor::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_sixteen_indices_are_the_themes_own_ansi_colours() {
        let theme = Theme::dark();
        assert_eq!(indexed_default(&theme, 1), theme.terminal.ansi.red);
    }

    #[test]
    fn the_colour_cube_starts_black_and_ends_white() {
        let theme = Theme::dark();
        assert_eq!(indexed_default(&theme, 16), ThemeColor::new(0, 0, 0));
        assert_eq!(indexed_default(&theme, 231), ThemeColor::new(255, 255, 255));
    }

    #[test]
    fn the_greyscale_ramp_starts_just_above_black_and_ends_just_below_white() {
        let theme = Theme::dark();
        assert_eq!(indexed_default(&theme, 232), ThemeColor::new(8, 8, 8));
        assert_eq!(indexed_default(&theme, 255), ThemeColor::new(238, 238, 238));
    }

    #[test]
    fn blending_a_colour_with_itself_changes_nothing() {
        let colour = ThemeColor::new(120, 40, 200);
        assert_eq!(blend(colour, colour, 0.7), colour);
    }

    #[test]
    fn blending_halfway_between_black_and_white_is_grey() {
        let black = ThemeColor::new(0, 0, 0);
        let white = ThemeColor::new(255, 255, 255);
        assert_eq!(blend(black, white, 0.5), ThemeColor::new(128, 128, 128));
    }

    #[test]
    fn inverse_swaps_foreground_and_background() {
        let theme = Theme::dark();
        let palette = AlacrittyColors::default();
        let mut cell = Cell::default();
        cell.flags.insert(Flags::INVERSE);
        let colors = cell_colors(&theme, &palette, &cell);
        assert_eq!(colors.fg, theme.terminal.background);
        assert_eq!(colors.bg, theme.terminal.foreground);
    }
}
