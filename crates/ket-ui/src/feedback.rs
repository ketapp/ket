//! The feedback popover: a short form behind the status bar's speech bubble
//! for telling the team what works, what broke and what ket could do.
//!
//! It opens over the bar's right end, where the bubble sits, and is a popover
//! rather than a dialog: writing a note should not feel like filing a ticket.
//! It still takes the keyboard while its form is up, because a message being
//! typed must not also drive the shell.
//!
//! What is sent, and the once-a-week limit, are `ket_core::feedback`'s; this
//! file only draws the form and hands it the report off the UI thread.
//! Closing the popover keeps the draft — a stray click outside it should not
//! cost anyone what they wrote — and only a report that went clears it.

use gpui::{
    AnyElement, Context, Entity, FontWeight, KeyDownEvent, Pixels, SharedString, Window, div,
    prelude::*, px,
};
use ket_core::feedback::{self, Kind, Report};

use crate::Shell;
use crate::fonts::Prose;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::caption;
use crate::ui::field::error as field_error;
use crate::ui::group::{button_group, segment};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::popup::{Placement, anchored};
use crate::ui::textarea::textarea;
use crate::ui::toggle::checkbox;
use crate::ui::tooltip::{Side, tooltip};

/// The composer the message is written in.
const MESSAGE_EDITOR: &str = "feedback:message";

/// The popover's width: a message of a few lines without wrapping every
/// sentence, and narrow enough to read as a note rather than a dialog.
const WIDTH: Pixels = px(380.0);

/// The panel's gutter.
const PAD: Pixels = px(16.0);

/// The header, which holds the title.
const HEADER_H: Pixels = px(44.0);

/// The title, at the usage popover's size, its neighbour on the bar.
const TITLE_TEXT: Pixels = px(14.0);

/// How tall the message well is.
const MESSAGE_HEIGHT: Pixels = px(132.0);

/// What the message is typed at.
const MESSAGE_TEXT: Pixels = px(13.0);

/// The feedback popover, and what it keeps between openings.
#[derive(Default)]
pub(crate) struct Feedback {
    /// Whether the popover is up.
    open: bool,
    /// Whether the pointer is on the bubble, which shows its tooltip.
    hovered: bool,
    /// The form, made the first time it is needed and kept, draft and all,
    /// until a report goes.
    form: Option<Form>,
    /// When another report may be sent, in Unix milliseconds, while one may
    /// not. Read from core each time the popover opens.
    wait_until: Option<u64>,
    /// Whether the wait is because a report just went, which the panel
    /// thanks the person for rather than explaining the limit.
    just_sent: bool,
}

/// The form's drafts. The message itself is the composer under
/// [`MESSAGE_EDITOR`].
struct Form {
    /// What it is about.
    kind: Kind,
    /// Where to reply, if they would like one.
    reply: Entity<TextInput>,
    /// Whether the macOS version goes with it.
    share_system: bool,
    /// Whether the message has the keyboard. Remembered rather than read off
    /// a focus handle: the composer types through the shell's own handle,
    /// which has the keyboard for plenty of other reasons.
    body_active: bool,
    /// A send is on its way.
    sending: bool,
    /// Why the last send did not go.
    error: Option<SharedString>,
}

impl Feedback {
    /// Whether the form is up and holding the keyboard.
    pub(crate) fn typing(&self) -> bool {
        self.open && self.wait_until.is_none() && self.form.is_some()
    }
}

/// `ms` from now as a person would say it: "in 6 days", "tomorrow", "in 3
/// hours".
fn from_now(ms: u64) -> String {
    let left = ms.saturating_sub(ket_core::now_ms()) / 1000;
    let hours = left.div_ceil(3600);
    let days = left.div_ceil(86_400);
    if hours <= 1 {
        "within the hour".to_owned()
    } else if hours < 24 {
        format!("in {hours} hours")
    } else if days == 1 {
        "tomorrow".to_owned()
    } else {
        format!("in {days} days")
    }
}

/// What the message well says before anything is written, by what it is
/// about.
fn prompt(kind: Kind) -> &'static str {
    match kind {
        Kind::Feedback => "What's working for you, and what isn't?",
        Kind::Bug => "What happened, and what did you expect instead?",
        Kind::Idea => "What would you like ket to do?",
    }
}

impl Shell {
    // ---- opening ---------------------------------------------------------------

    /// Opens the popover, or closes it when it is up.
    fn toggle_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.feedback.open {
            self.close_feedback(window);
            return;
        }
        self.popup = None;
        self.feedback.open = true;
        // Thanks are for the opening right after a send, not every one after.
        self.feedback.just_sent = false;
        self.feedback.wait_until = feedback::next_allowed_ms();
        if self.feedback.wait_until.is_some() {
            return;
        }
        if self.feedback.form.is_none() {
            let reply = TextInput::new("Email, if you'd like a reply", cx);
            self.watch_field(&reply, cx);
            self.feedback.form = Some(Form {
                kind: Kind::Feedback,
                reply,
                share_system: true,
                body_active: true,
                sending: false,
                error: None,
            });
        }
        if !self.editors.contains_key(MESSAGE_EDITOR) {
            self.open_composer(MESSAGE_EDITOR, "");
            self.set_composer_size(MESSAGE_EDITOR, MESSAGE_TEXT);
        }
        // The message takes the keyboard: it is what the popover is for.
        if let Some(form) = self.feedback.form.as_mut() {
            form.body_active = true;
        }
        window.focus(&self.focus);
    }

    /// Puts the popover away, keeping whatever was written in it.
    fn close_feedback(&mut self, window: &mut Window) {
        self.feedback.open = false;
        if let Some(form) = self.feedback.form.as_mut() {
            form.body_active = false;
        }
        window.focus(&self.focus);
    }

    /// Closes the popover for a press outside it. `true` when it was open.
    pub(crate) fn dismiss_feedback(&mut self) -> bool {
        let was = self.feedback.open;
        self.feedback.open = false;
        if let Some(form) = self.feedback.form.as_mut() {
            form.body_active = false;
        }
        was
    }

    // ---- sending ---------------------------------------------------------------

    /// Hands the report to core on a background thread; the answer comes
    /// back to [`Shell::feedback_answered`].
    fn send_feedback(&mut self, cx: &mut Context<Self>) {
        let message = self.composer_text(MESSAGE_EDITOR);
        let Some(form) = self.feedback.form.as_mut() else {
            return;
        };
        if form.sending {
            return;
        }
        if message.trim().is_empty() {
            form.error = Some("Write something to send first.".into());
            return;
        }
        let report = Report {
            kind: form.kind,
            message,
            reply_to: form.reply.read(cx).text(),
            share_system: form.share_system,
        };
        form.sending = true;
        form.error = None;

        let work = cx
            .background_executor()
            .spawn(async move { feedback::send(&report) });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = work.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.feedback_answered(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Takes core's answer to a send.
    fn feedback_answered(&mut self, result: ket_core::Result<()>) {
        match result {
            Ok(()) => {
                // The draft went, so it goes; the popover stays up to say so.
                self.close_composer(MESSAGE_EDITOR);
                self.feedback.form = None;
                self.feedback.wait_until = feedback::next_allowed_ms();
                self.feedback.just_sent = true;
            }
            Err(e) => {
                // Another window may have sent one meanwhile; if so the
                // panel says when the next can go rather than only that it
                // cannot.
                self.feedback.wait_until = feedback::next_allowed_ms();
                if let Some(form) = self.feedback.form.as_mut() {
                    form.sending = false;
                    form.error = Some(e.to_string().into());
                }
            }
        }
    }

    // ---- keys ------------------------------------------------------------------

    /// Handles a keystroke while the popover is up. Returns whether it was
    /// taken.
    ///
    /// Escape puts it away. With the form up it takes every key: ⌘↵ sends,
    /// Tab moves between the message and the reply address, and the field
    /// with the keyboard gets the rest.
    pub(crate) fn feedback_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A dialog opened by a chord while the popover was up is above it,
        // and has the keys.
        if !self.feedback.open || self.modal_open() {
            return false;
        }
        let key = &event.keystroke;
        if key.key == "escape" {
            self.close_feedback(window);
            return true;
        }
        if !self.feedback.typing() {
            return false;
        }
        let command = key.modifiers.platform || key.modifiers.control;
        if key.key == "enter" && command {
            self.send_feedback(cx);
            return true;
        }

        let Some(form) = self.feedback.form.as_ref() else {
            return false;
        };
        let reply = form.reply.clone();
        if reply.read(cx).is_focused(window) {
            // Text is never taken here. It has to keep travelling until
            // macOS's input context sees it — see [`crate::input`].
            if is_text(key) || reply.update(cx, |input, cx| input.key(key, cx)) {
                if let Some(form) = self.feedback.form.as_mut() {
                    form.error = None;
                }
                return true;
            }
            if key.key == "tab" {
                if let Some(form) = self.feedback.form.as_mut() {
                    form.body_active = true;
                }
                window.focus(&self.focus);
            }
            return true;
        }

        if form.body_active && self.focus.is_focused(window) {
            if key.key == "tab" {
                if let Some(form) = self.feedback.form.as_mut() {
                    form.body_active = false;
                }
                reply.update(cx, |input, _| input.select_all());
                window.focus(reply.read(cx).focus_handle());
            } else {
                self.composer_key(MESSAGE_EDITOR, key, cx);
                if let Some(form) = self.feedback.form.as_mut() {
                    form.error = None;
                }
            }
            return true;
        }
        // Swallowed: the form is up, and a key typed at it must not also
        // reach the shell behind.
        true
    }

    // ---- views -----------------------------------------------------------------

    /// The bubble at the status bar's right end and, while it is open, the
    /// popover over it.
    ///
    /// Built before the bar because the message is an editor pane, which
    /// needs the shell mutably.
    pub(crate) fn feedback_trigger(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel =
            (self.feedback.open && !self.modal_open()).then(|| self.feedback_panel(window, cx));
        let bar = self.theme.on_backdrop();
        let open = self.feedback.open;
        let bubble = icon_button("feedback", Icon::MessageSquare)
            .bare()
            .small()
            .pressed(open)
            .render(&bar)
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_feedback(window, cx);
                cx.notify();
            }));
        let bubble = tooltip(
            "feedback-tip",
            bubble.into_any_element(),
            "Send feedback",
            Side::TopEnd,
            self.feedback.hovered && !open,
            &self.theme,
        )
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            this.feedback.hovered = *hovered;
            cx.notify();
        }));
        anchored(
            "feedback-popover",
            bubble.into_any_element(),
            panel,
            Placement::AboveEnd,
            WIDTH,
            window,
            &self.theme,
        )
    }

    /// What the popover holds: the form, or why there is none this week.
    fn feedback_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let header = div()
            .flex()
            .items_center()
            .h(HEADER_H)
            .px(PAD)
            .border_b_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .text_size(TITLE_TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Send feedback"),
            );

        let body = match self.feedback.wait_until {
            Some(next) => self.feedback_waiting(next),
            None => self.feedback_form(window, cx),
        };
        div()
            .flex()
            .flex_col()
            .prose()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The panel while a report went less than a week ago.
    fn feedback_waiting(&self, next: u64) -> AnyElement {
        let t = &self.theme;
        let (mark, ink, title) = if self.feedback.just_sent {
            (
                Icon::CircleCheck,
                paint(t.status.running),
                "Thanks — it's on its way.",
            )
        } else {
            (
                Icon::Info,
                paint(t.text.dim),
                "You've sent a report this week.",
            )
        };
        div()
            .flex()
            .items_start()
            .gap(px(10.0))
            .p(PAD)
            .child(div().pt(px(1.0)).child(sized_icon(mark, px(16.0), ink)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(MESSAGE_TEXT)
                            .text_color(paint(t.text.primary))
                            .child(title),
                    )
                    .child(caption(
                        format!("You can send another {}.", from_now(next)),
                        t,
                    )),
            )
            .into_any_element()
    }

    /// The form: what it is about, the message, a reply address, whether the
    /// macOS version goes too, and Send.
    fn feedback_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = self.feedback.form.as_ref() else {
            return div().into_any_element();
        };
        let active = form.body_active && self.focus.is_focused(window);
        let kind = form.kind;
        let sending = form.sending;
        let share = form.share_system;
        let error = form.error.clone();
        let reply = form.reply.clone();

        self.set_composer_caret(MESSAGE_EDITOR, active);
        let empty = self.composer_text(MESSAGE_EDITOR).is_empty();
        let editor = self.editor_pane(MESSAGE_EDITOR, None, window, cx);
        let t = &self.theme;
        let connected = feedback::endpoint().is_some();

        let kinds = button_group("feedback-kind")
            .fill()
            .small()
            .children(Kind::ALL.map(|which| {
                segment(("feedback-kind", which as usize), which.label())
                    .selected(which == kind)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.feedback.form.as_mut() {
                            form.kind = which;
                        }
                        cx.notify();
                    }))
            }))
            .render(t);

        let mut message = textarea("feedback-message", editor, MESSAGE_HEIGHT).active(active);
        if empty {
            message = message.placeholder(prompt(kind), MESSAGE_TEXT);
        }
        let message = message
            .render(t)
            .cursor_text()
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                if let Some(form) = this.feedback.form.as_mut() {
                    form.body_active = true;
                }
                window.focus(&this.focus);
                cx.notify();
            }));

        let reply_field = text_field(
            &reply,
            "feedback-reply",
            false,
            Style::new(t, self.caret.visible),
            window,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(form) = this.feedback.form.as_mut() {
                form.body_active = false;
            }
            cx.notify();
        }));

        let system = checkbox("feedback-system", share, "Include my macOS version", t).on_click(
            cx.listener(|this, _, _, cx| {
                if let Some(form) = this.feedback.form.as_mut() {
                    form.share_system = !form.share_system;
                }
                cx.notify();
            }),
        );

        let note = if connected {
            "One report a week. ket's version is always included."
        } else {
            "Feedback isn't connected yet."
        };
        let send = button("feedback-send", "Send")
            .primary()
            .small()
            .hint("\u{2318}\u{21b5}")
            .loading_if(sending, "Sending…")
            .enabled(connected && !empty)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.send_feedback(cx);
                cx.notify();
            }));
        let footer = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .pt(px(4.0))
            .child(div().flex_1().min_w_0().child(caption(note, t)))
            .child(send);

        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(PAD)
            .child(kinds)
            .child(message)
            .child(reply_field)
            .child(div().ml(px(-6.0)).child(system))
            .children(error.map(|why| field_error(why, t)))
            .child(footer)
            .into_any_element()
    }
}
