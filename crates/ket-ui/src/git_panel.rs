//! The right panel's Git view: what changed in the worktree the panel is
//! showing, one row per file.
//!
//! **Two comparisons.** *Uncommitted* is the working tree against `HEAD` —
//! what `git status` would list. *Since base* is the working tree against
//! where the branch left its base, which is what a merge would carry. ket
//! checkpoints worktrees on its own, so a tree with nothing uncommitted is the
//! usual state mid-task, and the second list is the one that still says what
//! the worktree did. The repository's own checkout has no base and shows only
//! the first.
//!
//! **Read off the window's thread, only while seen.** A read is a status walk
//! plus a content diff of every changed file ([`ket_core::diff::of_worktree`],
//! bounded there). It runs when the view opens, when the comparison changes,
//! and on the shell's status tick while the view is on screen — never for a
//! panel showing something else.
//!
//! **Rows.** Grouped under their directory, because a narrow panel cannot fit
//! `crates/ket-ui/src` beside every file name in it. Each carries the file's
//! mark, its lines added and removed, and the letter `ket status` prints for
//! its kind. A click opens the file's diff in the editor area — see
//! [`crate::diff_view`] — except for a conflicted file, which opens as itself:
//! the markers in it are what there is to read. A right-click opens the menu
//! the file tree's folders do, plus Open File and Discard Changes — see
//! `Shell::open_path_menu` and `crate::discard_changes`.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, ScrollHandle, SharedString, div,
    prelude::*, px,
};
use ket_core::diff::FileDiff;
use ket_core::id::WorktreeId;
use ket_core::status::{ChangeKind, GitOperation, Tracking};

use crate::Shell;
use crate::diff_view::DiffKey;
use crate::explorer::PathChange;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::panel::PanelView;
use crate::tabs::TabKind;
use crate::tree::WorktreeNode;
use crate::ui::button::icon_button;
use crate::ui::filetype::tree_mark;
use crate::ui::icon::{Icon, sized_icon};

/// Height of one file row.
const ROW_HEIGHT: gpui::Pixels = px(26.0);

/// Which comparison the view is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GitScope {
    /// The working tree against `HEAD`.
    #[default]
    Uncommitted,
    /// The working tree against where the branch left its base.
    SinceBase,
}

/// One changed file, as a row needs it.
pub(crate) struct GitChange {
    /// Slash-separated from the worktree root.
    pub(crate) path: SharedString,
    /// Where a renamed file came from.
    pub(crate) old_path: Option<SharedString>,
    /// How it changed.
    pub(crate) kind: ChangeKind,
    /// Lines added and removed.
    pub(crate) added: usize,
    pub(crate) removed: usize,
    /// Either side looked binary, so there are no lines to count.
    pub(crate) binary: bool,
    /// Its hunks were over the budget, so the counts are not the whole story.
    pub(crate) truncated: bool,
}

impl GitChange {
    fn from_diff(file: FileDiff) -> Self {
        let (added, removed) = file.line_counts();
        Self {
            path: file.path.into(),
            old_path: file.old_path.map(SharedString::from),
            kind: file.kind,
            added,
            removed,
            binary: file.binary,
            truncated: file.truncated,
        }
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    fn dir(&self) -> &str {
        self.path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
    }
}

/// One finished read.
pub(crate) struct GitSnapshot {
    /// The worktree it was read from.
    root: PathBuf,
    /// The comparison it answers.
    scope: GitScope,
    /// Changed files, in the order they are drawn.
    pub(crate) files: Vec<GitChange>,
    /// How many files differed, including any past the diff's cap.
    total: usize,
    /// Ahead and behind the base, when there is one.
    tracking: Option<Tracking>,
    /// Git left mid-operation.
    in_progress: Option<GitOperation>,
    /// `HEAD`'s short id and commit time, read only when nothing is
    /// uncommitted: the clean view names them.
    head: Option<(SharedString, u64)>,
}

/// The Git view's state.
#[derive(Default)]
pub(crate) struct GitPanel {
    /// Which comparison is chosen.
    scope: GitScope,
    /// The last read, for whichever worktree and scope it was.
    snapshot: Option<GitSnapshot>,
    /// Why the last read failed, and for which worktree and comparison.
    error: Option<(PathBuf, GitScope, SharedString)>,
    /// Whether a read is in flight.
    reading: bool,
    /// Scroll position of the file list.
    scroll: ScrollHandle,
}

/// Paths drawn under their directory, the root's own files first.
fn draw_order(a: &GitChange, b: &GitChange) -> std::cmp::Ordering {
    (!a.dir().is_empty(), a.dir(), a.name().to_lowercase()).cmp(&(
        !b.dir().is_empty(),
        b.dir(),
        b.name().to_lowercase(),
    ))
}

/// The letter `ket status` prints for a kind, and the colour it is drawn in.
fn kind_mark(kind: ChangeKind, t: &ket_core::theme::Theme) -> (&'static str, gpui::Rgba) {
    match kind {
        ChangeKind::Added => ("A", paint(t.diff.added)),
        ChangeKind::Untracked => ("?", paint(t.diff.added)),
        ChangeKind::Modified => ("M", paint(t.diff.modified)),
        ChangeKind::TypeChange => ("T", paint(t.diff.modified)),
        ChangeKind::Deleted => ("D", paint(t.diff.removed)),
        ChangeKind::Renamed => ("R", paint(t.status.merging)),
        ChangeKind::Conflicted => ("U", paint(t.status.attention)),
    }
}

impl Shell {
    /// The sidebar's record for a worktree.
    pub(crate) fn worktree_node(&self, id: &WorktreeId) -> Option<&WorktreeNode> {
        self.projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .find(|node| &node.id == id)
    }

    /// The node behind whatever the right panel is showing.
    fn panel_node(&self) -> Option<&WorktreeNode> {
        self.worktree_node(self.explorer.worktree.as_ref()?)
    }

    /// Shows the Git view and reads it.
    pub(crate) fn open_git_panel(&mut self, cx: &mut Context<Self>) {
        self.sync_explorer(cx);
        self.panel_open = true;
        self.panel_view = PanelView::Git;
        self.persist_view();
        self.refresh_git_panel(cx);
    }

    /// The number on the strip's Git button.
    ///
    /// The view's own count when it has read this worktree's uncommitted
    /// files, so the badge and the list agree; the status tick's otherwise.
    pub(crate) fn git_badge_count(&self) -> usize {
        if let Some(snapshot) = &self.git_panel.snapshot
            && snapshot.scope == GitScope::Uncommitted
            && Some(&snapshot.root) == self.explorer.root.as_ref()
        {
            return snapshot.total;
        }
        self.panel_node().map(|node| node.changes).unwrap_or(0)
    }

    /// Re-reads the Git view, if it is on screen and not already reading.
    pub(crate) fn refresh_git_panel(&mut self, cx: &mut Context<Self>) {
        if !self.panel_open || self.panel_view != PanelView::Git || self.git_panel.reading {
            return;
        }
        let Some(root) = self.explorer.root.clone() else {
            return;
        };
        let base = self.panel_node().and_then(|node| node.base.clone());
        // A checkout with no base cannot be compared against one; fall back
        // rather than show an empty list under a choice that is not offered.
        if base.is_none() {
            self.git_panel.scope = GitScope::Uncommitted;
        }
        let scope = self.git_panel.scope;
        let base_rev = base.map(|(_, rev)| rev.to_string());

        self.git_panel.reading = true;
        let read_root = root.clone();
        let read = cx.background_executor().spawn(async move {
            let status = ket_core::status::of_worktree(&root, base_rev.as_deref())?;
            let rev = match (scope, &base_rev) {
                (GitScope::SinceBase, Some(base)) => since_rev(&root, base),
                _ => "HEAD".to_owned(),
            };
            let diff = ket_core::diff::of_worktree(&root, &rev)?;

            let mut files: Vec<GitChange> =
                diff.files.into_iter().map(GitChange::from_diff).collect();
            // The diff sees both sides of a conflicted file and calls it
            // modified; status knows it is unmerged, and that is what a row
            // has to say.
            for file in &mut files {
                if status.files.iter().any(|entry| {
                    entry.kind == ChangeKind::Conflicted && entry.path == file.path.as_ref()
                }) {
                    file.kind = ChangeKind::Conflicted;
                }
            }
            files.sort_by(draw_order);
            let head = (scope == GitScope::Uncommitted && files.is_empty())
                .then(|| ket_core::git::Git::new(&root).head_stamp())
                .flatten()
                .map(|(id, at)| (SharedString::from(id), at));
            Ok::<_, ket_core::KetError>(GitSnapshot {
                root,
                scope,
                files,
                total: diff.total_files,
                tracking: status.tracking,
                in_progress: status.in_progress,
                head,
            })
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.git_panel.reading = false;
                match result {
                    Ok(snapshot) => {
                        shell.git_panel.snapshot = Some(snapshot);
                        shell.git_panel.error = None;
                    }
                    Err(error) => {
                        shell.git_panel.snapshot = None;
                        shell.git_panel.error = Some((read_root, scope, error.to_string().into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_git_scope(&mut self, scope: GitScope, cx: &mut Context<Self>) {
        if self.git_panel.scope == scope {
            return;
        }
        self.git_panel.scope = scope;
        // The old list answers a different question; showing it under the new
        // label until the read lands would be showing a wrong answer.
        self.git_panel.snapshot = None;
        self.refresh_git_panel(cx);
        cx.notify();
    }

    /// The Git view.
    pub(crate) fn git_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_explorer(cx);
        let stale = self.git_panel.snapshot.as_ref().is_some_and(|snapshot| {
            Some(&snapshot.root) != self.explorer.root.as_ref()
                || snapshot.scope != self.git_panel.scope
        });
        let stale_error = self
            .git_panel
            .error
            .as_ref()
            .is_some_and(|(root, scope, _)| {
                Some(root) != self.explorer.root.as_ref() || *scope != self.git_panel.scope
            });
        if stale {
            self.git_panel.snapshot = None;
        }
        if stale_error {
            self.git_panel.error = None;
        }
        if self.git_panel.snapshot.is_none() && self.git_panel.error.is_none() {
            self.refresh_git_panel(cx);
        }

        let t = self.theme;
        let node = self.panel_node();
        let branch: SharedString = node
            .map(|node| node.branch.clone())
            .unwrap_or_else(|| self.panel_worktree_name());
        let base = node.and_then(|node| node.base.clone());
        let snapshot = self.git_panel.snapshot.as_ref();

        let operation = snapshot.and_then(|s| s.in_progress).map(|op| {
            crate::ui::chip::signal_tag(Icon::GitMerge, op.label(), paint(t.status.merging))
        });
        let tracking = snapshot.and_then(|s| s.tracking.clone()).map(|tracking| {
            div()
                .flex_none()
                .font_weight(FontWeight::NORMAL)
                .child(format!("↑{} ↓{}", tracking.ahead, tracking.behind))
        });

        let context = self
            .panel_context_row(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .child(sized_icon(Icon::GitBranch, px(12.0), paint(t.text.dim)))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(paint(t.text.primary))
                            .child(branch),
                    )
                    .children(operation),
            )
            .children(tracking)
            .child(
                icon_button("git-refresh", Icon::Refresh)
                    .bare()
                    .dense()
                    .render(&t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_git_panel(cx);
                        cx.notify();
                    })),
            );

        let scope = base
            .as_ref()
            .map(|(label, _)| self.git_scope_toggle(label, cx));

        let body = match (snapshot, &self.git_panel.error) {
            _ if self.explorer.root.is_none() => {
                crate::ui::picker::note("Select a worktree to see its changes", &t)
                    .into_any_element()
            }
            (_, Some((_, _, error))) => {
                crate::ui::picker::note(error.clone(), &t).into_any_element()
            }
            (None, None) => crate::ui::picker::note("Reading…", &t).into_any_element(),
            (Some(snapshot), None) if snapshot.files.is_empty() => {
                self.git_clean_state(base.as_ref().map(|(label, _)| label.clone()))
            }
            (Some(_), None) => {
                self.git_file_list(base.as_ref().map(|(label, _)| label.clone()), cx)
            }
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            // No fill: the card paints it, rounded — see `Shell::sidebar`.
            .child(self.panel_strip(cx))
            .child(context)
            .children(scope)
            .child(body)
            .into_any_element()
    }

    /// The Uncommitted / Since-base switch.
    fn git_scope_toggle(&self, base: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let chosen = self.git_panel.scope;
        let count = self
            .git_panel
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.total);
        let segment = |id: &'static str, label: String, scope: GitScope| {
            let on = chosen == scope;
            let segment = crate::ui::group::segment(id, label).selected(on);
            // Only the chosen side has been read, so only it has a count.
            let segment = match on.then_some(count).flatten() {
                Some(count) => segment.count(count.to_string()),
                None => segment,
            };
            segment.on_click(cx.listener(move |this, _, _, cx| this.set_git_scope(scope, cx)))
        };

        div()
            .flex()
            .flex_none()
            .mx_2()
            .mb(px(6.0))
            .child(
                crate::ui::group::button_group("git-scope")
                    .fill()
                    .child(segment(
                        "git-scope-uncommitted",
                        "Uncommitted".to_owned(),
                        GitScope::Uncommitted,
                    ))
                    .child(segment(
                        "git-scope-base",
                        format!("Since {base}"),
                        GitScope::SinceBase,
                    ))
                    .render(&t),
            )
            .into_any_element()
    }

    /// Nothing to list.
    fn git_clean_state(&self, base: Option<SharedString>) -> AnyElement {
        match (self.git_panel.scope, base) {
            (GitScope::Uncommitted, _) => self.git_clean_tree(),
            (GitScope::SinceBase, Some(base)) => {
                self.git_empty_note(format!("No changes since {base}"))
            }
            (GitScope::SinceBase, None) => self.git_empty_note("No changes".to_owned()),
        }
    }

    /// A working tree with nothing uncommitted: a moon circling its planet,
    /// and the commit the tree still matches.
    fn git_clean_tree(&self) -> AnyElement {
        let t = self.theme;
        let since = self
            .git_panel
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.as_ref())
            .map(|(id, at)| format!("no edits since {id} · {}", age(*at)))
            .unwrap_or_else(|| "no edits".to_owned());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .pt(px(72.0))
            .border_t_1()
            .border_color(paint(t.rule))
            .child(
                div()
                    .prose()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(paint(t.text.primary))
                    .child("Stable orbit"),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .text_size(px(10.0))
                    .text_color(paint(t.text.dim))
                    .child(since),
            )
            .child(div().w_full().mt(px(28.0)).child(crate::orbit::orbit(
                paint(t.text.primary),
                paint(t.text.dim),
                paint(t.status.running),
            )))
            .into_any_element()
    }

    fn git_empty_note(&self, headline: String) -> AnyElement {
        let t = self.theme;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .gap(px(8.0))
            .pt(px(96.0))
            .border_t_1()
            .border_color(paint(t.rule))
            .child(sized_icon(Icon::CircleCheck, px(20.0), paint(t.text.dim)))
            .child(
                div()
                    .mt_1()
                    .text_size(px(11.0))
                    .text_color(paint(t.text.primary))
                    .child(headline),
            )
            .into_any_element()
    }

    /// The sections and their rows.
    fn git_file_list(&self, base: Option<SharedString>, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let Some(snapshot) = self.git_panel.snapshot.as_ref() else {
            return div().into_any_element();
        };
        let mono = self.font_family.clone();
        let since = match self.git_panel.scope {
            GitScope::Uncommitted => None,
            GitScope::SinceBase => self.panel_node().and_then(|node| node.base.clone()),
        };
        let open_diff = match self.space().and_then(|space| space.active_tab()) {
            Some(tab) => match &tab.kind {
                TabKind::Diff(key) => Some(key.clone()),
                _ => None,
            },
            None => None,
        };

        let (conflicts, changes): (Vec<&GitChange>, Vec<&GitChange>) = snapshot
            .files
            .iter()
            .partition(|file| file.kind == ChangeKind::Conflicted);
        let (added, removed) = changes
            .iter()
            .fold((0, 0), |(a, r), file| (a + file.added, r + file.removed));

        let mut rows: Vec<AnyElement> = Vec::new();
        if !conflicts.is_empty() {
            rows.push(section(
                "CONFLICTS",
                conflicts.len(),
                None,
                paint(t.status.attention),
                &t,
            ));
            self.push_git_rows(
                &mut rows, &conflicts, snapshot, &since, &open_diff, &mono, cx,
            );
            rows.push(div().flex_none().h(px(8.0)).into_any_element());
        }
        if !changes.is_empty() {
            let title = match (self.git_panel.scope, &base) {
                (GitScope::SinceBase, Some(base)) => format!("SINCE {}", base.to_uppercase()),
                _ => "CHANGES".to_owned(),
            };
            rows.push(section(
                &title,
                changes.len(),
                Some((added, removed)),
                paint(t.text.dim),
                &t,
            ));
            self.push_git_rows(&mut rows, &changes, snapshot, &since, &open_diff, &mono, cx);
        }
        if snapshot.total > snapshot.files.len() {
            rows.push(
                crate::ui::picker::note(
                    format!(
                        "showing {} of {} changed files",
                        snapshot.files.len(),
                        snapshot.total
                    ),
                    &t,
                )
                .into_any_element(),
            );
        }

        div()
            .id("git-files")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.git_panel.scroll)
            .border_t_1()
            .border_color(paint(t.rule))
            .children(rows)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn push_git_rows(
        &self,
        rows: &mut Vec<AnyElement>,
        files: &[&GitChange],
        snapshot: &GitSnapshot,
        since: &Option<(SharedString, SharedString)>,
        open_diff: &Option<DiffKey>,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) {
        let t = self.theme;
        let root: SharedString = snapshot.root.display().to_string().into();
        let mut dir: Option<&str> = None;
        for file in files {
            if dir != Some(file.dir()) {
                dir = Some(file.dir());
                if !file.dir().is_empty() {
                    rows.push(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(24.0))
                            .pt(px(4.0))
                            .pl(px(14.0))
                            .pr_2()
                            .text_size(px(10.0))
                            .text_color(paint(t.text.dim))
                            .child(sized_icon(Icon::Folder, px(11.0), paint(t.text.dim)))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(file.dir().to_owned()),
                            )
                            .into_any_element(),
                    );
                }
            }

            let key = DiffKey {
                root: root.clone(),
                rel: file.path.clone(),
                base: since.clone(),
            };
            let selected = open_diff.as_ref() == Some(&key);
            let deleted = file.kind == ChangeKind::Deleted;
            let conflicted = file.kind == ChangeKind::Conflicted;
            let (letter, colour) = kind_mark(file.kind, &t);

            let counts: AnyElement = if conflicted {
                caption("both modified", &t)
            } else if file.binary {
                caption("binary", &t)
            } else if file.kind == ChangeKind::TypeChange && file.added == 0 && file.removed == 0 {
                caption("type", &t)
            } else {
                div()
                    .flex()
                    .flex_none()
                    .gap(px(6.0))
                    .text_size(px(10.5))
                    .when(file.added > 0, |el| {
                        el.child(
                            div()
                                .text_color(paint(t.diff.added))
                                .child(format!("+{}", file.added)),
                        )
                    })
                    .when(file.removed > 0, |el| {
                        el.child(
                            div()
                                .text_color(paint(t.diff.removed))
                                .child(format!("\u{2212}{}", file.removed)),
                        )
                    })
                    .when(file.truncated, |el| {
                        el.child(div().text_color(paint(t.text.dim)).child("\u{2026}"))
                    })
                    .into_any_element()
            };

            let abs = snapshot.root.join(file.path.as_ref());
            let rela = file.path.clone();
            let title = file.name().to_owned();
            rows.push(
                div()
                    .id(SharedString::from(format!("git-row-{}", file.path)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .h(ROW_HEIGHT)
                    .pl(px(14.0))
                    .pr_2()
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(paint(t.selection)))
                    .when(!selected, |el| el.hover(|style| style.bg(paint(t.hover))))
                    .child(tree_mark(file.name(), deleted, mono.clone(), &t))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(paint(if deleted { t.text.dim } else { t.text.primary }))
                            .when(deleted, |el| el.line_through())
                            .child(file.name().to_owned()),
                    )
                    .children(file.old_path.as_ref().map(|old| {
                        let old_name = old.rsplit('/').next().unwrap_or(old).to_owned();
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(paint(t.text.dim))
                            .child(format!("\u{2190} {old_name}"))
                    }))
                    .child(div().flex_1())
                    .child(counts)
                    .child(
                        div()
                            .flex_none()
                            .w(px(12.0))
                            .text_center()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colour)
                            .child(letter),
                    )
                    // `on_mouse_down`, for the reason the sidebar's rows
                    // give. See `Shell::open_path_menu`.
                    .on_mouse_down(MouseButton::Right, {
                        let abs = abs.clone();
                        let root = snapshot.root.clone();
                        let kind = file.kind;
                        let old_path = file.old_path.clone();
                        let uncommitted = snapshot.scope == GitScope::Uncommitted;
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let change = PathChange {
                                root: root.clone(),
                                kind,
                                old_path: old_path.clone(),
                                uncommitted,
                            };
                            this.open_path_menu(
                                abs.clone(),
                                rela.clone(),
                                Some(change),
                                event.position,
                            );
                            cx.stop_propagation();
                            cx.notify();
                        })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if conflicted {
                            this.open_editor_tab(abs.clone(), cx);
                        } else {
                            this.open_diff_tab(key.clone(), title.clone(), cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
    }
}

/// Where "since base" measures from: the commit the branch and its base
/// last shared, so work that landed on the base since is not read as this
/// branch taking it out again. The base itself when git cannot say.
pub(crate) fn since_rev(root: &std::path::Path, base: &str) -> String {
    ket_core::git::Git::new(root)
        .merge_base(base, "HEAD")
        .unwrap_or_else(|_| base.to_owned())
}

/// How long ago `at_ms` was, in its largest unit: `2h`, `3d`.
fn age(at_ms: u64) -> String {
    let since = ket_core::now_ms().saturating_sub(at_ms);
    if since < 60_000 {
        return "now".to_owned();
    }
    crate::status_bar::compact_duration(since)
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A section's header: its name, how many rows, and what they add up to.
fn section(
    title: &str,
    count: usize,
    totals: Option<(usize, usize)>,
    ink: gpui::Rgba,
    t: &ket_core::theme::Theme,
) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .h(px(28.0))
        .pl(px(12.0))
        .pr_2()
        .text_size(px(9.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .child(title.to_owned())
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.dim))
                .child(count.to_string()),
        )
        .child(div().flex_1())
        .children(totals.map(|(added, removed)| {
            div()
                .flex()
                .gap(px(6.0))
                .text_size(px(10.5))
                .font_weight(FontWeight::NORMAL)
                .child(
                    div()
                        .text_color(paint(t.diff.added))
                        .child(format!("+{added}")),
                )
                .child(
                    div()
                        .text_color(paint(t.diff.removed))
                        .child(format!("\u{2212}{removed}")),
                )
        }))
        .into_any_element()
}

/// A word where a row's line counts would be.
fn caption(text: &'static str, t: &ket_core::theme::Theme) -> AnyElement {
    div()
        .flex_none()
        .text_size(px(10.0))
        .text_color(paint(t.text.dim))
        .child(text)
        .into_any_element()
}
