//! Workspace text search in the right panel.
//!
//! Disk work belongs to `ket-core` and runs on the background executor. This
//! module owns only the mode, its three fields and the grouped result view.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{
    AnyElement, Context, KeyDownEvent, ScrollHandle, SharedString, Window, div, prelude::*, px,
};
use ket_core::text_search::{FileMatches, Matches, Query};

use crate::Shell;
use crate::input::{Style, is_text, text_field};
use crate::paint::paint;
use crate::panel::PanelView;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::{CAPTION, LABEL};

/// Delay after typing before a filesystem search begins.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Workspace-search UI state.
pub(crate) struct WorkspaceSearch {
    /// The last completed result.
    results: Matches,
    /// A message from pattern compilation or file discovery.
    error: Option<SharedString>,
    /// Root the visible results belong to.
    root: Option<PathBuf>,
    /// Whether the newest request is still running.
    searching: bool,
    /// Invalidates delayed and in-flight work when the query changes.
    revision: Arc<AtomicU64>,
    /// Scroll position of the grouped results.
    scroll: ScrollHandle,
}

impl Default for WorkspaceSearch {
    fn default() -> Self {
        Self {
            results: Matches::default(),
            error: None,
            root: None,
            searching: false,
            revision: Arc::new(AtomicU64::new(0)),
            scroll: ScrollHandle::new(),
        }
    }
}

impl Shell {
    /// Whether one of the visible workspace-search fields owns the keyboard.
    pub(crate) fn workspace_search_typing(&self, window: &Window, cx: &gpui::App) -> bool {
        self.panel_open
            && self.panel_view == PanelView::Search
            && [
                &self.searches.workspace,
                &self.searches.workspace_include,
                &self.searches.workspace_exclude,
            ]
            .into_iter()
            .any(|field| field.read(cx).is_focused(window))
    }

    /// Opens and focuses workspace search in the right panel.
    pub(crate) fn open_workspace_search(&mut self, cx: &mut Context<Self>) {
        self.sync_explorer(cx);
        self.panel_open = true;
        self.panel_view = PanelView::Search;
        self.persist_view();
        self.searches.workspace.update(cx, |input, cx| {
            input.request_focus();
            cx.notify();
        });
        self.queue_workspace_search(cx);
    }

    /// Returns the right panel to the file tree and cancels pending searches.
    fn close_workspace_search(&mut self) {
        self.cancel_workspace_search();
        self.panel_view = PanelView::Files;
        self.persist_view();
    }

    /// Cancels pending and in-flight searches, for a panel leaving search.
    pub(crate) fn cancel_workspace_search(&mut self) {
        self.workspace_search.searching = false;
        self.workspace_search
            .revision
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Debounces and starts a search for the current field values.
    pub(crate) fn queue_workspace_search(&mut self, cx: &mut Context<Self>) {
        let revision = self
            .workspace_search
            .revision
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if self.panel_view != PanelView::Search {
            return;
        }

        let text = self.searches.workspace.read(cx).text();
        if text.is_empty() {
            self.workspace_search.results = Matches::default();
            self.workspace_search.error = None;
            self.workspace_search.searching = false;
            cx.notify();
            return;
        }
        let Some(root) = self.explorer.root.clone() else {
            self.workspace_search.results = Matches::default();
            self.workspace_search.error = Some("select a worktree to search".into());
            self.workspace_search.searching = false;
            self.workspace_search.root = None;
            cx.notify();
            return;
        };
        if self.workspace_search.root.as_ref() != Some(&root) {
            self.workspace_search.results = Matches::default();
        }
        self.workspace_search.root = Some(root.clone());
        let query = Query {
            text,
            include: self.searches.workspace_include.read(cx).text(),
            exclude: self.searches.workspace_exclude.read(cx).text(),
        };
        self.workspace_search.searching = true;
        self.workspace_search.error = None;
        let gate = self.workspace_search.revision.clone();
        let executor = cx.background_executor().clone();
        let work = cx.background_executor().spawn(async move {
            executor.timer(DEBOUNCE).await;
            (gate.load(Ordering::Relaxed) == revision)
                .then(|| ket_core::text_search::search(&root, &query))
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let Some(result) = work.await else { return };
            let _ = shell.update(cx, |shell, cx| {
                if shell.workspace_search.revision.load(Ordering::Relaxed) != revision {
                    return;
                }
                shell.workspace_search.searching = false;
                match result {
                    Ok(results) => {
                        shell.workspace_search.results = results;
                        shell.workspace_search.error = None;
                    }
                    Err(error) => {
                        shell.workspace_search.results = Matches::default();
                        shell.workspace_search.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Routes keys while one of the workspace-search fields owns focus.
    pub(crate) fn workspace_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.panel_view != PanelView::Search || !self.panel_open {
            return false;
        }
        let fields = [
            self.searches.workspace.clone(),
            self.searches.workspace_include.clone(),
            self.searches.workspace_exclude.clone(),
        ];
        let Some(focused) = fields
            .iter()
            .position(|field| field.read(cx).is_focused(window))
        else {
            return false;
        };
        if event.keystroke.key == "escape" {
            self.close_workspace_search();
            return true;
        }
        if is_text(&event.keystroke) {
            return true;
        }
        if fields[focused].update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.queue_workspace_search(cx),
            "tab" => {
                fields[(focused + 1) % fields.len()].update(cx, |input, _| input.request_focus())
            }
            // Modified chords must continue to the shell's global shortcut
            // table; ordinary unused keys belong to this panel.
            _ if event.keystroke.modifiers.platform || event.keystroke.modifiers.control => {
                return false;
            }
            _ => {}
        }
        true
    }

    /// Search mode for the right panel.
    pub(crate) fn workspace_search_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.sync_explorer(cx);
        if self.workspace_search.root != self.explorer.root {
            if self.searches.workspace.read(cx).text().is_empty() {
                self.workspace_search.root = self.explorer.root.clone();
                self.workspace_search.results = Matches::default();
            } else {
                self.queue_workspace_search(cx);
            }
        }
        let t = self.theme;
        let header = self.panel_strip(cx);
        let context = self.panel_context_row(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(self.panel_worktree_name()),
        );

        let fields = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(6.0))
            .px(px(8.0))
            .pb(px(8.0))
            .child(
                text_field(
                    &self.searches.workspace,
                    "workspace-search-query",
                    false,
                    Style::new(&t, self.caret.visible).leading(Icon::Search),
                    window,
                    cx,
                )
                .h(px(32.0)),
            )
            .child(
                text_field(
                    &self.searches.workspace_include,
                    "workspace-search-include",
                    self.workspace_search.error.is_some(),
                    Style::new(&t, self.caret.visible),
                    window,
                    cx,
                )
                .h(px(30.0)),
            )
            .child(
                text_field(
                    &self.searches.workspace_exclude,
                    "workspace-search-exclude",
                    self.workspace_search.error.is_some(),
                    Style::new(&t, self.caret.visible),
                    window,
                    cx,
                )
                .h(px(30.0)),
            );

        let body = self.workspace_search_results(cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            // No fill: the card paints it, rounded — see `Shell::sidebar`.
            .child(header)
            .child(context)
            .child(fields)
            .child(body)
            .into_any_element()
    }

    /// Grouped result list and its status line.
    fn workspace_search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let state = &self.workspace_search;
        let query_empty = self.searches.workspace.read(cx).text().is_empty();
        let status: SharedString = if state.searching {
            "Searching…".into()
        } else if let Some(error) = &state.error {
            error.clone()
        } else if query_empty {
            "Enter text to search across this worktree".into()
        } else {
            format!(
                "{} result{} in {} file{}",
                state.results.total,
                if state.results.total == 1 { "" } else { "s" },
                state.results.files.len(),
                if state.results.files.len() == 1 {
                    ""
                } else {
                    "s"
                }
            )
            .into()
        };

        let mut rows = Vec::new();
        let mut result_index = 0usize;
        for file in &state.results.files {
            rows.push(file_header(file, &t));
            for line in &file.lines {
                let path = file.path.clone();
                let line_number = line.line;
                let column = line.column;
                rows.push(
                    div()
                        .id(("workspace-search-result", result_index))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(7.0))
                        .min_h(px(28.0))
                        .pl(px(18.0))
                        .pr(px(8.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(paint(t.hover)))
                        .child(
                            div()
                                .flex_none()
                                .w(px(34.0))
                                .text_right()
                                .text_size(CAPTION)
                                .text_color(paint(t.text.dim))
                                .child(line.line.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .flex()
                                .text_ellipsis()
                                .font_family(self.font_family.clone())
                                // The panel's label size, not the editor's:
                                // a preview is a row in this list, and the
                                // editor's size is for reading a whole file.
                                .text_size(LABEL)
                                .children(highlighted_preview(&line.preview, &line.ranges, &t)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_editor_tab_at(path.clone(), line_number, column, cx);
                            window.focus(&this.focus);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                result_index += 1;
            }
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .flex_none()
                    .px(px(9.0))
                    .py(px(6.0))
                    .text_size(CAPTION)
                    .text_color(paint(if state.error.is_some() {
                        t.status.failed
                    } else {
                        t.text.dim
                    }))
                    .child(status),
            )
            .child(
                div()
                    .id("workspace-search-results")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&state.scroll)
                    .children(rows),
            )
            .children(
                (state.results.truncated || state.results.index_truncated).then(|| {
                    div()
                        .flex_none()
                        .px(px(9.0))
                        .py(px(5.0))
                        .text_size(CAPTION)
                        .text_color(paint(t.text.dim))
                        .child(if state.results.truncated {
                            "Showing the first 500 matches"
                        } else {
                            "File index limit reached; results may be incomplete"
                        })
                }),
            )
            .into_any_element()
    }
}

/// One collapseless file group heading.
fn file_header(file: &FileMatches, t: &ket_core::theme::Theme) -> AnyElement {
    let count: usize = file.lines.iter().map(|line| line.ranges.len()).sum();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.0))
        .h(px(28.0))
        .px(px(8.0))
        .border_t_1()
        .border_color(paint(t.border))
        .text_size(LABEL)
        .text_color(paint(t.text.primary))
        .child(sized_icon(Icon::File, px(13.0), paint(t.text.dim)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(file.relative_path.clone()),
        )
        .child(
            div()
                .text_size(CAPTION)
                .text_color(paint(t.text.dim))
                .child(count.to_string()),
        )
        .into_any_element()
}

/// Text spans with every occurrence highlighted.
fn highlighted_preview(
    text: &str,
    matches: &[Range<usize>],
    t: &ket_core::theme::Theme,
) -> Vec<AnyElement> {
    let mut parts = Vec::new();
    let mut at = 0usize;
    for range in matches {
        if at < range.start {
            parts.push(
                div()
                    .child(text[at..range.start].to_owned())
                    .into_any_element(),
            );
        }
        parts.push(
            div()
                .bg(paint(t.selection))
                .child(text[range.clone()].to_owned())
                .into_any_element(),
        );
        at = range.end;
    }
    if at < text.len() {
        parts.push(div().child(text[at..].to_owned()).into_any_element());
    }
    parts
}
