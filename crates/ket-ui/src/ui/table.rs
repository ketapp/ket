//! A table: a row of dim column titles over rows of figures, each on a
//! hairline — an automation's runs.
//!
//! The columns are stated once and shared by the header and every row, so a
//! cell can never drift out from under its title. A column is a fixed width
//! or takes the room that is left, and its cells sit at its start or, for
//! figures that are compared down the column, its end.

use gpui::{
    AnyElement, Div, ElementId, FontWeight, IntoElement, ParentElement, Pixels, SharedString,
    Stateful, Styled, div, prelude::*, px,
};
use ket_core::theme::Theme;

use super::{CAPTION, LABEL};
use crate::paint::{alpha, paint};

/// The header row's height.
const HEAD_H: Pixels = px(30.0);

/// A row's height: room for a second, dim line under a cell's first.
const ROW_H: Pixels = px(42.0);

/// Space at each end of a cell.
const CELL_PAD: Pixels = px(8.0);

/// One column.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Column {
    /// What the header calls it; empty for a column of buttons.
    pub(crate) title: &'static str,
    /// Its width; `None` takes the room the fixed ones leave.
    pub(crate) width: Option<Pixels>,
    /// Whether its cells sit at its end: figures read down a column.
    pub(crate) end: bool,
}

impl Column {
    /// A column `width` wide.
    pub(crate) const fn fixed(title: &'static str, width: Pixels) -> Self {
        Self {
            title,
            width: Some(width),
            end: false,
        }
    }

    /// A column taking the room that is left.
    pub(crate) const fn fill(title: &'static str) -> Self {
        Self {
            title,
            width: None,
            end: false,
        }
    }

    /// The same column with its cells at its end.
    pub(crate) const fn at_end(mut self) -> Self {
        self.end = true;
        self
    }
}

/// One cell's frame in `column`, holding `content`.
fn cell(column: &Column, content: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .min_w_0()
        .px(CELL_PAD)
        .overflow_hidden()
        .when_some(column.width, |el, width| el.w(width).flex_none())
        .when(column.width.is_none(), |el| el.flex_1())
        .when(column.end, |el| el.justify_end())
        .child(content)
}

/// The row of column titles.
pub(crate) fn header(columns: &[Column], t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(HEAD_H)
        .border_b_1()
        .border_color(paint(t.border))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(paint(t.text.dim))
        .children(
            columns
                .iter()
                .map(|column| cell(column, SharedString::from(column.title))),
        )
}

/// One row: `cells` in order, one per column. Attach a click to what comes
/// back for a row that opens something; it lightens under the pointer.
pub(crate) fn row(
    id: impl Into<ElementId>,
    columns: &[Column],
    cells: Vec<AnyElement>,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(ROW_H)
        .border_b_1()
        .border_color(paint(t.rule))
        .text_size(LABEL)
        .text_color(paint(t.text.primary))
        .hover(|style| style.bg(alpha(paint(t.hover), 0.55)))
        .children(
            columns
                .iter()
                .zip(cells)
                .map(|(column, content)| cell(column, content)),
        )
}

/// A cell's two lines: what it says, and a dim note under it — "Due 2:00 AM
/// · Mac was asleep".
pub(crate) fn two_lines(first: impl IntoElement, note: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .min_w_0()
        .child(first)
        .child(
            div()
                .text_size(CAPTION)
                .text_color(paint(t.text.dim))
                .child(note.into()),
        )
}
