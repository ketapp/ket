//! Asking, as a tab whose agent ran out of usage is opened, whether to
//! wait for the limit to reset or hand the task to another agent: the Mac's
//! side of the phone's limit sheet (`mobile/app/src/components/LimitSheet.tsx`),
//! asked the way every question in [`crate::session_asks`] is.
//!
//! One dialog in two steps, as on the phone. First the stop: what ran out,
//! when it resets, and a bar of the wait. Wait for it closes it; Use Claude
//! unfolds the same card into the hand-over: which agent, how much of the
//! conversation it is told, where it opens, and the brief itself word for
//! word, since it goes to another company's model. Nothing is sent until
//! Send. The hand-over is wider than the question, to show the brief.
//!
//! Asked once per stop. A stop answered for is not asked about again; a
//! new one, after the session got going and was refused again, is.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, AsyncApp, Context, KeyDownEvent, MouseButton, Pixels, WeakEntity, Window, div,
    prelude::*, px, relative,
};
use ket_core::handoff::{Brief, Depth, Why, brief_of_session, label};
use ket_core::limits::{Limit, Scope, hand_choices, stop_sentence, waited};

use crate::Shell;
use crate::paint::paint;
use crate::status_bar::tabular;
use crate::tabs::TabKind;
use crate::terminal::{SessionLaunch, TerminalId};
use crate::ui::banner::banner;
use crate::ui::button::button;
use crate::ui::dialog::{body, card, centered, footer, header};
use crate::ui::field;
use crate::ui::group::{button_group, segment};
use crate::ui::icon::Icon;
use crate::ui::progress::progress;
use crate::ui::textarea::textarea;
use crate::ui::toast::Tone;
use crate::ui::{CAPTION, LABEL};

/// The hand-over's width: room for the brief.
const HAND_WIDTH: Pixels = px(480.0);

/// The well the brief is shown in.
const BRIEF_H: Pixels = px(180.0);

/// How often the wait is redrawn while the question is up: its words are to
/// the minute.
const TICK: Duration = Duration::from_secs(20);

/// How much of the conversation the brief carries, as the choice is worded.
const DEPTHS: [(Depth, &str); 3] = [
    (Depth::Task, "The task"),
    (Depth::Recent, "Recent"),
    (Depth::Full, "Everything"),
];

/// The question, while it is up.
pub(crate) struct OutOfUsage {
    /// The tab whose agent stopped.
    terminal: TerminalId,
    /// That agent, as ket names it.
    agent: String,
    /// The session the limit stopped.
    session: String,
    /// The stop.
    limit: Limit,
    /// Whom ket offers the task to — see [`offered`]. `None` when there is
    /// nobody, and only waiting is offered.
    to: Option<String>,
    /// The hand-over, once Use is pressed.
    handing: Option<Handing>,
}

/// How much an agent that could take the task has left, as far as ket knows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Left {
    /// Nothing says it is short.
    Plenty,
    /// A window of its plan reads full, or one of its models is spent: it
    /// may well still take the task — another model, or a reading from
    /// before a reset — so it is offered, and said.
    Low,
    /// A session of it is stopped at its plan's limit, which stands.
    Out,
}

/// Whom ket offers the task to: the first of `choices` with plenty left,
/// else the first with any.
fn offered(choices: Vec<(String, Left)>) -> Option<String> {
    let first = |left: Left| {
        choices
            .iter()
            .find(|(_, has)| *has == left)
            .map(|(agent, _)| agent.clone())
    };
    first(Left::Plenty).or_else(|| first(Left::Low))
}

/// The second step: the choices, and the brief they make.
struct Handing {
    /// The agents that could take the task, each with how much it has left.
    choices: Vec<(String, Left)>,
    /// The agent chosen.
    to: String,
    /// How much of the conversation it is told.
    depth: Depth,
    /// A new tab beside this one, rather than in its place.
    beside: bool,
    /// The brief at `depth`.
    brief: Preview,
    /// Which read of it `brief` waits for, so a slow read for a depth
    /// since changed, or for a dialog since closed, lands on nothing.
    asked: u64,
}

/// The brief, as far as it has been read.
enum Preview {
    Reading,
    Ready(Brief),
    Failed(String),
}

/// Everything the question needs between frames.
#[derive(Default)]
pub(crate) struct OutOfUsageState {
    /// The question, while it is up.
    pub(crate) dialog: Option<OutOfUsage>,
    /// Stops already answered for: each session, by when it was stopped.
    answered: HashMap<String, u64>,
    /// Whether the redraw for the wait is running.
    ticking: bool,
    /// How many briefs have been read, in every dialog: each read's number.
    reads: u64,
}

impl Shell {
    /// Asks about the tab the reader is on when its agent is stopped at a
    /// limit that has not been asked about. Called every frame, and cheap:
    /// the stop is on the pane's status, which the host keeps.
    pub(crate) fn check_out_of_usage(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.modal_open() || !window.is_window_active() {
            return;
        }
        let Some(TabKind::Terminal(terminal)) = self
            .space()
            .and_then(|space| space.active_tab())
            .map(|tab| tab.kind.clone())
        else {
            return;
        };
        let Some((agent, session, limit)) = self.out_of_usage_in(terminal) else {
            return;
        };
        if self.asks.usage.answered.get(&session) == Some(&limit.since_ms) {
            return;
        }
        let to = offered(self.hand_choices(&agent));
        self.asks.usage.dialog = Some(OutOfUsage {
            terminal,
            agent,
            session,
            limit,
            to,
            handing: None,
        });
        self.tick_out_of_usage(cx);
        cx.notify();
    }

    /// The agent in `terminal`, its session and the limit it is stopped at,
    /// while that limit stands.
    pub(crate) fn out_of_usage_in(&self, terminal: TerminalId) -> Option<(String, String, Limit)> {
        let key = self.terminal_keys.get(&terminal)?;
        let status = self.activity.pane_status(key)?;
        let limit = status
            .limit
            .clone()
            .filter(|limit| !limit.over(ket_core::now_ms()))?;
        Some((status.agent.clone(), status.session.clone()?, limit))
    }

    /// The agents that could take over from `stopped`, as the phone offers
    /// them, each with how much it has left — see [`Left`].
    fn hand_choices(&self, stopped: &str) -> Vec<(String, Left)> {
        let now = ket_core::now_ms();
        let agents = ket_core::host::published_agents();
        let limited = self.activity.limited_harnesses(now);
        let full: Vec<&'static str> = self
            .rate_limits
            .snapshots()
            .into_iter()
            .filter(|snapshot| {
                snapshot.windows.iter().any(|window| {
                    window.used_percent >= 99.0 && window.resets_at_ms.is_none_or(|at| at > now)
                })
            })
            .map(|snapshot| harness_of(snapshot.provider))
            .collect();
        hand_choices(stopped, &agents)
            .into_iter()
            .map(|agent| {
                let harness = ket_core::agents::harness(agent).to_ascii_lowercase();
                let stop = |scope| limited.contains(&(harness.clone(), scope));
                let left = if stop(Scope::Plan) {
                    Left::Out
                } else if stop(Scope::Model) || full.contains(&harness.as_str()) {
                    Left::Low
                } else {
                    Left::Plenty
                };
                (agent.to_owned(), left)
            })
            .collect()
    }

    /// Redraws the question while it is up, so its wait counts down, and
    /// puts it away when the stop is over before it was answered.
    fn tick_out_of_usage(&mut self, cx: &mut Context<Self>) {
        if self.asks.usage.ticking {
            return;
        }
        self.asks.usage.ticking = true;
        cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                let up = shell
                    .update(cx, |shell, cx| {
                        let state = &mut shell.asks.usage;
                        let over = state.dialog.as_ref().is_some_and(|dialog| {
                            dialog.handing.is_none()
                                && stop_gone(dialog, &shell.terminal_keys, &shell.activity)
                        });
                        if over {
                            state.dialog = None;
                        }
                        let up = state.dialog.is_some();
                        if !up {
                            state.ticking = false;
                        }
                        cx.notify();
                        up
                    })
                    .unwrap_or(false);
                if !up {
                    break;
                }
            }
        })
        .detach();
    }

    /// Closes the question, its stop answered for: Wait for it, the ✕ and
    /// Escape.
    fn out_of_usage_wait(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.asks.usage.dialog.take() {
            self.asks
                .usage
                .answered
                .insert(dialog.session, dialog.limit.since_ms);
        }
        cx.notify();
    }

    /// Answers for `terminal`'s stop from elsewhere — a phone handed its
    /// task over — so the question about it has nothing left to ask.
    pub(crate) fn out_of_usage_settled(&mut self, terminal: TerminalId) {
        let state = &mut self.asks.usage;
        if state
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.terminal == terminal)
            && let Some(dialog) = state.dialog.take()
        {
            state.answered.insert(dialog.session, dialog.limit.since_ms);
        }
    }

    /// Unfolds the question into the hand-over, its brief read at the
    /// default depth.
    fn out_of_usage_unfold(&mut self, cx: &mut Context<Self>) {
        let Some(stopped) = self.asks.usage.dialog.as_ref().map(|d| d.agent.clone()) else {
            return;
        };
        let choices = self.hand_choices(&stopped);
        let Some(dialog) = self.asks.usage.dialog.as_mut() else {
            return;
        };
        let Some(to) = dialog.to.clone() else {
            return;
        };
        dialog.handing = Some(Handing {
            choices,
            to,
            depth: Depth::default(),
            beside: false,
            brief: Preview::Reading,
            asked: 0,
        });
        self.read_brief(cx);
    }

    /// Reads the brief at the hand-over's depth, off the window's thread:
    /// it is a transcript read, up to its whole length for Everything.
    fn read_brief(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.asks.usage;
        let Some(dialog) = state.dialog.as_mut() else {
            return;
        };
        let Some(handing) = dialog.handing.as_mut() else {
            return;
        };
        state.reads += 1;
        handing.asked = state.reads;
        handing.brief = Preview::Reading;
        let asked = handing.asked;
        let (agent, session, depth) = (dialog.agent.clone(), dialog.session.clone(), handing.depth);
        let source = session.clone();
        let reading = cx
            .background_executor()
            .spawn(async move { brief_of_session(&agent, &session, depth, Why::Limit) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let read = reading.await;
            let _ = shell.update(cx, |shell, cx| {
                let handing = shell
                    .asks
                    .usage
                    .dialog
                    .as_mut()
                    .filter(|dialog| dialog.session == source)
                    .and_then(|dialog| dialog.handing.as_mut())
                    .filter(|handing| handing.asked == asked);
                if let Some(handing) = handing {
                    handing.brief = match read {
                        Ok(brief) => Preview::Ready(brief),
                        Err(why) => Preview::Failed(why),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Changes something about the hand-over.
    fn out_of_usage_choose(&mut self, choose: impl FnOnce(&mut Handing), cx: &mut Context<Self>) {
        let Some(handing) = self
            .asks
            .usage
            .dialog
            .as_mut()
            .and_then(|dialog| dialog.handing.as_mut())
        else {
            return;
        };
        let depth = handing.depth;
        choose(handing);
        if handing.depth != depth {
            self.read_brief(cx);
        }
        cx.notify();
    }

    /// Back from the hand-over to the question.
    fn out_of_usage_back(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.asks.usage.dialog.as_mut() {
            dialog.handing = None;
        }
        cx.notify();
    }

    /// Sends the brief shown: opens the chosen agent, types it in, and —
    /// in this tab — puts it in the old agent's place. The stop counts as
    /// answered while that goes on, so it isn't asked about meanwhile; one
    /// that didn't go is asked about again, to try again or wait.
    fn out_of_usage_send(&mut self, cx: &mut Context<Self>) {
        let ready = self
            .asks
            .usage
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.handing.as_ref())
            .is_some_and(|handing| matches!(handing.brief, Preview::Ready(_)));
        if !ready {
            return;
        }
        let Some(dialog) = self.asks.usage.dialog.take() else {
            return;
        };
        self.asks
            .usage
            .answered
            .insert(dialog.session.clone(), dialog.limit.since_ms);
        let Some(Handing {
            to,
            beside,
            brief: Preview::Ready(brief),
            ..
        }) = dialog.handing
        else {
            return;
        };
        let (session, since_ms) = (dialog.session.clone(), dialog.limit.since_ms);
        let sent = self.switch_tab(
            dialog.terminal,
            Some(dialog.session.clone()),
            &to,
            SessionLaunch::default(),
            brief.text,
            beside,
            cx,
            move |shell, outcome| {
                if outcome.is_err() {
                    shell.out_of_usage_unanswer(&session, since_ms);
                }
            },
        );
        if let Err(why) = sent {
            self.out_of_usage_unanswer(&dialog.session, since_ms);
            self.toast_detail(
                Tone::Error,
                format!("{} didn't get the task", label(&to)),
                why,
                cx,
            );
        }
        cx.notify();
    }

    /// Lets the stop `session` was refused at, at `since_ms`, be asked about
    /// again: the hand-over its answer started didn't go.
    fn out_of_usage_unanswer(&mut self, session: &str, since_ms: u64) {
        let answered = &mut self.asks.usage.answered;
        if answered.get(session) == Some(&since_ms) {
            answered.remove(session);
        }
    }

    /// Handles a key while the question is up: Escape waits, Enter unfolds
    /// the hand-over, and ⌘Enter in it sends.
    pub(crate) fn out_of_usage_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dialog) = self.asks.usage.dialog.as_ref() else {
            return false;
        };
        let handing = dialog.handing.is_some();
        let offered = dialog.to.is_some();
        let command = event.keystroke.modifiers.platform;
        match event.keystroke.key.as_str() {
            "escape" => self.out_of_usage_wait(cx),
            "enter" if !handing && offered => self.out_of_usage_unfold(cx),
            "enter" if !handing => self.out_of_usage_wait(cx),
            "enter" if command => self.out_of_usage_send(cx),
            _ => {}
        }
        true
    }

    /// The question, or nothing when it is not up.
    pub(crate) fn out_of_usage_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.asks.usage.dialog.as_ref()?;
        let slab = match &dialog.handing {
            None => self.stop_view(dialog, cx),
            Some(handing) => self.hand_view(dialog, handing, cx),
        };
        Some(
            centered(slab.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                .into_any_element(),
        )
    }

    /// The first step: the stop, its wait, and the two choices.
    fn stop_view(&self, dialog: &OutOfUsage, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        let now = ket_core::now_ms();
        let who = label(&dialog.agent);

        // The gauge leads the title in the quota's own orange: a limit is a
        // state, and that is what colour is kept for. Closing waits, as
        // Escape does.
        let title = header("out-of-usage-close", "Out of usage")
            .mark(Icon::Gauge, paint(t.quota.hot))
            .on_close(cx.listener(|this, _, _, cx| this.out_of_usage_wait(cx)))
            .render(t);
        let wait = dialog.limit.resets_at_ms.filter(|at| *at > now).map(|at| {
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    progress(waited(&dialog.limit, now))
                        .grow()
                        .height(px(6.0))
                        .fill(paint(t.quota.hot))
                        .render(t),
                )
                .child(
                    tabular(div())
                        .text_size(CAPTION)
                        .text_color(paint(t.text.dim))
                        .child(clock(at)),
                )
        });

        let mut wait_button = button("out-of-usage-wait", "Wait for it");
        if dialog.to.is_none() {
            wait_button = wait_button.primary().hint("⏎");
        }
        let wait_button = wait_button
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| this.out_of_usage_wait(cx)));
        let use_button = dialog.to.as_deref().map(|to| {
            button("out-of-usage-use", format!("Use {}", label(to)))
                .primary()
                .leading(crate::ui::agent::provider(to, t).0)
                .hint("⏎")
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| this.out_of_usage_unfold(cx)))
        });

        card("out-of-usage", super::WIDTH, t)
            .child(title)
            .child(body(stop_sentence(&who, &dialog.limit, now), t))
            .children(wait)
            .child(footer().child(wait_button).children(use_button))
    }

    /// The second step: who takes the task, what it is told, where.
    fn hand_view(
        &self,
        dialog: &OutOfUsage,
        handing: &Handing,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        let who = label(&dialog.agent);
        let name = label(&handing.to);
        let until = dialog
            .limit
            .resets_at_ms
            .filter(|at| *at > ket_core::now_ms())
            .map_or_else(
                || format!("{who} is out of usage."),
                |at| format!("{who} is out of usage until {}.", clock(at)),
            );

        let title = header("out-of-usage-close", "Continue with…")
            .on_close(cx.listener(|this, _, _, cx| this.out_of_usage_wait(cx)))
            .render(t);

        let agents = button_group("out-of-usage-agent").fill().children(
            handing.choices.iter().enumerate().map(|(at, (agent, _))| {
                let (mark, tint) = crate::ui::agent::provider(agent, t);
                let chosen = agent.clone();
                segment(("out-of-usage-agent", at), label(agent))
                    .leading(mark, tint)
                    .selected(*agent == handing.to)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let chosen = chosen.clone();
                        this.out_of_usage_choose(|handing| handing.to = chosen, cx);
                    }))
            }),
        );
        let left = handing
            .choices
            .iter()
            .find(|(agent, _)| *agent == handing.to)
            .map_or(Left::Plenty, |(_, left)| *left);
        let short = match left {
            Left::Plenty => None,
            Left::Low => Some(format!("{name} is nearly out of usage.")),
            Left::Out => Some(format!("{name} is out of usage too.")),
        };

        let depths =
            button_group("out-of-usage-depth")
                .fill()
                .children(DEPTHS.iter().enumerate().map(|(at, (depth, words))| {
                    let depth = *depth;
                    segment(("out-of-usage-depth", at), *words)
                        .selected(depth == handing.depth)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.out_of_usage_choose(|handing| handing.depth = depth, cx);
                        }))
                }));

        let tokens = match &handing.brief {
            Preview::Ready(brief) => format!("about {} tokens", brief.approx_tokens),
            _ => String::new(),
        };
        let brief = match &handing.brief {
            Preview::Reading => body("Reading the conversation…", t),
            Preview::Failed(why) => {
                banner("out-of-usage-brief-failed", Tone::Error, why.clone(), t).into_any_element()
            }
            Preview::Ready(brief) => textarea(
                "out-of-usage-brief",
                div()
                    .id("out-of-usage-brief-text")
                    .size_full()
                    .overflow_y_scroll()
                    .pr(px(12.0))
                    .pb(px(10.0))
                    .text_size(LABEL)
                    .line_height(relative(1.5))
                    .text_color(paint(t.text.primary))
                    .child(brief.text.clone())
                    .into_any_element(),
                BRIEF_H,
            )
            .inset((px(10.0), px(12.0)))
            .read_only()
            .render(t)
            .into_any_element(),
        };

        let places = button_group("out-of-usage-place").fill().children(
            [(false, "In this tab"), (true, "In a new tab beside it")]
                .into_iter()
                .enumerate()
                .map(|(at, (beside, words))| {
                    segment(("out-of-usage-place", at), words)
                        .selected(beside == handing.beside)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.out_of_usage_choose(|handing| handing.beside = beside, cx);
                        }))
                }),
        );
        let place_note = (!handing.beside).then(|| {
            body(
                format!("{who}'s session stays in its history, to resume once its limit resets."),
                t,
            )
        });

        let back = button("out-of-usage-back", "Back")
            .ghost()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| this.out_of_usage_back(cx)));
        let send = button("out-of-usage-send", format!("Send to {name}"))
            .primary()
            .hint("⌘⏎")
            .enabled(matches!(handing.brief, Preview::Ready(_)))
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| this.out_of_usage_send(cx)));

        card("out-of-usage", HAND_WIDTH, t)
            .child(title)
            .child(body(until, t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(field::label("Agent", t))
                    .child(agents.render(t))
                    .children(short.map(|short| div().pt(px(6.0)).child(body(short, t)))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(field::label("What it's told", t))
                    .child(depths.render(t)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        field::label("", t)
                            .flex()
                            .child(format!("{name} will be sent this"))
                            .child(div().flex_grow())
                            .child(tokens),
                    )
                    .child(brief),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(field::label("Where", t))
                    .child(places.render(t))
                    .children(place_note.map(|note| div().pt(px(6.0)).child(note))),
            )
            .child(footer().child(back).child(send))
    }
}

/// Whether the stop `dialog` asks about is over: the limit reset, or the
/// session got going again, as its pane last reported.
fn stop_gone(
    dialog: &OutOfUsage,
    keys: &HashMap<TerminalId, String>,
    activity: &ket_core::activity::Tracker,
) -> bool {
    let now = ket_core::now_ms();
    let standing = keys
        .get(&dialog.terminal)
        .and_then(|key| activity.pane_status(key))
        .and_then(|status| status.limit.as_ref())
        .is_some_and(|limit| !limit.over(now) && limit.since_ms == dialog.limit.since_ms);
    !standing
}

/// When a limit resets, on this Mac's clock: `4:37 pm`, as the phone says it.
fn clock(at_ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at_ms as i64).map_or_else(
        String::new,
        |time| {
            time.with_timezone(&chrono::Local)
                .format("%-I:%M %P")
                .to_string()
        },
    )
}

/// The harness a provider's usage is reckoned for.
fn harness_of(provider: ket_core::rate_limits::Provider) -> &'static str {
    use ket_core::rate_limits::Provider;
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::OpenCode => "opencode",
        Provider::Grok => "grok",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices(left: &[(&str, Left)]) -> Vec<(String, Left)> {
        left.iter()
            .map(|(agent, left)| ((*agent).to_owned(), *left))
            .collect()
    }

    #[test]
    fn a_nearly_spent_agent_is_still_offered_and_a_stopped_one_never() {
        let offer = |left: &[(&str, Left)]| offered(choices(left));
        assert_eq!(
            offer(&[("claude", Left::Low), ("opencode", Left::Plenty)]),
            Some("opencode".to_owned()),
            "one with plenty first"
        );
        assert_eq!(
            offer(&[("claude", Left::Out), ("opencode", Left::Low)]),
            Some("opencode".to_owned()),
            "else one that may still take it"
        );
        assert_eq!(offer(&[("claude", Left::Out)]), None);
        assert_eq!(offer(&[]), None);
    }
}
