//! The right panel's frame: the strip of views along its top, and which view
//! is showing under it.
//!
//! **One strip, fixed slots.** Files, Search and Git are buttons of one width
//! in one order, so choosing a view moves nothing but the bar under the chosen
//! one. The strip takes the panel's share of the band along the top edge — the
//! height of the tab bar beside it — and the space it leaves over still drags
//! the window, as the header it replaced did.
//!
//! **Context under it.** The worktree the panel is showing (or, in Git, the
//! branch) sits on a row of its own below the strip, with whatever buttons the
//! view has. A name as long as a branch slug does not share a row with three
//! icons at the widths this panel gets.

use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, prelude::*, px};

use crate::Shell;
use crate::paint::paint;
use crate::ui::icon::Icon;

/// Height of the row under the strip that says what the view is showing.
pub(crate) const CONTEXT_ROW: gpui::Pixels = px(32.0);

/// Which view the right panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanelView {
    /// The file tree.
    #[default]
    Files,
    /// Workspace text search.
    Search,
    /// What changed in the worktree.
    Git,
}

impl PanelView {
    /// Every view, in the strip's order.
    const ALL: [PanelView; 3] = [PanelView::Files, PanelView::Search, PanelView::Git];

    /// The name it is stored under.
    pub(crate) fn name(self) -> &'static str {
        match self {
            PanelView::Files => "files",
            PanelView::Search => "search",
            PanelView::Git => "git",
        }
    }

    /// The view a stored name means. Anything unknown is the file tree, so a
    /// name written by a later build does not strand the panel.
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "search" => PanelView::Search,
            "git" => PanelView::Git,
            _ => PanelView::Files,
        }
    }

    /// What its segment says.
    fn label(self) -> &'static str {
        match self {
            PanelView::Files => "Files",
            PanelView::Search => "Search",
            PanelView::Git => "Git",
        }
    }

    fn icon(self) -> Icon {
        match self {
            PanelView::Files => Icon::Folder,
            PanelView::Search => Icon::TextSearch,
            PanelView::Git => Icon::GitBranch,
        }
    }
}

impl Shell {
    /// Shows `view` in the right panel, opening the panel if it was hidden.
    pub(crate) fn show_panel_view(&mut self, view: PanelView, cx: &mut Context<Self>) {
        if self.panel_view == PanelView::Search && view != PanelView::Search {
            self.cancel_workspace_search();
        }
        match view {
            PanelView::Files => {
                self.panel_view = PanelView::Files;
                self.show_files_panel();
            }
            PanelView::Search => self.open_workspace_search(cx),
            PanelView::Git => self.open_git_panel(cx),
        }
        cx.notify();
    }

    /// The strip along the top of the right panel.
    pub(crate) fn panel_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let badge = self.git_badge_count();

        // The three views as one segmented switch across the panel's width.
        // Git carries its change count in its label: how much changed is the
        // thing worth knowing before switching to look.
        let switch = crate::ui::group::button_group("panel-views")
            .fill()
            .children(PanelView::ALL.map(|view| {
                let active = self.panel_view == view;
                let segment = crate::ui::group::segment(
                    SharedString::from(format!("panel-view-{}", view.name())),
                    view.label(),
                );
                let segment = match view {
                    PanelView::Git if badge > 99 => segment.count("99+"),
                    PanelView::Git if badge > 0 => segment.count(badge.to_string()),
                    _ => segment,
                };
                segment
                    .selected(active)
                    .leading(
                        view.icon(),
                        paint(if active { t.text.primary } else { t.text.dim }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_panel_view(view, cx);
                    }))
            }))
            .render(&t);

        div()
            .flex()
            .flex_none()
            .items_center()
            .h(crate::tree::TOP_STRIP)
            .px(px(8.0))
            .child(switch)
            .into_any_element()
    }

    /// The row under the strip: what the view is showing, then its buttons.
    pub(crate) fn panel_context_row(&self, label: impl IntoElement) -> Div {
        let t = self.theme;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .h(CONTEXT_ROW)
            .pl_3()
            .pr(px(6.0))
            .text_size(px(10.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(paint(t.text.dim))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .child(label),
            )
    }

    /// The name of the worktree the panel is showing, for the context row.
    pub(crate) fn panel_worktree_name(&self) -> SharedString {
        self.explorer
            .root
            .as_ref()
            .and_then(|root| root.file_name())
            // Keep the worktree's own casing: identifiers stay recognisable,
            // while the surrounding typography supplies the heading treatment.
            .map(|name| name.to_string_lossy().into_owned().into())
            .unwrap_or_else(|| SharedString::from("No worktree selected"))
    }
}
