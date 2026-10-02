//! Committing a worktree's work and merging it into its base, from one button.
//!
//! The everyday end of a worktree. An agent finished, the diff reads well, and
//! what remains is `git add`, `git commit`, switch the primary checkout, `git
//! merge` — four commands in a terminal ket was supposed to save you opening.
//! [`ket_core::workspace::Workspace::merge_worktree`] is the operation; this is
//! the surface over it.
//!
//! Three things shape what is here.
//!
//! **It asks first, by default.** The commit stages everything git does not
//! ignore, so a `.env` that provisioning copied into the worktree and the
//! repository never ignored is an ordinary file to `git add -A` — and the very
//! next step merges it into the branch everything else is cut from. The dialog
//! lists what would be committed for exactly that reason. `merge.confirm` can
//! be turned off; it defaults on, and should.
//!
//! **It refuses more than it forces.** Only one of the ways this can fail is
//! waivable — the primary checkout having uncommitted changes of its own — so
//! only that one grows an "anyway" button. A conflict, a primary checkout on
//! the wrong branch, a repository with no author identity: none of those is
//! fixed by asking again harder, and offering to would be a lie. That is the
//! deliberate difference from [`crate::worktree_menu`]'s delete dialog, whose
//! every refusal is waivable.
//!
//! **A toast is the receipt.** On success the dialog closes, a toast names the
//! base it landed in, and the sidebar reloads: the rail goes clean, the change
//! count goes to nothing, and with `keep_after_merge` off the row is gone. The
//! dialog stays open only when something needs saying — a refusal, or the one
//! success that looks identical to a broken button.

use gpui::{AnyElement, Context, FontWeight, MouseButton, SharedString, div, prelude::*, px};
use ket_core::id::WorktreeId;
use ket_core::workspace::{MergeError, MergeReport, Workspace};

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::ui::banner::banner;
use crate::ui::button::{Variant, button, icon_button};
use crate::ui::dialog::{card, centered, footer, header};
use crate::ui::filetype::file_mark;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::toast::Tone;
use crate::ui::tooltip::{Side, tooltip};
use crate::ui::{LABEL, RADIUS_MD};
use crate::worktree_menu::WorktreeTarget;

/// How many changed paths the dialog names before it stops listing them.
///
/// Enough to recognise a stray `.env` or a `node_modules` that is not ignored,
/// short enough that the dialog does not become a file browser.
const LISTED_PATHS: usize = 8;

/// The dialog's width. Wide enough that a long branch name in the commit
/// step and a full path in the file list both fit without truncating.
const WIDTH: gpui::Pixels = px(600.0);

/// A step's line: "Commit 9 changes to …", "Merge into …".
const STEP_TEXT: gpui::Pixels = px(13.0);

/// The circle each step hangs from. The file list gives its marks a column
/// this wide too, so a node and a mark share a centre.
const STEP_NODE: gpui::Pixels = px(24.0);

/// The gap between that column and the words, in a step and a file row
/// alike, so a step's line and a file's path start at one x.
const LEAD_GAP: gpui::Pixels = px(12.0);

/// How far a file row's content sits in from the list's outer edge: the
/// hairline and the row's padding. The steps are inset by the same, which is
/// what puts the nodes over the marks.
const LIST_INSET: gpui::Pixels = px(1.0 + 10.0);

/// A row in the list of what will be committed, and its text.
const FILE_ROW: gpui::Pixels = px(28.0);
const FILE_TEXT: gpui::Pixels = px(12.0);

/// The "are you sure" step before a worktree is committed and merged, and the
/// place its result is read when the row cannot say it.
pub(crate) struct MergeWorktreeConfirm {
    /// What is about to be merged.
    ///
    /// The menu's target: the fields a dialog needs — id, branch, path — are
    /// the same ones, and it is carried by value for the same reason, that the
    /// sidebar is rebuilt on every reload and an index would go stale.
    pub(crate) target: WorktreeTarget,
    /// The branch it would land in.
    pub(crate) base: SharedString,
    /// How many paths are uncommitted, read when the dialog opened.
    pub(crate) changes: usize,
    /// The first [`LISTED_PATHS`] of them. This is what `git add -A` will take.
    pub(crate) paths: Vec<SharedString>,
    /// Why an attempt was refused, if it was.
    pub(crate) refusal: Option<SharedString>,
    /// Whether forcing would get past that refusal.
    pub(crate) waivable: bool,
    /// What it did, when it succeeded and the row cannot show it.
    pub(crate) outcome: Option<SharedString>,
    /// Whether the outcome should offer to reclaim the checkout.
    ///
    /// Set when a merge found the branch already in the base with nothing to
    /// commit: the dialog is open anyway to say so, the work has provably
    /// landed, and a finished checkout is disk nobody will think about again.
    /// A merge that did land closes the dialog instead, and is deleted from
    /// the row's menu like any other worktree.
    pub(crate) offer_delete: bool,
    /// The project's primary checkout, where the merge lands. Named in the
    /// prompt [`Shell::merge_with_agent`] sends, since the agent works in
    /// the worktree and has to be told where the base is checked out.
    pub(crate) primary: SharedString,
    /// Whether the pointer is on "Merge with agent", which says on hover
    /// what it will ask for.
    pub(crate) agent_tip: bool,
}

/// What "Merge with agent" says it does, on hover.
const AGENT_TIP: &str = "Asks this worktree's agent to inspect the refusal, commit any remaining \
                         work, and finish the merge without discarding changes.";

impl Shell {
    /// Opens the merge dialog for one row, or merges outright.
    ///
    /// Reads the worktree once, here, so the dialog can say what is about to be
    /// committed. That read is `gix` in-process — the same one the sidebar's
    /// own tick does — rather than a `git` spawn.
    pub(crate) fn request_merge_worktree(
        &mut self,
        project: usize,
        worktree: usize,
        cx: &mut Context<Self>,
    ) {
        // One at a time. A second merge started while the first is in flight
        // would be two processes writing the same index.
        if self.merging.is_some() {
            return;
        }
        let Some(node) = self
            .projects
            .get(project)
            .and_then(|project| project.worktrees.get(worktree))
        else {
            return;
        };
        let primary: SharedString = self
            .projects
            .get(project)
            .and_then(|project| project.worktrees.iter().find(|w| w.primary))
            .map(|w| w.path.display().to_string().into())
            .unwrap_or_default();
        // The primary checkout is where a merge lands; it cannot also be what
        // is merged. The row draws no button for it, and the keyboard reaches
        // here without one.
        if node.primary || node.missing {
            return;
        }

        let target = WorktreeTarget {
            id: node.id.clone(),
            branch: node.branch.clone(),
            label: node.label(),
            path: node.path.display().to_string().into(),
            token_reduction: node.token_reduction,
            agent: node.agent.clone(),
            pinned: node.pinned,
            // The menu's anchor. A dialog is centred, so nothing reads this.
            at: gpui::Point::default(),
        };

        let Ok(workspace) = Workspace::open() else {
            return;
        };
        let confirm = workspace.config().merge.confirm;
        let base = workspace
            .worktrees(None)
            .ok()
            .and_then(|worktrees| {
                worktrees
                    .into_iter()
                    .find(|w| w.id == target.id)
                    .map(|w| w.base)
            })
            .unwrap_or_default();

        if !confirm {
            self.confirm_merge_worktree = None;
            self.merge_worktree_confirmed(false, cx);
            return;
        }

        let status = ket_core::status::of_worktree(&node.path, None).ok();
        let changes = status.as_ref().map_or(0, |status| status.total_changes);
        let paths = status
            .map(|status| {
                status
                    .files
                    .iter()
                    .take(LISTED_PATHS)
                    .map(|file| SharedString::from(file.path.clone()))
                    .collect()
            })
            .unwrap_or_default();

        self.confirm_merge_worktree = Some(MergeWorktreeConfirm {
            target,
            base: base.into(),
            changes,
            paths,
            refusal: None,
            waivable: false,
            outcome: None,
            offer_delete: false,
            primary,
            agent_tip: false,
        });
        cx.notify();
    }

    /// "Merge with agent": the refused merge, handed to the worktree's agent
    /// to finish the way a person would — commit, merge, and work around
    /// whatever made ket refuse.
    fn merge_with_agent(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_merge_worktree.take() else {
            return;
        };
        let branch = &confirm.target.branch;
        let base = &confirm.base;
        let primary = &confirm.primary;
        let refusal = confirm
            .refusal
            .as_ref()
            .map_or("ket could not complete the automatic merge", |why| {
                why.as_ref()
            });
        let prompt = format!(
            "Finish merging this worktree on `{branch}` into `{base}` in the primary checkout \
             at {primary}.\n\n\
             ket's automatic merge stopped with this error:\n{refusal}\n\n\
             Inspect the repository state, commit any remaining work on `{branch}`, and complete \
             the merge. Preserve unrelated uncommitted changes in the primary checkout; stash and \
             restore them if necessary. Resolve conflicts by reading both sides rather than taking \
             one wholesale, do not discard changes, and summarize what you changed."
        );
        self.toast(
            crate::ui::toast::Tone::Info,
            format!("Asked {} to merge into {base}", confirm.target.label),
            cx,
        );
        self.send_prompt_to_worktree(confirm.target.id.clone(), prompt, cx);
        cx.notify();
    }

    /// Runs the merge, off the window's thread.
    ///
    /// A commit runs the repository's own pre-commit hook and a merge walks
    /// history; neither is slow enough to notice usually and both are slow
    /// enough to freeze a window sometimes. Blocking the window on git is a
    /// mistake the status tick made once, and this one does not need to copy.
    pub(crate) fn merge_worktree_confirmed(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(id) = self
            .confirm_merge_worktree
            .as_ref()
            .map(|confirm| confirm.target.id.clone())
            .or_else(|| self.selected_id())
        else {
            return;
        };
        self.merge_worktree_now(id, force, cx);
    }

    /// Merges worktree `id` — the confirm dialog's target, or one a phone
    /// asked for — with ket's own merge.
    pub(crate) fn merge_worktree_now(
        &mut self,
        id: WorktreeId,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.merging.is_some() {
            return;
        }
        self.merging = Some(id.clone());
        if let Some(confirm) = self.confirm_merge_worktree.as_mut() {
            confirm.refusal = None;
            confirm.outcome = None;
        }

        // `Workspace` is opened inside the task rather than passed in: it owns
        // a store handle and a mutex of live sessions, and every other handler
        // in the shell opens its own.
        let work = cx.background_executor().spawn(async move {
            Workspace::open()
                .map_err(MergeError::from)
                .and_then(|workspace| workspace.merge_worktree(&id, force))
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = work.await;
            let _ = shell.update(cx, |shell, cx| shell.finish_merge(result, cx));
        })
        .detach();
        cx.notify();
    }

    /// Puts the result where it can be read.
    fn finish_merge(
        &mut self,
        result: std::result::Result<MergeReport, MergeError>,
        cx: &mut Context<Self>,
    ) {
        self.merging = None;

        match result {
            Ok(report) => {
                let committed = report
                    .committed
                    .as_ref()
                    .is_some_and(|c| c.commit.is_some());
                // A merge that found the branch already in the base and had
                // nothing to commit changed nothing at all. Saying so is not
                // optional: a button that does nothing and reports nothing is
                // indistinguishable from a broken one.
                if report.already_merged && !committed && !report.removed {
                    let outcome = format!("Already in {}. Nothing to commit.", report.into);
                    // Already in the base and nothing to commit: finished, and
                    // still taking up the disk. Worth offering to reclaim.
                    self.open_merge_outcome(&report, outcome, true, cx);
                    return;
                }
                if let Some(why) = report.remove_failed.clone() {
                    let outcome = format!("The checkout couldn't be removed: {why}");
                    // Removal was asked for and has just failed. Offering it
                    // again in the same breath would be the dialog arguing.
                    self.open_merge_outcome(&report, outcome, false, cx);
                    return;
                }
                // A merge that landed is not worth a dialog to dismiss, but
                // it is worth a word: the row's rail going clean is easy to
                // miss from wherever the reader is looking. Deleting the
                // checkout is the row's menu, as it is for any other worktree.
                self.confirm_merge_worktree = None;
                self.toast_detail(
                    Tone::Success,
                    format!("Merged into {}", report.into),
                    report.branch.clone(),
                    cx,
                );
                self.reload(cx);
            }
            Err(error) => {
                let waivable = matches!(error, MergeError::Waivable(_));
                let why = SharedString::from(error.to_string());
                match self.confirm_merge_worktree.as_mut() {
                    Some(confirm) => {
                        confirm.refusal = Some(why);
                        confirm.waivable = waivable;
                        confirm.outcome = None;
                    }
                    // Refused with the dialog turned off, so there is nowhere
                    // to put this but a dialog opened to hold it.
                    None => self.open_merge_refusal(why, waivable, cx),
                }
            }
        }
        cx.notify();
    }

    /// Keeps the dialog open to report something a row cannot.
    fn open_merge_outcome(
        &mut self,
        report: &MergeReport,
        outcome: String,
        offer_delete: bool,
        cx: &mut Context<Self>,
    ) {
        self.reload(cx);
        match self.confirm_merge_worktree.as_mut() {
            Some(confirm) => {
                confirm.refusal = None;
                confirm.outcome = Some(outcome.into());
                confirm.offer_delete = offer_delete;
            }
            None => {
                let Some(at) = self.position_of(&report.worktree) else {
                    return;
                };
                self.request_merge_worktree(at.project, at.worktree, cx);
                if let Some(confirm) = self.confirm_merge_worktree.as_mut() {
                    confirm.outcome = Some(outcome.into());
                    confirm.offer_delete = offer_delete;
                }
            }
        }
    }

    /// The same, for a refusal that arrived with no dialog open.
    fn open_merge_refusal(&mut self, why: SharedString, waivable: bool, cx: &mut Context<Self>) {
        let Some(id) = self.merging.clone().or_else(|| self.selected_id()) else {
            return;
        };
        let Some(at) = self.position_of(&id) else {
            return;
        };
        self.request_merge_worktree(at.project, at.worktree, cx);
        if let Some(confirm) = self.confirm_merge_worktree.as_mut() {
            confirm.refusal = Some(why);
            confirm.waivable = waivable;
        }
    }

    /// Handles a key while the merge dialog is open.
    ///
    /// Enter never escalates on its own: a refusal has to be read before it is
    /// waived, which is the rule the delete dialog keeps for the same reason.
    pub(crate) fn merge_worktree_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.confirm_merge_worktree.is_none() {
            return false;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.confirm_merge_worktree = None;
                cx.notify();
            }
            "enter" if self.merging.is_none() => self.merge_worktree_confirmed(false, cx),
            _ => {}
        }
        true
    }

    /// The merge dialog, or nothing when it is closed.
    pub(crate) fn merge_worktree_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm_merge_worktree.as_ref()?;
        let t = &self.theme;
        let running = self.merging.is_some();
        // Only a waivable refusal offers to go again. Everything else that can
        // refuse here is unfixable by a flag.
        let refused = confirm.refusal.is_some();
        let escalated = refused && confirm.waivable;
        let finished = confirm.outcome.is_some();

        let close = icon_button("close-merge-worktree", Icon::Close)
            .bare()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm_merge_worktree = None;
                cx.notify();
            }));
        let cancel = button(
            "cancel-merge-worktree",
            if finished { "Close" } else { "Cancel" },
        )
        .variant(if finished {
            Variant::Secondary
        } else {
            Variant::Ghost
        })
        .render(t)
        .on_click(cx.listener(|this, _, _, cx| {
            this.confirm_merge_worktree = None;
            cx.notify();
        }));

        // Primary rather than danger: merging is constructive, and the row it
        // acts on stays unless the settings say otherwise.
        let confirm_button = (!finished).then(|| {
            button(
                "confirm-merge-worktree",
                match (running, escalated) {
                    (true, _) => "Merging\u{2026}",
                    (false, true) => "Merge anyway",
                    (false, false) => "Merge",
                },
            )
            .primary()
            .leading(Icon::GitMerge)
            .enabled(!running && (confirm.refusal.is_none() || escalated))
            .render(t)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.merge_worktree_confirmed(escalated, cx);
                cx.notify();
            }))
        });

        // A refusal the automatic path cannot safely get past can still be
        // handed to the worktree's agent, which gets the exact error and can
        // inspect both sides before deciding how to finish the merge.
        let agent_button = refused.then(|| {
            tooltip(
                "merge-with-agent-tip",
                button("merge-with-agent", "Merge with agent")
                    .enabled(!running)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| this.merge_with_agent(cx)))
                    .into_any_element(),
                AGENT_TIP,
                Side::Top,
                confirm.agent_tip,
                t,
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if let Some(confirm) = this.confirm_merge_worktree.as_mut() {
                    confirm.agent_tip = *hovered;
                }
                cx.notify();
            }))
        });

        // Offered, never assumed. The merge dialog is where someone already
        // is when the work lands, so putting the reclaim here is the
        // difference between one click now and a checkout nobody revisits.
        let reclaim = confirm.offer_delete.then(|| {
            let target = confirm.target.clone();
            button("reclaim-merged-worktree", "Delete")
                .danger()
                .leading(Icon::Trash)
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_merge_worktree = None;
                    // Handed to the ordinary delete dialog rather than run
                    // from here: that dialog is the one place that states what
                    // removing a worktree does, and the only one that knows
                    // how to ask again when git refuses.
                    this.request_remove_worktree(target.clone(), cx);
                    cx.notify();
                }))
        });

        // The top is what is about to happen, in order: the commit first,
        // because it is the step people forget this does, then the merge. A
        // refusal hangs off the step it stopped, so it reads as "this is
        // where it stopped" rather than as a notice about the dialog.
        let mono = crate::fonts::chrome();
        let done = paint(t.status.running);
        let tone = if confirm.waivable {
            Tone::Warning
        } else {
            Tone::Error
        };
        let node = |which: Icon, glyph: gpui::Rgba, ring: gpui::Rgba, fill: gpui::Rgba| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(STEP_NODE)
                .rounded_full()
                .border_1()
                .border_color(ring)
                .bg(fill)
                .child(sized_icon(which, px(12.0), glyph))
        };
        let phrase = |words: &str, name: SharedString, colour: gpui::Rgba| {
            div()
                .flex()
                .min_w_0()
                .gap(px(5.0))
                .text_size(STEP_TEXT)
                .font_weight(FontWeight::MEDIUM)
                .text_color(colour)
                .child(SharedString::from(words.to_owned()))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(mono.clone())
                        .child(name),
                )
        };
        let step = |node: gpui::Div, title: gpui::Div, trunk: bool, under: Option<AnyElement>| {
            div()
                .flex()
                .gap(LEAD_GAP)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .w(STEP_NODE)
                        .gap(px(2.0))
                        .child(node)
                        .when(trunk, |el| {
                            el.child(div().flex_grow().w(px(1.5)).bg(paint(t.border)))
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .pt(px(4.0))
                        .when(trunk, |el| el.pb(px(14.0)))
                        .child(title)
                        .children(under),
                )
        };
        let clear: gpui::Rgba = gpui::transparent_black().into();

        let commit = step(
            if finished {
                node(Icon::Check, done, alpha(done, 0.45), clear)
            } else {
                node(
                    Icon::GitCommit,
                    paint(t.text.primary),
                    paint(t.border),
                    clear,
                )
            },
            {
                let verb = if finished { "Committed" } else { "Commit" };
                let (words, colour) = match confirm.changes {
                    0 => ("Nothing to commit on".to_owned(), paint(t.text.dim)),
                    1 => (format!("{verb} 1 change to"), paint(t.text.primary)),
                    many => (format!("{verb} {many} changes to"), paint(t.text.primary)),
                };
                phrase(&words, confirm.target.branch.clone(), colour)
            },
            true,
            None,
        );

        let under_merge = match (&confirm.refusal, &confirm.outcome) {
            (Some(why), _) => {
                Some(banner("merge-refusal", tone, why.clone(), t).into_any_element())
            }
            (None, Some(outcome)) => Some(
                div()
                    .text_size(LABEL)
                    .text_color(paint(t.text.dim))
                    .child(outcome.clone())
                    .into_any_element(),
            ),
            (None, None) => None,
        };
        let merge = step(
            match (finished, refused) {
                (true, _) => node(Icon::Check, done, alpha(done, 0.45), clear),
                (false, true) => {
                    let colour = tone.colour(t);
                    node(Icon::GitMerge, colour, colour, alpha(colour, 0.12))
                }
                (false, false) => node(
                    Icon::GitMerge,
                    paint(t.text.primary),
                    paint(t.border),
                    clear,
                ),
            },
            phrase(
                if finished {
                    "Merged into"
                } else {
                    "Merge into"
                },
                confirm.base.clone(),
                paint(t.text.primary),
            ),
            false,
            under_merge,
        );

        let top = div()
            .flex()
            .flex_col()
            .px(LIST_INSET)
            .child(commit)
            .child(merge);

        // What `git add -A` is about to sweep up, named. This is the only place
        // a person sees a secret about to be committed — see the module docs.
        let listed = (!finished && !confirm.paths.is_empty()).then(|| {
            let hidden = confirm.changes.saturating_sub(confirm.paths.len());
            let mono = self.font_family.clone();
            div()
                .flex()
                .flex_col()
                .py(px(4.0))
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(paint(t.rule))
                .text_size(FILE_TEXT)
                .children(confirm.paths.iter().map(|path| {
                    let (dir, name) = match path.rfind('/') {
                        Some(at) => (&path[..=at], &path[at + 1..]),
                        None => ("", path.as_ref()),
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(LEAD_GAP)
                        .h(FILE_ROW)
                        .px(LIST_INSET - px(1.0))
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .justify_center()
                                .w(STEP_NODE)
                                .child(file_mark(name, false, mono.clone(), t)),
                        )
                        .child(
                            div()
                                .flex()
                                .min_w_0()
                                .truncate()
                                .child(
                                    div()
                                        .text_color(paint(t.text.dim))
                                        .child(SharedString::from(dir.to_owned())),
                                )
                                .child(
                                    div()
                                        .text_color(paint(t.text.primary))
                                        .child(SharedString::from(name.to_owned())),
                                ),
                        )
                }))
                .when(hidden > 0, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .h(FILE_ROW)
                            // Under the paths, not under the marks.
                            .pl(LIST_INSET - px(1.0) + STEP_NODE + LEAD_GAP)
                            .pr(LIST_INSET - px(1.0))
                            .prose()
                            .text_color(paint(t.text.dim))
                            .child(format!("and {hidden} more")),
                    )
                })
        });

        Some(
            centered(
                card("merge-worktree", WIDTH, t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        header(if finished { "Merged" } else { "Merge worktree" }, t).child(close),
                    )
                    .child(top)
                    .children(listed)
                    .child(
                        footer()
                            .child(cancel)
                            .children(reclaim)
                            .children(agent_button)
                            .children(confirm_button),
                    ),
            )
            .into_any_element(),
        )
    }
}

/// The worktree this merge is for, when one is in flight.
///
/// A newtype would be ceremony; the field is on [`Shell`] and this alias only
/// names what it holds.
pub(crate) type Merging = Option<WorktreeId>;
