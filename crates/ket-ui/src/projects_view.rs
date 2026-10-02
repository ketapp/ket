//! Compact, status-first project list used to find active worktrees.

use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*, px,
};
use ket_core::activity::Signal;
use ket_core::theme::Color;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::tree::{Selection, row_mark, state_word};
use crate::ui::ICON_GAP;
use crate::ui::agent::MARK;
use crate::ui::row::block_row;

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
    /// What the agent is actually doing, in its own words — empty when
    /// nothing has ever run here. See [`ket_core::activity::Activity::detail`].
    detail: String,
    /// Pinned rows lead the list, ahead of the status order.
    pinned: bool,
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
            .flat_map(|(project, node)| {
                node.worktrees
                    .iter()
                    .enumerate()
                    .map(move |(worktree, worktree_node)| {
                        let activity = self.activity.activity(&worktree_node.id, now);
                        Entry {
                            project,
                            worktree,
                            project_name: node.name.clone(),
                            project_color: node.color,
                            branch: worktree_node.label(),
                            signal: self.row_signal(worktree_node, now),
                            detail: activity.detail(),
                            pinned: worktree_node.pinned,
                        }
                    })
            })
            // The sidebar's filter holds here too: sorted or grouped, it is
            // the same question about the same worktrees.
            .filter(|entry| self.sidebar_filter.admits(entry.signal))
            .collect();
        entries.sort_by_key(|entry| {
            (
                std::cmp::Reverse(entry.pinned),
                status_rank(entry.signal),
                entry.project,
                entry.worktree,
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
                let meta = self.worktree_meta(node, project, worktree, None, window, cx);
                let state = state_word(node, entry.signal, &self.theme);
                let t = &self.theme;
                block_row(("activity-worktree", row_id), selected, t)
                    .group(crate::tree::worktree_row_group(project, worktree))
                    .flex_row()
                    .items_center()
                    .gap(ICON_GAP)
                    .text_size(px(13.5))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(MARK)
                            .child(row_mark(&activity, node.dirty, node.missing, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(entry.branch))
                            // Which project, in its own colour: a list sorted
                            // across repositories is scanned by whose a row is.
                            .child(
                                div()
                                    .prose()
                                    .mt(px(2.0))
                                    .text_size(px(12.0))
                                    .truncate()
                                    .text_color(paint(t.text.dim))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .size(px(6.0))
                                                    .rounded(px(2.0))
                                                    .bg(paint(entry.project_color)),
                                            )
                                            .child(div().truncate().child(entry.project_name))
                                            .when(!entry.detail.is_empty(), |el| {
                                                el.child(div().flex_none().child("·")).child(
                                                    div().truncate().child(entry.detail.clone()),
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .children(state)
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

fn status_rank(signal: Signal) -> u8 {
    match signal {
        Signal::Blocked => 0,
        Signal::Working => 1,
        Signal::Merging => 2,
        Signal::Running => 3,
        Signal::Failed => 4,
        Signal::Quiet => 5,
    }
}
