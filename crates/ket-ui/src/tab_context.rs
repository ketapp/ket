//! A tab's context mark: how full its agent's context window is, drawn only
//! once that is worth doing something about.
//!
//! The reading is the one [`crate::usage_card`] already files per worktree,
//! from Claude's status line and from Codex's rollout, as a
//! [`SessionUsage`]. What this adds is which tab a reading belongs to, and
//! when the tab says so.
//!
//! **High and critical only.** Below [`crate::status_bar::HOT`] nothing is
//! drawn: most of a session is spent with room to spare, and a gauge on every
//! tab all day is the dashboard the usage card cut context pressure to avoid
//! becoming. From there the ring takes the quota ramp's hot and critical
//! colours, so it reads as the same warning the quota meters give.
//!
//! **On the tab, not the worktree's row.** A worktree can hold several agents,
//! each with its own window, and the row would have to pick one of them.
//!
//! **Matched by session first.** A pane's hook reports name its agent's
//! session, and a reading names the session that drew it, so the two meet
//! there. That is the only join Codex has, since its rollout knows nothing of
//! panes; for Claude it is what keeps a tab that has just run `/clear` from
//! wearing the session it left. A pane that has named no session falls back to
//! the reading filed against the pane itself.

use gpui::{AnyElement, Context, Pixels, canvas, div, prelude::*, px};
use ket_core::event::SessionUsage;
use ket_core::id::WorktreeId;
use ket_core::theme::Theme;

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::tabs::TabKind;
use crate::terminal::TerminalId;
use crate::ui::tooltip::{Side, tooltip};

/// The ring's side: a step under the tab's 16px mark cell, so it sits beside
/// the label as an annotation rather than as a second mark.
const RING: Pixels = px(12.0);

/// The track's and the arc's stroke, the sidebar's Economy ring's hairline.
const STROKE: Pixels = px(1.7);

impl Shell {
    /// The marks after one tab's label in `worktree`'s strip, when it is a
    /// terminal: what its agent left running in the background (see
    /// `tree::jobs`), then this ring once its agent is past the line. What the
    /// tab strip calls. An asleep tab has no agent running to show either.
    pub(crate) fn tab_marks(
        &self,
        worktree: Option<&WorktreeId>,
        kind: &TabKind,
        now_ms: u64,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let TabKind::Terminal(terminal) = kind else {
            return Vec::new();
        };
        if self.asleep_terminals.contains_key(terminal) {
            return Vec::new();
        }
        let jobs = self.tab_jobs_mark(*terminal, t, cx);
        let context = worktree
            .and_then(|worktree| self.tab_context(worktree, *terminal, now_ms))
            .and_then(|usage| self.context_mark(*terminal, usage, t, cx));
        jobs.into_iter().chain(context).collect()
    }

    /// What `terminal`'s agent last said about its context window, while that
    /// agent is still there to have said it.
    ///
    /// `None` for a tab whose agent's report has lapsed — the session may
    /// have ended and left a shell behind — and for one ket has heard no
    /// measured reading from: absent is not empty, and an unknown window is
    /// drawn as nothing rather than as room to spare.
    fn tab_context(
        &self,
        worktree: &WorktreeId,
        terminal: TerminalId,
        now_ms: u64,
    ) -> Option<&SessionUsage> {
        // The same two keys `terminal_status` tries: the one ket handed the
        // agent's environment, and the bare id older launches used.
        let numeric = terminal.0.to_string();
        let key = [
            self.terminal_keys.get(&terminal).map(String::as_str),
            Some(numeric.as_str()),
        ]
        .into_iter()
        .flatten()
        .find(|key| self.activity.pane(key, now_ms).is_some())?;
        let session = self
            .activity
            .pane_status(key)
            .and_then(|status| status.session.as_deref());
        let usage = &self
            .session_usage
            .get(worktree)?
            .for_tab(session, terminal)?
            .reading
            .usage;
        usage.fraction().is_some().then_some(usage)
    }

    /// The ring a tab wears once its context is at least
    /// [`crate::status_bar::HOT`] full, with the numbers on hover.
    fn context_mark(
        &self,
        terminal: TerminalId,
        usage: &SessionUsage,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let fraction = usage.fraction()?;
        // Floored, so the figure on hover never reads a step higher than the
        // ring's own colour says.
        let percent = (fraction * 100.0).floor().min(100.0) as f32;
        if percent < crate::status_bar::HOT {
            return None;
        }
        let ink = paint(crate::status_bar::pressure(percent, t)?);
        let track = alpha(paint(t.text.dim), 0.22);
        let sweep = fraction.min(1.0) as f32;

        let ring = div()
            .flex_none()
            .size(RING)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        crate::ui::chip::stroke_arc(bounds, 0.0, 1.0, track, STROKE, window);
                        crate::ui::chip::stroke_arc(bounds, 0.0, sweep, ink, STROKE, window);
                    },
                )
                .size_full(),
            )
            .into_any_element();
        let label = format!(
            "{} of {} context \u{b7} {percent:.0}%",
            crate::usage_card::tokens(usage.used),
            crate::usage_card::tokens(usage.size),
        );

        let hovered = self.hovered_context == Some(terminal);
        Some(
            tooltip(
                ("tab-context", terminal.0),
                ring,
                label,
                Side::Bottom,
                hovered,
                t,
            )
            .flex_none()
            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                if *is_hovered {
                    this.hovered_context = Some(terminal);
                } else if this.hovered_context == Some(terminal) {
                    this.hovered_context = None;
                }
                cx.notify();
            }))
            .into_any_element(),
        )
    }
}
