//! Merging a worktree's GitHub pull request, from the worktree menu.
//!
//! Distinct from [`crate::merge_worktree`]: that lands the branch in the
//! primary checkout with git. This asks GitHub to land the pull request,
//! with the merge method the repository allows. It always asks first: a
//! merge on GitHub is not undone from here, and a branch that is behind
//! its base may be refused.

use gpui::{AnyElement, Context, MouseButton, SharedString, WeakEntity, div, prelude::*, px};
use ket_core::checks::checkout;
use ket_core::github::{self, MergeMethod, MergeMethods};
use ket_core::id::WorktreeId;

use crate::Shell;
use crate::paint::paint;
use crate::ui::banner::banner;
use crate::ui::button::{Variant, button, icon_button};
use crate::ui::chip::caption;
use crate::ui::dialog::{card, centered, footer, header};
use crate::ui::field;
use crate::ui::group::{button_group, segment};
use crate::ui::icon::Icon;
use crate::ui::toast::Tone;
use crate::worktree_menu::WorktreeTarget;

/// The dialog's width: the open-pull-request dialog's, so they read as
/// siblings.
const WIDTH: gpui::Pixels = px(600.0);

/// The dialog, from the method it will use to GitHub's answer.
pub(crate) struct MergePullRequestConfirm {
    /// What is about to be merged.
    pub(crate) target: WorktreeTarget,
    /// The pull request's number.
    pub(crate) number: u64,
    /// The branch it asks to merge into.
    pub(crate) base: SharedString,
    /// Whether its branch is behind that base.
    pub(crate) behind: bool,
    /// How GitHub will land it.
    pub(crate) method: MergeMethod,
    /// Merge methods the repository allows.
    pub(crate) methods: MergeMethods,
    /// Why the last attempt was refused, if it was.
    pub(crate) refusal: Option<SharedString>,
}

impl Shell {
    /// Opens the dialog for `target`'s pull request.
    pub(crate) fn request_merge_pull_request(
        &mut self,
        target: WorktreeTarget,
        cx: &mut Context<Self>,
    ) {
        if self.pull_request_action.is_some() {
            return;
        }
        let Some(status) = self
            .pull_requests
            .of(&target.id)
            .filter(|status| status.can_merge())
            .cloned()
        else {
            self.toast(Tone::Error, "This pull request cannot be merged yet", cx);
            return;
        };
        let Some(method) = status.merge_methods.default_method() else {
            self.toast(Tone::Error, "This repository allows no merge methods", cx);
            return;
        };
        let id = target.id.clone();
        self.confirm_merge_pull_request = Some(MergePullRequestConfirm {
            target,
            number: status.number,
            base: status.base.clone().into(),
            behind: status.behind,
            method,
            methods: status.merge_methods,
            refusal: None,
        });
        if let Some(project) = self.project_of_worktree(&id) {
            let read = cx
                .background_executor()
                .spawn(async move { github::merge_methods(&checkout(&project)?) });
            cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
                let read = read.await;
                let _ = shell.update(cx, |shell, cx| {
                    if let Ok(methods) = read
                        && let Some(confirm) = shell.confirm_merge_pull_request.as_mut()
                        && confirm.target.id == id
                    {
                        confirm.methods = methods;
                        if !methods.allows(confirm.method)
                            && let Some(method) = methods.default_method()
                        {
                            confirm.method = method;
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Merges on GitHub, off the window's thread.
    fn merge_pull_request_confirmed(&mut self, cx: &mut Context<Self>) {
        if self.pull_request_action.is_some() {
            return;
        }
        let Some(confirm) = self.confirm_merge_pull_request.as_ref() else {
            return;
        };
        if !confirm.methods.allows(confirm.method) {
            return;
        }
        let id = confirm.target.id.clone();
        let number = confirm.number;
        let method = confirm.method;
        let base = confirm.base.clone();
        self.run_pull_request_action(
            id,
            number,
            format!("Merging #{number}\u{2026}"),
            format!("Merged #{number} into {base}"),
            cx,
            move |at| github::merge_pull_request(at, number, method),
        );
    }

    /// Updates `id`'s pull request branch from its base.
    pub(crate) fn update_pull_request(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let Some(status) = self.pull_requests.of(&id).cloned() else {
            self.toast(
                Tone::Error,
                "ket has not found a pull request for this worktree",
                cx,
            );
            return;
        };
        if !status.can_update() {
            self.toast(
                Tone::Error,
                "This pull request's branch is not behind its base",
                cx,
            );
            return;
        }
        let number = status.number;
        self.run_pull_request_action(
            id,
            number,
            format!("Updating #{number}\u{2026}"),
            format!("Updated #{number} from {}", status.base),
            cx,
            move |at| github::update_branch(at, number),
        );
    }

    /// Marks `id`'s draft pull request ready for review.
    pub(crate) fn ready_pull_request(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let Some(status) = self.pull_requests.of(&id).cloned() else {
            self.toast(
                Tone::Error,
                "ket has not found a pull request for this worktree",
                cx,
            );
            return;
        };
        if !status.can_ready() {
            self.toast(Tone::Error, "This pull request is not a draft", cx);
            return;
        }
        let number = status.number;
        self.run_pull_request_action(
            id,
            number,
            format!("Marking #{number} ready\u{2026}"),
            format!("#{number} is ready for review"),
            cx,
            move |at| github::mark_ready(at, number),
        );
    }

    /// Reopens `id`'s closed unmerged pull request.
    pub(crate) fn reopen_pull_request(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let Some(status) = self.pull_requests.of(&id).cloned() else {
            self.toast(
                Tone::Error,
                "ket has not found a pull request for this worktree",
                cx,
            );
            return;
        };
        if !status.can_reopen() {
            self.toast(Tone::Error, "This pull request is not closed", cx);
            return;
        }
        let number = status.number;
        self.run_pull_request_action(
            id,
            number,
            format!("Reopening #{number}\u{2026}"),
            format!("Reopened #{number}"),
            cx,
            move |at| github::reopen_pull_request(at, number),
        );
    }

    fn run_pull_request_action(
        &mut self,
        id: WorktreeId,
        number: u64,
        doing: String,
        done: String,
        cx: &mut Context<Self>,
        run: impl FnOnce(&github::Checkout) -> ket_core::Result<()> + Send + 'static,
    ) {
        if self.pull_request_action.is_some() {
            return;
        }
        let Some(project) = self.project_of_worktree(&id) else {
            self.toast(Tone::Error, "That worktree isn't here any more", cx);
            return;
        };
        self.pull_request_action = Some(id.clone());
        self.toast(Tone::Info, doing, cx);
        let ran = cx
            .background_executor()
            .spawn(async move { run(&checkout(&project)?) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let ran = ran.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.pull_request_action = None;
                match ran {
                    Ok(()) => {
                        if shell
                            .confirm_merge_pull_request
                            .as_ref()
                            .is_some_and(|confirm| confirm.target.id == id)
                        {
                            shell.confirm_merge_pull_request = None;
                        }
                        shell.toast(Tone::Info, done, cx);
                        shell.refresh_pull_request(id, number, cx);
                    }
                    Err(error) => {
                        if let Some(confirm) = shell.confirm_merge_pull_request.as_mut()
                            && confirm.target.id == id
                        {
                            confirm.refusal = Some(error.to_string().into());
                        } else {
                            shell.toast(Tone::Error, error.to_string(), cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Handles a key while the merge-pull-request dialog is open.
    pub(crate) fn merge_pull_request_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.confirm_merge_pull_request.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" if self.pull_request_action.is_none() => {
                self.confirm_merge_pull_request = None;
                cx.notify();
            }
            "enter" if self.pull_request_action.is_none() => {
                self.merge_pull_request_confirmed(cx);
            }
            _ => {}
        }
        true
    }

    /// The dialog, or nothing when it is closed.
    pub(crate) fn merge_pull_request_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm_merge_pull_request.as_ref()?;
        let t = &self.theme;
        let pending = self
            .pull_request_action
            .as_ref()
            .is_some_and(|id| *id == confirm.target.id);
        let number = confirm.number;
        let base = confirm.base.clone();
        let behind = confirm.behind;
        let method = confirm.method;
        let methods = confirm.methods;
        let refusal = confirm.refusal.clone();
        let allowed = methods.allowed();

        let close = icon_button("close-merge-pull-request", Icon::Close)
            .bare()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                if this.pull_request_action.is_some() {
                    return;
                }
                this.confirm_merge_pull_request = None;
                cx.notify();
            }));
        let cancel = button("cancel-merge-pull-request", "Cancel")
            .variant(Variant::Ghost)
            .enabled(!pending)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                if this.pull_request_action.is_some() {
                    return;
                }
                this.confirm_merge_pull_request = None;
                cx.notify();
            }));
        let confirm_button = button("confirm-merge-pull-request", format!("Merge #{number}"))
            .primary()
            .leading(Icon::GitMerge)
            .hint("\u{2318}\u{21b5}")
            .loading_if(pending, "Merging\u{2026}")
            .enabled(!pending && methods.allows(method))
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.merge_pull_request_confirmed(cx);
                cx.notify();
            }));

        let picker = (!allowed.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(field::label("How GitHub lands it", t))
                .child(
                    button_group("merge-pull-request-method")
                        .fill()
                        .children(allowed.into_iter().map(|choice| {
                            segment(
                                (
                                    "merge-pull-request-method",
                                    match choice {
                                        MergeMethod::Merge => 0usize,
                                        MergeMethod::Squash => 1,
                                        MergeMethod::Rebase => 2,
                                    },
                                ),
                                choice.label(),
                            )
                            .selected(choice == method)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(confirm) = this.confirm_merge_pull_request.as_mut()
                                    {
                                        confirm.method = choice;
                                    }
                                    cx.notify();
                                },
                            ))
                        }))
                        .render(t),
                )
                .into_any_element()
        });

        Some(
            centered(
                card("merge-pull-request", WIDTH, t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(header("Merge pull request", t).child(close))
                    .child(
                        caption(
                            format!(
                                "Merge #{number} into {base} with a {}.",
                                method.name()
                            ),
                            t,
                        )
                        .text_color(paint(t.text.dim)),
                    )
                    .children(behind.then(|| {
                        banner(
                            "merge-pull-request-behind",
                            Tone::Warning,
                            format!(
                                "This branch is behind {base}. GitHub may refuse until you update it."
                            ),
                            t,
                        )
                    }))
                    .children(picker)
                    .children(refusal.map(|why| {
                        banner("merge-pull-request-refused", Tone::Error, why, t)
                    }))
                    .child(footer().child(cancel).child(confirm_button)),
            )
            .into_any_element(),
        )
    }
}

/// Whether a GitHub pull-request action is running for a worktree.
pub(crate) type PullRequestAction = Option<WorktreeId>;
