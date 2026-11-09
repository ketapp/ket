//! Whether a session's prompt cache has gone cold, and what picking it up
//! again would cost.
//!
//! Every request resends the whole conversation, and the prompt cache is what
//! reprices it: a read is a tenth of fresh input, and on Fable a fortieth. A
//! cache lives five minutes or an hour from the last request that used it.
//! Past that, the next prompt writes the entire conversation to the cache
//! again, at the write price — and on a long session that one prompt is the
//! dearest of the day. Measured across a month of one person's Fable
//! sessions, returning to a session more than an hour later was over a third
//! of all spend: two hundred returns, about 320,000 tokens re-cached each.
//!
//! Read off the end of the session's transcript rather than ket's own
//! records. The last response says when it was, how large the context was,
//! and which lifetime it cached at — exactly, where a status line or a hook
//! says none of the three. And asking is cheap: only the tail is read.
//!
//! This only answers the question. Nothing here holds a message: whoever asks
//! does so as a session is opened, and a prompt always goes as it was typed.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of a transcript's end is read for its last response.
const TAIL_BYTES: u64 = 256 * 1024;

/// Five minutes, the lifetime a cache gets when nothing asks for longer.
const FIVE_MINUTES_MS: u64 = 5 * 60 * 1000;

/// An hour, the longer lifetime.
const ONE_HOUR_MS: u64 = 60 * 60 * 1000;

/// The newest request a session made, as its transcript recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCall {
    /// When the response was written, in epoch milliseconds.
    pub at_ms: u64,
    /// Every token the next request will send before the new prompt: what
    /// this one sent — fresh, written and read — plus what it answered.
    pub context_tokens: u64,
    /// Whether it cached at the one-hour lifetime rather than five minutes.
    pub one_hour: bool,
    /// The model it ran, as the API names it — `claude-fable-5-1`.
    pub model: Option<String>,
}

/// A session whose cache has expired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cold {
    /// How long since its last request, in milliseconds.
    pub idle_ms: u64,
    /// Tokens the next prompt writes to the cache again.
    pub tokens: u64,
    /// What that write costs at the published list price, in millionths of a
    /// dollar, when ket knows the model. List price rather than a configured
    /// contract table, so the desktop and a phone — whose figures the host
    /// works out, with no table of its own — always say the same.
    pub cost_micros: Option<i64>,
}

impl LastCall {
    /// How long a cache written by this call lives.
    pub fn ttl_ms(&self) -> u64 {
        if self.one_hour {
            ONE_HOUR_MS
        } else {
            FIVE_MINUTES_MS
        }
    }

    /// The cold state at `now_ms`, or `None` while the cache is still warm.
    pub fn cold(&self, now_ms: u64) -> Option<Cold> {
        let idle_ms = now_ms.saturating_sub(self.at_ms);
        if idle_ms <= self.ttl_ms() || self.context_tokens == 0 {
            return None;
        }
        let ttl = if self.one_hour { "1h" } else { "5m" };
        let cost_micros = self
            .model
            .as_deref()
            .map(display_model)
            .and_then(|model| crate::rates::list_price(&model))
            .map(|rate| rate.cache_write_micros(self.context_tokens, Some(ttl)));
        Some(Cold {
            idle_ms,
            tokens: self.context_tokens,
            cost_micros,
        })
    }
}

impl Cold {
    /// Whether this is worth asking about: at least `min_tokens` to re-cache.
    pub fn worth_asking(&self, min_tokens: u64) -> bool {
        self.tokens >= min_tokens
    }

    /// The figures as one line: "re-caches 312k tokens, about $6.24 at API
    /// prices". The price is left out when the model's rates are unknown.
    pub fn summary(&self) -> String {
        let tokens = short_tokens(self.tokens);
        match self.cost_micros {
            Some(micros) => format!(
                "re-caches {tokens} tokens, about {} at API prices",
                crate::rates::money(micros)
            ),
            None => format!("re-caches {tokens} tokens"),
        }
    }
}

/// The last main-thread response at the end of Claude transcript `path`.
/// `None` when the end holds none — a session that has not answered yet, or a
/// file that is not a transcript.
pub fn claude_last_call(path: &Path) -> Option<LastCall> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    last_call_in(&bytes)
}

/// [`claude_last_call`], remembered until the transcript changes.
///
/// For a caller that asks about every session again and again — the phone
/// snapshot does, every time anything changes — and would otherwise read
/// the same quarter-megabyte each time for an answer that only moves when
/// the agent writes.
pub fn claude_last_call_cached(path: &Path) -> Option<LastCall> {
    use std::collections::HashMap;
    use std::sync::{Mutex, PoisonError};
    use std::time::SystemTime;

    type Seen = HashMap<std::path::PathBuf, (u64, SystemTime, Option<LastCall>)>;
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok()?);
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some((len, modified, last)) = seen.get(path)
        && (*len, *modified) == stamp
    {
        return last.clone();
    }
    let last = claude_last_call(path);
    // Bounded by the sessions a desktop has open, but a long-running host
    // sees many come and go.
    if seen.len() > 256 {
        seen.clear();
    }
    seen.insert(path.to_owned(), (stamp.0, stamp.1, last.clone()));
    last
}

/// [`claude_last_call`] over bytes already read.
fn last_call_in(bytes: &[u8]) -> Option<LastCall> {
    bytes.split(|&b| b == b'\n').rev().find_map(call_from_line)
}

fn call_from_line(line: &[u8]) -> Option<LastCall> {
    // Cheap refusals first: most lines are tool results and bookkeeping.
    if !contains(line, b"\"assistant\"") || !contains(line, b"\"usage\"") {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(line).ok()?;
    if record.get("type").and_then(|t| t.as_str()) != Some("assistant")
        || record
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    {
        return None;
    }
    let message = record.get("message")?;
    // An API error or a rate limit is written as an assistant record from
    // a `<synthetic>` model, with nothing in its usage: not a request, and
    // never the one a cache was last written by.
    if message.get("model").and_then(|m| m.as_str()) == Some("<synthetic>") {
        return None;
    }
    let usage = message.get("usage")?;
    let count = |key: &str| {
        usage
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let context_tokens = count("input_tokens")
        + count("cache_creation_input_tokens")
        + count("cache_read_input_tokens")
        + count("output_tokens");
    // Claude Code splits what it wrote by lifetime. A response that wrote
    // nothing says nothing about the lifetime, so it reads as five minutes:
    // the shorter guess asks a little early rather than never.
    let one_hour = usage
        .get("cache_creation")
        .and_then(|c| c.get("ephemeral_1h_input_tokens"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|tokens| tokens > 0);
    let at_ms = record
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(crate::rate_limits::parse_rfc3339_ms)?;
    Some(LastCall {
        at_ms,
        context_tokens,
        one_hour,
        model: message
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::to_owned),
    })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// `claude-fable-5-1` as the rate table names it, `Fable 5.1`;
/// `claude-haiku-4-5-20251001` as `Haiku 4.5`.
fn display_model(id: &str) -> String {
    let parts: Vec<&str> = id
        .trim_start_matches("claude-")
        .split('-')
        .filter(|part| !(part.len() == 8 && part.bytes().all(|b| b.is_ascii_digit())))
        .collect();
    let Some((family, version)) = parts.split_first() else {
        return id.to_owned();
    };
    let name = match family.get(..1) {
        Some(first) => format!("{}{}", first.to_uppercase(), &family[1..]),
        None => (*family).to_owned(),
    };
    if version.is_empty() {
        name
    } else {
        format!("{name} {}", version.join("."))
    }
}

/// A token count at a glance: `312k`, `1.2M`, `850`.
pub fn short_tokens(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{}k", (tokens + 500) / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// How long ago, at a glance: `3 h 12 m`, `45 m`, `2 d`.
pub fn short_idle(idle_ms: u64) -> String {
    let minutes = idle_ms / 60_000;
    match minutes {
        0..=59 => format!("{minutes} m"),
        60..=1_439 => match minutes % 60 {
            0 => format!("{} h", minutes / 60),
            rest => format!("{} h {rest} m", minutes / 60),
        },
        _ => format!("{} d", minutes / 1_440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(timestamp: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","message":{{"model":"claude-fable-5-1","usage":{usage}}}}}"#
        )
    }

    const WARM_1H: &str = r#"{"input_tokens":2,"cache_creation_input_tokens":3000,"cache_read_input_tokens":309000,"output_tokens":400,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":3000}}"#;

    #[test]
    fn the_last_main_thread_response_is_the_one_read() {
        let older = line("2026-10-29T10:00:00.000Z", WARM_1H);
        let newer = line("2026-10-29T12:00:00.000Z", WARM_1H);
        let sidechain = r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-29T13:00:00.000Z","message":{"usage":{"input_tokens":5}}}"#;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#;
        let bytes = format!("{older}\n{newer}\n{sidechain}\n{result}\n");
        let call = last_call_in(bytes.as_bytes()).expect("a call");
        assert_eq!(
            call.at_ms,
            crate::rate_limits::parse_rfc3339_ms("2026-10-29T12:00:00.000Z").unwrap()
        );
        assert_eq!(call.context_tokens, 2 + 3_000 + 309_000 + 400);
        assert!(call.one_hour);
        assert_eq!(call.model.as_deref(), Some("claude-fable-5-1"));
    }

    #[test]
    fn an_error_written_as_a_response_is_passed_over() {
        let real = line("2026-10-29T12:00:00.000Z", WARM_1H);
        let synthetic = r#"{"type":"assistant","timestamp":"2026-10-29T14:00:00.000Z","message":{"model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let call = last_call_in(format!("{real}\n{synthetic}\n").as_bytes()).expect("the real one");
        assert_eq!(call.context_tokens, 2 + 3_000 + 309_000 + 400);
    }

    #[test]
    fn a_transcript_with_no_response_has_no_last_call() {
        let bytes = br#"{"type":"user","message":{"content":"hi"}}"#;
        assert_eq!(last_call_in(bytes), None);
        assert_eq!(last_call_in(b"not json at all\n"), None);
    }

    #[test]
    fn incomplete_or_invalid_tail_records_do_not_hide_the_last_response() {
        let real = line("2026-10-29T12:00:00Z", WARM_1H);
        let expected = last_call_in(real.as_bytes()).unwrap();
        for tail in [
            r#"{"type":"assistant","message":{"usage":{}"#,
            r#"{"type":"assistant","timestamp":"not a timestamp","message":{"usage":{"input_tokens":900000}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-29T13:00:00Z","message":{}}"#,
            r#"{"type":"user","timestamp":"2026-10-29T13:00:00Z","message":{"role":"assistant","usage":{"input_tokens":900000}}}"#,
        ] {
            assert_eq!(
                last_call_in(format!("{real}\n{tail}").as_bytes()),
                Some(expected.clone()),
                "tail: {tail}"
            );
        }
    }

    #[test]
    fn a_new_response_without_cache_writes_does_not_inherit_the_old_hour_lifetime() {
        let older = line("2026-10-29T12:00:00Z", WARM_1H);
        let newer = line(
            "2026-10-29T12:01:00Z",
            r#"{"input_tokens":5,"cache_read_input_tokens":200000,"output_tokens":10,"cache_creation":{"ephemeral_1h_input_tokens":0}}"#,
        );
        let last = last_call_in(format!("{older}\n{newer}").as_bytes()).unwrap();
        assert_eq!(last.context_tokens, 200_015);
        assert!(!last.one_hour);
        assert_eq!(last.cold(last.at_ms + 300_000), None);
        let cold = last.cold(last.at_ms + 300_001).unwrap();
        // Five-minute writes use $12.50/M, rather than the hour's $20/M.
        assert_eq!(cold.cost_micros, Some(2_500_188));
    }

    #[test]
    fn a_future_timestamp_or_empty_context_never_prompts() {
        let call = LastCall {
            at_ms: 1_000_000,
            context_tokens: 100_000,
            one_hour: false,
            model: None,
        };
        assert_eq!(call.cold(call.at_ms - 1), None);
        let empty = LastCall {
            context_tokens: 0,
            ..call
        };
        assert_eq!(empty.cold(u64::MAX), None);
    }

    #[test]
    fn a_cache_is_warm_inside_its_lifetime_and_cold_after_it() {
        let call = LastCall {
            at_ms: 1_000_000,
            context_tokens: 312_000,
            one_hour: true,
            model: Some("claude-fable-5-1".to_owned()),
        };
        assert_eq!(call.cold(1_000_000 + ONE_HOUR_MS), None);
        let cold = call.cold(1_000_000 + ONE_HOUR_MS + 1).expect("cold");
        assert_eq!(cold.tokens, 312_000);
        // Fable 5.1 writes at $20/M for an hour: 312k is $6.24.
        assert_eq!(cold.cost_micros, Some(6_240_000));

        let short = LastCall {
            one_hour: false,
            ..call
        };
        assert!(short.cold(1_000_000 + FIVE_MINUTES_MS + 1).is_some());
    }

    #[test]
    fn a_response_that_cached_nothing_reads_as_five_minutes() {
        let usage = r#"{"input_tokens":50,"cache_read_input_tokens":200000,"output_tokens":10}"#;
        let call = last_call_in(line("2026-10-29T12:00:00Z", usage).as_bytes()).unwrap();
        assert!(!call.one_hour);
        assert_eq!(call.ttl_ms(), FIVE_MINUTES_MS);
    }

    #[test]
    fn an_unknown_model_is_cold_without_a_price() {
        let call = LastCall {
            at_ms: 0,
            context_tokens: 150_000,
            one_hour: true,
            model: Some("some-future-model".to_owned()),
        };
        let cold = call.cold(2 * ONE_HOUR_MS).unwrap();
        assert_eq!(cold.cost_micros, None);
        assert_eq!(cold.summary(), "re-caches 150k tokens");
    }

    #[test]
    fn only_a_session_over_the_threshold_is_worth_asking_about() {
        let cold = Cold {
            idle_ms: 0,
            tokens: 99_999,
            cost_micros: None,
        };
        assert!(!cold.worth_asking(100_000));
        assert!(cold.worth_asking(99_999));
    }

    #[test]
    fn model_ids_read_as_the_rate_table_names_them() {
        assert_eq!(display_model("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(display_model("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
    }

    #[test]
    fn figures_read_at_a_glance() {
        assert_eq!(short_tokens(312_000), "312k");
        assert_eq!(short_tokens(1_250_000), "1.2M");
        assert_eq!(short_idle(192 * 60_000), "3 h 12 m");
        assert_eq!(short_idle(120 * 60_000), "2 h");
        assert_eq!(short_idle(45 * 60_000), "45 m");
        assert_eq!(short_idle(3 * 1_440 * 60_000), "3 d");
        let cold = Cold {
            idle_ms: 0,
            tokens: 312_000,
            cost_micros: Some(6_240_000),
        };
        assert_eq!(
            cold.summary(),
            "re-caches 312k tokens, about $6.24 at API prices"
        );
    }
}
