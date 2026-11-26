//! The questions ket asks without being asked, as a session is opened.
//!
//! Two so far, one module each:
//!
//! - [`out_of_usage`]: the agent's plan ran out partway through its task.
//!   Wait for the limit to reset, or hand the task to another agent.
//! - [`cold_resume`]: the session's prompt cache has gone cold. Continue,
//!   paying to read it all again, or start fresh.
//!
//! They are one kind of thing, so they behave as one, and a third joins
//! here rather than inventing its own:
//!
//! - **When.** As the reader opens the tab, or as the thing happens on the
//!   tab in front of them. Never when they send: a message in ket always
//!   goes as it was typed. Only while the window is in front, and never
//!   over another dialog.
//! - **How often.** Once: a question answered, by any of its buttons, its ✕
//!   or Escape, is not asked again until what it asks about happens again.
//! - **Which first.** One at a time, out of usage first: a cold cache is
//!   moot while nothing will answer.
//! - **How it looks.** The shared dialog card at one width; a title led by
//!   a mark saying what kind of thing it is (`ui::dialog::Header::mark`),
//!   dim for a fact to weigh, a state's colour for a state; one sentence;
//!   then the choices, the one ket would make last and primary.
//! - **Its keys.** Escape and the ✕ take the choice that changes nothing.
//!   Enter takes the primary button, which never sends or clears anything
//!   by itself.

mod cold_resume;
mod out_of_usage;
/// The hand-over itself, shared with a phone's: see [`Shell::switch_tab`].
mod switch_agent;

use gpui::{AnyElement, Context, KeyDownEvent, Pixels, Window, px};

use crate::Shell;

/// Every question's card: the one width, so they read as one kind.
pub(crate) const WIDTH: Pixels = px(340.0);

/// The questions, and what each needs between frames.
#[derive(Default)]
pub(crate) struct SessionAsks {
    pub(crate) cold: cold_resume::ColdResumeState,
    pub(crate) usage: out_of_usage::OutOfUsageState,
}

impl SessionAsks {
    /// Whether one is up.
    pub(crate) fn open(&self) -> bool {
        self.cold.dialog.is_some() || self.usage.dialog.is_some()
    }
}

impl Shell {
    /// Looks at the tab the reader is on, for anything to ask. Called every
    /// frame; each question keeps its own note of what it last looked at.
    pub(crate) fn check_session_asks(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.check_out_of_usage(window, cx);
        self.check_cold_resume(window, cx);
    }

    /// The question that is up, or nothing.
    pub(crate) fn session_ask_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.out_of_usage_view(cx)
            .or_else(|| self.cold_resume_view(cx))
    }

    /// Handles a key while a question is up. Every key is the question's
    /// then: none reaches the tab behind it.
    pub(crate) fn session_ask_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        self.out_of_usage_key(event, cx) || self.cold_resume_key(event, cx)
    }
}
