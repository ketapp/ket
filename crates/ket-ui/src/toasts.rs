//! The shell's transient reports, and how long they stay.
//!
//! See `ui::toast` for what one looks like and why it looks that way. This is
//! the state behind them: which are up, what takes them away, and the one
//! entry point — [`Shell::toast`] — that every surface with an outcome to
//! report should be reaching for.
//!
//! **Dismissal is never work the reader has to do** — for news. Every toast
//! goes away on its own, and clicking one only makes that happen sooner. A
//! report of something that *worked* which then waits to be closed has
//! charged the reader for good news.
//!
//! **An alert is the exception** — see [`Shell::alert`]: a warning the reader
//! must not miss stays until they dismiss it, and a second one about the
//! same thing updates the first rather than stacking under it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Pixels, SharedString, Window, div, prelude::*};

use crate::Shell;
use crate::ui::button::button;
use crate::ui::toast::{EXIT, Spec, Tone, stack, toast};

/// How long a toast stays up.
///
/// Long enough to look over from whatever you were doing when it appeared,
/// short enough that a run of them does not become a wall.
const LIFETIME: Duration = Duration::from_secs(5);

/// How long a failure stays up.
///
/// Longer, because it is the only one carrying something the reader may have
/// to act on, and because a failure that vanished before it was read leaves
/// them with an action that silently did nothing.
const ERROR_LIFETIME: Duration = Duration::from_secs(9);

/// How many toasts can be on screen at once.
const MAX_VISIBLE: usize = 4;

/// One report on screen.
pub(crate) struct Toast {
    /// Identifies it for dismissal.
    ///
    /// A counter rather than an index: a toast's timer fires long after it was
    /// pushed, by which time the ones before it may have gone and any index
    /// taken at the time would name somebody else's.
    pub(crate) id: u64,
    /// How it reads.
    pub(crate) tone: Tone,
    /// What it says.
    pub(crate) message: SharedString,
    /// What it happened to, on a quieter second line.
    pub(crate) detail: Option<SharedString>,
    /// When it started leaving. It stays in the stack until its exit is done.
    pub(crate) leaving: Option<Instant>,
    /// For an alert, what it is about — a second alert with the same one
    /// updates it in place — and what its button does. `None` for a toast,
    /// which leaves on its own.
    pub(crate) alert: Option<Alert>,
    /// Its height as last laid out — see `ui::toast::Spec::height`.
    pub(crate) height: Rc<Cell<Pixels>>,
}

/// What makes a toast an alert: see [`Shell::alert`].
pub(crate) struct Alert {
    /// What it is about. One alert per subject is up at a time.
    pub(crate) subject: SharedString,
    /// Its button's label, and what pressing it does. Dismisses it too.
    pub(crate) action: Option<(SharedString, AlertAction)>,
}

/// What an alert's button does.
pub(crate) type AlertAction = Rc<dyn Fn(&mut Shell, &mut Context<Shell>)>;

impl Shell {
    /// Warns of something the reader must not miss: it stays until they
    /// dismiss it or press its button.
    ///
    /// For what needs doing, not what happened — everything else is a
    /// [`Shell::toast`]. One already up about the same `subject` is updated
    /// in place, so a run of the same warning is one card saying how many.
    pub(crate) fn alert(
        &mut self,
        tone: Tone,
        message: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        alert: Alert,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        let detail = Some(detail.into());
        if let Some(up) = self.toasts.iter_mut().find(|toast| {
            toast.leaving.is_none()
                && toast
                    .alert
                    .as_ref()
                    .is_some_and(|up| up.subject == alert.subject)
        }) {
            up.tone = tone;
            up.message = message;
            up.detail = detail;
            up.alert = Some(alert);
            cx.notify();
            return;
        }
        let id = self.next_toast_id;
        self.next_toast_id = self.next_toast_id.wrapping_add(1);
        self.toasts.push(Toast {
            id,
            tone,
            message,
            detail,
            leaving: None,
            alert: Some(alert),
            height: Rc::default(),
        });
        cx.notify();
    }

    /// Whether an alert about `subject` is up and not leaving.
    pub(crate) fn alert_up(&self, subject: &str) -> bool {
        self.toasts.iter().any(|toast| {
            toast.leaving.is_none()
                && toast
                    .alert
                    .as_ref()
                    .is_some_and(|alert| alert.subject == subject)
        })
    }

    /// Turns the alert about `subject` into news: it says `message` in place
    /// and then leaves on its own, like any toast.
    ///
    /// For an alert that was reporting work in progress, whose outcome is
    /// news rather than a warning. If the reader already dismissed it, the
    /// outcome is still theirs to hear, so it arrives as a fresh toast.
    pub(crate) fn settle_alert(
        &mut self,
        subject: &str,
        tone: Tone,
        message: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        let detail = Some(detail.into());
        let Some(up) = self.toasts.iter_mut().find(|toast| {
            toast.leaving.is_none()
                && toast
                    .alert
                    .as_ref()
                    .is_some_and(|alert| alert.subject == subject)
        }) else {
            self.push_toast(tone, message, detail, cx);
            return;
        };
        up.tone = tone;
        up.message = message;
        up.detail = detail;
        up.alert = None;
        let id = up.id;
        self.dismiss_after(id, tone, cx);
        cx.notify();
    }

    /// Reports something, briefly.
    ///
    /// The message is the whole of what the reader gets, so it has to stand on
    /// its own: what happened, to what. Not "Done" — a toast that has faded is
    /// a toast nobody can go back and get more context from.
    pub(crate) fn toast(
        &mut self,
        tone: Tone,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.push_toast(tone, message.into(), None, cx);
    }

    /// [`Shell::toast`] with a second, quieter line: the headline says what
    /// happened and the detail says to what.
    pub(crate) fn toast_detail(
        &mut self,
        tone: Tone,
        message: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.push_toast(tone, message.into(), Some(detail.into()), cx);
    }

    fn push_toast(
        &mut self,
        tone: Tone,
        message: SharedString,
        detail: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_toast_id;
        self.next_toast_id = self.next_toast_id.wrapping_add(1);
        self.toasts.push(Toast {
            id,
            tone,
            message,
            detail,
            leaving: None,
            alert: None,
            height: Rc::default(),
        });

        // Oldest out first. A burst that outruns the reader should leave the
        // most recent news on screen rather than the stalest. They leave the
        // way any toast does, rather than vanishing and snapping the stack.
        // Alerts are not news and are not pushed out by it.
        let staying: Vec<u64> = self
            .toasts
            .iter()
            .filter(|toast| toast.leaving.is_none() && toast.alert.is_none())
            .map(|toast| toast.id)
            .collect();
        for &old in staying
            .iter()
            .take(staying.len().saturating_sub(MAX_VISIBLE))
        {
            self.dismiss_toast(old, cx);
        }

        self.dismiss_after(id, tone, cx);
        cx.notify();
    }

    /// Starts the clock on a toast's stay: longer for a failure.
    fn dismiss_after(&mut self, id: u64, tone: Tone, cx: &mut Context<Self>) {
        let lifetime = if tone == Tone::Error {
            ERROR_LIFETIME
        } else {
            LIFETIME
        };
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            cx.background_executor().timer(lifetime).await;
            let _ = shell.update(cx, |shell, cx| shell.dismiss_toast(id, cx));
        })
        .detach();
    }

    /// Takes one away, whether its time ran out or it was clicked.
    ///
    /// It animates out first and leaves the stack when that is over. Silent
    /// about an id that is already gone or already leaving: a toast clicked a
    /// moment before its timer fires is the ordinary case, not a fault.
    pub(crate) fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return;
        };
        if toast.leaving.is_some() {
            return;
        }
        toast.leaving = Some(Instant::now());
        cx.notify();

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            cx.background_executor().timer(EXIT).await;
            let _ = shell.update(cx, |shell, cx| {
                shell.toasts.retain(|toast| toast.id != id);
                cx.notify();
            });
        })
        .detach();
    }

    /// The stack, or nothing when there is none.
    ///
    /// Not deferred here — the shell defers its own overlays, which is what
    /// keeps one place deciding what draws above what.
    pub(crate) fn toasts_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.toasts.is_empty() {
            return None;
        }

        // The exit runs off `Toast::leaving` rather than an animation element
        // of its own (see `ui::toast::toast` for why), so nothing else asks
        // for its frames.
        if self.toasts.iter().any(|toast| toast.leaving.is_some()) {
            window.request_animation_frame();
        }

        let t = &self.theme;
        let corner = self.toast_corner;

        let mut order: Vec<&Toast> = self.toasts.iter().collect();
        if corner.newest_first() {
            order.reverse();
        }

        Some(
            stack(corner)
                .children(order.into_iter().map(|item| {
                    let id = item.id;
                    let actions = item.alert.as_ref().map(|alert| {
                        let action = alert.action.as_ref().map(|(label, run)| {
                            let run = run.clone();
                            button(("toast-action", id as usize), label.clone())
                                .small()
                                .primary()
                                .render(t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    // Away first: an action that raises a
                                    // fresh alert on the same subject — a
                                    // retry — would otherwise be updating
                                    // this one, just in time to lose it.
                                    this.dismiss_toast(id, cx);
                                    run(this, cx);
                                }))
                        });
                        div()
                            .flex()
                            .gap(gpui::px(6.0))
                            .children(action)
                            .child(
                                button(("toast-dismiss", id as usize), "Dismiss")
                                    .small()
                                    .render(t)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.dismiss_toast(id, cx);
                                    })),
                            )
                            .into_any_element()
                    });
                    toast(
                        ("toast", id as usize),
                        Spec {
                            tone: item.tone,
                            message: item.message.clone(),
                            detail: item.detail.clone(),
                            corner,
                            exit: item
                                .leaving
                                .map(|since| since.elapsed().as_secs_f32() / EXIT.as_secs_f32()),
                            actions,
                            height: item.height.clone(),
                        },
                        t,
                        cx.listener(move |this, _, _, cx| {
                            // Stopped, or the shell's own click handler reads
                            // this as a click on the window and starts closing
                            // menus behind it.
                            cx.stop_propagation();
                            this.dismiss_toast(id, cx);
                        }),
                    )
                }))
                .into_any_element(),
        )
    }
}
