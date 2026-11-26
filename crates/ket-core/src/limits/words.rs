//! What a stop says to the person, and who is offered the task.
//!
//! Said the same on the Mac as on the phone, whose `lib/limits.ts` says it
//! in TypeScript: the two are kept in step, as the notices are.

use super::{Limit, Scope};

/// The one sentence a stop is told in, the phone's `stopSentence`:
/// `Codex has reached its usage limit and resets in 2 hours 6 minutes.`
/// `who` is the agent as a person reads it — see `crate::handoff::label`.
pub fn stop_sentence(who: &str, limit: &Limit, now_ms: u64) -> String {
    let what = match limit.scope {
        Scope::Model => format!("{who} has reached its limit for this model"),
        Scope::Plan => format!("{who} has reached its usage limit"),
    };
    match limit
        .resets_at_ms
        .and_then(|at| at.checked_sub(now_ms))
        .filter(|left| *left > 0)
    {
        Some(left) => format!("{what} and resets in {}.", wait_words(left)),
        None => format!("{what}."),
    }
}

/// A wait to the minute, in its two largest units, the phone's
/// `waitWords`: `2 hours 6 minutes`, `1 day 3 hours`, `45 minutes`.
pub fn wait_words(ms: u64) -> String {
    let minutes = ms.div_ceil(60_000).max(1);
    let unit = |count: u64, name: &str| {
        if count == 1 {
            format!("1 {name}")
        } else {
            format!("{count} {name}s")
        }
    };
    let (days, hours, mins) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    match (days, hours, mins) {
        (0, 0, mins) => unit(mins, "minute"),
        (0, hours, 0) => unit(hours, "hour"),
        (0, hours, mins) => format!("{} {}", unit(hours, "hour"), unit(mins, "minute")),
        (days, 0, _) => unit(days, "day"),
        (days, hours, _) => format!("{} {}", unit(days, "day"), unit(hours, "hour")),
    }
}

/// How much of the wait has gone, `0.0..=1.0`, from the stop to the reset:
/// the bar under [`stop_sentence`]. Nothing gone when the reset is not known.
pub fn waited(limit: &Limit, now_ms: u64) -> f32 {
    let Some(resets) = limit.resets_at_ms.filter(|at| *at > limit.since_ms) else {
        return 0.0;
    };
    let gone = now_ms.saturating_sub(limit.since_ms) as f64;
    (gone / (resets - limit.since_ms) as f64).clamp(0.0, 1.0) as f32
}

/// The agents of `agents` that could take over from `stopped`, in order:
/// any of another harness, never a model run bare on Ollama — a local model
/// is offered through OpenCode, which can work in a worktree. The phone's
/// `handChoices`.
pub fn hand_choices<'a>(stopped: &str, agents: &'a [String]) -> Vec<&'a str> {
    let own = harness(stopped);
    agents
        .iter()
        .map(String::as_str)
        .filter(|agent| {
            let theirs = harness(agent);
            theirs != own && theirs != "ollama"
        })
        .collect()
}

/// An agent's harness, as its name's first part spells it, in lower case.
fn harness(agent: &str) -> String {
    crate::agents::harness(agent.trim()).to_ascii_lowercase()
}
