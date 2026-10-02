//! An icon cluster: a handful of buttons sharing one pill, split by hairlines.
//!
//! The toolbar counterpart to [`super::group::ButtonGroup`]. A group is one
//! setting's answers, labelled, in a sunken track; a cluster is a few icon
//! buttons that belong together — a view toggle's two modes, or the actions
//! at the end of a strip — and says so by sharing a frame, the way a row of
//! switches shares a panel.
//!
//! ## Why the cluster draws its own cells
//!
//! Every cell is the same width and every glyph the same size, and the two
//! end cells follow the pill's curve. None of that can be left to the caller:
//! a cell sized at its call site is how one came to be wider than its
//! neighbours, and gpui clips children to a rectangle, not to a rounded
//! corner, so an end cell with a square ground paints its hover and pressed
//! fill straight over the frame's curve. So the caller hands over an
//! [`IconButton`] and the cluster builds the cell from it, knowing where in the
//! run it sits; the caller gets the built cell back to attach its click
//! handler and tooltip to.

use gpui::{AnyElement, Div, ElementId, Pixels, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::button::IconButton;
use super::{CONTROL_H, RADIUS_MD, WELL_XS};
use crate::paint::paint;

/// How wide every cell is, ends included: wider than the control is tall, so
/// a glyph has room either side of it between the hairlines.
const CELL_W: Pixels = px(36.0);

/// A run of icon buttons in one pill-shaped frame, split by hairlines.
pub(crate) struct IconCluster<'a> {
    id: ElementId,
    cells: Vec<Cell<'a>>,
    small: bool,
}

/// A small cluster's cell width, beside [`CELL_W`].
const SMALL_CELL_W: Pixels = px(28.0);

/// One button, and what the caller does with it once it is built.
struct Cell<'a> {
    button: IconButton,
    finish: Box<dyn FnOnce(Stateful<Div>) -> AnyElement + 'a>,
}

/// An empty cluster. Add cells to it.
pub(crate) fn icon_cluster<'a>(id: impl Into<ElementId>) -> IconCluster<'a> {
    IconCluster {
        id: id.into(),
        cells: Vec::new(),
        small: false,
    }
}

impl<'a> IconCluster<'a> {
    /// Adds one cell.
    ///
    /// `button` says what the cell shows — its icon, whether it is pressed,
    /// its latch light. Its size and corners are the cluster's. `finish` is
    /// handed the built cell to attach a click handler to, and to wrap in a
    /// tooltip if it has one.
    pub(crate) fn cell(
        mut self,
        button: IconButton,
        finish: impl FnOnce(Stateful<Div>) -> AnyElement + 'a,
    ) -> Self {
        self.cells.push(Cell {
            button,
            finish: Box::new(finish),
        });
        self
    }

    /// Draws the cluster.
    /// The small size, for a cluster in a compact bar: dense cells in a
    /// frame that fits a 32px strip with air around it.
    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub(crate) fn render(self, t: &Theme) -> AnyElement {
        let small = self.small;
        let rule = paint(t.border);
        let last = self.cells.len().saturating_sub(1);
        let mut frame = div()
            .id(self.id)
            .flex()
            .flex_none()
            .h(if small { WELL_XS + px(2.0) } else { CONTROL_H })
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(rule);
        for (index, cell) in self.cells.into_iter().enumerate() {
            if index > 0 {
                frame = frame.child(div().flex_none().w(px(1.0)).bg(rule));
            }
            // Square between the hairlines, so a ground runs edge to edge
            // with no slivers beside it; round on the outer side of an end,
            // where the pill's curve is.
            let button = cell.button.bare().square();
            let button = if small {
                button.dense().square()
            } else {
                button
            };
            let built = button
                .render(t)
                .w(if small { SMALL_CELL_W } else { CELL_W });
            let built = built
                // Inside the frame's border, so a pixel under its corner.
                .when(index == 0, |el| {
                    el.rounded_tl(RADIUS_MD - px(1.0))
                        .rounded_bl(RADIUS_MD - px(1.0))
                })
                .when(index == last, |el| {
                    el.rounded_tr(RADIUS_MD - px(1.0))
                        .rounded_br(RADIUS_MD - px(1.0))
                });
            frame = frame.child((cell.finish)(built));
        }
        frame.into_any_element()
    }
}
