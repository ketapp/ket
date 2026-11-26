//! A row of the tree that hangs under a worktree — a subagent's, a background
//! job's: the elbow off the guide into it, and the guide on to the next.

use gpui::{Div, ElementId, Pixels, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::{ELBOW_RADIUS, ELBOW_RUN, SUBAGENT_ROW_H, guide, mark_axis};

/// A row of the tree that is not a control of its own: a click on it lands
/// where a click on the worktree above would, so it takes no hover and no
/// pointer — a row that lit up would promise something and then not do it.
/// See [`branch_of`].
pub(super) fn branch(
    id: ElementId,
    i: usize,
    last: usize,
    end: Pixels,
    t: &Theme,
) -> Stateful<Div> {
    branch_of(
        div().id(id).flex().items_center().h(SUBAGENT_ROW_H),
        i,
        last,
        end,
        t,
    )
}

/// `row` as a row of the tree: the elbow off the guide into it, the guide on
/// to the next row unless it is the `last`, and the padding that puts its
/// node at the elbow's end and its figures under the worktree's, which end
/// `end` from the right. A row that is a control of its own brings the
/// shared recipe's surface — `ui::row::sized_row` at [`SUBAGENT_ROW_H`].
pub(super) fn branch_of(
    row: Stateful<Div>,
    i: usize,
    last: usize,
    end: Pixels,
    t: &Theme,
) -> Stateful<Div> {
    let axis = mark_axis();
    let line = guide(t);
    row.relative()
        .pl(axis + ELBOW_RUN)
        .pr(end)
        .text_size(px(11.0))
        // ╰ from the guide into this row, turning into its node.
        .child(
            div()
                .absolute()
                .left(axis - px(0.5))
                .top_0()
                .h(SUBAGENT_ROW_H / 2.0 + px(0.5))
                .w(ELBOW_RUN + px(0.5))
                .border_l_1()
                .border_b_1()
                .border_color(line)
                .rounded_bl(ELBOW_RADIUS),
        )
        // │ on to the next sibling, starting where the elbow begins to curve
        // away from the vertical. Starting at the node's centre leaves a
        // break above every junction. Not under the last, which is where the
        // tree ends.
        .when(i < last, |el| {
            el.child(
                div()
                    .absolute()
                    .left(axis - px(0.5))
                    .top(SUBAGENT_ROW_H / 2.0 - ELBOW_RADIUS)
                    .bottom_0()
                    .w(px(1.0))
                    .bg(line),
            )
        })
}
