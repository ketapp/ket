//! Which sessions a limit has stopped, and until when.
//!
//! A [`Ledger`] is a list of [`Mark`]s, one per session whose last turn was
//! refused. It holds a level, not a history: a mark is there for as long as
//! its session stands where the limit left it, and is taken off when the
//! session gets going again. Everything that reacts to a limit — a card in
//! the chat, a banner on the pane, a notification — reads the level and
//! works out for itself what changed, the way it already does for a prompt
//! that is waiting.
//!
//! Marks are kept by session, not by pane or by provider. A pane is gone the
//! moment an idle tab is put to sleep, hours before a weekly window resets;
//! and a provider is not one account, when two projects sign `claude` into
//! different ones. The session is what was refused, and it outlives the tab.

use serde::{Deserialize, Serialize};

use super::{Hit, Scope, Window};
use crate::rate_limits::RateWindow;

/// How long a mark is kept with no word of a reset: a day past the longest
/// window any plan has, a week.
const KEPT_MS: u64 = 8 * 24 * 60 * 60 * 1000;

/// How long a mark is kept after its window reset. Long enough for "it's
/// back" to be seen by someone who was away overnight.
const KEPT_AFTER_RESET_MS: u64 = 24 * 60 * 60 * 1000;

/// How full a window has to be to count as the one that ran out.
const FULL_PERCENT: f32 = 99.0;

/// A limit a session is stopped at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limit {
    /// What ran out.
    pub scope: Scope,
    /// Which window, where the agent said.
    pub window: Window,
    /// When the session was first refused. A retry refused again does not
    /// move it: it is the same stop.
    pub since_ms: u64,
    /// When the window resets, where anything said. `None` is "not known",
    /// never "never".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    /// The agent's own words for it.
    #[serde(default)]
    pub said: String,
}

impl Limit {
    /// Whether the window has reset by `now_ms`, so the session can go on.
    pub fn over(&self, now_ms: u64) -> bool {
        self.resets_at_ms.is_some_and(|at| at <= now_ms)
    }
}

/// One session stopped at a limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    /// The harness whose session it is: `claude`, `codex`.
    pub agent: String,
    /// The agent's own id for the session.
    pub session: String,
    /// Which of the agent's accounts, where it has more than one: for
    /// Claude, the config directory the session is filed under. Empty for
    /// an agent with one login.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account: String,
    /// The pane the session was last seen in, by its key. A tab put to
    /// sleep keeps its key and loses everything else the host knew of it, so
    /// this is how an asleep tab is still known to be stopped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pane: String,
    /// The limit.
    #[serde(flatten)]
    pub limit: Limit,
}

/// The sessions stopped at a limit — see the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    marks: Vec<Mark>,
}

impl Ledger {
    /// Every mark, oldest first.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// The limit `agent`'s session `session` is stopped at, if it is.
    pub fn of(&self, agent: &str, session: &str) -> Option<&Limit> {
        self.marks
            .iter()
            .find(|mark| mark.is(agent, session))
            .map(|mark| &mark.limit)
    }

    /// The limit the session last seen in pane `pane` is stopped at.
    pub fn in_pane(&self, pane: &str) -> Option<&Limit> {
        self.marks
            .iter()
            .find(|mark| !mark.pane.is_empty() && mark.pane == pane)
            .map(|mark| &mark.limit)
    }

    /// Notes that `agent`'s session `session` is now in pane `pane`: resumed
    /// in another tab, it is stopped there; and a stop of another session
    /// last seen there, after `/new` or a resume in that tab, no longer is.
    /// That one keeps its mark, by its session. Returns whether that was news.
    pub fn seen_in(&mut self, agent: &str, session: &str, pane: &str) -> bool {
        let mut news = false;
        for mark in &mut self.marks {
            if mark.is(agent, session) {
                if mark.pane != pane {
                    pane.clone_into(&mut mark.pane);
                    news = true;
                }
            } else if !pane.is_empty() && mark.pane == pane {
                mark.pane.clear();
                news = true;
            }
        }
        news
    }

    /// Records that `mark`'s session was refused. Returns whether anything
    /// changed.
    ///
    /// A session already marked, whose window has not reset, is the same
    /// stop: its `since_ms` stays, so nothing that announced the stop
    /// announces it again for a retry, and a reset time it had is kept where
    /// the retry brought none. One whose window had reset is stopped anew.
    pub fn hit(&mut self, mark: Mark) -> bool {
        let mark = Mark {
            agent: harness(&mark.agent),
            ..mark
        };
        let Some(held) = self
            .marks
            .iter_mut()
            .find(|held| held.is(&mark.agent, &mark.session))
        else {
            self.marks.push(mark);
            return true;
        };
        let merged = if held.limit.over(mark.limit.since_ms) {
            mark
        } else {
            Mark {
                limit: Limit {
                    since_ms: held.limit.since_ms,
                    resets_at_ms: mark.limit.resets_at_ms.or(held.limit.resets_at_ms),
                    said: if mark.limit.said.is_empty() {
                        held.limit.said.clone()
                    } else {
                        mark.limit.said
                    },
                    ..mark.limit
                },
                account: if mark.account.is_empty() {
                    held.account.clone()
                } else {
                    mark.account
                },
                pane: if mark.pane.is_empty() {
                    held.pane.clone()
                } else {
                    mark.pane
                },
                ..mark
            }
        };
        let changed = *held != merged;
        *held = merged;
        changed
    }

    /// Takes the mark off `agent`'s session `session`: it got going again.
    /// Returns whether it had one.
    pub fn clear(&mut self, agent: &str, session: &str) -> bool {
        let before = self.marks.len();
        self.marks.retain(|mark| !mark.is(agent, session));
        self.marks.len() != before
    }

    /// Fills in when the window resets, for a mark made before anything
    /// said. Returns whether it did; a time already known is left alone.
    pub fn learn_reset(&mut self, agent: &str, session: &str, resets_at_ms: u64) -> bool {
        match self
            .marks
            .iter_mut()
            .find(|mark| mark.is(agent, session) && mark.limit.resets_at_ms.is_none())
        {
            Some(mark) => {
                mark.limit.resets_at_ms = Some(resets_at_ms);
                true
            }
            None => false,
        }
    }

    /// Drops marks nothing will come back to: a day past their reset, or
    /// eight days old with no reset ever named. Returns whether any went.
    pub fn prune(&mut self, now_ms: u64) -> bool {
        let before = self.marks.len();
        self.marks.retain(|mark| match mark.limit.resets_at_ms {
            Some(at) => now_ms < at.saturating_add(KEPT_AFTER_RESET_MS),
            None => now_ms < mark.limit.since_ms.saturating_add(KEPT_MS),
        });
        self.marks.len() != before
    }
}

impl Mark {
    fn is(&self, agent: &str, session: &str) -> bool {
        self.session == session && self.agent == harness(agent)
    }
}

/// The harness of `agent`, in the one spelling marks are kept by.
fn harness(agent: &str) -> String {
    crate::agents::harness(agent).to_ascii_lowercase()
}

/// When the window `hit` ran out of resets, read from `windows`: the plan's
/// windows as last reported, by a status line, a rollout or a fetch.
///
/// The window the agent named, where it named one and that window says when.
/// Otherwise the latest reset among the windows that are full: with a
/// session window and a weekly one both spent, work resumes when the weekly
/// one does. `None` when no window is full, which is what an old reading
/// looks like — a time taken from one would be a guess shown as a fact. A
/// time already past by `now_ms` is the same: a reading from before the
/// last reset.
pub fn resets_at(hit: Hit, windows: &[RateWindow], now_ms: u64) -> Option<u64> {
    let ahead = |window: &RateWindow| window.resets_at_ms.filter(|&at| at > now_ms);
    let named = match (hit.scope, hit.window) {
        (Scope::Plan, Window::Session) => Some("Session"),
        (Scope::Plan, Window::Weekly) => Some("Weekly"),
        _ => None,
    };
    if let Some(at) = named.and_then(|name| {
        windows
            .iter()
            .find(|window| window.name == name)
            .and_then(ahead)
    }) {
        return Some(at);
    }
    windows
        .iter()
        .filter(|window| window.used_percent >= FULL_PERCENT)
        .filter_map(ahead)
        .max()
}
