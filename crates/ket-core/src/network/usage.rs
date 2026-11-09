//! What a reply says the request cost, in tokens.
//!
//! Anthropic puts usage in the reply: a streamed reply in its
//! `message_start` event (what was read, cached or not) and its
//! `message_delta` events (what was written, a running total), a whole one
//! in its body's `usage`. OpenAI's Responses API puts it in the event that
//! ends a reply, `response.completed`, and in a whole reply's `usage`. Only
//! those fields are read; nothing else of the reply is kept.

use std::io::Read;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tokens, as the provider counted them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Read fresh, at the full input price.
    pub input: u64,
    /// Written by the model.
    pub output: u64,
    /// Read back from the provider's prompt cache.
    pub cache_read: u64,
    /// Written to the prompt cache.
    pub cache_write: u64,
}

impl Usage {
    fn take(&mut self, usage: &Value) {
        let field = |name: &str| usage.get(name).and_then(Value::as_u64);
        if let Some(n) = field("input_tokens") {
            self.input = n;
        }
        if let Some(n) = field("output_tokens") {
            self.output = n;
        }
        if let Some(n) = field("cache_read_input_tokens") {
            self.cache_read = n;
        }
        if let Some(n) = field("cache_creation_input_tokens") {
            self.cache_write = n;
        }
        // OpenAI counts what it read from its cache in `input_tokens`, and
        // says how much of it that was beside it.
        if let Some(n) = usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
        {
            self.cache_read = n;
            self.input = self.input.saturating_sub(n);
        }
    }
}

/// The usage in `reply`, as received: `encoding` is its `Content-Encoding`,
/// if it had one. `None` when it says none, or is in an encoding not read
/// here.
pub fn parse(reply: &[u8], encoding: Option<&str>) -> Option<Usage> {
    let decoded;
    let reply = match encoding.map(str::trim) {
        None | Some("" | "identity") => reply,
        Some("gzip") => {
            decoded = inflate(flate2::read::GzDecoder::new(reply))?;
            &decoded
        }
        Some("deflate") => {
            decoded = inflate(flate2::read::ZlibDecoder::new(reply))?;
            &decoded
        }
        Some(_) => return None,
    };
    let text = std::str::from_utf8(reply).ok()?;
    if let Ok(whole) = serde_json::from_str::<Value>(text) {
        let mut usage = Usage::default();
        usage.take(whole.get("usage")?);
        return Some(usage);
    }
    let mut usage = None::<Usage>;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        let found = match event.get("type").and_then(Value::as_str) {
            Some("message_start") => event.pointer("/message/usage"),
            Some("message_delta") => event.get("usage"),
            Some("response.completed" | "response.incomplete" | "response.failed") => {
                event.pointer("/response/usage")
            }
            _ => None,
        };
        if let Some(found) = found.filter(|found| found.is_object()) {
            usage.get_or_insert_default().take(found);
        }
    }
    usage
}

/// Everything that decodes. A reply cut off part way — the agent stopped it —
/// ends without its trailer and fails at the end, but its `message_start`,
/// and so what it read, came first.
fn inflate(mut decoder: impl Read) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let _ = decoder.read_to_end(&mut out);
    (!out.is_empty()).then_some(out)
}
