//! What a worktree row carries besides its mark and its two lines.
//!
//! At its end, in this order: the Economy ring when a level was chosen, the
//! pull request when there is one, and last the work mark, so the one mark
//! read down the column never moves from the edge. After its name, as small
//! tags: the fold for the rows hanging under it, and its size on disk once
//! that is worth knowing.

use gpui::{AnyElement, Context, Pixels, Rgba, Window, div, prelude::*, px};
use ket_core::theme::Theme;

use super::WorktreeNode;
use crate::Shell;
use crate::paint::{alpha, paint};
use crate::ui::WELL_XS;
use crate::ui::button::icon_button;
use crate::ui::chip::{tag, tinted_tag};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::tooltip::{Side, tooltip};

/// From here the size on disk is a warning: it shows without the pointer on
/// the row, in the theme's warm ink.
const SIZE_WARM: u64 = 2 << 30;

/// From here it shows in the theme's critical ink.
const SIZE_CRITICAL: u64 = 4 << 30;

/// The tags after a name are set a step under it, so a line carrying one is
/// no taller than a line without.
const TAG_TEXT: Pixels = px(9.5);

/// What a worktree holds that its base does not have yet.
#[derive(Clone, Copy)]
enum Work {
    /// Changes nobody has committed.
    Uncommitted,
    /// Commits, all of them committed, not yet merged.
    Commits,
}

/// The work mark's state for `node`. Not on the repository's own checkout,
/// which is the base rather than something waiting to reach it, nor on a
/// checkout that is gone.
fn work_of(node: &WorktreeNode) -> Option<Work> {
    if node.primary || node.missing {
        None
    } else if node.dirty {
        Some(Work::Uncommitted)
    } else if node.unlanded {
        Some(Work::Commits)
    } else {
        None
    }
}

/// The ink a size is told in: `None` under [`SIZE_WARM`], where it is a
/// fact for the pointer rather than a warning.
fn size_tone(bytes: u64, t: &Theme) -> Option<Rgba> {
    if bytes > SIZE_CRITICAL {
        Some(paint(t.quota.critical))
    } else if bytes > SIZE_WARM {
        Some(paint(t.quota.warm))
    } else {
        None
    }
}

impl Shell {
    /// The marks at a worktree row's end, shared by both worktree list
    /// views: they describe the worktree itself, so they are the same
    /// wherever it is listed.
    pub(crate) fn worktree_meta(
        &self,
        node: &WorktreeNode,
        project: usize,
        worktree: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let row = project * 1000 + worktree;
        // The pull request the branch is open as, when there is one: a mark
        // that opens its page, where the rest of its state lives. Its checks
        // tint it — failed when one failed, running ink when they all
        // passed — and while they run it keeps its own ink, as a row at rest
        // does. Read from GitHub when it has been; else the one ket opened.
        let live = self.pull_requests.of(&node.id);
        let link = live
            .map(ket_core::github::PullRequestStatus::link)
            .or_else(|| node.pull_request.clone());
        let pull_request = link.map(|open| {
            use ket_core::github::CheckState;
            let url = open.url.clone();
            let hovered = self.hovered_pull_request.as_ref() == Some(&node.id);
            let node_id = node.id.clone();
            let mark = icon_button(("worktree-pull-request-button", row), Icon::GitHub)
                .bare()
                .dense();
            let mark = match live.and_then(ket_core::github::PullRequestStatus::checks_state) {
                Some(CheckState::Failed) => mark.tint(paint(t.status.failed)),
                Some(CheckState::Passed) => mark.tint(paint(t.status.running)),
                _ => mark,
            };
            let mark = mark
                .render(t)
                .on_click(move |_, _, cx| {
                    cx.open_url(&url);
                    cx.stop_propagation();
                })
                .into_any_element();
            let checks = live
                .map(ket_core::github::PullRequestStatus::checks_summary)
                .unwrap_or_default();
            let words = {
                use ket_core::github::PullRequestState;
                let mut parts = vec![format!("#{} on GitHub", open.number)];
                if let Some(live) = live {
                    match live.state {
                        PullRequestState::Closed => parts.push("closed".to_owned()),
                        PullRequestState::Merged => parts.push("merged".to_owned()),
                        PullRequestState::Open if live.draft => parts.push("draft".to_owned()),
                        PullRequestState::Open => {}
                    }
                    if live.behind {
                        parts.push("behind".to_owned());
                    }
                }
                if !checks.is_empty() {
                    parts.push(checks);
                }
                parts.join(" \u{b7} ")
            };
            crate::ui::tooltip::tooltip(
                ("worktree-pull-request", row),
                mark,
                words,
                crate::ui::tooltip::Side::Top,
                hovered,
                t,
            )
            .flex_none()
            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                if *is_hovered {
                    this.hovered_pull_request = Some(node_id.clone());
                } else if this.hovered_pull_request.as_ref() == Some(&node_id) {
                    this.hovered_pull_request = None;
                }
                cx.notify();
            }))
        });

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            // Only a level someone chose earns a ring: the default is what
            // every worktree has, and a ring on every row says nothing.
            .when(
                node.token_reduction != ket_core::worktree::default_token_reduction(),
                |el| el.child(self.token_reduction_ring(node, row, window, cx)),
            )
            .children(pull_request)
            .children(self.work_mark(node, row, cx))
            .into_any_element()
    }

    /// The mark for work not yet on the worktree's base: the merge glyph the
    /// row's Commit and merge button wore, now a mark rather than a button
    /// (the action is in the row's menu). Full grey for uncommitted changes;
    /// fainter for commits that are all committed and not yet merged.
    fn work_mark(
        &self,
        node: &WorktreeNode,
        row: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let work = work_of(node)?;
        let t = &self.theme;
        let (ink, words) = match work {
            Work::Uncommitted => (paint(t.text.dim), "Uncommitted changes".to_owned()),
            Work::Commits => {
                let base = node.base.as_ref().map_or("its base", |(_, name)| name);
                (
                    alpha(paint(t.text.dim), 0.45),
                    format!("Commits not on {base} yet"),
                )
            }
        };
        let glyph = div()
            .flex()
            .items_center()
            .justify_center()
            .size(WELL_XS)
            .child(sized_icon(Icon::GitMerge, px(12.0), ink))
            .into_any_element();
        let node_id = node.id.clone();
        Some(
            tooltip(
                ("worktree-work", row),
                glyph,
                words,
                Side::Top,
                self.hovered_work.as_ref() == Some(&node.id),
                t,
            )
            .flex_none()
            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                if *is_hovered {
                    this.hovered_work = Some(node_id.clone());
                } else if this.hovered_work.as_ref() == Some(&node_id) {
                    this.hovered_work = None;
                }
                cx.notify();
            }))
            .into_any_element(),
        )
    }

    /// The worktree's size on disk, as a tag after its name. Under
    /// [`SIZE_WARM`] only while the pointer is on the row, and only once it
    /// is large enough to be build output worth clearing; from there at
    /// rest, warm, then critical past [`SIZE_CRITICAL`].
    pub(crate) fn size_tag(
        &self,
        node: &WorktreeNode,
        row: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if node.missing {
            return None;
        }
        let footprint = self
            .footprint(&node.id)
            .filter(|footprint| footprint.total >= crate::storage::SHOWN_FROM)?;
        let t = &self.theme;
        let tone = size_tone(footprint.total, t);
        if tone.is_none() && self.hovered_worktree.as_ref() != Some(&node.id) {
            return None;
        }
        let words = ket_core::storage::format_bytes(footprint.total);
        let size = match tone {
            Some(tone) => tinted_tag(words, tone),
            None => tag(words, t),
        }
        .text_size(crate::fonts::row_text(TAG_TEXT))
        .py(px(0.0))
        .into_any_element();
        // It is only here because it is large, so its tooltip says what can
        // be done about it, and points at the menu that does it.
        let tip = match footprint.build {
            0 => "On disk, none of it build output".to_owned(),
            build => format!(
                "{} is build output \u{2014} right-click \u{203a} Clear build output",
                ket_core::storage::format_bytes(build)
            ),
        };
        let node_id = node.id.clone();
        Some(
            tooltip(
                ("worktree-footprint", row),
                size,
                tip,
                Side::Top,
                self.hovered_footprint.as_ref() == Some(&node.id),
                t,
            )
            .flex_none()
            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                if *is_hovered {
                    this.hovered_footprint = Some(node_id.clone());
                } else if this.hovered_footprint.as_ref() == Some(&node_id) {
                    this.hovered_footprint = None;
                }
                cx.notify();
            }))
            .into_any_element(),
        )
    }

    /// Folds away the rows hanging under a worktree, as a tag after its
    /// name: how many it is hiding while folded, the open chevron while
    /// they show.
    pub(crate) fn subagent_fold(
        &self,
        node: &WorktreeNode,
        row: usize,
        folded: bool,
        hanging: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let node_id = node.id.clone();
        let (count, chevron) = match folded {
            true => (format!("+{hanging}"), Icon::ChevronRight),
            false => (String::new(), Icon::ChevronDown),
        };
        tag(count, t)
            .id(("subagent-fold", row))
            .flex()
            .items_center()
            .gap(px(1.0))
            .text_size(crate::fonts::row_text(TAG_TEXT))
            .py(px(0.0))
            .pr(px(3.0))
            .when(!folded, |el| el.pl(px(3.0)))
            .cursor_pointer()
            .hover(|style| style.bg(paint(t.selection)))
            .child(sized_icon(chevron, px(9.0), paint(t.text.dim)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed_subagents.remove(&node_id) {
                    this.collapsed_subagents.insert(node_id.clone());
                }
                // The row selects on click too.
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }
}
