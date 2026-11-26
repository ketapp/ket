//! Codex's side of [`super`]: where a session's rollout is, and what its
//! records say.

use std::path::{Path, PathBuf};

use super::{Read_, Turn, TurnKind, carried, is_image, push_text, read_with, tool_line};
use crate::Result;

/// The rollout of Codex session `session`: the one file in Codex's dated
/// sessions store whose name ends in its id. Remembered once found — the
/// store is a directory per day, and a phone asks every couple of seconds.
pub fn transcript(session: &str) -> Option<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, PoisonError};

    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    static FOUND: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let found = FOUND.get_or_init(Mutex::default);
    if let Some(path) = found
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(session)
        .filter(|path| path.is_file())
    {
        return Some(path.clone());
    }

    // `<root>/<year>/<month>/<day>/rollout-<when>-<id>.jsonl`, newest first:
    // a session being looked at is almost always a recent one.
    let suffix = format!("-{session}.jsonl");
    let newest_first = |dir: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .collect();
        entries.sort_unstable_by(|a, b| b.cmp(a));
        entries
    };
    let root = crate::sessions::codex_sessions_dir();
    let path = newest_first(&root)
        .iter()
        .flat_map(|year| newest_first(year))
        .flat_map(|month| newest_first(&month))
        .flat_map(|day| newest_first(&day))
        .find(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&suffix))
        })?;
    found
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(session.to_owned(), path.clone());
    Some(path)
}

/// [`super::claude::read`], for a Codex rollout.
pub fn read(path: &Path, after: u64) -> Result<Read_> {
    read_with(path, after, turns_of)
}

/// The error the rollout's last turn ended on. Codex records it on the
/// turn's own `task_complete`, with its name for it in `codex_error_info` —
/// `usage_limit_exceeded` for a plan run out — and, for a dropped stream, as
/// a bare `error` event that may follow. The turn's own account wins over
/// that one. `None` for a turn that completed without one, or once the next
/// turn's records have begun.
pub(super) fn ended_on(path: &Path) -> Option<super::Ended> {
    let bytes = super::ending(path)?;
    // A bare `error` met on the way back, should no turn's end name one.
    let mut loose: Option<super::Ended> = None;
    for line in bytes.split(|&b| b == b'\n').rev() {
        if !holds(line, b"event_msg") {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let Some(payload) = record.get("payload") else {
            continue;
        };
        let said = |message: Option<&serde_json::Value>| {
            message
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        match payload.get("type").and_then(|t| t.as_str()) {
            Some("task_complete") => {
                let Some(error) = payload.get("error").filter(|e| e.is_object()) else {
                    return loose;
                };
                return Some(super::Ended {
                    failure: crate::limits::Failure {
                        code: error
                            .get("codex_error_info")
                            .and_then(|info| info.as_str())
                            .map(str::to_owned),
                        message: said(error.get("message")),
                    },
                    at_ms: super::claude::dated(&record),
                    closed: true,
                });
            }
            Some("error") if loose.is_none() => {
                loose = Some(super::Ended {
                    failure: crate::limits::Failure::said(said(payload.get("message"))),
                    at_ms: super::claude::dated(&record),
                    closed: false,
                });
            }
            // The turn at work, or the one before it: nothing has ended yet.
            Some("task_started" | "item_started" | "item_completed" | "user_message") => {
                return loose;
            }
            // Token counts, settings applied.
            _ => {}
        }
    }
    loose
}

/// The plan windows in the rollout's newest `token_count`, which carries a
/// `rate_limits` block beside the counts.
pub(super) fn quota(path: &Path) -> Vec<crate::rate_limits::RateWindow> {
    let Some(bytes) = super::ending(path) else {
        return Vec::new();
    };
    bytes
        .split(|&b| b == b'\n')
        .rev()
        .filter(|line| holds(line, b"rate_limits"))
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find_map(|record| {
            let limits = record.pointer("/payload/rate_limits")?;
            limits
                .is_object()
                .then(|| crate::rate_limits::rollout_windows(limits))
        })
        .unwrap_or_default()
}

/// Whether `line` has `word` in it: a look before parsing, since most of a
/// rollout's lines are not the ones asked about.
fn holds(line: &[u8], word: &[u8]) -> bool {
    line.windows(word.len()).any(|found| found == word)
}

/// The turns one Codex rollout record holds.
///
/// Read from the `item_completed` events, which carry each finished item
/// once: what the person typed, what the agent said, the commands it ran and
/// the files it changed. The rollout's `response_item` records say the same
/// things again, alongside the instructions and context Codex feeds the
/// model as though the person had typed them.
pub(super) fn turns_of(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    if record.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
        return;
    }
    let Some(payload) = record.get("payload") else {
        return;
    };
    // A turn Codex ended on an error — a usage limit, most often — says so
    // in its TUI and nowhere in its items. Without this the conversation just
    // stopped, and read as a phone that had lost it.
    let ended_with = match payload.get("type").and_then(|t| t.as_str()) {
        Some("task_complete") => payload.get("error").and_then(|e| e.get("message")),
        Some("error") => payload.get("message"),
        _ => None,
    };
    if let Some(message) = ended_with.and_then(|m| m.as_str()) {
        push_text(turns, TurnKind::Assistant, message);
        return;
    }
    if payload.get("type").and_then(|t| t.as_str()) != Some("item_completed") {
        return;
    }
    let Some(item) = payload.get("item") else {
        return;
    };
    let words = || -> String {
        item.get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let tool = |turns: &mut Vec<Turn>, name: &str, input: serde_json::Value| {
        turns.push(Turn {
            kind: TurnKind::Tool,
            text: tool_line(name, Some(&input)),
            tool: name.to_owned(),
            images: 0,
        });
    };
    match item.get("type").and_then(|t| t.as_str()) {
        Some("UserMessage") => {
            let from = turns.len();
            push_text(turns, TurnKind::User, &words());
            let images = item
                .get("content")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
                .filter(|block| is_image(block))
                .count();
            carried(turns, from, u32::try_from(images).unwrap_or(u32::MAX));
        }
        Some("AgentMessage") => push_text(turns, TurnKind::Assistant, &words()),
        Some("CommandExecution") => {
            // `["/bin/zsh", "-lc", "<the command>"]`: the script is what was
            // run, the shell around it is Codex's.
            let command = match item.get("command") {
                Some(serde_json::Value::Array(parts)) => {
                    let parts: Vec<&str> = parts.iter().filter_map(|p| p.as_str()).collect();
                    match parts.as_slice() {
                        [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
                            (*script).to_owned()
                        }
                        _ => parts.join(" "),
                    }
                }
                Some(serde_json::Value::String(command)) => command.clone(),
                _ => return,
            };
            tool(turns, "Bash", serde_json::json!({ "command": command }));
        }
        Some("FileChange") => {
            for path in item
                .get("changes")
                .and_then(|c| c.as_object())
                .into_iter()
                .flat_map(|changes| changes.keys())
            {
                tool(turns, "Edit", serde_json::json!({ "file_path": path }));
            }
        }
        Some("Extension") if item.get("kind").and_then(|k| k.as_str()) == Some("web.search") => {
            if let Some(query) = item
                .get("query")
                .and_then(|q| q.as_str())
                .filter(|q| !q.trim().is_empty())
            {
                tool(turns, "WebSearch", serde_json::json!({ "query": query }));
            }
        }
        _ => {}
    }
}
