//! Where the text was painted: the pane's measured size and one
//! character's advance, which is what soft wrap and every click are
//! worked out from.

use std::sync::Arc;

use gpui::{
    Font, FontFeatures, FontId, FontStyle, FontWeight, Pixels, SharedString, Window,
    WindowTextSystem, px,
};

/// The wrap width used for the one frame before [`canvas`] has reported the
/// pane's real bounds. Corrected on the frame after.
///
/// [`canvas`]: gpui::canvas
const FALLBACK_COLUMNS: usize = 100;

/// How wide the text column is, in the only two units soft wrap needs.
///
/// Deliberately not what a click is measured against: a click is answered by
/// the row's own shaping, which is exact. This is a wrap width, where being
/// a character out is invisible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Geometry {
    /// The code face's advance for one character.
    pub(super) cell_width: Pixels,
    /// How many characters fit across the text column, and so the width
    /// soft wrap breaks at.
    pub(super) columns: usize,
    /// The text column's width itself, for a buffer that wraps by advances
    /// rather than columns. Zero until the pane has been measured once.
    pub(super) text_width: Pixels,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            cell_width: px(8.0),
            columns: FALLBACK_COLUMNS,
            text_width: px(0.0),
        }
    }
}

/// A proportional face's advances, for wrapping text that is not on a grid.
#[derive(Clone)]
pub(super) struct Measure {
    pub(super) text_system: Arc<WindowTextSystem>,
    pub(super) font: FontId,
    pub(super) size: Pixels,
}

impl Measure {
    /// How far `c` moves the pen. A character the face has no glyph for is
    /// drawn from a fallback face nobody measured, so it is counted as a
    /// whole em: wrapping a few pixels early beats clipping it.
    fn advance(&self, c: char) -> f32 {
        self.text_system
            .advance(self.font, self.size, c)
            .map_or(f32::from(self.size), |advance| f32::from(advance.width))
    }
}

/// What soft wrap breaks at: a column count on the code face's grid, or a
/// width in pixels summed from real advances for a proportional face.
#[derive(Clone, Copy)]
pub(super) enum WrapWidth<'a> {
    Columns(usize),
    Pixels(f32, &'a Measure),
}

impl WrapWidth<'_> {
    /// What [`LineIndex`] compares to decide whether it is out of date.
    ///
    /// [`LineIndex`]: super::line_index::LineIndex
    pub(super) fn key(&self) -> (usize, u32) {
        match self {
            WrapWidth::Columns(columns) => (*columns, 0),
            WrapWidth::Pixels(width, _) => (0, width.round() as u32),
        }
    }

    /// The first character from `start` that does not fit on the row, or
    /// past the end of `text` when the rest of it does. Always past `start`,
    /// so a character wider than the whole row still gets a row of its own.
    pub(super) fn limit(&self, text: &[char], start: usize) -> usize {
        match self {
            WrapWidth::Columns(columns) => start + columns,
            WrapWidth::Pixels(width, measure) => {
                let mut used = 0.0;
                for (index, &c) in text.iter().enumerate().skip(start) {
                    used += measure.advance(c);
                    if used > *width {
                        return index.max(start + 1);
                    }
                }
                text.len()
            }
        }
    }
}

/// The advance of one character in the code face — the cell width of a grid
/// the editor never draws but does measure against. The same question
/// `terminal/element.rs` asks of its own font, and asked the same way.
pub(crate) fn code_cell_width(window: &Window, family: &SharedString, size: Pixels) -> Pixels {
    let font = Font {
        family: family.clone(),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    };
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&font);
    text_system
        .advance(font_id, size, 'm')
        .map(|advance| advance.width)
        .unwrap_or(px(8.0))
}
