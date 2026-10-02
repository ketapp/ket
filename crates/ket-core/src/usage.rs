//! What one agent session has actually consumed.
//!
//! [`crate::event::SessionUsage`] is the vocabulary; this module is the second
//! way to fill it in. The first is a transport that reports usage itself — ACP
//! publishes a `UsageUpdate` per turn, and [`crate::agent::SessionContext`]
//! records it. A pty-driven session publishes nothing, which is every agent a
//! person started themselves and every agent ket launched into a terminal:
//! the common case, and until now the case with no numbers at all.
//!
//! The one place those sessions do say what they are spending is the payload
//! Claude Code hands its status line, several times a second, whether or not
//! anybody is reading it. ket is already receiving that payload for the quota
//! block in it — see [`crate::rate_limits::snapshot_from_statusline`] — so the
//! context and cost sitting beside the quota are free.
//!
//! **Two of its fields are not what their names say**, and both were read the
//! obvious way here until the documentation was checked against real session
//! history. `cost.total_cost_usd` is an estimate Claude Code computes at list
//! price, which is not the bill on a plan that already paid for the tokens; and
//! `context_window.total_output_tokens` is the last response's output, not the
//! session's. Each is handled where it is read — [`from_statusline`] and
//! [`History::observe`] — and neither is passed on under a name that would
//! invite the same mistake again.
//!
//! **Absent is not zero.** Claude fills `total_input_tokens` in with `0` when
//! it has no reading to give, and a status bar that renders that as "0% of
//! context" is claiming an empty window on a session that may be nearly full.
//! The reading is only believed when `current_usage` is actually there; when it
//! is not, this reports the window as unknown and lets the consumer say so.
//! [`crate::event::SessionUsage::fraction`] already returns `None` for that.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{KetError, Result};
use crate::event::{Money, SessionUsage};
use crate::id::WorktreeId;

/// Everything one status line payload says about the session that drew it.
///
/// More than [`SessionUsage`] carries, because the history needs two things a
/// context-pressure reading has no room for: which session this is, so
/// cumulative numbers can be updated in place rather than added up, and which
/// model produced them, because cost per turn is meaningless across models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Claude's own session id, when the payload names one.
    pub session: Option<String>,
    /// The session's name, once it has one — `--name`, `/rename`, or the
    /// title Claude writes for itself. Absent until then, which is most of a
    /// short session.
    pub name: Option<String>,
    /// The model's display name, e.g. `"Opus 5"`.
    pub model: Option<String>,
    /// Which Claude Code drew it, so a reading that looks wrong can be traced
    /// to a version rather than argued about.
    pub version: Option<String>,
    /// The reasoning effort in force, when the model takes one.
    pub effort: Option<String>,
    /// Whether the session is running in fast mode.
    ///
    /// A premium rate for more output tokens per second, and the only setting
    /// here that costs more the longer nobody notices it is on. It survives
    /// into later sessions, and nothing in a terminal says so — which is the
    /// whole reason to carry it.
    pub fast_mode: bool,
    /// The directory the session is working in.
    ///
    /// Not the same question as which worktree ket launched it for: a session
    /// that has been `cd`'d, or opened with `--add-dir`, is somewhere else,
    /// and a panel claiming otherwise would be guessing.
    pub cwd: Option<String>,
    /// Output tokens in the **most recent response** — not the session's
    /// total, whatever the field's name suggests.
    ///
    /// `context_window.total_output_tokens` is documented as "the output
    /// tokens from the most recent response", alongside `total_input_tokens`,
    /// which is what is *in* the window. Reading it as a session total is what
    /// [`History`] used to do, and it produced an "output per turn" figure
    /// that was really the largest single response ever seen divided by the
    /// number of prompts. [`History::observe`] turns these into a real total
    /// by summing what changes between readings.
    pub last_output_tokens: u64,
    /// Fresh, uncached input tokens in the **most recent response**, from
    /// `context_window.current_usage.input_tokens`.
    ///
    /// The per-class split `total_input_tokens` had already summed away. Kept
    /// because a contracted-rate recompute needs each class at its own price,
    /// and fresh input is the dearest of the three by an order of magnitude
    /// over a cache read.
    ///
    /// Same per-response caveat as [`Reading::last_output_tokens`], and
    /// [`History::observe`] turns it into a total the same way. `0` before the
    /// first API call, and again after a `/compact` until the next one.
    pub last_input_tokens: u64,
    /// Cache-read input tokens in the most recent response, from
    /// `context_window.current_usage.cache_read_input_tokens`. See
    /// [`Reading::last_input_tokens`].
    pub last_cache_read_tokens: u64,
    /// What the session has left behind besides spend.
    pub work: Work,
    /// How well its prompt cache is working, once a response has said.
    pub cache: Option<CacheHealth>,
    /// Whether this session's tokens cost money or plan headroom, once there
    /// is enough in the payload to tell. See [`Billing`].
    pub billing: Option<Billing>,
    /// Context pressure and cost, as the status bar wants them.
    pub usage: SessionUsage,
}

/// How well a session's prompt cache is working.
///
/// The largest single cost lever there is: an agent loop resends the whole
/// conversation every turn, and caching does not stop that — it reprices it at
/// a tenth. Anthropic's own measurement puts a well-cached loop at 2.5–3.7×
/// cheaper. ket has been receiving all of this on every status line and keeping
/// none of it.
///
/// `None` on [`Reading::cache`] rather than a zeroed value of this type, for
/// the reason stated at the top of this module: a session before its first
/// response, and any Claude Code older than 2.1.251, report nothing here, and a
/// zero is a different claim from a silence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CacheHealth {
    /// Whether the cached prefix is inside its lifetime *right now*.
    ///
    /// Goes `false` the moment a response reports no cache tokens — which
    /// happens both when a cache has gone cold and when there was never one to
    /// go cold. [`CacheHealth::observed`] is what tells those apart, and they
    /// want opposite advice.
    pub warm: bool,
    /// Whether any response this session has reported cache tokens at all.
    ///
    /// `false` means caching is off, or something between ket and the model —
    /// an LLM gateway, typically — is dropping the `cache_control` markers that
    /// would turn it on. That is a configuration to fix, and it must never be
    /// drawn as a hit ratio of zero, which is a cache working badly. Two
    /// different problems with two different answers.
    pub observed: bool,
    /// The cache's lifetime, `"5m"` or `"1h"`, when a response has said.
    ///
    /// Kept because it is the receipt for the cheapest lever there is: a
    /// session on an API key defaults to five minutes, and `promptCacheTtl`
    /// moving it to an hour is visible here and nowhere else ket can see.
    pub ttl: Option<String>,
    /// Cache reads as a percentage of all input tokens this session.
    ///
    /// Normalised to a percentage at the parse boundary, the way
    /// [`crate::rate_limits::RateWindow::used_percent`] already normalises its
    /// own and for the same stated reason: Claude reports a `0.0..=1.0`
    /// fraction here, Codex reports token counts to divide (see the plan's
    /// Phase 0), and nothing drawing this should have to know which provider it
    /// came from. `None` while every underlying count is still zero.
    ///
    /// **Also `None` whenever [`CacheHealth::observed`] is false**, whatever the
    /// payload said. A gateway that strips `cache_control` markers leaves Claude
    /// Code reporting `caching_observed: false` beside a perfectly arithmetic
    /// `hit_ratio: 0.0` — nought cache reads out of N input tokens — and that
    /// number is true, useless and dangerous in equal measure: drawn anywhere it
    /// reads as a cache working badly, when the truth is that there is no cache
    /// at all. Those are different problems wanting opposite advice, which is
    /// what [`CacheHealth::observed`] exists to separate.
    ///
    /// The card already asks `observed` first and so never showed the zero. This
    /// makes that ordering unnecessary rather than load-bearing: the ratio is
    /// simply absent when there was nothing to measure, so no future reader can
    /// render 0% by forgetting to check.
    pub hit_percent: Option<u8>,
    /// Requests the main conversation has made this session. Subagents are not
    /// counted, by Claude Code, not by us.
    pub requests: u32,
    /// Requests that re-processed content the cache already held.
    pub misses: u32,
    /// Misses that are not faults: a compaction or a tool-result clearing
    /// rewrote the conversation, so the rebuild is what the session chose to
    /// spend, not something that went wrong.
    pub expected_rebuilds: u32,
    /// What Claude Code blamed the most recent miss on — `tools_changed`,
    /// `system_prompt_changed`, `ttl_expired_5m`, `likely_server_side`.
    ///
    /// The first cause only. The payload carries an array, with counts beside
    /// it for two of the causes, and a hover row has space for one phrase; the
    /// rest is there to reach for when something needs it. Requires Claude
    /// Code 2.1.260.
    pub last_miss_cause: Option<String>,
    /// How many of this session's misses had each cause, keyed by the same
    /// names as [`CacheHealth::last_miss_cause`].
    ///
    /// The difference between an event and a habit. `last_miss_cause` can only
    /// ever describe a miss that has already happened; a count above one says
    /// the same thing is going to keep happening until something changes —
    /// which is the only version of this worth interrupting a reader for.
    pub miss_causes: BTreeMap<String, u32>,
    /// Tokens the next request re-caches if the cache has gone cold by then.
    ///
    /// What resuming after a long break actually costs, and the multiplier
    /// behind "this model switch will cost you $X". `None` immediately after a
    /// compaction, until the next request has measured the rewritten
    /// conversation.
    pub recache_if_cold: Option<u64>,
    /// Every token written to the cache this session, the first request's
    /// initial write included.
    ///
    /// Session-cumulative as the payload publishes it, unlike the per-response
    /// counts in `context_window.current_usage` — so this one is read straight
    /// off rather than summed from deltas, and is exact rather than a floor.
    /// It is the write half of a contracted-rate recompute; see
    /// [`Record::contracted_micros`].
    pub write_tokens: Option<u64>,
}

/// What a session has left behind, in the terms a person recognises.
///
/// Lines only. The payload also carries elapsed and API time, and the card
/// drew them for a while — but a hover panel that answers "is this reduction
/// level doing anything" has no use for how long the session has been open,
/// and every row that does not answer that question is one more to read past.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Work {
    /// Lines the session has added, and removed.
    pub lines_added: u64,
    /// See [`Work::lines_added`].
    pub lines_removed: u64,
}

/// Reads a whole [`Reading`] out of a status line payload.
///
/// `None` on exactly the same terms as [`from_statusline`], which does the
/// judging: a payload with no numbers in it has nothing to record either.
pub fn reading_from_statusline(payload: &serde_json::Value) -> Option<Reading> {
    let usage = from_statusline(payload)?;

    /// A non-empty string at `path`, walked from the payload's root.
    fn text(payload: &serde_json::Value, path: &[&str]) -> Option<String> {
        path.iter()
            .try_fold(payload, |value, key| value.get(key))?
            .as_str()
            .filter(|found| !found.is_empty())
            .map(str::to_owned)
    }

    /// A count at `path`, or zero. Absent and zero mean the same for these:
    /// nothing has happened yet.
    fn count(payload: &serde_json::Value, path: &[&str]) -> u64 {
        path.iter()
            .try_fold(payload, |value, key| value.get(key))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    }

    // Read off the sub-object rather than through long paths from the root:
    // every field below is one key inside it, and half of them are absent on a
    // Claude Code older than the version that added them.
    let cache = payload
        .get("prompt_cache")
        .filter(|block| block.is_object())
        .map(|block| {
            let flag = |key: &str| {
                block
                    .get(key)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or_default()
            };
            let counter = |key: &str| {
                block
                    .get(key)
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default() as u32
            };

            CacheHealth {
                warm: flag("warm"),
                observed: flag("caching_observed"),
                ttl: block
                    .get("ttl")
                    .and_then(serde_json::Value::as_str)
                    .filter(|ttl| !ttl.is_empty())
                    .map(str::to_owned),
                // Only when caching was observed at all — see the field's own
                // docs. A gateway stripping the markers reports a truthful,
                // meaningless zero, and this is where it stops being carried.
                hit_percent: flag("caching_observed")
                    .then(|| {
                        block
                            .get("hit_ratio")
                            .and_then(serde_json::Value::as_f64)
                            .filter(|ratio| ratio.is_finite())
                            .map(|ratio| (ratio.clamp(0.0, 1.0) * 100.0).round() as u8)
                    })
                    .flatten(),
                requests: counter("requests"),
                misses: counter("misses"),
                expected_rebuilds: counter("expected_rebuilds"),
                last_miss_cause: block
                    .get("last_miss_cause")
                    .and_then(|cause| cause.get("causes"))
                    .and_then(serde_json::Value::as_array)
                    .and_then(|causes| causes.first())
                    .and_then(serde_json::Value::as_str)
                    .filter(|cause| !cause.is_empty())
                    .map(str::to_owned),
                miss_causes: block
                    .get("miss_causes")
                    .and_then(serde_json::Value::as_object)
                    .map(|causes| {
                        causes
                            .iter()
                            .filter_map(|(cause, count)| {
                                Some((cause.clone(), count.as_u64()? as u32))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                recache_if_cold: block
                    .get("recache_tokens_if_cold")
                    .and_then(serde_json::Value::as_u64),
                write_tokens: block
                    .get("cache_write_tokens")
                    .and_then(serde_json::Value::as_u64),
            }
        });

    Some(Reading {
        session: text(payload, &["session_id"]),
        name: text(payload, &["session_name"]),
        model: text(payload, &["model", "display_name"]),
        version: text(payload, &["version"]),
        effort: text(payload, &["effort", "level"]),
        fast_mode: payload
            .get("fast_mode")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_default(),
        // `workspace.current_dir` in preference to the `cwd` beside it: the
        // documentation calls them the same value and prefers the former.
        cwd: text(payload, &["workspace", "current_dir"]).or_else(|| text(payload, &["cwd"])),
        // Not folded into `SessionUsage::used`: that field is what is *in* the
        // window, and output tokens are what left it.
        last_output_tokens: count(payload, &["context_window", "total_output_tokens"]),
        last_input_tokens: count(
            payload,
            &["context_window", "current_usage", "input_tokens"],
        ),
        last_cache_read_tokens: count(
            payload,
            &["context_window", "current_usage", "cache_read_input_tokens"],
        ),
        work: Work {
            lines_added: count(payload, &["cost", "total_lines_added"]),
            lines_removed: count(payload, &["cost", "total_lines_removed"]),
        },
        cache,
        billing: billing_from_statusline(payload),
        usage,
    })
}

/// Reads context pressure and cost out of a Claude Code status line payload.
///
/// `None` when the payload carries neither — most payloads on a session that
/// has not sent a request yet, and every payload from something that is not
/// Claude Code.
///
/// **Read against the shape Claude Code 2.1.260 publishes**, whose
/// `context_window` block is
///
/// ```text
/// { total_input_tokens, total_output_tokens, context_window_size,
///   current_usage, used_percentage, remaining_percentage }
/// ```
///
/// where `total_input_tokens` is already the sum of the fresh, cache-write and
/// cache-read input tokens — which is exactly "how much is in the window" —
/// and `current_usage` is the raw reading it was summed from, or absent.
/// Output tokens are deliberately not added in: they are what the turn
/// produced, not what the next turn has to carry.
pub fn from_statusline(payload: &serde_json::Value) -> Option<SessionUsage> {
    let window = payload.get("context_window");

    // The reading itself, not the totals derived from it: the totals are `0`
    // both when the context is empty and when there is nothing to report, and
    // this is the field that tells those apart.
    let measured = window
        .and_then(|w| w.get("current_usage"))
        .is_some_and(serde_json::Value::is_object);

    let number = |key: &str| {
        window
            .and_then(|w| w.get(key))
            .and_then(serde_json::Value::as_u64)
    };

    // **An estimate, not a bill.** Claude Code documents `total_cost_usd` as
    // "computed client-side at list price… may differ from your actual bill",
    // which on a Pro or Max plan it certainly does: the plan is what was paid
    // for, and this is what the same tokens would have cost through the API.
    // ket carries the number because it is the only per-session figure there
    // is, and every surface that shows it has to say what it is — see
    // `ket_ui::status_bar`, which does.
    let cost = payload
        .get("cost")
        .and_then(|cost| cost.get("total_cost_usd"))
        .and_then(serde_json::Value::as_f64)
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
        // The field is named for its currency, so the currency is not a guess.
        .map(|amount| Money::from_amount(amount, "USD"));

    if !measured && cost.is_none() {
        return None;
    }

    Some(SessionUsage {
        used: if measured {
            number("total_input_tokens").unwrap_or_default()
        } else {
            0
        },
        // Zero is this type's own spelling of "window unknown" — see
        // `SessionUsage::fraction` — which is what an unmeasured payload, or
        // one whose window size is missing, has to read as.
        size: if measured {
            number("context_window_size").unwrap_or_default()
        } else {
            0
        },
        cost,
    })
}

/// What a session's tokens actually cost the person running it.
///
/// The distinction the cost figure on its own cannot make, and the one that
/// decides whether reducing tokens is worth anything at all. On a metered key
/// every token is money. On a plan the tokens are already paid for, and what
/// they consume is *headroom* — the five-hour and weekly windows — so a dollar
/// figure shown to that reader is describing a bill nobody will receive.
///
/// Deliberately not called `Currency`: [`Money::currency`] already means USD or
/// EUR a few types away, and two things called currency meaning entirely
/// different kinds of thing is how the wrong one gets read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Billing {
    /// An API key, a cloud provider, or a subscription past its included usage
    /// and drawing on credits. Tokens are billed.
    Metered,
    /// Pro, Max, Team or Enterprise, inside the plan's own allowance. Tokens
    /// are spent against a window, not a balance.
    Plan,
}

/// Which of the two a status line payload is describing, when it can be told.
///
/// **Absence of `rate_limits` does not mean a metered key**, which is the trap
/// here. Claude Code publishes that block only for subscribers *and only after
/// the session's first API response* — so every session, on every plan, looks
/// metered for its first few payloads. Reading absence alone would tell a Max
/// subscriber they were being billed, every time they opened a worktree.
///
/// So absence only counts once the payload shows the session has actually made
/// a request: a cost, a cache reading, or a measured context window. Before any
/// of those, this is `None` — not a guess, and not a default.
pub fn billing_from_statusline(payload: &serde_json::Value) -> Option<Billing> {
    if payload
        .get("rate_limits")
        .is_some_and(|limits| limits.is_object())
    {
        return Some(Billing::Plan);
    }

    let responded = payload
        .get("prompt_cache")
        .is_some_and(serde_json::Value::is_object)
        || payload
            .get("context_window")
            .and_then(|window| window.get("current_usage"))
            .is_some_and(serde_json::Value::is_object)
        || payload
            .get("cost")
            .and_then(|cost| cost.get("total_cost_usd"))
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|spent| spent > 0.0);

    responded.then_some(Billing::Metered)
}

// ---- codex -------------------------------------------------------------------

/// Cache health from one Codex `token_count` payload.
///
/// The same [`CacheHealth`] Claude's status line fills in, so nothing drawing
/// it has to know which agent it came from — the reason
/// [`crate::rate_limits::RateWindow::used_percent`] normalises at this same
/// boundary. Codex has no status line, so the payload comes from its rollout
/// file instead; see [`crate::sessions::CodexRollout`].
///
/// **Three of Claude's fields are inferred here rather than reported, and the
/// ratio is not the obvious one.** Codex publishes cumulative token counts and
/// nothing at all about a cache's lifetime:
///
/// - `hit_percent` is `cached_input_tokens / input_tokens`, **not**
///   `cached / (cached + input)`. The cached count is a *subset* of the input
///   count, not a sibling of it: across 3,690 `token_count` records in the
///   local rollout store, `total_tokens == input_tokens + output_tokens` holds
///   in every one, and `cached_input_tokens` never once exceeds
///   `input_tokens`. The sibling reading would have drawn a session cached at
///   96.8% as one at 49.2% — an excellent cache reported as a failing one.
/// - `observed` is "some request has read from cache", the only evidence Codex
///   gives that caching happens at all. It cannot tell a provider that never
///   caches from a session still on its first request.
/// - `warm` is "the *most recent* request read from cache", off
///   `last_token_usage`. Codex exposes no TTL and no expiry, so this is the
///   nearest true statement rather than the same fact Claude's `warm` reports.
///
/// `requests`, `misses`, `expected_rebuilds`, `last_miss_cause`, `ttl` and
/// `recache_if_cold` stay empty, because Codex reports none of them and a zero
/// would read as "no misses" where the truth is "not measured".
///
/// `cache_write_input_tokens` is deliberately not read: it is `0` in all 3,690
/// of those records, so a cache-write figure would be a row that is always the
/// same number. Worth revisiting if a plan or provider is ever seen to fill it.
pub fn cache_from_codex(payload: &serde_json::Value) -> Option<CacheHealth> {
    let info = payload.get("info")?;
    let count = |usage: Option<&serde_json::Value>, key: &str| -> u64 {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    };

    let total = info.get("total_token_usage");
    let input = count(total, "input_tokens");
    // Clamped rather than trusted: the subset relation is what every record in
    // the store shows, and a future one that broke it should give a ratio of
    // 100% rather than something above it.
    let cached = count(total, "cached_input_tokens").min(input);

    // Nothing has been asked for yet, so there is nothing to divide by. A 0%
    // hit rate on a session that has not made a request is a lie, and the same
    // "absent is not zero" rule this module opens with.
    if input == 0 {
        return None;
    }

    Some(CacheHealth {
        warm: count(info.get("last_token_usage"), "cached_input_tokens") > 0,
        observed: cached > 0,
        ttl: None,
        // The same invariant Claude's path keeps: no cache observed, no ratio.
        // Codex reaches the zero by a different route — `cached_input_tokens`
        // still at nought — but a reader shown "0%" would draw exactly the wrong
        // conclusion either way.
        hit_percent: (cached > 0).then(|| ((cached as f64 / input as f64) * 100.0).round() as u8),
        requests: 0,
        misses: 0,
        expected_rebuilds: 0,
        last_miss_cause: None,
        miss_causes: BTreeMap::new(),
        recache_if_cold: None,
        // `None`, not `0`, for the reason given above: the field Codex would
        // fill is empty in every record on this machine, and a zero here would
        // claim a session wrote nothing to its cache when the truth is that
        // nobody counted.
        write_tokens: None,
    })
}

/// A whole [`Reading`] from one Codex `token_count` payload and what the
/// tailer knows about the session it came from.
///
/// The counterpart to [`reading_from_statusline`], for the agent that has no
/// status line. Codex splits across two record types — `token_count` carries
/// the numbers, `turn_context` the model and effort — so the metadata arrives
/// as arguments from [`crate::sessions::CodexRollout`], which retains it.
///
/// **What is missing here is missing honestly.** Codex publishes no cost, so
/// [`SessionUsage::cost`] is `None` and every surface that draws money simply
/// has nothing to draw — rather than a zero, which would read as "free". The
/// context window it does publish, so pressure is real.
#[allow(clippy::too_many_arguments)]
pub fn reading_from_codex(
    tokens: &serde_json::Value,
    session: Option<&str>,
    cwd: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Option<Reading> {
    let info = tokens.get("info")?;
    let count = |usage: Option<&serde_json::Value>, key: &str| -> u64 {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    };
    let total = info.get("total_token_usage");

    Some(Reading {
        session: session.map(str::to_owned),
        // Codex names threads in its own index, which is a different file and a
        // different poll; the card falls back to the id, which it already does.
        name: None,
        model: model.map(str::to_owned),
        version: None,
        effort: effort.map(str::to_owned),
        // Codex has no equivalent, so this is `false` rather than unknown.
        fast_mode: false,
        cwd: cwd.map(str::to_owned),
        last_output_tokens: count(info.get("last_token_usage"), "output_tokens"),
        // Codex's `input_tokens` *includes* the cached ones — the subset
        // relation this module verified against 3,690 real records — so fresh
        // input is the difference, and reading it straight would double-count
        // every cached token at the fresh-input price. Saturating, because a
        // future record that broke the relation should read as no fresh input
        // rather than as an enormous negative wrapped around.
        last_input_tokens: count(info.get("last_token_usage"), "input_tokens")
            .saturating_sub(count(info.get("last_token_usage"), "cached_input_tokens")),
        last_cache_read_tokens: count(info.get("last_token_usage"), "cached_input_tokens"),
        work: Work::default(),
        cache: cache_from_codex(tokens),
        // `rate_limits` rides on the same record, and its presence means the
        // same thing it means for Claude: an account with plan windows.
        billing: tokens
            .get("rate_limits")
            .is_some_and(serde_json::Value::is_object)
            .then_some(Billing::Plan),
        usage: SessionUsage {
            used: count(total, "input_tokens"),
            size: info
                .get("model_context_window")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            cost: None,
        },
    })
}

// ---- history -----------------------------------------------------------------

/// What a record from before packs — or from a ket with none configured —
/// records as its pack.
///
/// A name rather than an empty string, so the built-in levels are one table
/// among others rather than an absence that has to be special-cased at every
/// comparison.
pub fn builtin_pack_id() -> String {
    "builtin".to_owned()
}

/// The share of output each level is assumed to trim, indexed by level: `0`
/// (maximum reduction) to `4` (none).
///
/// **An assumption, not a measurement**, and the only way Economy's saving is
/// estimated. A level's instruction asks for less narration, so output is the
/// one token class it moves; input and cache reads are the task's, not the
/// level's. Measuring it by comparing sessions was tried three times — cost,
/// then tokens, then estimated dollars per prompt, each against the median of
/// the no-reduction sessions — and every time the figure followed task size,
/// because a session's total is ~98% cache reads and those scale with context
/// and tool calls. It reported Economy as costing more whenever the reduced
/// sessions happened to be the heavy ones. **Never compare a session with other
/// sessions to say what Economy saved it.**
pub const ASSUMED_OUTPUT_REDUCTION: [f64; 5] = [0.50, 0.35, 0.20, 0.10, 0.0];

/// [`ASSUMED_OUTPUT_REDUCTION`] for `level`: zero at the default level and at
/// any level the table does not cover.
pub fn assumed_output_reduction(level: u8) -> f64 {
    if level == crate::worktree::default_token_reduction() {
        return 0.0;
    }
    ASSUMED_OUTPUT_REDUCTION
        .get(usize::from(level))
        .copied()
        .unwrap_or_default()
}

/// How many sessions are kept.
///
/// Old enough sessions describe a version of ket, a model and a way of working
/// that no longer exist. The cap also keeps the file small enough to rewrite
/// whole, which is what makes the write atomic without a lock.
pub const MAX_RECORDS: usize = 500;

/// What one agent session spent, and under which configuration.
///
/// One record per session id, updated in place as its status line reports
/// again. Cost is cumulative and read off; the token counts are summed from
/// per-response readings — see each field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// Claude's own session id, which is what makes this idempotent.
    pub session: String,
    /// Where it ran.
    pub worktree: WorktreeId,
    /// The token-reduction level in force when ket first saw the session.
    pub level: u8,
    /// The model's display name, once a status line has said. `None` until
    /// then, and a record without one is never aggregated: cost per turn means
    /// nothing across models.
    #[serde(default)]
    pub model: Option<String>,
    /// Output tokens summed over the readings ket saw, which is the half of
    /// the bill narration actually moves.
    ///
    /// Summed rather than read off, because there is nothing to read off:
    /// see [`Reading::last_output_tokens`]. A session that ran while ket was
    /// closed is undercounted by whatever it produced then — status lines are
    /// not spooled — which is a floor rather than a fiction.
    #[serde(default)]
    pub output_tokens: u64,
    /// The last response's output tokens, so the next reading can be turned
    /// into the delta that moves [`Record::output_tokens`].
    #[serde(default)]
    pub last_output_tokens: u64,
    /// Fresh, uncached input tokens summed over the readings ket saw.
    ///
    /// The dearest class per token, and the one every lever in this plan is
    /// trying to move: a cache read is a tenth of this. Accumulated from
    /// [`Reading::last_input_tokens`] exactly as `output_tokens` is, with the
    /// same floor caveat.
    #[serde(default)]
    pub input_tokens: u64,
    /// See [`Record::last_output_tokens`].
    #[serde(default)]
    pub last_input_tokens: u64,
    /// Cache-read input tokens summed over the readings ket saw.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// See [`Record::last_output_tokens`].
    #[serde(default)]
    pub last_cache_read_tokens: u64,
    /// Every token this session wrote to the cache.
    ///
    /// Not accumulated: [`CacheHealth::write_tokens`] is already cumulative, so
    /// this takes the largest figure any reading reported. Monotonic on
    /// purpose, for the reason [`Record::cost_micros`] is — a status line drawn
    /// early in a new session that recycled an id must not erase the old one's
    /// total.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// The cache lifetime the session ran at, once a reading has said.
    ///
    /// Kept because a cache write is priced by its lifetime, so a contracted
    /// figure cannot be recomputed from token counts alone — see
    /// [`crate::rates::ModelRate::cache_write_micros`]. It is also the receipt
    /// for the TTL lever, and a record that outlives the session is the only
    /// place that receipt survives.
    #[serde(default)]
    pub cache_ttl: Option<String>,
    /// Cumulative cost in millionths of a dollar, as Claude reports it: the
    /// whole conversation's, including whatever it spent before ket attached.
    #[serde(default)]
    pub cost_micros: i64,
    /// What the session had already spent when ket first saw it.
    ///
    /// ket resumes a worktree's last conversation rather than starting a new
    /// one — see `ket_ui::terminal`'s `resume_args` — so `cost_micros` on a
    /// resumed session opens at whatever the whole conversation had cost, while
    /// [`Record::turns`] can only start at zero. Dividing one by the other
    /// charged every prompt from a previous sitting to the first prompt of this
    /// one: a real history had a session reading $38.62 across a single
    /// turn. Only the spend past this line is attributed.
    ///
    /// `None` until a reading has carried a cost.
    #[serde(default)]
    pub baseline_cost_micros: Option<i64>,
    /// Which pack, and which version of it, the level came from.
    ///
    /// A level number means nothing outside the table it indexes, and a level
    /// *name* means nothing outside the version that worded it. Both are kept
    /// so a resume can refuse to carry a conversation across two policies —
    /// see [`History::can_resume_with_economy`].
    ///
    /// Defaulted for every record written before packs existed, which puts them
    /// all under the built-in table.
    #[serde(default = "builtin_pack_id")]
    pub pack_id: String,
    /// See [`Record::pack_id`].
    #[serde(default)]
    pub pack_version: u32,
    /// The level's stable name within that pack.
    #[serde(default)]
    pub level_id: Option<String>,
    /// Whether this session's tokens were billed or drawn from a plan.
    ///
    /// Kept on the record so it outlives the session that observed it. The
    /// cache-lifetime flag has to be decided *at launch*, before the new
    /// session has said anything, and this is the only evidence of an account's
    /// billing that survives a restart — see
    /// [`History::latest_billing`].
    #[serde(default)]
    pub billing: Option<Billing>,
    /// The reasoning effort in force, once a status line has said.
    ///
    /// Shown on the usage card. Not part of Economy's estimate: lowering it is
    /// one of the levers a pack's levels pull. `None` on a model that takes no
    /// effort, and on any session older than this field.
    #[serde(default)]
    pub effort: Option<String>,
    /// Tool calls per MCP server, keyed by the server's own name.
    ///
    /// The share Claude Code shows subscribers in its own `/usage` breakdown,
    /// rebuilt from what ket already receives: a `PreToolUse` hook names the
    /// tool, and an MCP tool is named `mcp__<server>__<tool>`. Which servers a
    /// worktree actually uses is the evidence behind turning the unused ones
    /// off — the cheapest input-hygiene win there is, since a server's schemas
    /// sit in the prefix of every request whether or not it is ever called.
    #[serde(default)]
    pub mcp_calls: BTreeMap<String, u32>,
    /// Prompts the reader sent, counted from `UserPromptSubmit` hooks.
    #[serde(default)]
    pub turns: u32,
    /// Tool calls the agent made, counted from `PreToolUse` hooks.
    #[serde(default)]
    pub tool_calls: u32,
    /// When ket first heard about the session, and last.
    pub first_seen_ms: u64,
    /// See [`Record::first_seen_ms`].
    pub last_seen_ms: u64,
    /// Whether the level changed under the session.
    ///
    /// A session is launched with its level in its environment and cannot be
    /// told about a later change — see `crate::worktree::TOKEN_REDUCTION_ENV`
    /// — so a worktree whose level moved mid-session leaves a record that
    /// belongs to neither level. It is kept, because it is still real spend,
    /// and left out of Economy's estimate, because it ran at no one level.
    #[serde(default)]
    pub tainted: bool,
}

impl Record {
    /// What this session spent while ket was counting its prompts.
    ///
    /// The only cost figure that can be divided by [`Record::turns`] — see
    /// [`Record::baseline_cost_micros`].
    pub fn attributed_micros(&self) -> i64 {
        self.cost_micros
            .saturating_sub(self.baseline_cost_micros.unwrap_or_default())
            .max(0)
    }

    /// What this session cost at the organisation's own rates, rebuilt from counts.
    ///
    /// `None` unless a rate table is in force and covers this model — see
    /// [`crate::rates`]. Never a fallback to list price: a figure that is not
    /// what the reader pays is worse than no figure, because it is the one they
    /// would repeat to somebody else.
    ///
    /// Rebuilt from the four token classes rather than corrected from
    /// [`Record::cost_micros`], which is unfixable: list price has already been
    /// multiplied through it, and dividing it back out would need the very
    /// rates that were not used and would compound every rounding Claude Code
    /// did on the way.
    ///
    /// **A floor, and knowingly so.** Three of the four classes are summed from
    /// per-response readings, so a session that ran while ket was closed — or
    /// two consecutive responses that happened to report identical counts —
    /// is undercounted. Status lines are not spooled and there is nothing to
    /// recover them from. It is the same bargain [`Record::output_tokens`]
    /// already makes, and the honest direction to be wrong in for a number
    /// somebody may put in front of a team.
    pub fn contracted_micros(&self) -> Option<i64> {
        let rate = crate::rates::for_model(self.model.as_deref()?)?;
        Some(
            rate.input_micros(self.input_tokens)
                .saturating_add(rate.output_micros(self.output_tokens))
                .saturating_add(rate.cache_read_micros(self.cache_read_tokens))
                .saturating_add(
                    rate.cache_write_micros(self.cache_write_tokens, self.cache_ttl.as_deref()),
                ),
        )
    }

    /// This record as the reading it was written from, as far as a record can
    /// say.
    ///
    /// Status lines are not spooled: a worktree whose agent is idle, or whose
    /// window closed with ket's, says nothing at all until it is prompted
    /// again, and until then every per-worktree surface has an empty slot where
    /// the session's own numbers were. The history is the only place the last
    /// thing it spent survives, so this hands that back.
    ///
    /// Lossy, and only ever downwards. Cost, model, effort, billing and the
    /// per-response counts are stored, so they are stated. Cache health,
    /// context pressure, fast mode and the diff are not stored, so they are
    /// absent rather than zeroed — `None` is how everything reading a
    /// [`Reading`] already spells "nobody said", and a zero would be a claim.
    /// A record with no cost carries none, for the same reason.
    pub fn restored_reading(&self) -> Reading {
        Reading {
            session: Some(self.session.clone()),
            name: None,
            model: self.model.clone(),
            version: None,
            effort: self.effort.clone(),
            fast_mode: false,
            cwd: None,
            last_output_tokens: self.last_output_tokens,
            last_input_tokens: self.last_input_tokens,
            last_cache_read_tokens: self.last_cache_read_tokens,
            work: Work::default(),
            cache: None,
            billing: self.billing,
            usage: SessionUsage {
                // Both of these are this type's own spelling of "not
                // measured" — see `SessionUsage::fraction`. What a context
                // window held is a property of a conversation that is no
                // longer being read, and nothing on the record remembers it.
                used: 0,
                size: 0,
                cost: (self.cost_micros > 0).then(|| Money {
                    micros: self.cost_micros,
                    currency: "USD".to_owned(),
                }),
            },
        }
    }

    /// What this session cost and what Economy saved it, both estimated from
    /// its own token counts — see [`EconomyEstimate`].
    ///
    /// `None` when there is nothing honest to say: the level changed under the
    /// session ([`Record::tainted`]), or its model has no rate, configured or
    /// listed — see [`crate::rates::estimate_for`]. A session at the default
    /// level is priced with a saving of zero.
    pub fn economy_estimate(&self) -> Option<EconomyEstimate> {
        if self.tainted {
            return None;
        }
        let rate = crate::rates::estimate_for(self.model.as_deref()?)?;
        let cost_micros = rate
            .input_micros(self.input_tokens)
            .saturating_add(rate.output_micros(self.output_tokens))
            .saturating_add(rate.cache_read_micros(self.cache_read_tokens))
            .saturating_add(
                rate.cache_write_micros(self.cache_write_tokens, self.cache_ttl.as_deref()),
            );
        let trim = assumed_output_reduction(self.level);
        let unwritten = (self.output_tokens as f64 * trim / (1.0 - trim)).round() as u64;
        let saved_micros = rate.output_micros(unwritten);
        debug_assert!(cost_micros >= 0 && saved_micros >= 0);
        Some(EconomyEstimate {
            cost_micros,
            saved_micros,
        })
    }

    /// Whether this record belongs to the Economy catalogue in force now.
    fn uses_active_pack(&self) -> bool {
        self.pack_id == crate::worktree::active_pack_id()
            && self.pack_version == crate::worktree::active_pack_version()
    }
}

/// One session's estimated cost, and what Economy saved it — see
/// [`Record::economy_estimate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomyEstimate {
    /// Every token class the session consumed, at its model's rate, in micros.
    pub cost_micros: i64,
    /// The output the session would also have written with Economy off, at
    /// the output rate, in micros. Never negative.
    pub saved_micros: i64,
}

/// Every session ket has watched.
///
/// Its own file rather than a corner of `state.json`: this grows with use and
/// is read by nothing that draws the sidebar, and a registry that has to be
/// parsed before the window opens should not carry a spending log.
#[derive(Debug)]
pub struct History {
    /// Where it is kept.
    path: PathBuf,
    /// Sessions by id, ordered so the oldest is first to go.
    records: BTreeMap<String, Record>,
    /// Whether there is anything worth writing.
    dirty: bool,
}

impl History {
    /// Loads the history from ket's data directory, or an empty one when there
    /// is nothing there yet.
    ///
    /// A corrupt file reads as empty rather than as an error: this is a record
    /// of what things cost, and refusing to open the window over it would be a
    /// worse failure than losing it.
    pub fn load() -> Self {
        let path = crate::paths::data_dir()
            .map(|dir| dir.join("usage.json"))
            .unwrap_or_else(|_| PathBuf::from("usage.json"));
        Self::load_from(path)
    }

    /// Loads from an explicit path, so this is exercisable without a home.
    pub fn load_from(path: PathBuf) -> Self {
        let records = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<Record>>(&text).ok())
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| (record.session.clone(), record))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            path,
            records,
            dirty: false,
        }
    }

    /// Whether anything has changed since the last [`History::save`].
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Records what a status line just said about a session.
    ///
    /// `level` is the worktree's level *now*; a session whose worktree has
    /// since been set to something else is marked [`Record::tainted`] rather
    /// than quietly filed under whichever level was read last.
    pub fn observe(
        &mut self,
        session: &str,
        worktree: &WorktreeId,
        level: u8,
        reading: &Reading,
        now_ms: u64,
    ) {
        let known = self.records.contains_key(session);
        let record = self.entry(session, worktree, level, now_ms);
        if record.level != level {
            record.tainted = true;
        }
        if let Some(model) = reading.model.as_deref() {
            record.model = Some(model.to_owned());
        }
        if let Some(effort) = reading.effort.as_deref() {
            record.effort = Some(effort.to_owned());
        }
        if let Some(billing) = reading.billing {
            record.billing = Some(billing);
        }

        // Each reading names the last response's output, so a total has to be
        // built from what changes between them. Redraws repeat a figure and
        // add nothing; a new response replaces it and its whole value is new.
        // Two responses that happen to produce the same count read as one,
        // which loses tokens rather than inventing them.
        if reading.last_output_tokens != record.last_output_tokens {
            record.output_tokens = record
                .output_tokens
                .saturating_add(reading.last_output_tokens);
            record.last_output_tokens = reading.last_output_tokens;
        }
        // The other two classes, on the same terms and for the same reason.
        // Split rather than summed, because they are priced an order of
        // magnitude apart and a total that averaged them would answer no
        // question anybody has.
        if reading.last_input_tokens != record.last_input_tokens {
            record.input_tokens = record
                .input_tokens
                .saturating_add(reading.last_input_tokens);
            record.last_input_tokens = reading.last_input_tokens;
        }
        if reading.last_cache_read_tokens != record.last_cache_read_tokens {
            record.cache_read_tokens = record
                .cache_read_tokens
                .saturating_add(reading.last_cache_read_tokens);
            record.last_cache_read_tokens = reading.last_cache_read_tokens;
        }

        if let Some(cache) = reading.cache.as_ref() {
            // Already cumulative, so this is a maximum rather than a sum.
            if let Some(written) = cache.write_tokens {
                record.cache_write_tokens = record.cache_write_tokens.max(written);
            }
            if let Some(ttl) = cache.ttl.as_deref() {
                record.cache_ttl = Some(ttl.to_owned());
            }
        }

        if let Some(cost) = reading.usage.cost.as_ref() {
            // The first reading is the line spend is measured from. A session
            // ket resumed opens at what the whole conversation has cost, and
            // none of that belongs to a prompt ket is about to count.
            if record.baseline_cost_micros.is_none() {
                record.baseline_cost_micros = Some(cost.micros);
                // Unless prompts were already counted against it, in which
                // case the two cannot be separated any more. Real spend, and
                // no business in a comparison — the bargain `tainted` exists
                // for.
                if known && record.turns > 0 {
                    record.tainted = true;
                }
            }
            // Cumulative, so the newest reading wins — but never backwards: a
            // status line drawn early in a *new* session with a recycled id
            // would otherwise erase what the old one spent.
            record.cost_micros = record.cost_micros.max(cost.micros);
        }
        record.last_seen_ms = now_ms;
        self.dirty = true;
    }

    /// Counts one prompt from the reader.
    pub fn note_turn(&mut self, session: &str, worktree: &WorktreeId, level: u8, now_ms: u64) {
        let record = self.entry(session, worktree, level, now_ms);
        record.turns = record.turns.saturating_add(1);
        record.last_seen_ms = now_ms;
        self.dirty = true;
    }

    /// Counts one tool call, and which MCP server served it.
    ///
    /// `server` is `None` for a built-in tool, which is most of them — see
    /// [`crate::agent_hooks::mcp_server`], which does the naming. The total in
    /// [`Record::tool_calls`] counts every call either way; the per-server map
    /// only counts the ones a server answered.
    pub fn note_tool_call(
        &mut self,
        session: &str,
        worktree: &WorktreeId,
        level: u8,
        server: Option<&str>,
        now_ms: u64,
    ) {
        let record = self.entry(session, worktree, level, now_ms);
        record.tool_calls = record.tool_calls.saturating_add(1);
        if let Some(server) = server {
            let count = record.mcp_calls.entry(server.to_owned()).or_default();
            *count = count.saturating_add(1);
        }
        record.last_seen_ms = now_ms;
        self.dirty = true;
    }

    /// The record for `session`, created if this is the first ket has heard of
    /// it, with the oldest evicted once there are too many.
    fn entry(
        &mut self,
        session: &str,
        worktree: &WorktreeId,
        level: u8,
        now_ms: u64,
    ) -> &mut Record {
        if !self.records.contains_key(session) {
            while self.records.len() >= MAX_RECORDS {
                // By first sight rather than by key: the ids are opaque, so
                // their order says nothing about age.
                let oldest = self
                    .records
                    .iter()
                    .min_by_key(|(_, record)| record.first_seen_ms)
                    .map(|(id, _)| id.clone());
                match oldest {
                    Some(id) => {
                        self.records.remove(&id);
                    }
                    None => break,
                }
            }
            self.records.insert(
                session.to_owned(),
                Record {
                    session: session.to_owned(),
                    worktree: worktree.clone(),
                    level,
                    model: None,
                    // Whatever table is in force as this session starts. A
                    // record is stamped once, at creation, rather than on every
                    // reading: the pack cannot change mid-run, and a stamp that
                    // moved would describe a session that never happened.
                    pack_id: crate::worktree::active_pack_id(),
                    pack_version: crate::worktree::active_pack_version(),
                    level_id: crate::worktree::token_reduction(level).id.clone().into(),
                    billing: None,
                    effort: None,
                    mcp_calls: BTreeMap::new(),
                    output_tokens: 0,
                    last_output_tokens: 0,
                    input_tokens: 0,
                    last_input_tokens: 0,
                    cache_read_tokens: 0,
                    last_cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cache_ttl: None,
                    cost_micros: 0,
                    baseline_cost_micros: None,
                    turns: 0,
                    tool_calls: 0,
                    first_seen_ms: now_ms,
                    last_seen_ms: now_ms,
                    tainted: false,
                },
            );
        }

        self.records
            .get_mut(session)
            .expect("just inserted if it was missing")
    }

    /// Writes the history out, atomically.
    pub fn save(&mut self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
        }

        let records: Vec<&Record> = self.records.values().collect();
        let text = serde_json::to_string_pretty(&records)
            .map_err(|e| KetError::Config(format!("serialising usage history: {e}")))?;

        // Same directory, so the rename stays on one filesystem — the same
        // bargain `crate::store` makes.
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, text.as_bytes()).map_err(|e| KetError::io(&tmp, e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            KetError::io(&self.path, e)
        })?;

        self.dirty = false;
        Ok(())
    }

    /// Where the history is kept.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The most recently observed billing, from any session.
    ///
    /// An account-wide fact, so the newest answer from anywhere is the right
    /// one — a worktree opened for the first time has no history of its own,
    /// and the account it is about to talk to is the same account every other
    /// worktree has been talking to.
    ///
    /// `None` until some session has actually reported, which is the answer
    /// that matters: nothing downstream may guess a billing model, because the
    /// flag it decides costs real money on a write when it is wrong.
    pub fn latest_billing(&self) -> Option<Billing> {
        self.records
            .values()
            .filter(|record| record.billing.is_some())
            .max_by_key(|record| record.last_seen_ms)
            .and_then(|record| record.billing)
    }

    /// One session, if it is known.
    pub fn record(&self, session: &str) -> Option<&Record> {
        self.records.get(session)
    }

    /// Whether resuming `session` would keep one Economy policy throughout it.
    ///
    /// An unseen session is safe to resume: there is no evidence it began
    /// under anything else. Once ket has recorded it, its level and exact pack
    /// must match the launch being prepared. Otherwise a level change followed
    /// by reopening the worktree would resume the old conversation forever,
    /// taint every reading, and never produce a sample for the new level.
    pub fn can_resume_with_economy(&self, session: &str, level: u8) -> bool {
        self.records.get(session).is_none_or(|record| {
            !record.tainted && record.level == level && record.uses_active_pack()
        })
    }

    /// The last session heard from in each worktree.
    ///
    /// One per worktree rather than all of them: this exists so a surface can
    /// open on a worktree that has not spoken this run — see
    /// [`Record::restored_reading`] — and a list of everything ket has ever
    /// heard there is a different question, asked by whoever has the room to
    /// answer it.
    pub fn latest_per_worktree(&self) -> BTreeMap<&WorktreeId, &Record> {
        let mut latest: BTreeMap<&WorktreeId, &Record> = BTreeMap::new();
        for record in self.records.values() {
            latest
                .entry(&record.worktree)
                .and_modify(|held| {
                    if record.last_seen_ms > held.last_seen_ms {
                        *held = record;
                    }
                })
                .or_insert(record);
        }
        latest
    }
}

impl History {
    /// What every worktree's Economy sessions cost and saved since `since_ms`,
    /// estimated one session at a time — see [`Record::economy_estimate`].
    ///
    /// Every session run at a reduced level counts, whatever its worktree is
    /// set to now. Sessions at the default level are only counted, in
    /// [`WorktreeSavings::unreduced`], so a worktree set to reduce after its
    /// session began can say why it has nothing to show. A reduced session
    /// that cannot be priced is counted in [`WorktreeSavings::unmeasured`]
    /// rather than dropped, so a total can say how much of the picture it is.
    pub fn savings_by_worktree(&self, since_ms: u64) -> BTreeMap<WorktreeId, WorktreeSavings> {
        let default = crate::worktree::default_token_reduction();
        let mut out: BTreeMap<WorktreeId, WorktreeSavings> = BTreeMap::new();

        for record in self.records.values() {
            if record.last_seen_ms < since_ms {
                continue;
            }
            let entry = out.entry(record.worktree.clone()).or_default();
            if record.level == default {
                entry.unreduced += 1;
                continue;
            }
            let Some(estimate) = record.economy_estimate() else {
                entry.unmeasured += 1;
                continue;
            };
            entry.measured += 1;
            entry.turns += record.turns;
            entry.cost_micros = entry.cost_micros.saturating_add(estimate.cost_micros);
            entry.saved_micros = entry.saved_micros.saturating_add(estimate.saved_micros);
        }
        out
    }
}

/// What one worktree's Economy sessions cost and saved — see
/// [`History::savings_by_worktree`].
///
/// Additive, so a project's figure is its worktrees' summed and the whole
/// window's is its projects'.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeSavings {
    /// Economy sessions priced into the figures.
    pub measured: usize,
    /// Economy sessions that could not be — the level changed under them, or
    /// their model has no rate — counted so a total can say what it leaves out.
    pub unmeasured: usize,
    /// Sessions that ran at the default level, measured nowhere. A session
    /// keeps the level it started with, so a worktree set to reduce mid-session
    /// has only these until its next session.
    pub unreduced: usize,
    /// Prompts across the measured sessions.
    pub turns: u32,
    /// Estimated cost of the measured sessions, in micros.
    pub cost_micros: i64,
    /// Estimated saving of the measured sessions, in micros. Never negative.
    pub saved_micros: i64,
}

impl WorktreeSavings {
    /// Folds another worktree's figures into this one.
    pub fn add(&mut self, other: &WorktreeSavings) {
        self.measured += other.measured;
        self.unmeasured += other.unmeasured;
        self.unreduced += other.unreduced;
        self.turns += other.turns;
        self.cost_micros = self.cost_micros.saturating_add(other.cost_micros);
        self.saved_micros = self.saved_micros.saturating_add(other.saved_micros);
    }

    /// The saving as a share of what the same sessions would have cost with
    /// Economy off, when there is anything to divide by.
    pub fn fraction(&self) -> Option<f64> {
        let whole = self.cost_micros.saturating_add(self.saved_micros);
        (self.measured > 0 && whole > 0).then(|| self.saved_micros as f64 / whole as f64)
    }
}
