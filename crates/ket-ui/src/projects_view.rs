//! Compact, status-first project list used to find active worktrees.

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, Pixels, SharedString, Window,
    div, prelude::*, px,
};
use ket_core::activity::Signal;
use ket_core::theme::Color;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{mix, paint};
use crate::projects::rail_colour;
use crate::tree::Selection;
use crate::tree::state::{self, lead_word, row_state};
use crate::ui::chip::BADGE;
use crate::ui::row::block_row_with_rail;
use crate::ui::{CAPTION, ICON_GAP, LABEL};

/// A row's padding above and below, roomier than the grouped tree's: with
/// no heading between them, rows from different projects run together, and
/// the space is what keeps each one a thing to read on its own.
const ROW_PAD_Y: Pixels = px(9.0);

struct Entry {
    project: usize,
    worktree: usize,
    project_name: SharedString,
    /// The project's own badge colour, so a list sorted across several
    /// repositories can still be scanned by which one a row belongs to —
    /// the one thing the branch and project name in plain text cannot say
    /// at a glance.
    project_color: Color,
    branch: SharedString,
    signal: Signal,
    /// Pinned rows lead the list, ahead of the status order.
    pinned: bool,
    updated_at_ms: Option<u64>,
}

impl Shell {
    pub(crate) fn activity_project_rows(
        &self,
        now: u64,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut entries: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            // A hidden project is left out here as in the grouped list.
            .filter(|(_, node)| self.project_shown(node, now))
            .flat_map(|(project, node)| {
                node.worktrees
                    .iter()
                    .enumerate()
                    .map(move |(worktree, worktree_node)| Entry {
                        project,
                        worktree,
                        project_name: node.name.clone(),
                        project_color: node.color,
                        branch: worktree_node.label(),
                        signal: self.row_signal(worktree_node, now),
                        pinned: worktree_node.pinned,
                        updated_at_ms: self.activity.last_updated_ms(&worktree_node.id),
                    })
            })
            // The sidebar's filter holds here too: sorted or grouped, it is
            // the same question about the same worktrees.
            .filter(|entry| self.sidebar_filter.admits(entry.signal))
            .collect();
        entries.sort_by_key(|entry| {
            status_sort_key(
                entry.pinned,
                entry.signal,
                entry.updated_at_ms,
                Selection {
                    project: entry.project,
                    worktree: entry.worktree,
                },
            )
        });

        entries
            .into_iter()
            .map(|entry| {
                let selection = Selection {
                    project: entry.project,
                    worktree: entry.worktree,
                };
                let project = entry.project;
                let worktree = entry.worktree;
                let selected = self.selection == Some(selection);
                let row_id = entry.project * 1000 + entry.worktree;
                let node = &self.projects[project].worktrees[worktree];
                let activity = self.activity.activity(&node.id, now);
                // Its session's `/color`, as the grouped tree draws it.
                let color = self.row_color(&node.id, &activity, now);
                let meta = self.worktree_meta(node, project, worktree, window, cx);
                // Its size, after the name, as the grouped tree has it.
                let size = self.size_tag(node, row_id, cx);
                let checkout =
                    state::checkout_lead(node.missing, self.deleting(&node.id), &self.theme);
                let unread = self.activity.unread(&node.id);
                let state = row_state(node, entry.signal, unread, &self.theme);
                let t = &self.theme;
                let rail = rail_colour(entry.project_color, t);
                let group = crate::tree::worktree_row_group(project, worktree);
                // The ring round the mark's dot is the row's own fill.
                let ground = paint(if selected { t.hover } else { t.panel });
                let hover_ground =
                    (!selected).then(|| (group.clone(), mix(paint(t.panel), paint(t.hover), 0.55)));
                block_row_with_rail(("activity-worktree", row_id), selected, rail, t)
                    .py(ROW_PAD_Y)
                    .group(group)
                    .flex_row()
                    .items_center()
                    .gap(ICON_GAP)
                    .text_size(crate::fonts::row_text(LABEL))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            // As wide as a project's badge, as the grouped
                            // tree's is: room for the mark's well.
                            .w(BADGE)
                            .child(state::with_dot(
                                crate::session_color::row_mark(
                                    &activity,
                                    color,
                                    state::lit(state.as_ref()),
                                    node.dirty,
                                    node.missing,
                                    t,
                                ),
                                state.as_ref(),
                                color.is_some(),
                                ground,
                                hover_ground,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            // Unread in bold, as the grouped tree draws it.
                                            .when(unread, |el| el.font_weight(FontWeight::BOLD))
                                            .when_some(color, |el, color| {
                                                el.text_color(paint(crate::session_color::label(
                                                    color, t,
                                                )))
                                            })
                                            .child(entry.branch),
                                    )
                                    .children(size),
                            )
                            // Which project, in its own colour: a list sorted
                            // across repositories is scanned by whose a row is.
                            // Led by the state's word where it has one, as the
                            // grouped tree's second line is; nothing else: this
                            // list is for finding a worktree.
                            .child(
                                div()
                                    .prose()
                                    .mt(px(1.0))
                                    .text_size(crate::fonts::row_text(CAPTION))
                                    .truncate()
                                    .text_color(paint(t.text.dim))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .children(
                                                checkout.or_else(|| lead_word(state.as_ref())),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .size(px(6.0))
                                                    .rounded(px(2.0))
                                                    .bg(paint(entry.project_color)),
                                            )
                                            .child(div().truncate().child(entry.project_name)),
                                    ),
                            ),
                    )
                    .child(meta)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.open_worktree_menu(project, worktree, event.position);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let reselected = this.selection == Some(selection);
                        this.select(selection, cx);
                        if reselected {
                            this.focus_main_tab();
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }
}

/// Pinned rows first, then status, then the most recently updated session.
/// Original project/worktree order breaks ties and places rows with no activity.
pub(crate) fn status_sort_key(
    pinned: bool,
    signal: Signal,
    updated_at_ms: Option<u64>,
    at: Selection,
) -> (
    std::cmp::Reverse<bool>,
    u8,
    std::cmp::Reverse<Option<u64>>,
    usize,
    usize,
) {
    (
        std::cmp::Reverse(pinned),
        status_rank(signal),
        std::cmp::Reverse(updated_at_ms),
        at.project,
        at.worktree,
    )
}

/// Where a state sorts in the status view: waiting on a person first.
pub(crate) fn status_rank(signal: Signal) -> u8 {
    match signal {
        Signal::Blocked => 0,
        Signal::Working => 1,
        Signal::Merging => 2,
        Signal::Running => 3,
        Signal::Failed => 4,
        Signal::Quiet => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_sort_keeps_priority_then_orders_by_recency_across_projects() {
        let mut rows = [
            (
                false,
                Signal::Quiet,
                Some(900),
                Selection {
                    project: 0,
                    worktree: 0,
                },
            ),
            (
                false,
                Signal::Working,
                Some(100),
                Selection {
                    project: 0,
                    worktree: 1,
                },
            ),
            (
                false,
                Signal::Working,
                Some(200),
                Selection {
                    project: 1,
                    worktree: 0,
                },
            ),
            (
                false,
                Signal::Blocked,
                Some(50),
                Selection {
                    project: 1,
                    worktree: 1,
                },
            ),
            (
                true,
                Signal::Quiet,
                None,
                Selection {
                    project: 2,
                    worktree: 0,
                },
            ),
            (
                false,
                Signal::Working,
                None,
                Selection {
                    project: 0,
                    worktree: 2,
                },
            ),
            (
                false,
                Signal::Working,
                Some(200),
                Selection {
                    project: 1,
                    worktree: 2,
                },
            ),
        ];
        rows.sort_by_key(|&(pinned, signal, updated, at)| {
            status_sort_key(pinned, signal, updated, at)
        });
        assert_eq!(
            rows.map(|(_, _, _, at)| (at.project, at.worktree)),
            [(2, 0), (1, 1), (1, 0), (1, 2), (0, 1), (0, 2), (0, 0)]
        );
    }
}
