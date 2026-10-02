//! What an organisation actually pays per token, as against what Claude Code
//! assumes.
//!
//! Claude Code's `cost.total_cost_usd` is computed client-side at Anthropic's
//! published list price. An organisation on a negotiated contract is not paying
//! that, and the gap is what makes a cost figure one anyone believes: a figure
//! quoted at list price is either wrong or, worse, wrong in the flattering
//! direction and discovered later.
//!
//! Anthropic's own answer is the `modelPricing` managed setting, which is
//! exactly the right mechanism and is unavailable here — managed settings are
//! set by an enterprise's own administrators, not by a tool running inside a
//! developer's terminal. So ket applies the rates itself.
//!
//! ## Applied to counts, never to a cost
//!
//! A contracted figure has to be rebuilt from token counts. `total_cost_usd`
//! has already had list price baked into it and cannot be corrected after the
//! fact — dividing it back out would need the very rates that were not used,
//! and would compound every rounding Claude Code did on the way. That is why
//! this module takes tokens and returns money, and never takes money.
//!
//! ## Configured, never fetched
//!
//! A rate table is a term of somebody's contract. It arrives at setup, in
//! the config file, the same way [`crate::config::PackConfig::url`] does — not
//! from the network, because there is no source that could be authoritative
//! about what a particular organisation pays, and a wrong number here is a
//! wrong number somebody will repeat.
//!
//! ## List prices, for estimates only
//!
//! [`for_model`] answers from the configured table alone, so nothing labelled
//! "at your rates" is ever a list price. [`estimate_for`] falls back to a small
//! compiled-in list-price table, because Economy's figures are estimates by
//! definition and an estimate with no price at all is no figure: Codex reports
//! no cost, and an empty `[rates]` left every Codex session unpriced. The table
//! rots when a price is republished; a configured entry always wins over it.

use serde::{Deserialize, Serialize};

/// One model's rates, in US dollars per million tokens.
///
/// Dollars per million is the unit every published price list uses, so a rate
/// can be copied out of a contract without arithmetic — and arithmetic done by
/// hand at the moment of transcription is the single most likely place for this
/// whole feature to go quietly wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelRate {
    /// Which model these rates are for.
    ///
    /// Matched against the model name ket has — a display name from Claude
    /// Code's status line (`Opus 5`), or a slug from Codex's rollout
    /// (`gpt-5-codex`) — case-insensitively: exactly first, then as a substring,
    /// longest entry winning. The substring pass is what lets one entry cover
    /// `Opus 5` and `Claude Opus 5` without knowing which spelling arrives; the
    /// longest-wins rule is what stops a broad `Opus` entry from swallowing a
    /// specific `Opus 5.1` one.
    pub model: String,
    /// Fresh input tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Tokens read from a warm cache — the cheap ones, and the reason any of
    /// the cache levers in this plan pay for themselves.
    pub cache_read: f64,
    /// Tokens written into a five-minute cache.
    pub cache_write: f64,
    /// Tokens written into a one-hour cache, which costs more to write and is
    /// the point of the TTL lever: a longer life at a higher write price is
    /// cheaper overall exactly when the session outlives five minutes.
    ///
    /// `None` falls back to [`ModelRate::cache_write`], which understates a
    /// one-hour write rather than inventing a multiplier — an understatement
    /// that is visible in the table is better than a guess that is not.
    pub cache_write_1h: Option<f64>,
}

impl Default for ModelRate {
    fn default() -> Self {
        Self {
            model: String::new(),
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            cache_write_1h: None,
        }
    }
}

impl ModelRate {
    /// Whether this entry can be used at all.
    ///
    /// A negative rate is not a price, and a nameless entry can never match. An
    /// all-zero entry is allowed through deliberately: "this model is free to
    /// us" is a real term in some contracts, and refusing it would make the
    /// table unable to express it.
    pub fn usable(&self) -> bool {
        !self.model.trim().is_empty()
            && [
                self.input,
                self.output,
                self.cache_read,
                self.cache_write,
                self.cache_write_1h.unwrap_or_default(),
            ]
            .into_iter()
            .all(|rate| rate.is_finite() && rate >= 0.0)
    }

    /// Cost in micros for `tokens` at `per_million` dollars per million.
    ///
    /// The conversion is a no-op by construction, which is the nicest property
    /// this module has: dollars-per-million times tokens *is* micros, because a
    /// millionth of a dollar and a millionth of a million tokens are the same
    /// denominator. There is no scale factor here to get wrong.
    fn micros(tokens: u64, per_million: f64) -> i64 {
        (tokens as f64 * per_million).round() as i64
    }

    /// What fresh input tokens cost.
    pub fn input_micros(&self, tokens: u64) -> i64 {
        Self::micros(tokens, self.input)
    }

    /// What output tokens cost.
    pub fn output_micros(&self, tokens: u64) -> i64 {
        Self::micros(tokens, self.output)
    }

    /// What reading from a warm cache costs.
    pub fn cache_read_micros(&self, tokens: u64) -> i64 {
        Self::micros(tokens, self.cache_read)
    }

    /// What writing a cache costs, at the lifetime a session is actually using.
    ///
    /// `ttl` is [`crate::usage::CacheHealth::ttl`] as the payload spells it.
    /// Anything other than `"1h"` — including nothing at all — is priced at the
    /// five-minute rate, which is what an unset `promptCacheTtl` gets.
    pub fn cache_write_micros(&self, tokens: u64, ttl: Option<&str>) -> i64 {
        let per_million = match ttl {
            Some("1h") => self.cache_write_1h.unwrap_or(self.cache_write),
            _ => self.cache_write,
        };
        Self::micros(tokens, per_million)
    }
}

/// The organisation's rate table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RateConfig {
    /// One entry per model. Empty — the default — means ket quotes nothing in
    /// contracted dollars anywhere, which is every install until someone fills
    /// this in.
    pub models: Vec<ModelRate>,
}

impl RateConfig {
    /// Whether there is anything to quote with.
    pub fn is_empty(&self) -> bool {
        !self.models.iter().any(ModelRate::usable)
    }

    /// The entry covering `model`, if the table has one.
    ///
    /// See [`ModelRate::model`] for the matching rule.
    pub fn for_model(&self, model: &str) -> Option<&ModelRate> {
        let model = model.trim().to_lowercase();
        if model.is_empty() {
            return None;
        }

        let usable = || self.models.iter().filter(|rate| rate.usable());

        usable()
            .find(|rate| rate.model.trim().eq_ignore_ascii_case(&model))
            .or_else(|| {
                usable()
                    .filter(|rate| model.contains(&rate.model.trim().to_lowercase()))
                    // Longest match wins, so a specific entry beats a broad one
                    // whichever order they were written in.
                    .max_by_key(|rate| rate.model.trim().len())
            })
    }
}

/// The table in force for this run.
///
/// A `OnceLock` set at startup, for the reasons
/// [`crate::worktree::activate_pack`] gives at length: money on a card must not
/// change denomination while somebody is looking at it, and a lock would stop
/// every reader handing out `&'static`.
static ACTIVE: std::sync::OnceLock<RateConfig> = std::sync::OnceLock::new();

/// Puts a rate table in force. Called once, at startup, before anything renders.
///
/// Returns whether it took. A table with no usable entry is refused rather than
/// installed empty, so that [`configured`] answers the question a caller
/// actually has — "can I quote money?" — rather than "was a table present?".
pub fn activate(rates: RateConfig) -> bool {
    if rates.is_empty() {
        return false;
    }
    ACTIVE.set(rates).is_ok()
}

/// Whether anything can be quoted in the organisation's own money.
pub fn configured() -> bool {
    ACTIVE.get().is_some()
}

/// The rates for `model`, when a table is in force and covers it.
pub fn for_model(model: &str) -> Option<&'static ModelRate> {
    ACTIVE.get()?.for_model(model)
}

/// Published list prices in $/M tokens: model, input, output, cache read,
/// five-minute cache write, one-hour cache write.
///
/// Anthropic's from the published price list (cache writes at 1.25× and 2×
/// input); OpenAI's as published for the API, which has no cache-write price
/// because Codex reports no cache writes. Matched by [`RateConfig::for_model`],
/// so `Opus 5.5` beats `Opus 5` on `Opus 5.5 (1M context)`.
const LIST_PRICES: &[(&str, f64, f64, f64, f64, f64)] = &[
    ("Fable 5.1", 10.0, 50.0, 0.25, 12.5, 20.0),
    ("Fable 5", 10.0, 50.0, 1.0, 12.5, 20.0),
    ("Opus 5.5", 4.0, 20.0, 0.2, 5.0, 8.0),
    ("Opus 5", 5.0, 25.0, 0.5, 6.25, 10.0),
    ("Opus 4", 5.0, 25.0, 0.5, 6.25, 10.0),
    ("Sonnet 5", 2.0, 10.0, 0.2, 2.5, 4.0),
    ("Sonnet 4", 3.0, 15.0, 0.3, 3.75, 6.0),
    ("Haiku 4", 1.0, 5.0, 0.1, 1.25, 2.0),
    ("gpt-6-sol", 2.0, 10.0, 0.2, 2.5, 2.5),
    ("gpt-6-luna", 0.1, 0.5, 0.01, 0.125, 0.125),
    ("gpt-5.6-sol", 5.0, 30.0, 0.5, 5.0, 5.0),
    ("gpt-5.6-terra", 2.0, 12.0, 0.2, 2.0, 2.0),
    ("gpt-5.6-luna", 0.2, 1.2, 0.02, 0.2, 0.2),
];

static LIST: std::sync::LazyLock<RateConfig> = std::sync::LazyLock::new(|| RateConfig {
    models: LIST_PRICES
        .iter()
        .map(
            |&(model, input, output, cache_read, cache_write, cache_write_1h)| ModelRate {
                model: model.to_owned(),
                input,
                output,
                cache_read,
                cache_write,
                cache_write_1h: Some(cache_write_1h),
            },
        )
        .collect(),
});

/// The rates to estimate `model` at: the configured table's when it covers the
/// model, else the list price. For estimates only — see the module docs.
pub fn estimate_for(model: &str) -> Option<&'static ModelRate> {
    for_model(model).or_else(|| LIST.for_model(model))
}

/// Formats micros as money, at the precision the amount deserves.
///
/// Cents below ten dollars and whole dollars above: a re-cache quoted as
/// `$0.42` and a month's spend quoted as `$1,284` are both read at a glance,
/// and `$1,284.37` is neither more useful nor more true, given every count
/// underneath it is a floor.
///
/// Something too small to round to a cent reads as `<$0.01`, never as `$0.00`
/// — matching `ket_ui::usage_card`'s own rule, and for its reason: a session
/// that has spent something must not be told it has spent nothing.
///
/// The threshold is tested against the *rounded* figure so the two branches
/// meet cleanly. Testing the raw one put $9.999999 in the cents branch, where
/// it rendered `$10.00` immediately below the whole-dollar branch's `$10`.
pub fn money(micros: i64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    let cents = (dollars * 100.0).round() / 100.0;

    if micros != 0 && cents == 0.0 {
        return if micros < 0 { "-<$0.01" } else { "<$0.01" }.to_owned();
    }

    if cents.abs() < 10.0 {
        format!("${cents:.2}")
    } else {
        let whole = dollars.round() as i64;
        let mut digits = whole.abs().to_string();
        let mut grouped = String::new();
        while digits.len() > 3 {
            let rest = digits.split_off(digits.len() - 3);
            grouped = if grouped.is_empty() {
                rest
            } else {
                format!("{rest},{grouped}")
            };
        }
        let grouped = if grouped.is_empty() {
            digits
        } else {
            format!("{digits},{grouped}")
        };
        format!("{}${grouped}", if whole < 0 { "-" } else { "" })
    }
}
