//! What a request adds to the conversation it carries.
//!
//! An agent sends the whole conversation every turn: the system prompt, the
//! tools, every message so far, and then the one new thing. Logged as it is,
//! a long session would store the same files a hundred times over. So each
//! request is held up against the ones before it in its session, and only
//! the messages it adds are kept; the rest is counted as resent.
//!
//! **Threads, not one history per session.** Beside its conversation an agent
//! makes side requests in the same session — a title, a quota check, a token
//! count — each its own short history. A session keeps a few of these
//! [`THREADS`], and a request continues the one it shares the longest run of
//! leading messages with. One that shares none starts a thread of its own,
//! all of it new: the first turn, a side request, or a conversation the agent
//! rewrote (compacted, say), which is said as such rather than taken for a
//! small change.
//!
//! **Compared as the agent means them.** Claude Code moves its cache marks
//! along the conversation every turn, so the message that carried one last
//! time does not now; the marks are left out of the comparison.
//!
//! **Two shapes.** Anthropic's Messages API, with its `messages`, and
//! OpenAI's Responses API, with its `input`. A Responses request may name
//! the reply it follows on from instead of resending what led to it, as
//! Codex does over its WebSocket: then all of it is new, and it carries on
//! its session's conversation.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use serde_json::Value;

/// Side histories kept per session, beside its conversation.
const THREADS: usize = 8;

/// Sessions remembered at once; the one used longest ago goes first.
const SESSIONS: usize = 256;

/// A request taken apart: what was resent, and what is new.
#[derive(Debug, Default)]
pub struct Split {
    /// The serialized size of each part, in bytes.
    pub system_bytes: usize,
    pub tools_bytes: usize,
    pub resent_bytes: usize,
    pub new_bytes: usize,
    /// Messages this request added.
    pub new_messages: usize,
    /// Whether it carried on from an earlier request in its session.
    pub continued: bool,
    /// What to keep: the new messages, and the system prompt and tools when
    /// they differ from the thread's last.
    pub kept: Value,
}

/// Where a request body keeps its parts, by the API it is for.
struct Shape {
    /// The conversation, oldest first.
    turns: &'static str,
    system: &'static str,
    tools: &'static str,
}

/// Anthropic's Messages API.
const MESSAGES: Shape = Shape {
    turns: "messages",
    system: "system",
    tools: "tools",
};

/// OpenAI's Responses API.
const RESPONSES: Shape = Shape {
    turns: "input",
    system: "instructions",
    tools: "tools",
};

#[derive(Default)]
struct Thread {
    messages: Vec<u64>,
    system: u64,
    tools: u64,
}

#[derive(Default)]
struct Session {
    threads: Vec<Thread>,
    used: u64,
}

/// The sessions seen so far, and the shape of each one's threads.
#[derive(Default)]
pub struct Delta {
    sessions: HashMap<String, Session>,
    clock: u64,
}

impl Delta {
    /// Splits `request`, a Messages or Responses API body, against what
    /// `pane`'s session sent before. `None` for a body that is neither.
    pub fn split(&mut self, pane: &str, request: &Value) -> Option<Split> {
        let shape = [MESSAGES, RESPONSES]
            .into_iter()
            .find(|shape| request.get(shape.turns).is_some_and(Value::is_array))?;
        let messages = request.get(shape.turns)?.as_array()?;
        let key = session_key(pane, request);
        let hashes: Vec<u64> = messages.iter().map(fingerprint).collect();
        let system = request.get(shape.system).map_or(0, fingerprint);
        let tools = request.get(shape.tools).map_or(0, fingerprint);
        let follows = request
            .get("previous_response_id")
            .is_some_and(|id| !id.is_null());

        self.clock += 1;
        let clock = self.clock;
        if !self.sessions.contains_key(&key) && self.sessions.len() >= SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.sessions.remove(&oldest);
            }
        }
        let session = self.sessions.entry(key).or_default();
        session.used = clock;

        // The thread sharing the longest run of leading messages.
        let best = session
            .threads
            .iter()
            .enumerate()
            .map(|(at, thread)| (at, common(&thread.messages, &hashes)))
            .filter(|&(_, common)| common > 0)
            .max_by_key(|&(_, common)| common);
        // Said after a reply it names: it carries on from the session's
        // latest, whatever it shares with any of them.
        let best = if follows { None } else { best };
        let (resent, previous) = match best {
            Some((at, common)) => {
                // Carried on from where it ended: this takes its place. One
                // that went its own way after a start in common — a side
                // request opening on the same first message — leaves it be,
                // for the conversation's next turn to carry on from.
                let extends = common == session.threads[at].messages.len();
                let thread = if extends {
                    session.threads.remove(at)
                } else {
                    let thread = &session.threads[at];
                    Thread {
                        messages: Vec::new(),
                        system: thread.system,
                        tools: thread.tools,
                    }
                };
                // A request that only repeats its thread — a retry — still
                // counts its last message as new, so it is not said to have
                // added nothing at all.
                let resent = if common == hashes.len() {
                    common - 1
                } else {
                    common
                };
                (resent, Some(thread))
            }
            None if follows => {
                let latest = (!session.threads.is_empty()).then(|| session.threads.remove(0));
                (0, latest)
            }
            None => (0, None),
        };
        let continued = previous.is_some() || follows;
        let system_changed = previous.as_ref().is_none_or(|p| p.system != system);
        let tools_changed = previous.as_ref().is_none_or(|p| p.tools != tools);
        // What a request after a reply adds to the history that led to it,
        // so a later one that resends the lot still lines up with it.
        let history = match (&previous, follows) {
            (Some(previous), true) => previous.messages.iter().chain(&hashes).copied().collect(),
            _ => hashes,
        };
        // Most recent first, so the oldest is the one to drop.
        session.threads.insert(
            0,
            Thread {
                messages: history,
                system,
                tools,
            },
        );
        session.threads.truncate(THREADS);

        let size = |value: &Value| serde_json::to_string(value).map_or(0, |text| text.len());
        let mut kept = serde_json::Map::new();
        if system_changed && let Some(value) = request.get(shape.system) {
            kept.insert(shape.system.into(), value.clone());
        }
        if tools_changed && let Some(value) = request.get(shape.tools) {
            kept.insert(shape.tools.into(), value.clone());
        }
        let added = &messages[resent..];
        kept.insert(shape.turns.into(), Value::Array(added.to_vec()));
        Some(Split {
            system_bytes: request.get(shape.system).map_or(0, size),
            tools_bytes: request.get(shape.tools).map_or(0, size),
            resent_bytes: messages[..resent].iter().map(size).sum(),
            new_bytes: added.iter().map(size).sum(),
            new_messages: added.len(),
            continued,
            kept: Value::Object(kept),
        })
    }
}

/// The session a request belongs to: Claude Code's, from the metadata it
/// sends with every request, or Codex's thread, by the cache key it names
/// with every request; else the pane's own.
fn session_key(pane: &str, request: &Value) -> String {
    request
        .pointer("/metadata/user_id")
        .or_else(|| request.get("prompt_cache_key"))
        .and_then(Value::as_str)
        .map_or_else(|| pane.to_owned(), str::to_owned)
}

/// How many leading entries `before` and `now` have in common.
fn common(before: &[u64], now: &[u64]) -> usize {
    before
        .iter()
        .zip(now)
        .take_while(|(before, now)| before == now)
        .count()
}

/// A value's identity for comparing, its cache marks left out.
fn fingerprint(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_value(value, &mut hasher);
    hasher.finish()
}

fn hash_value(value: &Value, hasher: &mut DefaultHasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(b) => (1u8, b).hash(hasher),
        Value::Number(n) => (2u8, n.to_string()).hash(hasher),
        Value::String(s) => (3u8, s).hash(hasher),
        Value::Array(items) => {
            (4u8, items.len()).hash(hasher);
            for item in items {
                hash_value(item, hasher);
            }
        }
        Value::Object(map) => {
            // Its size as well as its entries: without one, `{"a":{},"b":1}`
            // and `{"a":{"b":1}}` would hash the same.
            let kept = map.keys().filter(|key| *key != "cache_control").count();
            (5u8, kept).hash(hasher);
            for (key, item) in map {
                if key == "cache_control" {
                    continue;
                }
                key.hash(hasher);
                hash_value(item, hasher);
            }
            6u8.hash(hasher);
        }
    }
}
