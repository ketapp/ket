//! Pushing a worktree's branch and opening a pull request for it, from one
//! dialog.
//!
//! The other way out of a worktree, beside [`crate::merge_worktree`]: the
//! same commit, and then the work goes to the remote as a pull request
//! instead of into the primary checkout.
//! [`ket_core::workspace::Workspace::open_pull_request`] is the operation;
//! this is the surface over it.
//!
//! **It asks first, always.** A merge can be configured to go without a
//! dialog; this cannot, because a pull request has words on it — a title
//! and a description — and those are offered, not decided. The title comes
//! from the backlog note the branch was started from, or the branch name
//! made readable; the description names the issue the branch closes. Both
//! are there to be edited before the button is pressed.
//!
//! **It refuses, and never forces.** Nothing here is waivable: a conflict,
//! a half-finished rebase, a repository with no remote, a push the remote
//! rejects. Each is a banner under the fields and a dialog left open with
//! the words still in it, so a second try costs nothing.
//!
//! **The dialog is the receipt.** A merge's result is visible on the row;
//! a pull request's lives on GitHub, so the dialog stays open on success
//! with the number and a link, and the row grows a mark that opens it
//! again later.

use gpui::{
    AnyElement, Context, Entity, KeyDownEvent, MouseButton, SharedString, Window, div, prelude::*,
    px,
};
use ket_core::backlog::Backlog;
use ket_core::id::WorktreeId;
use ket_core::workspace::{PullRequestDraft, PullRequestOptions, PullRequestReport, Workspace};

use crate::Shell;
use crate::input::{Style as InputStyle, TextInput, is_text, text_field};
use crate::merge_worktree::LISTED_PATHS;
use crate::paint::paint;
use crate::ui::LABEL;
use crate::ui::banner::banner;
use crate::ui::button::{Variant, button, icon_button};
use crate::ui::chip::caption;
use crate::ui::dialog::{card, centered, footer, header};
use crate::ui::field;
use crate::ui::icon::Icon;
use crate::ui::textarea::textarea;
use crate::ui::toast::Tone;
use crate::ui::toggle::checkbox;
use crate::worktree_menu::WorktreeTarget;

/// The composer the description is written in.
const BODY_EDITOR: &str = "pull-request-body";

/// The dialog's width: the merge dialog's, so the two read as siblings.
const WIDTH: gpui::Pixels = px(600.0);

/// The description's well, and the size it is set in.
const BODY_HEIGHT: gpui::Pixels = px(120.0);
const BODY_TEXT: gpui::Pixels = px(13.0);

/// The dialog, from the first draft of its words to the link it ends on.
pub(crate) struct PullRequestConfirm {
    /// What is about to be pushed. Carried by value, as the merge dialog's
    /// is, because the sidebar is rebuilt on every reload.
    pub(crate) target: WorktreeTarget,
    /// The facts and the first draft of the words, read when it opened.
    pub(crate) draft: PullRequestDraft,
    /// The first [`LISTED_PATHS`] uncommitted paths: what the commit takes.
    pub(crate) paths: Vec<SharedString>,
    /// The title, being edited.
    pub(crate) title: Entity<TextInput>,
    /// Whether to open it as a draft pull request.
    pub(crate) draft_pr: bool,
    /// Whether the description, rather than the title, has the keyboard.
    pub(crate) body_active: bool,
    /// Why the last attempt was refused, if it was.
    pub(crate) refusal: Option<SharedString>,
    /// What it did, once it has.
    pub(crate) outcome: Option<PullRequestReport>,
}

impl Shell {
    /// Opens the dialog for `target`.
    ///
    /// Everything read here is local: the draft is words and names, and the
    /// path list is the same in-process `gix` read the merge dialog makes.
    /// The remote is named, not asked, until the button is pressed.
    pub(crate) fn request_pull_request(&mut self, target: WorktreeTarget, cx: &mut Context<Self>) {
        // One at a time, for the same reason as a merge: two pushes of one
        // branch at once is two processes writing the same refs.
        if self.opening_pull_request.is_some() {
            return;
        }
        let Some(at) = self.position_of(&target.id) else {
            return;
        };
        let Some(node) = self
            .projects
            .get(at.project)
            .and_then(|project| project.worktrees.get(at.worktree))
        else {
            return;
        };
        // The primary checkout is the base a pull request asks to merge
        // into; it cannot also be the head. The menu disables the item for
        // it, and nothing else reaches here.
        if node.primary || node.missing {
            return;
        }
        let path = node.path.clone();

        let Ok(workspace) = Workspace::open() else {
            return;
        };
        let project = workspace.worktrees(None).ok().and_then(|worktrees| {
            worktrees
                .into_iter()
                .find(|w| w.id == target.id)
                .map(|w| w.project_id)
        });
        // The backlog is where a title worth offering lives. A project
        // without one, or one that cannot be read, costs only the better
        // title: the branch is still there to name the pull request.
        let notes = project
            .and_then(|project| Backlog::load(&project).ok())
            .map(|backlog| backlog.notes)
            .unwrap_or_default();
        let draft = match workspace.pull_request_draft(&target.id, &notes) {
            Ok(draft) => draft,
            Err(error) => {
                self.toast(Tone::Error, error.to_string(), cx);
                return;
            }
        };

        let paths = ket_core::status::of_worktree(&path, None)
            .ok()
            .map(|status| {
                status
                    .files
                    .iter()
                    .take(LISTED_PATHS)
                    .map(|file| SharedString::from(file.path.clone()))
                    .collect()
            })
            .unwrap_or_default();

        // The title is selected whole, as a field opened with a value is:
        // the draft is an offer, and typing replaces it.
        let title = TextInput::with_text("Title", &draft.title, cx);
        title.update(cx, |input, _| input.request_focus());
        self.watch_field(&title, cx);
        self.open_composer(BODY_EDITOR, &draft.body);
        self.set_composer_size(BODY_EDITOR, BODY_TEXT);

        self.confirm_pull_request = Some(PullRequestConfirm {
            target,
            draft,
            paths,
            title,
            draft_pr: false,
            body_active: false,
            refusal: None,
            outcome: None,
        });
        cx.notify();
    }

    /// Puts the dialog away, words and all.
    fn close_pull_request(&mut self) {
        self.close_composer(BODY_EDITOR);
        self.confirm_pull_request = None;
    }

    /// Runs the push and the pull request, off the window's thread.
    ///
    /// A push talks to the network and a commit runs the repository's own
    /// hooks; neither belongs on the thread that draws the window.
    fn pull_request_confirmed(&mut self, cx: &mut Context<Self>) {
        if self.opening_pull_request.is_some() {
            return;
        }
        let body = self.composer_text(BODY_EDITOR);
        let Some(confirm) = self.confirm_pull_request.as_mut() else {
            return;
        };
        if confirm.outcome.is_some() {
            return;
        }
        let title = confirm.title.read(cx).text();
        if title.trim().is_empty() {
            confirm.refusal = Some("A pull request needs a title.".into());
            cx.notify();
            return;
        }
        confirm.refusal = None;
        let id = confirm.target.id.clone();
        let options = PullRequestOptions {
            title,
            body,
            draft: confirm.draft_pr,
        };
        self.opening_pull_request = Some(id.clone());

        // `Workspace` is opened inside the task rather than passed in, as
        // every other handler in the shell does: it owns a store handle and
        // a mutex of live sessions.
        let work = cx.background_executor().spawn(async move {
            Workspace::open().and_then(|workspace| workspace.open_pull_request(&id, &options))
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = work.await;
            let _ = shell.update(cx, |shell, cx| shell.finish_pull_request(result, cx));
        })
        .detach();
        cx.notify();
    }

    /// Puts the result where it can be read.
    fn finish_pull_request(
        &mut self,
        result: ket_core::Result<PullRequestReport>,
        cx: &mut Context<Self>,
    ) {
        self.opening_pull_request = None;
        match result {
            Ok(report) => {
                // The row's mark is the lasting receipt; the toast is for
                // whoever looked away while it pushed.
                self.toast_detail(
                    Tone::Success,
                    format!("Pushed {} to {}", report.branch, report.remote),
                    format!("#{}", report.pull_request.number),
                    cx,
                );
                if let Some(confirm) = self.confirm_pull_request.as_mut() {
                    confirm.refusal = None;
                    confirm.outcome = Some(report);
                }
                self.reload(cx);
            }
            Err(error) => {
                if let Some(confirm) = self.confirm_pull_request.as_mut() {
                    confirm.refusal = Some(error.to_string().into());
                }
            }
        }
        cx.notify();
    }

    /// Handles a key while the dialog is open. Returns whether it was taken.
    ///
    /// Every key is taken while the dialog is up: a character typed at a
    /// title must not also drive the shell behind it. ⌘↵ pushes; Enter alone
    /// does not, because the description is a composer and Enter there is a
    /// new line. Tab moves between the title and the description.
    pub(crate) fn pull_request_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(confirm) = self.confirm_pull_request.as_ref() else {
            return false;
        };
        // Nothing moves while it pushes: not the fields, not Cancel.
        if self.opening_pull_request.is_some() {
            return true;
        }
        let key = &event.keystroke;
        let finished = confirm.outcome.is_some();
        if key.key == "escape" || (finished && key.key == "enter") {
            self.close_pull_request();
            return true;
        }
        if finished {
            return true;
        }
        let command = key.modifiers.platform || key.modifiers.control;
        if key.key == "enter" && command {
            self.pull_request_confirmed(cx);
            return true;
        }

        let title = confirm.title.clone();
        if title.read(cx).is_focused(window) {
            // Text is never taken here. It has to keep travelling until
            // macOS's input context sees it — see [`crate::input`].
            if is_text(key) || title.update(cx, |input, cx| input.key(key, cx)) {
                return true;
            }
            if key.key == "tab" {
                if let Some(confirm) = self.confirm_pull_request.as_mut() {
                    confirm.body_active = true;
                }
                window.focus(&self.focus);
            }
            return true;
        }
        if confirm.body_active && self.focus.is_focused(window) {
            if key.key == "tab" {
                if let Some(confirm) = self.confirm_pull_request.as_mut() {
                    confirm.body_active = false;
                }
                title.update(cx, |input, _| input.select_all());
                window.focus(title.read(cx).focus_handle());
            } else {
                self.composer_key(BODY_EDITOR, key, cx);
            }
        }
        true
    }

    /// The dialog, or nothing when it is closed.
    pub(crate) fn pull_request_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirm = self.confirm_pull_request.as_ref()?;
        let pending = self.opening_pull_request.is_some();
        let finished = confirm.outcome.is_some();
        let body_active = confirm.body_active && self.focus.is_focused(window);
        let title = confirm.title.clone();
        let draft_pr = confirm.draft_pr;
        let refusal = confirm.refusal.clone();
        let outcome = confirm.outcome.clone();
        let branch = confirm.target.branch.clone();
        let base: SharedString = confirm.draft.base.clone().into();
        let remote: SharedString = confirm.draft.remote.clone().unwrap_or_default().into();
        let existing = confirm.draft.existing.clone();
        let changes = confirm.draft.changes;
        let paths = confirm.paths.clone();

        self.set_composer_caret(BODY_EDITOR, body_active);
        let body_empty = self.composer_text(BODY_EDITOR).is_empty();
        let editor = (!finished).then(|| self.editor_pane(BODY_EDITOR, None, window, cx));
        let listed = (!finished)
            .then(|| self.committed_paths_list(changes, &paths))
            .flatten();
        let t = &self.theme;

        let close = icon_button("close-pull-request", Icon::Close)
            .bare()
            .render(t)
            .on_click(cx.listener(move |this, _, _, cx| {
                // Held still while it pushes, like everything else here.
                if pending {
                    return;
                }
                this.close_pull_request();
                cx.notify();
            }));
        let cancel = button(
            "cancel-pull-request",
            if finished { "Close" } else { "Cancel" },
        )
        .variant(if finished {
            Variant::Secondary
        } else {
            Variant::Ghost
        })
        .enabled(!pending)
        .render(t)
        .on_click(cx.listener(|this, _, _, cx| {
            this.close_pull_request();
            cx.notify();
        }));

        // Primary: opening a pull request is constructive, and the one
        // thing this dialog exists for.
        let confirm_button = (!finished).then(|| {
            let label = match &existing {
                Some(open) => format!("Push to #{}", open.number),
                None => "Push and open".to_owned(),
            };
            button("confirm-pull-request", label)
                .primary()
                .leading(Icon::GitHub)
                .hint("\u{2318}\u{21b5}")
                .loading_if(pending, "Pushing\u{2026}")
                .enabled(!pending)
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pull_request_confirmed(cx);
                    cx.notify();
                }))
        });

        // What is about to happen, in one line, before the words for it.
        let plan: SharedString = match (&existing, changes) {
            (Some(open), 0) => format!("Push {branch} to {remote}. #{} is open for it.", open.number),
            (Some(open), _) => format!(
                "Commit, then push {branch} to {remote}. #{} is open for it.",
                open.number
            ),
            (None, 0) => format!("Push {branch} to {remote} and open a pull request into {base}."),
            (None, 1) => format!(
                "Commit 1 change, push {branch} to {remote} and open a pull request into {base}."
            ),
            (None, many) => format!(
                "Commit {many} changes, push {branch} to {remote} and open a pull request into {base}."
            ),
        }
        .into();

        let form = (!finished).then(|| {
            let title_field = text_field(
                &title,
                "pull-request-title",
                false,
                InputStyle::new(t, self.caret.visible),
                window,
                cx,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(confirm) = this.confirm_pull_request.as_mut() {
                    confirm.body_active = false;
                }
                cx.notify();
            }));

            let mut description = textarea(
                "pull-request-description",
                editor.expect("the editor is built whenever the form is"),
                BODY_HEIGHT,
            )
            .active(body_active);
            if body_empty {
                description = description.placeholder("What this changes, and why.", BODY_TEXT);
            }
            let description =
                description
                    .render(t)
                    .cursor_text()
                    .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                        if let Some(confirm) = this.confirm_pull_request.as_mut() {
                            confirm.body_active = true;
                        }
                        window.focus(&this.focus);
                        cx.notify();
                    }));

            let draft_box = checkbox("pull-request-draft", draft_pr, "Open as a draft", t)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(confirm) = this.confirm_pull_request.as_mut() {
                        confirm.draft_pr = !confirm.draft_pr;
                    }
                    cx.notify();
                }));

            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(caption(plan.clone(), t))
                .children(listed)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(field::label("Title", t))
                        .child(title_field),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(field::label("Description", t))
                        .child(description),
                )
                .child(div().ml(px(-6.0)).child(draft_box))
        });

        // The receipt: the number, and the way to it. A toast cannot hold a
        // link, which is why the dialog stays up to hold this one.
        let receipt = outcome.map(|report| {
            let number = report.pull_request.number;
            let url = report.pull_request.url.clone();
            let words: SharedString = if report.existed {
                format!(
                    "{} is pushed to {}, and #{number} was already open for it.",
                    report.branch, report.remote
                )
                .into()
            } else {
                format!(
                    "{} is pushed to {}, and #{number} is open into {}.",
                    report.branch, report.remote, report.base
                )
                .into()
            };
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(LABEL)
                        .text_color(paint(t.text.primary))
                        .child(words),
                )
                .child(
                    div().child(
                        button("open-pull-request", format!("Open #{number} on GitHub"))
                            .link()
                            .trailing(Icon::ExternalLink)
                            .render(t)
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
                )
        });

        let refused = refusal.map(|why| banner("pull-request-refusal", Tone::Error, why, t));

        Some(
            centered(
                card("pull-request", WIDTH, t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        header(
                            if finished {
                                "Pull request open"
                            } else {
                                "Open a pull request"
                            },
                            t,
                        )
                        .child(close),
                    )
                    .children(form)
                    .children(receipt)
                    .children(refused)
                    .child(footer().child(cancel).children(confirm_button)),
            )
            .into_any_element(),
        )
    }
}

/// The worktree whose pull request is in flight, when one is.
///
/// The field is on [`Shell`], as [`crate::merge_worktree::Merging`] is, and
/// this alias only names what it holds.
pub(crate) type OpeningPullRequest = Option<WorktreeId>;
