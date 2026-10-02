//! The shell's transient reports, and how long they stay.
//!
//! See `ui::toast` for what one looks like and why it looks that way. This is
//! the state behind them: which are up, what takes them away, and the one
//! entry point — [`Shell::toast`] — that every surface with an outcome to
//! report should be reaching for.
//!
//! **Dismissal is never work the reader has to do.** Every toast goes away on
//! its own, and clicking one only makes that happen sooner. A report of
//! something that *worked* which then waits to be closed has charged the
//! reader for good news.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, Pixels, SharedString, Window, prelude::*};

use crate::Shell;
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
    /// Its height as last laid out — see `ui::toast::Spec::height`.
    pub(crate) height: Rc<Cell<Pixels>>,
}

impl Shell {
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
        let lifetime = if tone == Tone::Error {
            ERROR_LIFETIME
        } else {
            LIFETIME
        };
        let id = self.next_toast_id;
        self.next_toast_id = self.next_toast_id.wrapping_add(1);
        self.toasts.push(Toast {
            id,
            tone,
            message,
            detail,
            leaving: None,
            height: Rc::default(),
        });

        // Oldest out first. A burst that outruns the reader should leave the
        // most recent news on screen rather than the stalest. They leave the
        // way any toast does, rather than vanishing and snapping the stack.
        let staying: Vec<u64> = self
            .toasts
            .iter()
            .filter(|toast| toast.leaving.is_none())
            .map(|toast| toast.id)
            .collect();
        for &old in staying
            .iter()
            .take(staying.len().saturating_sub(MAX_VISIBLE))
        {
            self.dismiss_toast(old, cx);
        }

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            cx.background_executor().timer(lifetime).await;
            let _ = shell.update(cx, |shell, cx| shell.dismiss_toast(id, cx));
        })
        .detach();

        cx.notify();
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
