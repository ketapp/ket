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
use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, HighlightStyle, KeyDownEvent, MouseButton, StyledText, div,
    prelude::*, px, relative,
};
use ket_core::storage::{Clearable, format_bytes};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::paint::paint;
use crate::toasts::{Alert, AlertAction};
use crate::ui::LABEL;
use crate::ui::button::button;
use crate::ui::dialog::{card, centered, footer, header};
use crate::ui::icon::Icon;
use crate::ui::toast::Tone;
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
}

impl Shell {
    /// Works out what clearing this worktree would remove, then asks.
    ///
    /// Off the window's thread: finding the directories means walking the
    /// checkout, which is the same walk that makes sizes expensive.
    pub(crate) fn request_clear_build(&mut self, target: WorktreeTarget, cx: &mut Context<Self>) {
        // Asking again about a tree that is already going would offer to
        // delete what is being deleted, sized by a walk racing the unlinks.
        if self.clearing_builds.contains(&target.id) {
            self.toast_detail(Tone::Info, "Already clearing", target.branch.clone(), cx);
            return;
        }
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
                    // A clear started while this walk ran: the plan is of a
                    // tree that is going, and its Clear would do nothing.
                    Some(_) if shell.clearing_builds.contains(&target.id) => shell.toast_detail(
                        Tone::Info,
                        "Already clearing",
                        target.branch.clone(),
                        cx,
                    ),
                    Some(plan) if !plan.is_empty() => {
                        let total = plan.iter().map(|item| item.bytes).sum();
                        shell.confirm_clear_build = Some(ClearBuildConfirm {
                            target,
                            plan,
                            total,
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

    /// Closes the dialog and removes the planned directories.
    pub(crate) fn clear_build_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_clear_build.take() else {
            return;
        };
        self.start_clear_build(confirm.target, confirm.plan, 0, cx);
    }

    /// Removes `plan` from the worktree, reporting on an alert.
    ///
    /// Hundreds of gigabytes of small files take minutes to unlink, and a
    /// dialog held open for that long holds the whole window with it. So the
    /// dialog is already gone and the alert carries the news: it says the
    /// clear is running, becomes the result in place when it finishes, and on
    /// a failure stays with a Try again beside the reason.
    ///
    /// `freed` is what earlier attempts at this plan already reclaimed, so a
    /// retry after a half-failure reports the whole of what went rather than
    /// only the last handful of directories.
    fn start_clear_build(
        &mut self,
        target: WorktreeTarget,
        plan: Vec<Clearable>,
        freed: u64,
        cx: &mut Context<Self>,
    ) {
        // The plan is a list of `remove_dir_all` calls. Running it twice over
        // is at best wasted work on a tree that is already going.
        if !self.clearing_builds.insert(target.id.clone()) {
            self.toast_detail(Tone::Info, "Already clearing", target.branch.clone(), cx);
            return;
        }

        let subject = format!("clear-build:{}", target.id);
        let total: u64 = plan.iter().map(|item| item.bytes).sum();
        self.alert(
            Tone::Info,
            format!("Clearing {} of build output…", format_bytes(total)),
            format!("from {}", target.branch),
            Alert {
                subject: subject.clone().into(),
                action: None,
            },
            cx,
        );

        // The id comes from the dialog, never from `worktree_target`: that
        // field is set by the sidebar's context menu, and the storage card's
        // own Clear button reaches this dialog without going through it. Read
        // from there, a clear started from the card remeasured nothing — or,
        // worse, whichever worktree the last context menu left behind.
        let id = target.id.clone();
        let planned = plan.len();
        let work = cx.background_executor().spawn({
            let plan = plan.clone();
            async move {
                let workspace = Workspace::open().ok()?;
                let worktree = workspace
                    .worktrees(None)
                    .ok()?
                    .into_iter()
                    .find(|w| w.id == id)?;
                Some(ket_core::storage::clear(&worktree.path, &plan))
            }
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let outcome = work.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.clearing_builds.remove(&target.id);

                // Whatever went is off the disk whether or not the rest
                // followed. Measured again rather than adjusted by what was
                // freed: a build running in that checkout right now is putting
                // some of it straight back.
                if outcome.is_some() {
                    shell.remeasure_worktree(target.id.clone(), cx);
                }

                let branch = target.branch.clone();
                let freed = freed + outcome.map_or(0, |cleared| cleared.freed);
                let why = match outcome {
                    Some(cleared) if cleared.failed == 0 => {
                        shell.settle_alert(
                            &subject,
                            Tone::Success,
                            format!("Cleared {}", format_bytes(freed)),
                            format!("from {branch}"),
                            cx,
                        );
                        cx.notify();
                        return;
                    }
                    // The directories that would not go are usually held open
                    // by a build running in that very checkout, so the useful
                    // next move is to stop it and press again.
                    Some(cleared) => format!(
                        "{} of {planned} couldn't be removed from {branch} — something may \
                         still be building there",
                        cleared.failed,
                    ),
                    None => format!("{branch} could not be read"),
                };

                // The same plan again, not a fresh one — see the module docs.
                let retry: AlertAction = Rc::new(move |shell, cx| {
                    shell.start_clear_build(target.clone(), plan.clone(), freed, cx);
                });
                let message = match freed {
                    0 => "Couldn't clear build output".to_string(),
                    freed => format!("Cleared {}, but not all of it", format_bytes(freed)),
                };
                shell.alert(
                    Tone::Error,
                    message,
                    why,
                    Alert {
                        subject: subject.into(),
                        action: Some(("Try again".into(), retry)),
                    },
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// Handles a key while the confirmation is up.
    pub(crate) fn clear_build_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.confirm_clear_build.is_none() {
            return false;
        }

        match event.keystroke.key.as_str() {
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
        // The size is the figure above, not repeated here.
        let clear = button("confirm-clear-build", "Clear")
            .primary()
            .leading(Icon::Eraser)
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
                    .child(footer().child(cancel).child(clear)),
            )
            .into_any_element(),
        )
    }
}
