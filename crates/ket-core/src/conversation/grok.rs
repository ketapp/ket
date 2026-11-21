//! Grok's side of [`super`]: where a session's transcript is, what its
//! records say, and the error a turn ended on, which Grok keeps beside the
//! transcript rather than in it.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{Read_, Turn, TurnKind, carried, is_image, push_text, read_with, tagged, tool_line};
use crate::Result;

/// How much of a Grok session's `updates.jsonl` is searched for how its
/// last turn ended. Its records carry whole tool results, so this is more
/// than the few records at the end need.
const UPDATES_READ_BYTES: u64 = 256 * 1024;

/// Set on a Grok cursor whose read already showed the error the session's
/// last turn ended on — see [`read`]. Far above any transcript's length,
/// so an offset never reaches it.
const ERROR_SHOWN: u64 = 1 << 48;

/// The transcript of Grok session `session`: `chat_history.jsonl` in its own
/// directory, under whichever cwd's directory holds it — see
/// [`crate::sessions::GrokSessions`] for the layout. Only the cwd directories
/// are listed, never the sessions directory's parent, which holds Grok's
/// credentials.
pub fn transcript(session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    let root = crate::sessions::grok_sessions_dir();
    std::fs::read_dir(&root)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|cwd| cwd.path().join(session).join("chat_history.jsonl"))
        .find(|path| path.is_file())
}

/// Reads Grok transcript `path` from cursor `after`.
///
/// A turn Grok ended on an error — a usage limit, most often — says so in
/// its TUI and in the session's `updates.jsonl`, never in the transcript, so
/// the chat just stopped. Once a read has caught up, an error the session
/// ended on and has not moved on from is added as the agent's words, as
/// Codex's are. The cursor that read hands back carries [`ERROR_SHOWN`],
/// so the next read from the same place does not add it again; one after the
/// transcript has moved on reads it plain.
pub(super) fn read(path: &Path, after: u64) -> Result<Read_> {
    let shown = after & ERROR_SHOWN != 0;
    let after = after & !ERROR_SHOWN;
    let mut chunk = read_with(path, after, turns_of)?;
    if shown && !chunk.fresh && chunk.next == after {
        chunk.next |= ERROR_SHOWN;
        return Ok(chunk);
    }
    let caught_up = std::fs::metadata(path).is_ok_and(|m| m.len() == chunk.next);
    if let Some(error) = caught_up
        .then(|| ended_on(&path.with_file_name("updates.jsonl")))
        .flatten()
    {
        push_text(&mut chunk.turns, TurnKind::Assistant, &error);
        chunk.next |= ERROR_SHOWN;
    }
    Ok(chunk)
}

/// The error a Grok session's last turn ended on, from the end of its
/// `updates.jsonl`, unless the session has moved on since — a prompt, the
/// agent's words or thinking, or a tool started after it. `None` for a turn
/// that ended cleanly or was cancelled.
fn ended_on(updates: &Path) -> Option<String> {
    let mut file = std::fs::File::open(updates).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(UPDATES_READ_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    // A read that began mid-file began mid-record.
    let begin = match start {
        0 => 0,
        _ => bytes
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |at| at + 1),
    };
    for line in bytes[begin..].split(|&b| b == b'\n').rev() {
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let Some(update) = record.get("params").and_then(|p| p.get("update")) else {
            continue;
        };
        match update.get("sessionUpdate").and_then(|u| u.as_str()) {
            Some("turn_completed") => {
                if update.get("stop_reason").and_then(|r| r.as_str()) != Some("error") {
                    return None;
                }
                let said = [
                    update.get("agent_result"),
                    update.pointer("/error/data/message"),
                    update.pointer("/error/message"),
                ]
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str())
                .find(|m| !m.trim().is_empty());
                return Some(said.unwrap_or("Grok's turn ended on an error.").to_owned());
            }
            Some(
                "user_message_chunk" | "agent_message_chunk" | "agent_thought_chunk" | "tool_call",
            ) => return None,
            // Hooks, retries, a tool's late result, background bookkeeping:
            // none of them is the session moving on.
            _ => {}
        }
    }
    None
}

/// The turns one Grok `chat_history.jsonl` record holds.
///
/// What the person typed carries a `prompt_index`, wrapped in
/// `<user_query>`; Grok files the context it
/// feeds the model — the project's instructions, its system reminders — as
/// user records too, without one. The agent's words are its `content`, and
/// each of its `tool_calls` is a tool, named and keyed the way Grok's hooks
/// are before [`crate::agent_hooks`] renames them to Claude's.
fn turns_of(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    match record.get("type").and_then(|t| t.as_str()) {
        Some("user") if record.get("prompt_index").is_some() => {
            let images = match record.get("content") {
                Some(serde_json::Value::Array(blocks)) => {
                    blocks.iter().filter(|b| is_image(b)).count()
                }
                _ => 0,
            };
            let words = match record.get("content") {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(serde_json::Value::Array(blocks)) => blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                _ => return,
            };
            // Grok wraps what was typed in `<user_query>`, which would read
            // as the harness talking.
            let words = tagged(&words, "user_query").unwrap_or(&words);
            let from = turns.len();
            push_text(turns, TurnKind::User, words);
            carried(turns, from, u32::try_from(images).unwrap_or(u32::MAX));
        }
        Some("assistant") => {
            if let Some(text) = record.get("content").and_then(|c| c.as_str()) {
                push_text(turns, TurnKind::Assistant, text);
            }
            for call in record
                .get("tool_calls")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
            {
                let Some(name) = call.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                let mut input = call
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
                    .unwrap_or(serde_json::Value::Null);
                if let Some(input) = input.as_object_mut() {
                    crate::agent_hooks::grok_input(input);
                }
                let name = crate::agent_hooks::grok_tool(name).unwrap_or(name);
                turns.push(Turn {
                    kind: TurnKind::Tool,
                    text: tool_line(name, Some(&input)),
                    tool: name.to_owned(),
                    images: 0,
                });
            }
        }
        _ => {}
    }
}
