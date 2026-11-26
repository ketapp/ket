//! A tab's task handed to another agent: the new agent opened in its own
//! tab, beside the old one or in its place, its first message the brief of
//! what the old one was doing — see `ket_core::handoff`. Asked for from a
//! phone, or from the Mac's out-of-usage question
//! (`crate::session_asks`); both come through [`Shell::switch_tab`].
//!
//! "In its place" is done as open, then move, then close: the new tab opens
//! at the end of the pane's strip, waits for its agent to be ready and gets
//! the brief, and only then takes the old tab's slot while the old tab
//! closes. An agent that never comes up leaves the old tab where it was.
//! The old agent's session stays in its own history, to resume from the
//! session list once its limit resets.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Context, WeakEntity};
use ket_core::activity::Foreground;
use ket_core::event::AgentState;
use ket_core::host::PhoneSwitch;
use ket_core::subagents::SubagentState;

use crate::Shell;
use crate::phone_work::{LANDING_POLL, TERMINAL_OPEN, wait_for_agent_input};
use crate::tabs::{TabDrop, TabKind};
use crate::terminal::{SessionLaunch, TerminalId};
use crate::ui::toast::Tone;

impl Shell {
    /// Does the switches phones asked for.
    pub(crate) fn take_phone_switches(&mut self, cx: &mut Context<Self>) {
        while let Some(switch) = ket_core::host::take_phone_switch() {
            let req = switch.req;
            if let Err(why) = self.switch_from_phone(switch, cx) {
                ket_core::host::finish_phone_switch(req, Err(why));
            }
        }
    }

    /// Opens `switch.agent` for the tab `switch.key` names and hands it the
    /// brief; the host is answered once the agent has it. `Err` says why
    /// nothing was opened.
    fn switch_from_phone(
        &mut self,
        switch: PhoneSwitch,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let old = self
            .terminal_keys
            .iter()
            .find(|(_, key)| **key == switch.key)
            .map(|(terminal, _)| *terminal)
            .ok_or("that tab isn't open in ket on your Mac any more")?;
        let req = switch.req;
        let agent = crate::ui::agent::agent_label(&switch.agent);
        self.switch_tab(
            old,
            switch.session,
            &switch.agent,
            SessionLaunch {
                model: switch.model,
                effort: switch.effort,
            },
            switch.text,
            switch.beside,
            cx,
            move |shell, outcome| {
                let answer = outcome.and_then(|opened| {
                    shell
                        .terminal_keys
                        .get(&opened)
                        .cloned()
                        .ok_or_else(|| format!("{agent}'s tab closed before it had the task"))
                });
                ket_core::host::finish_phone_switch(req, answer);
            },
        )?;
        // Answered from the phone: the desktop's own question about this
        // tab's stop has nothing left to ask.
        self.out_of_usage_settled(old);
        Ok(())
    }

    /// Opens `agent` for tab `old`'s task and hands it `text`: beside `old`,
    /// or — unless `beside` — in its place once the agent has it. `held` is
    /// the session `text` was read from, where known: once `old` holds
    /// another, nothing is opened. `then` is told the new agent's terminal,
    /// or why it didn't get the task, which is also toasted. `Err` says why
    /// nothing was opened.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn switch_tab(
        &mut self,
        old: TerminalId,
        held: Option<String>,
        agent: &str,
        launch: SessionLaunch,
        text: String,
        beside: bool,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Shell, Result<TerminalId, String>) + 'static,
    ) -> Result<(), String> {
        // What the old tab is doing as the switch begins. It gives up its
        // place only if it is just as it was at the end: used meantime, it
        // is someone's work and stays.
        let before = self.untouched(old);
        if let (Some(read), Some(holds)) = (held.as_deref(), before.session.as_deref())
            && read != holds
        {
            return Err("that tab has moved on to another session since".to_owned());
        }
        let opened = self.open_beside(old, agent, launch, cx)?;
        let name = crate::ui::agent::agent_label(agent);
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let outcome = hand_over(&shell, opened, &text, cx).await;
            let _ = shell.update(cx, |shell, cx| {
                let outcome = match outcome {
                    Ok(()) => {
                        let replace = !beside;
                        if replace && shell.left_as_it_was(old, &before) {
                            shell.take_place_of(old, opened);
                        } else if replace {
                            shell.toast_detail(
                                Tone::Info,
                                format!("{name} has the task, beside the old tab"),
                                "The old tab was picked up again, so it stays open.".to_owned(),
                                cx,
                            );
                        }
                        Ok(opened)
                    }
                    Err(why) => {
                        shell.toast_detail(
                            Tone::Error,
                            format!("{name} didn't get the task"),
                            why.clone(),
                            cx,
                        );
                        Err(why)
                    }
                };
                then(shell, outcome);
                cx.notify();
            });
        })
        .detach();
        Ok(())
    }

    /// Opens `agent` in a new tab at the end of the strip `old` is in, its
    /// worktree shown first. The new tab's terminal, or why there is none.
    fn open_beside(
        &mut self,
        old: TerminalId,
        agent: &str,
        launch: SessionLaunch,
        cx: &mut Context<Self>,
    ) -> Result<TerminalId, String> {
        let (worktree, pane) = self
            .spaces
            .iter()
            .find_map(|(worktree, space)| {
                space
                    .root
                    .find_tab(&TabKind::Terminal(old))
                    .map(|(pane, _)| (worktree.clone(), pane))
            })
            .ok_or("that tab isn't open in ket on your Mac any more")?;
        if self.selected_id().as_ref() != Some(&worktree) {
            let selection = self
                .position_of(&worktree)
                .ok_or("that worktree isn't in ket's sidebar any more")?;
            self.suppress_auto_session = true;
            self.select(selection, cx);
            self.suppress_auto_session = false;
        }
        let before = self.next_terminal;
        self.open_agent_tab_with(pane, agent, launch, cx);
        if self.next_terminal == before {
            return Err(format!(
                "ket couldn't open {} on your Mac",
                crate::ui::agent::agent_label(agent)
            ));
        }
        Ok(TerminalId(before))
    }

    /// What tab `terminal` is doing, as far as ket can tell.
    fn untouched(&self, terminal: TerminalId) -> Untouched {
        let status = self
            .terminal_keys
            .get(&terminal)
            .and_then(|key| self.activity.pane_status(key));
        Untouched {
            session: status.and_then(|status| status.session.clone()),
            heard_at_ms: status.map_or(0, |status| status.heard_at_ms()),
            typed_at_ms: self.terminals.get(&terminal).map_or(0, |handle| {
                handle
                    .typed_at_ms
                    .load(std::sync::atomic::Ordering::Acquire)
            }),
        }
    }

    /// Whether tab `old` is just as it was `before`, and at rest: nothing
    /// typed into it, no word from its agent or a subagent of its, the same
    /// session, and none of them at work.
    fn left_as_it_was(&self, old: TerminalId, before: &Untouched) -> bool {
        let status = self
            .terminal_keys
            .get(&old)
            .and_then(|key| self.activity.pane_status(key));
        let busy = status.is_some_and(|status| {
            matches!(
                status.state,
                AgentState::Thinking | AgentState::ExecutingTool | AgentState::AwaitingPermission
            ) || status
                .subagents()
                .iter()
                .any(|subagent| subagent.state != SubagentState::Idle)
        });
        self.untouched(old) == *before && !busy
    }

    /// Puts tab `new` where tab `old` is, and closes `old`: the new agent
    /// takes the old one's place in the strip.
    fn take_place_of(&mut self, old: TerminalId, new: TerminalId) {
        for space in self.spaces.values_mut() {
            let (Some((pane, at)), Some((from, index))) = (
                space.root.find_tab(&TabKind::Terminal(old)),
                space.root.find_tab(&TabKind::Terminal(new)),
            ) else {
                continue;
            };
            if from == pane && index != at {
                space.move_tab(from, index, TabDrop::Strip { pane, before: at });
            }
            break;
        }
        self.close_terminal(old);
    }
}

/// What a tab was doing as a switch began — see [`Shell::left_as_it_was`].
#[derive(PartialEq)]
struct Untouched {
    /// The session its agent last said it holds.
    session: Option<String>,
    /// When its agent, or one of its subagents, last reported.
    heard_at_ms: u64,
    /// When anything was last typed into it here.
    typed_at_ms: u64,
}

/// Waits for `terminal`'s agent to be up and ready for words, then types
/// `text` into it and sends it; `Ok` once Enter has gone in. Nothing is
/// typed unless the agent is known to be in front: a program other than the
/// shell it was launched from, and for Claude and Codex their input box on
/// screen. Typed into a shell, or a screen still starting, the brief would
/// run as commands or be lost.
async fn hand_over(
    shell: &WeakEntity<Shell>,
    terminal: TerminalId,
    text: &str,
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    while waited < TERMINAL_OPEN {
        let open = shell
            .update(cx, |shell, _| shell.terminals.contains_key(&terminal))
            .unwrap_or(false);
        if open {
            break;
        }
        cx.background_executor().timer(LANDING_POLL).await;
        waited += LANDING_POLL;
    }
    if !wait_for_agent_input(shell, terminal, text, cx).await {
        return Err("its terminal didn't open".to_owned());
    }
    // Claude's wait gives up on its input box at a cap rather than failing;
    // this one doesn't, and gives a slow start longer.
    let mut waited = Duration::ZERO;
    while !ready_for_words(shell, terminal, cx)? {
        if waited >= TERMINAL_OPEN {
            return Err("its input box didn't show".to_owned());
        }
        cx.background_executor().timer(LANDING_POLL).await;
        waited += LANDING_POLL;
    }
    let entered = Rc::new(Cell::new(None));
    shell
        .update(cx, |shell, cx| {
            let told = entered.clone();
            let done =
                move |_: &mut Shell, went: bool, _: &mut Context<Shell>| told.set(Some(went));
            if harness_in(shell, terminal) == "codex" {
                shell.submit_guarded_to_terminal_then(terminal, text, true, cx, done);
            } else if shell.runs_local_agent(terminal) {
                shell.submit_local_now_then(terminal, text, cx, done)?;
            } else {
                shell.submit_to_terminal_then(terminal, text, cx, done);
            }
            Ok::<(), String>(())
        })
        .map_err(|_| "ket's window closed".to_owned())??;
    loop {
        match entered.get() {
            Some(true) => return Ok(()),
            Some(false) => return Err("it didn't take the brief: its screen changed".to_owned()),
            None => {}
        }
        // Not told at all once the window has gone.
        if shell.update(cx, |_, _| ()).is_err() {
            return Err("ket's window closed".to_owned());
        }
        cx.background_executor().timer(LANDING_POLL).await;
    }
}

/// Whether `terminal`'s agent is in front and showing where words go;
/// `Err` once it can't be: the tab or its agent has gone.
fn ready_for_words(
    shell: &WeakEntity<Shell>,
    terminal: TerminalId,
    cx: &mut gpui::AsyncApp,
) -> Result<bool, String> {
    shell
        .update(cx, |shell, _| {
            let handle = shell
                .terminals
                .get(&terminal)
                .filter(|handle| !handle.term.is_closed())
                .ok_or("its terminal closed before it started")?;
            match handle.term.foreground() {
                Some(Foreground::Running(_)) => {}
                Some(Foreground::Shell) => {
                    return Err("it hadn't started: its tab is at a shell prompt".to_owned());
                }
                // Not known yet — a hosted terminal's first reading.
                None => return Ok(false),
            }
            Ok(match harness_in(shell, terminal).as_str() {
                "claude" => handle
                    .term
                    .with_term(|term| ket_core::screen::claude_prompt_ready(term)),
                "codex" => handle.term.with_term(|term| {
                    ket_core::screen::codex_prompt_ready(term)
                        || ket_core::screen::codex_startup_question(&ket_core::screen::bottom_rows(
                            term,
                            usize::MAX,
                        ))
                        .is_some()
                }),
                // Nothing on their screens says so: the program in front is
                // the agent ket launched, once its start has had its time.
                _ => true,
            })
        })
        .map_err(|_| "ket's window closed".to_owned())?
}

/// The harness of the agent ket launched in `terminal`, in lower case.
fn harness_in(shell: &Shell, terminal: TerminalId) -> String {
    shell
        .terminals
        .get(&terminal)
        .and_then(|handle| handle.agent.as_deref())
        .map(|agent| ket_core::agents::harness(agent).to_ascii_lowercase())
        .unwrap_or_default()
}
