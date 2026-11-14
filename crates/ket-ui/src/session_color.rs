//! A Claude session's `/color` on its sidebar row and its tab: its mark on a
//! tile of the colour, and the name beside it in the colour — see
//! `ket_core::agent_color`, which reads it from the session's transcript.
//!
//! Read off the window's thread each tick, for the sessions this window has
//! tabs for, and kept here for drawing: a frame only looks it up.

use std::collections::HashMap;

use gpui::{AsyncApp, Context, Div, Pixels, WeakEntity, div, prelude::*, px};
use ket_core::agent_color::SessionColor;
use ket_core::theme::{Color, Theme, contrast_ratio};

use crate::Shell;
use crate::paint::paint;
use crate::ui::icon::{Icon, sized_icon};

/// The tile on a sidebar row: a step smaller than the project badge heading
/// the column it stands in, so the two don't read as the same thing.
pub(crate) const ROW_TILE: Pixels = px(18.0);

/// The tile on a tab, inside the tab mark's own square.
const TAB_TILE: Pixels = px(16.0);

/// The mark on a tab's tile.
const TAB_MARK: Pixels = px(12.0);

/// The sessions' colours, as last read.
#[derive(Default)]
pub(crate) struct SessionColors {
    by_session: HashMap<String, SessionColor>,
    /// A read under way, so ticks don't pile them up.
    reading: bool,
}

impl Shell {
    /// Reads the `/color` of every session this window has a tab for, off its
    /// thread, and redraws when one changed.
    pub(crate) fn refresh_session_colors(&mut self, cx: &mut Context<Self>) {
        if self.session_colors.reading {
            return;
        }
        let sessions: Vec<(String, String)> = self
            .terminal_keys
            .values()
            .filter_map(|key| {
                let status = self.activity.pane_status(key)?;
                Some((status.agent.clone(), status.session.clone()?))
            })
            .collect();
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

    /// The colour of the session running in the tab with `key`.
    pub(crate) fn pane_color(&self, key: &str) -> Option<SessionColor> {
        let session = self.activity.pane_status(key)?.session.as_deref()?;
        self.session_color(session)
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

/// `which` on a `side` tile of the session's colour, in whichever of the
/// card and the text stands off the tile more — the rule a project badge's
/// letter follows.
pub(crate) fn tiled(
    which: Icon,
    mark: Pixels,
    side: Pixels,
    color: SessionColor,
    t: &Theme,
) -> Div {
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
        .size(side)
        .rounded(crate::ui::RADIUS_SM)
        .bg(paint(fill))
        .child(sized_icon(which, mark, paint(ink)))
}

/// A tab's mark on its tile, in the tab mark's own square, faded with the
/// rest of an inactive tab's mark.
pub(crate) fn tab_mark(which: Icon, color: SessionColor, dim: bool, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(crate::ui::filetype::SIZE)
        .when(dim, |el| el.opacity(crate::ui::filetype::DIMMED))
        .child(tiled(which, TAB_MARK, TAB_TILE, color, t))
}
