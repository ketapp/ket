//! Throwing away one file's uncommitted changes, from the Git view.
//!
//! **It asks first.** Discarding is the one thing in the change list that
//! cannot be taken back: git keeps no copy of an edit that was never
//! committed, and an untracked file has no copy anywhere. So the row's menu
//! opens a question rather than acting, and the question says what will
//! happen to *this* file — edits lost, or the file deleted outright — since
//! "discard" means a different thing for each.
//!
//! **Only uncommitted work.** The menu offers it while the view compares
//! against `HEAD`. Against the base, a row is mostly committed work, and
//! putting a file back to the base is a revert, not a discard.
//!
//! **Clicked, never Entered.** The rule the delete-worktree dialog keeps:
//! discarding work has to be clicked, so a stray Return cannot do it.
//!
//! The git side is [`ket_core::git::Git::discard`].

use std::path::PathBuf;

use gpui::{AnyElement, Context, KeyDownEvent, MouseButton, SharedString, div, prelude::*, px};
use ket_core::status::ChangeKind;

use crate::Shell;
use crate::paint::paint;
use crate::ui::button::button;
use crate::ui::dialog::{body, card, centered, footer, header};
use crate::ui::icon::Icon;
use crate::ui::toast::Tone;

/// The dialog's width.
const WIDTH: gpui::Pixels = px(420.0);

/// The "discard changes?" question for one file.
pub(crate) struct DiscardConfirm {
    /// The worktree it changed in.
    pub(crate) root: PathBuf,
    /// Slash-separated from the root.
    pub(crate) rela: SharedString,
    /// Where a renamed file came from, which a discard puts back.
    pub(crate) old_path: Option<SharedString>,
    /// How it changed, which decides what discarding it does.
    pub(crate) kind: ChangeKind,
}

impl DiscardConfirm {
    /// What discarding does to this file, in one sentence.
    fn consequence(&self) -> String {
        match (self.kind, &self.old_path) {
            (ChangeKind::Untracked, _) => {
                "Git has never tracked it, so it is deleted and cannot be recovered.".to_owned()
            }
            (ChangeKind::Added, _) => {
                "It is new since the last commit, so it is deleted. This cannot be undone."
                    .to_owned()
            }
            (ChangeKind::Deleted, _) => "It comes back as the last commit has it.".to_owned(),
            (ChangeKind::Renamed, Some(old)) => format!("It goes back to being {old}."),
            _ => "It goes back to the last commit, and its edits, staged or not, are lost. \
                  This cannot be undone."
                .to_owned(),
        }
    }
}

impl Shell {
    /// Asks before discarding `rela`'s changes in `root`.
    pub(crate) fn request_discard(
        &mut self,
        root: PathBuf,
        rela: SharedString,
        old_path: Option<SharedString>,
        kind: ChangeKind,
    ) {
        self.confirm_discard = Some(DiscardConfirm {
            root,
            rela,
            old_path,
            kind,
        });
    }

    /// Closes the dialog and discards, off the window's thread. The answer
    /// is a notification: the file's row leaves the change list once git is
    /// done, or a notification says why it couldn't be.
    fn discard_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_discard.take() else {
            return;
        };
        let root = confirm.root;
        let rela = confirm.rela;
        let mut paths = vec![rela.to_string()];
        paths.extend(confirm.old_path.as_ref().map(ToString::to_string));

        let work = cx.background_executor().spawn(async move {
            let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
            ket_core::git::Git::new(&root).discard(&paths)
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                match outcome {
                    Ok(()) => {
                        shell.toast_detail(Tone::Success, "Discarded changes", rela, cx);
                        // The watcher would get there, but only for the
                        // selected worktree; the view may be showing another.
                        shell.refresh_git_panel(cx);
                    }
                    Err(error) => shell.toast_detail(
                        Tone::Error,
                        format!("Changes to {rela} not discarded"),
                        error.to_string(),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Handles a key while the dialog is up. It takes every key; only Escape
    /// does anything. See the module docs on Enter.
    pub(crate) fn discard_key(&mut self, event: &KeyDownEvent) -> bool {
        if self.confirm_discard.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.confirm_discard = None;
        }
        true
    }

    /// The dialog, or nothing when it is closed.
    pub(crate) fn discard_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm_discard.as_ref()?;
        let t = &self.theme;

        let cancel = button("cancel-discard", "Cancel")
            .ghost()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm_discard = None;
                cx.notify();
            }));
        let discard = button("confirm-discard", "Discard")
            .danger()
            .leading(Icon::RotateCcw)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.discard_confirmed(cx);
                cx.notify();
            }));

        // The path in the code face, whole: two files of one name in
        // different directories are the case where the name alone misleads.
        let path = div()
            .font_family(self.font_family.clone())
            .text_size(px(12.0))
            .text_color(paint(t.text.primary))
            .truncate()
            .child(confirm.rela.clone());

        Some(
            centered(
                card("discard-changes", WIDTH, t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        header("close-discard", "Discard changes?")
                            .on_close(cx.listener(|this, _, _, cx| {
                                this.confirm_discard = None;
                                cx.notify();
                            }))
                            .render(t),
                    )
                    .child(path)
                    .child(body(confirm.consequence(), t))
                    .child(footer().child(cancel).child(discard)),
            )
            .into_any_element(),
        )
    }
}
