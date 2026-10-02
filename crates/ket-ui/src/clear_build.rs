//! Reclaiming a worktree's build output.
//!
//! The gentler half of cleaning up. Deleting a worktree costs the branch and
//! the checkout; clearing its build output costs a rebuild, and for a Rust
//! checkout it is most of the disk either way — eight gigabytes of `target/`
//! against a couple of hundred megabytes of actual source. So this is the
//! action to reach for on a worktree somebody is still using.
//!
//! **It asks first, and it names what it would remove.** The whole claim this
//! action rests on is that everything it deletes is regenerable, and that
//! claim is made by a list in a config file — see
//! `ket_core::config::StorageConfig`. A list can be wrong, so the reader is
//! shown the directories, and what they add up to, before anything happens
//! rather than being asked to trust it.
//!
//! The plan is computed off the window's thread and *kept*: what gets deleted
//! is exactly what was shown. Recomputing it at the last moment would risk
//! removing a directory that appeared in between and that nobody named.

use std::ops::Range;

use gpui::{
    AnyElement, Context, FontWeight, HighlightStyle, KeyDownEvent, MouseButton, SharedString,
    StyledText, div, prelude::*, px, relative,
};
use ket_core::storage::{Clearable, format_bytes};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::ui::button::button;
use crate::ui::dialog::{card, centered, footer, header};
use crate::ui::icon::Icon;
use crate::ui::toast::Tone;
use crate::ui::{CAPTION, LABEL, RADIUS_SM};
use crate::worktree_menu::WorktreeTarget;

/// How many directories the dialog names before it stops listing them.
const LISTED: usize = 6;

/// The "are you sure" step before build output is removed.
pub(crate) struct ClearBuildConfirm {
    /// Which worktree this is about.
    pub(crate) target: WorktreeTarget,
    /// Exactly what would be removed, largest first. Kept rather than
    /// recomputed — see the module docs.
    pub(crate) plan: Vec<Clearable>,
    /// What that adds up to.
    pub(crate) total: u64,
    /// Why an attempt failed, if one did.
    pub(crate) failure: Option<SharedString>,
    /// Set while a delete is running, so the button can say so and a second
    /// Enter cannot start the same plan again.
    pub(crate) clearing: bool,
    /// What earlier attempts on this dialog already reclaimed. Carried so a
    /// retry after a half-failure reports the whole of what went rather than
    /// only the last handful of directories.
    pub(crate) freed: u64,
}

impl Shell {
    /// Works out what clearing this worktree would remove, then asks.
    ///
    /// Off the window's thread: finding the directories means walking the
    /// checkout, which is the same walk that makes sizes expensive.
    pub(crate) fn request_clear_build(&mut self, target: WorktreeTarget, cx: &mut Context<Self>) {
        let id = target.id.clone();

        let work = cx.background_executor().spawn(async move {
            let workspace = Workspace::open().ok()?;
            let worktree = workspace
                .worktrees(None)
                .ok()?
                .into_iter()
                .find(|w| w.id == id)?;
            let dirs = workspace.build_dirs(&worktree.project_id);
            Some(ket_core::storage::plan_clear(&worktree.path, &dirs))
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let planned = work.await;
            let _ = shell.update(cx, |shell, cx| {
                match planned {
                    Some(plan) if !plan.is_empty() => {
                        let total = plan.iter().map(|item| item.bytes).sum();
                        shell.confirm_clear_build = Some(ClearBuildConfirm {
                            target,
                            plan,
                            total,
                            failure: None,
                            clearing: false,
                            freed: 0,
                        });
                    }
                    // Said rather than swallowed. A menu item that opens
                    // nothing is indistinguishable from one that is broken.
                    _ => shell.toast_detail(
                        Tone::Info,
                        "Nothing to clear",
                        target.branch.clone(),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Removes the planned directories.
    ///
    /// Several gigabytes of small files take seconds to unlink, so the dialog
    /// stays up and the button says what is happening rather than sitting
    /// there looking unpressed. It closes on a clean result and not before:
    /// a dialog that vanishes the moment it is clicked has told the reader
    /// only that the click registered.
    pub(crate) fn clear_build_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_clear_build.as_mut() else {
            return;
        };
        // Enter is wired to this as well as the button, and the plan is a list
        // of `remove_dir_all` calls. Running it twice over is at best wasted
        // work on a tree that is already going.
        if confirm.clearing {
            return;
        }
        confirm.clearing = true;
        confirm.failure = None;

        // The id comes from the dialog, never from `worktree_target`: that
        // field is set by the sidebar's context menu, and the storage card's
        // own Clear button reaches this dialog without going through it. Read
        // from there, a clear started from the card remeasured nothing — or,
        // worse, whichever worktree the last context menu left behind.
        let id = confirm.target.id.clone();
        let branch = confirm.target.branch.clone();
        let plan = confirm.plan.clone();
        let planned = plan.len();
        let mine = id.clone();

        let work = cx.background_executor().spawn(async move {
            let workspace = Workspace::open().ok()?;
            let worktree = workspace
                .worktrees(None)
                .ok()?
                .into_iter()
                .find(|w| w.id == id)?;
            Some(ket_core::storage::clear(&worktree.path, &plan))
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                // Whatever went is off the disk whether or not the rest
                // followed, and whether or not anyone is still looking at the
                // dialog. Measured again rather than adjusted by what was
                // freed: a build running in that checkout right now is putting
                // some of it straight back.
                if outcome.is_some() {
                    shell.remeasure_worktree(mine.clone(), cx);
                }

                // Escape closes the dialog without calling the unlinks back,
                // so by now it may be gone — or, worse, be a *second* dialog
                // opened on another worktree while this delete ran. Only the
                // one this attempt started from may be written on or closed;
                // anything else hears the outcome as a toast.
                let ours = shell
                    .confirm_clear_build
                    .as_ref()
                    .is_some_and(|confirm| confirm.target.id == mine);

                let freed = match (ours, outcome) {
                    (true, Some(cleared)) => {
                        let confirm = shell.confirm_clear_build.as_mut().expect("just checked");
                        confirm.clearing = false;
                        confirm.freed += cleared.freed;
                        confirm.freed
                    }
                    (true, None) => {
                        let confirm = shell.confirm_clear_build.as_mut().expect("just checked");
                        confirm.clearing = false;
                        confirm.freed
                    }
                    (false, outcome) => outcome.map_or(0, |cleared| cleared.freed),
                };

                match outcome {
                    Some(cleared) if cleared.failed == 0 => {
                        if ours {
                            shell.confirm_clear_build = None;
                        }
                        shell.toast_detail(
                            Tone::Success,
                            format!("Cleared {}", format_bytes(freed)),
                            format!("from {branch}"),
                            cx,
                        );
                    }
                    // Half-done is its own answer, and the dialog is the right
                    // place for it: the directories that would not go are
                    // usually held open by a build running in that very
                    // checkout, so the useful next move is to stop it and
                    // press again rather than to read a toast about it.
                    Some(cleared) => shell.clear_build_failed(
                        ours,
                        format!(
                            "{} of {planned} couldn't be removed — something may still be \
                             building there",
                            cleared.failed,
                        ),
                        &branch,
                        cx,
                    ),
                    None => shell.clear_build_failed(
                        ours,
                        "the worktree could not be read".to_string(),
                        &branch,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Says why a clear did not finish, wherever the reader is still looking.
    ///
    /// Normally that is the dialog, which stays up so the answer sits beside
    /// the button that would try again. `ours` is false once it has been
    /// dismissed or replaced, and then there is nothing left to write on — a
    /// failure with nowhere to go is a failure nobody hears about, so it falls
    /// back to a toast, which has to name the branch because by then the
    /// reader is looking at something else.
    fn clear_build_failed(
        &mut self,
        ours: bool,
        why: String,
        branch: &str,
        cx: &mut Context<Self>,
    ) {
        match self.confirm_clear_build.as_mut().filter(|_| ours) {
            Some(confirm) => confirm.failure = Some(why.into()),
            None => self.toast(Tone::Error, format!("Couldn't clear {branch} — {why}"), cx),
        }
    }

    /// Handles a key while the confirmation is up.
    pub(crate) fn clear_build_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.confirm_clear_build.is_none() {
            return false;
        }

        match event.keystroke.key.as_str() {
            // Escape still dismisses mid-delete. It cannot call the unlinks
            // back, and pinning the dialog open until the filesystem is
            // finished would trap the reader in front of a progress report
            // they never asked for; the outcome finds them either way, as a
            // toast — see `clear_build_failed`.
            "escape" => self.confirm_clear_build = None,
            "enter" => self.clear_build_confirmed(cx),
            _ => {}
        }
        true
    }

    /// The confirmation dialog, or nothing when it is closed.
    pub(crate) fn clear_build_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm_clear_build.as_ref()?;
        let t = &self.theme;

        let cancel = button("cancel-clear-build", "Cancel")
            .ghost()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm_clear_build = None;
                cx.notify();
            }));

        // Primary, not danger. Every other destructive button in the shell is
        // red because what it removes is gone; this one removes something a
        // build command puts back, and dressing the two the same would teach
        // the reader to read past the colour.
        //
        // The label is where the status goes. A spinner beside an unchanged
        // "Clear" would leave the button still offering to start something
        // that is already running; saying so in the label means the one
        // control the reader is looking at is also the one telling them
        // where things stand. The size is the figure above, not repeated
        // here.
        let label = match (confirm.clearing, confirm.failure.is_some()) {
            (true, _) => "Clearing…",
            (false, true) => "Try again",
            (false, false) => "Clear",
        };
        let clear = button("confirm-clear-build", label)
            .primary()
            .leading(Icon::Eraser)
            .enabled(!confirm.clearing)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.clear_build_confirmed(cx);
                cx.notify();
            }));

        // What survives, when the sidebar has measured this worktree. It is
        // the other half of the decision and the dialog never said it: 5.2 GB
        // is a big number until you are told the checkout under it is 5 MB.
        let kept = self
            .footprint(&confirm.target.id)
            .map(|footprint| footprint.kept())
            .filter(|kept| *kept > 0);

        // The whole dialog is a question and its answer: the figure sits in
        // the title, and one sentence under it says what goes and what stays.
        // Two type sizes, two lines. The directories are still named one by
        // one — see the module docs on why the reader is shown them rather
        // than asked to trust the config list that found them — but inside
        // the sentence, up to `LISTED` of them.
        //
        // The identifiers are picked out in the primary ink rather than a
        // second face: a text run can carry a colour and a weight but not a
        // family, and colour is the split the shell's rows already use for
        // "identifier" against "words about it".
        let mut sentence = String::from("Removes ");
        let mut names = Vec::new();
        let mut name = |sentence: &mut String, text: &str| {
            let start = sentence.len();
            sentence.push_str(text);
            names.push(start..sentence.len());
        };
        let shown = confirm.plan.len().min(LISTED);
        for (index, item) in confirm.plan.iter().take(shown).enumerate() {
            if index > 0 {
                let last = index + 1 == confirm.plan.len();
                sentence.push_str(if last { " and " } else { ", " });
            }
            name(&mut sentence, &item.path);
        }
        if confirm.plan.len() > shown {
            let more = confirm.plan.len() - shown;
            sentence.push_str(&format!(" and {more} more"));
        }
        sentence.push_str(" from ");
        name(&mut sentence, &confirm.target.branch);
        sentence.push_str(". ");
        match kept {
            Some(kept) => sentence.push_str(&format!(
                "The {} checkout and anything tracked or uncommitted stay.",
                format_bytes(kept)
            )),
            None => sentence.push_str("Anything tracked or uncommitted stays."),
        }
        let highlights: Vec<(Range<usize>, HighlightStyle)> = names
            .into_iter()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        color: Some(paint(t.text.primary).into()),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..Default::default()
                    },
                )
            })
            .collect();

        Some(
            centered(
                card("clear-build", px(460.0), t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(header(
                        format!("Clear {} of build output?", format_bytes(confirm.total)),
                        t,
                    ))
                    .child(
                        div()
                            .text_size(LABEL)
                            .line_height(relative(1.5))
                            .text_color(paint(t.text.dim))
                            .child(StyledText::new(sentence).with_highlights(highlights)),
                    )
                    .children(confirm.failure.clone().map(|why| {
                        div()
                            .p(px(10.0))
                            .rounded(RADIUS_SM)
                            .border_1()
                            .border_color(alpha(paint(t.status.failed), 0.45))
                            .bg(alpha(paint(t.status.failed), 0.08))
                            .text_size(CAPTION)
                            .text_color(paint(t.status.failed))
                            .child(why)
                    }))
                    .child(footer().child(cancel).child(clear)),
            )
            .into_any_element(),
        )
    }
}
