//! A Claude session's `/color` on its worktree's sidebar row: its mark on a
//! tile of the colour, and the name beside it in the colour — see
//! `ket_core::agent_color`, which reads it from the session's transcript.
//!
//! Read off the window's thread each tick, for the sessions this window has
//! tabs for, and kept here for drawing: a frame only looks it up.

use std::collections::HashMap;

use gpui::{AnyElement, AsyncApp, Context, Div, Pixels, WeakEntity, div, prelude::*, px};
use ket_core::activity::Activity;
use ket_core::agent_color::SessionColor;
use ket_core::id::WorktreeId;
use ket_core::theme::{Color, Theme, contrast_ratio};

use crate::Shell;
use crate::paint::paint;
use crate::ui::agent::{MARK, provider};
use crate::ui::icon::{Icon, sized_icon};

/// The sessions' colours, as last read.
#[derive(Default)]
pub(crate) struct SessionColors {
    by_session: HashMap<String, SessionColor>,
    /// A read under way, so ticks don't pile them up.
    reading: bool,
}

impl Shell {
    /// Reads the `/color` of every session a row may be drawn for, off the
    /// window's thread, and redraws when one changed: the host's panes', not
    /// only those of tabs this window has opened, since a worktree's tabs
    /// wait to be put back until it is first shown.
    pub(crate) fn refresh_session_colors(&mut self, cx: &mut Context<Self>) {
        if self.session_colors.reading {
            return;
        }
        let sessions = self.activity.known_sessions();
        if sessions.is_empty() && self.session_colors.by_session.is_empty() {
            return;
        }
        self.session_colors.reading = true;
        cx.spawn(async move |shell: WeakEntity<Shell>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    sessions
                        .into_iter()
                        .filter_map(|(agent, session)| {
                            ket_core::agent_color::of(&agent, &session)
                                .map(|color| (session, color))
                        })
                        .collect::<HashMap<_, _>>()
                })
                .await;
            let _ = shell.update(cx, |shell, cx| {
                shell.session_colors.reading = false;
                if shell.session_colors.by_session != read {
                    shell.session_colors.by_session = read;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// `session`'s colour, as last read.
    pub(crate) fn session_color(&self, session: &str) -> Option<SessionColor> {
        self.session_colors.by_session.get(session).copied()
    }

    /// The colour a row for `worktree`, doing `activity`, wears while its
    /// mark is an agent's. The session it speaks for first; then its newest
    /// on disk, for a row whose tabs this window has not put back yet; then
    /// any of its panes running that agent, latest heard first, since a
    /// `/color` fires no hook and the pane given it is often one whose
    /// report has lapsed. Every list of worktrees asks this: grouped, sorted
    /// by status, the folded rail's.
    pub(crate) fn row_color(
        &self,
        worktree: &WorktreeId,
        activity: &Activity,
        now_ms: u64,
    ) -> Option<SessionColor> {
        let agent = activity.agent()?;
        let reported = self
            .activity
            .leading_session(worktree, now_ms)
            .map(|(_, session)| session);
        let recorded = match activity {
            Activity::Recorded { session, .. } => Some(session.as_str()),
            _ => None,
        };
        let harness = ket_core::agents::harness(agent);
        reported
            .into_iter()
            .chain(recorded)
            .chain(
                self.activity
                    .pane_sessions(worktree)
                    .into_iter()
                    .filter(|(pane_agent, _)| ket_core::agents::harness(pane_agent) == harness)
                    .map(|(_, session)| session),
            )
            .find_map(|session| self.session_color(session))
    }
}

fn fill(color: SessionColor) -> Color {
    Color::from_hex(color.hex()).expect("every session colour's hex parses")
}

/// A name in the session's colour: moved towards the text until it reads at
/// 4.5:1 on the sidebar, as a pinned project's pin is.
pub(crate) fn label(color: SessionColor, t: &Theme) -> Color {
    let fill = fill(color);
    (0..=20)
        .map(|step| fill.mix(t.text.primary, f64::from(step) / 20.0))
        .find(|candidate| contrast_ratio(*candidate, t.panel) >= 4.5)
        .unwrap_or(t.text.primary)
}

/// The tile a coloured session's mark stands on: square, and a step smaller
/// than the project badge heading the column, so the two don't read as the
/// same thing.
const ROW_TILE: Pixels = px(18.0);

/// `which` on a square tile of the session's colour, in whichever of the
/// card and the text stands off the tile more — the rule a project badge's
/// letter follows. One size wherever a worktree is listed.
fn tiled(which: Icon, color: SessionColor, t: &Theme) -> Div {
    let fill = fill(color);
    let ink = if contrast_ratio(t.surface, fill) >= contrast_ratio(t.text.primary, fill) {
        t.surface
    } else {
        t.text.primary
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(ROW_TILE)
        .rounded(crate::ui::RADIUS_SM)
        .bg(paint(fill))
        .child(sized_icon(which, MARK, paint(ink)))
}

/// A worktree row's mark: its agent's on a tile of the session's colour,
/// else the state light every row has.
pub(crate) fn row_mark(
    activity: &Activity,
    color: Option<SessionColor>,
    dirty: bool,
    missing: bool,
    t: &Theme,
) -> AnyElement {
    match (color, activity) {
        (
            Some(color),
            Activity::Session { agent, .. }
            | Activity::Observed { agent, .. }
            | Activity::Recorded { agent, .. }
            | Activity::Ended { agent, .. },
        ) => tiled(provider(agent, t).0, color, t).into_any_element(),
        _ => crate::tree::row_mark(activity, dirty, missing, t),
    }
}
