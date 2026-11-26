//! When an agent stops because the plan behind it has run out.
//!
//! [`crate::rate_limits`] says how much of a plan is used. This says a turn
//! *ended* on it: the agent was told no, and nothing more will happen in that
//! chat until the window resets or the person moves the work elsewhere.
//!
//! The two are kept apart on purpose. A usage window at 100% is not a limit
//! hit: Claude goes on working at 100% of a session window when the account
//! has extra usage, and at 100% of one model's weekly window when it falls
//! back to another model. Only the turn knows whether it was refused, so a
//! hit is read from how the turn ended, and the windows are left to say when
//! it resets.
//!
//! [`classify`] is the whole of that reading: one agent's words for a turn
//! that failed, in, and whether that was a limit, out. It is pure, so every
//! wording an agent has been seen to use is a line in a test rather than a
//! thing to reproduce by running out of quota.
//!
//! What is then known — which sessions are stopped, and until when — is kept
//! in a [`Ledger`]. The host fills it in and everything that shows a limit
//! reads it; see `host/server/limits.rs` for the filling.

use serde::{Deserialize, Serialize};

mod ledger;
mod words;

pub use ledger::{Ledger, Limit, Mark, resets_at};
pub use words::{hand_choices, stop_sentence, wait_words, waited};

/// How a turn ended badly, as its agent reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    /// The agent's own name for the kind of error, where it gives one.
    /// Claude Code does: `rate_limit`, `server_error`,
    /// `authentication_failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// What it said, in its words.
    #[serde(default)]
    pub message: String,
}

impl Failure {
    /// A failure known only by what was said.
    pub fn said(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }

    /// A failure the agent named as well as described.
    pub fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: Some(code.into()),
            message: message.into(),
        }
    }
}

/// What ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// The plan: no model of this agent's will answer until it resets.
    Plan,
    /// One model's allowance. The same agent goes on with another model, so
    /// changing model is the first thing to offer, before changing agent.
    Model,
}

/// Which of a plan's windows ran out, where the agent said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Window {
    /// The short rolling one: Claude's five hours.
    Session,
    /// The weekly one.
    Weekly,
    /// Not said.
    Unknown,
}

/// A turn that ended because a limit was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// What ran out.
    pub scope: Scope,
    /// Which window, where known.
    pub window: Window,
}

/// Whether `failure`, the way a turn of `agent`'s ended, is a limit reached.
///
/// `agent` is the agent's name as ket knows it, a local agent's included:
/// its harness decides whose wordings apply (see [`crate::agents::harness`]).
/// `None` for any other failure — a dropped connection, an expired login, a
/// server error — which is over when the person tries again and is no reason
/// to move the work.
pub fn classify(agent: &str, failure: &Failure) -> Option<Hit> {
    // Codex writes "You’ve" with a typographic apostrophe; the phrases below
    // are spelled with a plain one.
    let said = failure.message.to_lowercase().replace('’', "'");
    let code = failure.code.as_deref();
    match crate::agents::harness(agent) {
        "claude" => claude(code, &said),
        "codex" => codex(code, &said),
        _ => general(&said),
    }
}

/// Claude Code names its errors, so the name decides where there is one, and
/// the words only say which limit. As Claude Code 2.1 writes them:
///
/// ```text
/// You've hit your session limit · resets 10:40pm (America/New_York)
/// You've hit your weekly limit · resets Oct 19 at 1am (America/New_York)
/// You've reached your Opus limit. Run /usage-credits to continue or switch models with /model.
/// ```
fn claude(code: Option<&str>, said: &str) -> Option<Hit> {
    let limit = match code {
        Some(code) => code == "rate_limit",
        None => {
            let yours = said.contains("you've hit your") || said.contains("you've reached your");
            (yours && said.contains("limit")) || said.contains("usage limit")
        }
    };
    if !limit {
        return None;
    }
    let scope = if said.contains("/model") || said.contains("switch models") {
        Scope::Model
    } else {
        Scope::Plan
    };
    Some(Hit {
        scope,
        window: window(said),
    })
}

/// Codex names the plan running out `usage_limit_exceeded` in its rollout,
/// and says "You've hit your usage limit. Upgrade to Pro … or try again at
/// 4:37 PM." Its other refusals — "exceeded retry limit, last status: 429",
/// a per-minute "Rate limit reached … try again in 1.2s" — are over in
/// seconds and are not this.
fn codex(code: Option<&str>, said: &str) -> Option<Hit> {
    (code == Some("usage_limit_exceeded") || said.contains("usage limit")).then(|| Hit {
        scope: Scope::Plan,
        window: window(said),
    })
}

/// Every other agent: Grok, OpenCode and whatever backend OpenCode's key is
/// for, each with its own words. This is only ever asked about a turn that
/// already failed, so it looks for the ways a provider says "no more for
/// now" rather than for one exact sentence. "Limit" alone is never enough:
/// a context window and an output cap are limits too.
fn general(said: &str) -> Option<Hit> {
    const SAYS_SO: [&str; 12] = [
        "usage limit",
        "rate limit",
        "rate_limit",
        "rate-limit",
        "ratelimit",
        "too many requests",
        "quota",
        "balance exhausted",
        "credit balance",
        "out of credits",
        "insufficient credits",
        "payment required",
    ];
    let limit = SAYS_SO.iter().any(|phrase| said.contains(phrase))
        || has_status(said, "429")
        || has_status(said, "402");
    limit.then(|| Hit {
        scope: Scope::Plan,
        window: window(said),
    })
}

/// Which window the words name, if they name one.
fn window(said: &str) -> Window {
    if said.contains("weekly limit") || said.contains("week limit") {
        Window::Weekly
    } else if said.contains("session limit")
        || said.contains("5-hour limit")
        || said.contains("five-hour limit")
    {
        Window::Session
    } else {
        Window::Unknown
    }
}

/// Whether `said` has HTTP status `status` standing as a number of its own,
/// so a 429 is found in "status 429" and "(429)" and not in "4290 tokens" or
/// in a request id.
fn has_status(said: &str, status: &str) -> bool {
    let alone = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric());
    said.match_indices(status).any(|(at, found)| {
        alone(said[..at].chars().next_back()) && alone(said[at + found.len()..].chars().next())
    })
}
