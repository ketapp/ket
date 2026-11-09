//! Asking, as a session is opened, whether to pick it up again once its
//! prompt cache has gone cold — see `ket_core::cold` for why that matters.
//!
//! Asked when the reader opens the tab, never when they send: a message in
//! ket always goes as it was typed, so the one moment left to offer a choice
//! is before anything has been typed at all. Continue leaves the session
//! exactly as it was. Start fresh opens a new session beside it in the same
//! pane and leaves the cold one open, its conversation as it was, for the
//! reader to close: whatever is typed in it and not sent is theirs.
//!
//! Asked once per cold spell: a session answered for is not asked about
//! again until it has made another request and gone cold after that one.
//! And only of an idle agent: a prompt sent a moment ago may not have
//! reached the transcript yet, and a turn in flight is not a cold session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Context, FontWeight, KeyDownEvent, MouseButton, WeakEntity, Window, div,
    prelude::*, px,
};
use ket_core::activity::{Activity, Signal};
use ket_core::cold::{Cold, LastCall};
use ket_core::event::AgentState;

use crate::Shell;
use crate::paint::paint;
use crate::tabs::TabKind;
use crate::terminal::TerminalId;
use crate::ui::button::button;
use crate::ui::dialog::{body, card, centered, footer};
use crate::ui::icon::{Icon, icon};
use crate::ui::toast::Tone;

/// How long a fresh session is given to report itself started before a phone
/// that asked for it is told it didn't.
const FRESH_OPEN: Duration = Duration::from_secs(30);

/// How often that wait looks.
const FRESH_POLL: Duration = Duration::from_millis(100);

/// The question, while it is up.
pub(crate) struct ColdResume {
    /// The tab whose session went cold.
    terminal: TerminalId,
    /// The cold session.
    session: String,
    /// When the request that cached it was made: what tells this cold spell
    /// from the next one.
    at_ms: u64,
    /// What continuing would cost.
    cold: Cold,
}

/// Everything the question needs between frames.
#[derive(Default)]
pub(crate) struct ColdResumeState {
    /// The question, while it is up.
    pub(crate) dialog: Option<ColdResume>,
    /// The tab and session last looked at, so a frame that changes nothing
    /// reads no transcript. Cleared whenever the reader is not on a Claude
    /// tab, so coming back to one looks again.
    checked: Option<(TerminalId, String)>,
    /// Sessions already answered for, by the request their cold spell
    /// started after.
    answered: HashMap<String, u64>,
    /// Whether the window was the one in front last frame: coming back to
    /// it is opening the tab again, as far as the reader is concerned.
    active: bool,
}

impl Shell {
    /// Looks at the tab the reader is on, and asks when its session has gone
    /// cold. Called every frame; does nothing unless the tab or its session
    /// changed since the last look, or the window has just come back to the
    /// front.
    ///
    /// Only while the window is the one in front: a worktree a phone selected
    /// while the Mac sat unattended is not one the reader is opening.
    pub(crate) fn check_cold_resume(&mut self, window: &Window, cx: &mut Context<Self>) {
        let active = window.is_window_active();
        if active && !self.cold_resume.active {
            self.cold_resume.checked = None;
        }
        self.cold_resume.active = active;
        if self.modal_open() || !active {
            return;
        }
        let Some((terminal, session)) = self.idle_claude_tab() else {
            self.cold_resume.checked = None;
            return;
        };
        let looked = (terminal, session.clone());
        if self.cold_resume.checked.as_ref() == Some(&looked) {
            return;
        }
        self.cold_resume.checked = Some(looked);

        // The transcript and the config off the window's thread: the first
        // look for a transcript can be a shell startup, and a long one is a
        // read of its last quarter-megabyte.
        let lookup = session.clone();
        let found = cx.background_executor().spawn(async move {
            let config = ket_core::config::Config::load().ok()?;
            if !config.agent.asks_on_cold_resume() {
                return None;
            }
            let last = last_call(&lookup)?;
            let cold = last.cold(ket_core::now_ms())?;
            cold.worth_asking(config.agent.cold_resume_min_tokens())
                .then_some((last.at_ms, cold))
        });
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let Some((at_ms, cold)) = found.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                // The reader may have moved on while the transcript was read,
                // the agent may have started a turn, or this spell may have
                // been answered already.
                let still_here = shell.cold_resume.checked.as_ref()
                    == Some(&(terminal, session.clone()))
                    && shell.idle_claude_tab().as_ref() == Some(&(terminal, session.clone()));
                let answered = shell.cold_resume.answered.get(&session) == Some(&at_ms);
                if !still_here || answered || shell.modal_open() {
                    return;
                }
                shell.cold_resume.dialog = Some(ColdResume {
                    terminal,
                    session,
                    at_ms,
                    cold,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The tab the reader is on, when it is a Claude session that is idle,
    /// and that session.
    fn idle_claude_tab(&self) -> Option<(TerminalId, String)> {
        let TabKind::Terminal(terminal) = self.space()?.active_tab()?.kind.clone() else {
            return None;
        };
        let session = self.idle_session(terminal)?;
        let claude = self
            .terminals
            .get(&terminal)
            .and_then(|handle| handle.agent.as_deref())
            .is_some_and(|agent| ket_core::agents::harness(agent) == "claude");
        claude.then_some((terminal, session))
    }

    /// The session `terminal`'s agent is in, while it is idle.
    ///
    /// Idle as the sidebar reckons it, not merely as last reported: a session
    /// just resumed reports Starting until its first turn, though its agent
    /// is up and waiting — and that is exactly the session this asks about.
    /// Its subagents count too: a lead at its prompt with one still working
    /// in the background has work that ending it would lose.
    fn idle_session(&self, terminal: TerminalId) -> Option<String> {
        let key = self.terminal_keys.get(&terminal)?;
        let session = self.activity.pane_status(key)?.session.clone()?;
        let idle = matches!(
            self.activity.pane(key, ket_core::now_ms()),
            Some((
                Activity::Session {
                    state: AgentState::Idle,
                    ..
                },
                Signal::Quiet
            ))
        );
        idle.then_some(session)
    }

    /// The session `terminal`'s agent has reported, idle or not: proof its
    /// agent started, where an open terminal is only proof its shell did.
    fn reported_session(&self, terminal: TerminalId) -> Option<String> {
        let key = self.terminal_keys.get(&terminal)?;
        self.activity.pane_status(key)?.session.clone()
    }

    /// Leaves the session as it is.
    fn cold_resume_continue(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.cold_resume.dialog.take() {
            self.cold_resume
                .answered
                .insert(dialog.session, dialog.at_ms);
        }
        cx.notify();
    }

    /// Starts fresh from the question.
    fn cold_resume_start_fresh(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.cold_resume.dialog.take() else {
            return;
        };
        self.cold_resume
            .answered
            .insert(dialog.session.clone(), dialog.at_ms);
        if let Err(why) =
            self.start_fresh_beside(dialog.terminal, &dialog.session, dialog.at_ms, None, cx)
        {
            self.toast(Tone::Error, why, cx);
        }
        cx.notify();
    }

    /// Starts fresh in the terminals phones asked about, if each is still
    /// the cold spell the phone was shown: the host checked, but a turn can
    /// have started between its look and this one.
    pub(crate) fn take_phone_freshes(&mut self, cx: &mut Context<Self>) {
        while let Some(fresh) = ket_core::host::take_phone_fresh() {
            let terminal = self
                .terminal_keys
                .iter()
                .find(|(_, held)| **held == fresh.key)
                .map(|(terminal, _)| *terminal);
            let Some(terminal) = terminal else {
                ket_core::host::finish_phone_fresh(
                    fresh.req,
                    Err("that tab isn't open in ket on your Mac any more".to_owned()),
                );
                continue;
            };
            let started = self.start_fresh_beside(
                terminal,
                &fresh.session,
                fresh.since_ms,
                Some(fresh.req),
                cx,
            );
            if let Err(why) = started {
                ket_core::host::finish_phone_fresh(fresh.req, Err(why));
                continue;
            }
            // Answered from the phone: the desktop's own question about it
            // has nothing left to ask.
            self.cold_resume
                .answered
                .insert(fresh.session, fresh.since_ms);
            if self
                .cold_resume
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.terminal == terminal)
            {
                self.cold_resume.dialog = None;
            }
        }
    }

    /// Opens a fresh session of `terminal`'s agent in its pane, beside the
    /// cold one, which stays open: a prompt half-written in it — typed here,
    /// or from a phone, which goes through the host and leaves the window no
    /// trace of it — is not ket's to close, and an idle session costs nothing
    /// until it is sent to. The old conversation stays in the agent's
    /// history. `Err` says why nothing was started.
    ///
    /// Refused unless `terminal` is still idle in `session` and its last
    /// request is still the one at `at_ms`: one used since — from the
    /// phone, or a prompt whose turn is still in flight — is not cold. When
    /// `req` names a phone's request, the host is told the fresh tab's key
    /// once its agent has reported itself started, or why there is none.
    ///
    /// Its worktree is selected first when it is not, the way a phone's new
    /// session is: a tab opens in the worktree on screen.
    fn start_fresh_beside(
        &mut self,
        terminal: TerminalId,
        session: &str,
        at_ms: u64,
        req: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.idle_session(terminal).as_deref() != Some(session) {
            return Err("It's working, or it's moved on — nothing was started fresh".to_owned());
        }
        if last_call(session).is_none_or(|last| last.at_ms != at_ms) {
            return Err("That session has been used since — it isn't cold any more".to_owned());
        }
        let agent = self
            .terminals
            .get(&terminal)
            .and_then(|handle| handle.agent.clone())
            .ok_or("That tab isn't an agent's any more")?;
        let (worktree, pane) = self
            .spaces
            .iter()
            .find_map(|(worktree, space)| {
                space
                    .root
                    .find_tab(&TabKind::Terminal(terminal))
                    .map(|(pane, _)| (worktree.clone(), pane))
            })
            .ok_or("That tab isn't open any more")?;
        if self.selected_id().as_ref() != Some(&worktree) {
            let selection = self
                .position_of(&worktree)
                .ok_or("That worktree isn't in the sidebar any more")?;
            self.suppress_auto_session = true;
            self.select(selection, cx);
            self.suppress_auto_session = false;
        }
        let before = self.next_terminal;
        self.open_agent_tab_in(pane, &agent, cx);
        if self.next_terminal == before {
            return Err(format!("Couldn't open a fresh {agent} session"));
        }
        let fresh = TerminalId(before);
        let Some(req) = req else {
            return Ok(());
        };
        // Told to the phone only once the fresh agent has said it started, so
        // the phone is never sent to a shell whose agent failed to.
        let fresh_key = self.terminal_keys.get(&fresh).cloned();
        let session = session.to_owned();
        cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let mut waited = Duration::ZERO;
            let answer = loop {
                let state = shell
                    .update(cx, |shell, _| {
                        (
                            shell.terminal_keys.contains_key(&fresh),
                            shell
                                .reported_session(fresh)
                                .is_some_and(|reported| reported != session),
                        )
                    })
                    .ok();
                match state {
                    // Gone: the window, or the fresh tab closed meanwhile.
                    None | Some((false, _)) => {
                        break Err("the fresh tab was closed before it started".to_owned());
                    }
                    Some((true, true)) => {
                        break fresh_key
                            .ok_or_else(|| "the fresh tab has no key to find it by".to_owned());
                    }
                    Some((true, false)) if waited >= FRESH_OPEN => {
                        break Err("the fresh session didn't start on your Mac".to_owned());
                    }
                    Some((true, false)) => {}
                }
                cx.background_executor().timer(FRESH_POLL).await;
                waited += FRESH_POLL;
            };
            ket_core::host::finish_phone_fresh(req, answer);
        })
        .detach();
        Ok(())
    }

    /// Handles a key while the question is up: Enter and Escape both
    /// continue, the choice that changes nothing.
    pub(crate) fn cold_resume_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.cold_resume.dialog.is_none() {
            return false;
        }
        if matches!(event.keystroke.key.as_str(), "escape" | "enter") {
            self.cold_resume_continue(cx);
        }
        true
    }

    /// The question, or nothing when it is not up.
    pub(crate) fn cold_resume_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.cold_resume.dialog.as_ref()?;
        let t = &self.theme;

        let fresh = button("cold-resume-fresh", "Start fresh")
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| this.cold_resume_start_fresh(cx)));
        let continue_ = button("cold-resume-continue", "Continue")
            .primary()
            .hint("⏎")
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| this.cold_resume_continue(cx)));

        // The frost mark leads the title, in the dim ink of a label: a cold
        // cache is a fact to weigh, not a fault, and colour is kept for state.
        let title = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(icon(Icon::Snowflake, paint(t.text.dim)).size(px(16.0)))
            .child(
                div()
                    .text_size(crate::ui::TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(paint(t.text.primary))
                    .child("This session went cold"),
            );
        Some(
            centered(
                card("cold-resume", px(340.0), t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(title)
                    .child(body(format!("Continuing {}.", dialog.cold.summary()), t))
                    .child(footer().child(fresh).child(continue_)),
            )
            .into_any_element(),
        )
    }
}

/// The newest request `session` made, off the end of its transcript.
fn last_call(session: &str) -> Option<LastCall> {
    let path: PathBuf = ket_core::conversation::claude_transcript(session)?;
    ket_core::cold::claude_last_call(&path)
}
