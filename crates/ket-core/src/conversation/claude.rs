//! Claude Code's side of [`super`]: where a session's transcript is, what
//! its records say, the messages still queued in it, and the step it is on.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{
    Read_, Turn, TurnKind, carried, clip, is_image, push_text, read_with, spoken, tool_line,
    without_image_marks,
};
use crate::{KetError, Result};

/// How much of a transcript's end is searched for messages still queued.
const QUEUE_READ_BYTES: u64 = 1024 * 1024;

/// What the chat says where Claude Code backgrounded the session.
pub const BACKGROUNDED: &str = "Claude Code backgrounded this session.";

/// The transcript of Claude Code session `session`, wherever Claude keeps
/// its projects — the one file named after it.
pub fn transcript(session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    let file = format!("{session}.jsonl");
    // Every config directory, not just the default one: a project whose own
    // `claude` command points somewhere else files its transcripts there.
    crate::sessions::claude_config_dirs()
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir.join("projects")).ok())
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// Reads Claude transcript `path` from byte `after` — `0` for a first read,
/// which looks at the recent end only and keeps the last [`super::FIRST_TURNS`]
/// turns. Stops at the last whole line, so a record still being written is
/// read next time.
pub fn read(path: &Path, after: u64) -> Result<Read_> {
    read_with(path, after, turns_of)
}

/// Messages the person has sent that the agent has not read yet, oldest
/// first: typed while it was working, they wait in Claude's queue until it
/// takes them in mid-turn or starts its next turn with them. Claude records
/// each step — `enqueue` with the text, `dequeue` taking the oldest,
/// `remove` taking one by its text — and what is left is still waiting.
/// Only the transcript's recent end is searched; a message queued before it
/// has long since been read.
pub fn queued(path: &Path) -> Result<Vec<String>> {
    let mut file = std::fs::File::open(path).map_err(|e| KetError::io(path, e))?;
    let length = file.metadata().map_err(|e| KetError::io(path, e))?.len();
    let start = length.saturating_sub(QUEUE_READ_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| KetError::io(path, e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| KetError::io(path, e))?;

    let marker = b"\"queue-operation\"";
    let mut waiting: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for line in bytes.split(|&b| b == b'\n') {
        if !line.windows(marker.len()).any(|w| w == marker) {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let content = record.get("content").and_then(|c| c.as_str());
        match record.get("operation").and_then(|o| o.as_str()) {
            Some("enqueue") => waiting.push_back(content.unwrap_or_default().to_owned()),
            Some("dequeue") => {
                waiting.pop_front();
            }
            Some("remove") => {
                if let Some(at) = waiting.iter().position(|w| Some(w.as_str()) == content) {
                    waiting.remove(at);
                }
            }
            _ => {}
        }
    }
    // Task notifications and the like queue too; only what a person typed,
    // as the conversation will show it once it is read.
    Ok(waiting
        .into_iter()
        .map(|text| without_image_marks(&text).trim().to_owned())
        .filter(|text| !text.is_empty() && !text.starts_with('<'))
        .map(|text| clip(&text))
        .collect())
}

/// The turns one transcript record holds, if it holds any.
fn turns_of(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    let kind = match record.get("type").and_then(|t| t.as_str()) {
        Some("user") => TurnKind::User,
        Some("assistant") => TurnKind::Assistant,
        Some("attachment") => return absorbed(record, turns),
        // Claude Code backgrounded the session: the rest of its turn, and
        // everything after, is written under a new session id.
        Some("continued-in") => {
            push_text(turns, TurnKind::Assistant, BACKGROUNDED);
            return;
        }
        // What a local command printed, and sometimes the command itself,
        // filed as the harness's own note: read as the person's, where
        // `push_text` keeps only those two.
        Some("system") => {
            if record.get("subtype").and_then(|t| t.as_str()) == Some("local_command")
                && let Some(text) = record.get("content").and_then(|c| c.as_str())
            {
                push_text(turns, TurnKind::User, text);
            }
            return;
        }
        _ => return,
    };
    // A subagent's own conversation, and records the agent adds for itself.
    let flag = |name: &str| record.get(name).and_then(serde_json::Value::as_bool) == Some(true);
    if flag("isSidechain") || flag("isMeta") || flag("isCompactSummary") {
        return;
    }
    // Answers to the agent's own questions come back as its tool's result:
    // what was picked is the person's turn.
    if let Some(answers) = record
        .get("toolUseResult")
        .and_then(|r| r.get("answers"))
        .and_then(serde_json::Value::as_object)
    {
        let picked: Vec<&str> = answers.values().filter_map(|a| a.as_str()).collect();
        push_text(turns, TurnKind::User, &picked.join("\n"));
        return;
    }
    let Some(content) = record.get("message").and_then(|m| m.get("content")) else {
        return;
    };
    match content {
        serde_json::Value::String(text) => push_text(turns, kind, text),
        serde_json::Value::Array(blocks) => {
            let from = turns.len();
            let mut images = 0;
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            push_text(turns, kind, text);
                        }
                    }
                    Some("image") => images += 1,
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("Tool");
                        if let Some(said) = spoken(name, block.get("input")) {
                            push_text(turns, TurnKind::Assistant, &said);
                        } else {
                            turns.push(Turn {
                                kind: TurnKind::Tool,
                                text: tool_line(name, block.get("input")),
                                tool: name.to_owned(),
                                images: 0,
                            });
                        }
                    }
                    // Thinking and tool results: not the conversation.
                    _ => {}
                }
            }
            if kind == TurnKind::User {
                carried(turns, from, images);
            }
        }
        _ => {}
    }
}

/// A message typed while the agent was working: Claude folds it into the
/// turn in progress and records it as an attachment, not a user record.
fn absorbed(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    let Some(attachment) = record.get("attachment") else {
        return;
    };
    let field = |name: &str| attachment.get(name).and_then(|v| v.as_str());
    if field("type") != Some("queued_command") || field("commandMode") != Some("prompt") {
        return;
    }
    match attachment.get("prompt") {
        Some(serde_json::Value::String(text)) => push_text(turns, TurnKind::User, text),
        Some(serde_json::Value::Array(blocks)) => {
            let from = turns.len();
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(text) = block.get("text").and_then(|t| t.as_str())
                {
                    push_text(turns, TurnKind::User, text);
                }
            }
            let images = blocks.iter().filter(|block| is_image(block)).count();
            carried(turns, from, u32::try_from(images).unwrap_or(u32::MAX));
        }
        _ => {}
    }
}

/// What a Claude session last did, read off the end of its transcript —
/// see [`last_step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastStep {
    /// The newest tool call, as the chat names one: `$ npm test`, `Edited
    /// TabBar.tsx`.
    pub doing: String,
    /// The same call as a request waiting on a person: `Wants to run npm
    /// test`, `Has a question`.
    pub asking: String,
    /// Whether its result has come back.
    pub answered: bool,
}

/// How much of a transcript's end [`last_step`] reads.
const LAST_STEP_BYTES: u64 = 256 * 1024;

/// The newest tool call at the end of Claude transcript `path`, and whether
/// its result has come back — what a list says a session is doing, or
/// waiting for. A subagent's calls are its own, not the session's, and are
/// passed over. `None` when the end holds no call.
pub fn last_step(path: &Path) -> Option<LastStep> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(LAST_STEP_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let mut last: Option<(String, String, Option<serde_json::Value>)> = None;
    let mut answered = false;
    for line in bytes.split(|&b| b == b'\n') {
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if record
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let kind = record.get("type").and_then(|t| t.as_str());
        let Some(content) = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for part in content {
            match (kind, part.get("type").and_then(|t| t.as_str())) {
                (Some("assistant"), Some("tool_use")) => {
                    let id = part.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                    let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("Tool");
                    last = Some((id.to_owned(), name.to_owned(), part.get("input").cloned()));
                    answered = false;
                }
                (Some("user"), Some("tool_result")) => {
                    let id = part.get("tool_use_id").and_then(|i| i.as_str());
                    if last
                        .as_ref()
                        .is_some_and(|(asked, _, _)| Some(asked.as_str()) == id)
                    {
                        answered = true;
                    }
                }
                _ => {}
            }
        }
    }
    let (_, name, input) = last?;
    let field = |key: &str| {
        input
            .as_ref()
            .and_then(|i| i.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.lines().next().unwrap_or(s).trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let leaf =
        |key: &str| field(key).map(|path| path.rsplit('/').next().unwrap_or(&path).to_owned());
    let asking = match name.as_str() {
        "AskUserQuestion" => "Has a question".to_owned(),
        "ExitPlanMode" => "Has a plan ready".to_owned(),
        "Bash" => field("command").map_or_else(
            || "Wants to run a command".to_owned(),
            |c| format!("Wants to run {c}"),
        ),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => leaf("file_path")
            .or_else(|| leaf("notebook_path"))
            .map_or_else(
                || "Wants to edit a file".to_owned(),
                |f| format!("Wants to edit {f}"),
            ),
        "WebFetch" => field("url").map_or_else(
            || "Wants to fetch a page".to_owned(),
            |u| format!("Wants to fetch {u}"),
        ),
        other => format!("Wants to use {other}"),
    };
    Some(LastStep {
        doing: tool_line(&name, input.as_ref()),
        asking,
        answered,
    })
}

#[cfg(test)]
mod last_step_tests {
    use super::*;

    fn transcript(name: &str, records: &[serde_json::Value]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ket-last-step-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn call(id: &str, name: &str, input: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":id,"name":name,"input":input}]}})
    }

    fn result(id: &str) -> serde_json::Value {
        serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":id,"content":"done"}]}})
    }

    #[test]
    fn the_last_step_is_the_newest_call_and_whether_it_was_answered() {
        use std::io::Write;

        let path = transcript(
            "bash",
            &[
                call(
                    "t1",
                    "Read",
                    serde_json::json!({"file_path":"/a/b/main.rs"}),
                ),
                result("t1"),
                call(
                    "t2",
                    "Bash",
                    serde_json::json!({"command":"npm test\n# second line"}),
                ),
            ],
        );
        let step = last_step(&path).unwrap();
        assert_eq!(step.doing, "$ npm test");
        assert_eq!(step.asking, "Wants to run npm test");
        assert!(!step.answered);
        // Its result comes back: answered, the same step.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", result("t2")).unwrap();
        let step = last_step(&path).unwrap();
        assert_eq!(step.doing, "$ npm test");
        assert!(step.answered);
    }

    #[test]
    fn a_question_and_an_edit_are_said_as_what_they_want() {
        let path = transcript(
            "ask",
            &[call(
                "t1",
                "AskUserQuestion",
                serde_json::json!({"questions":[]}),
            )],
        );
        assert_eq!(last_step(&path).unwrap().asking, "Has a question");
        let path = transcript(
            "edit",
            &[call(
                "t2",
                "Edit",
                serde_json::json!({"file_path":"/src/ui/TabBar.tsx"}),
            )],
        );
        let step = last_step(&path).unwrap();
        assert_eq!(step.doing, "Edited TabBar.tsx");
        assert_eq!(step.asking, "Wants to edit TabBar.tsx");
        let path = transcript("plan", &[call("t3", "ExitPlanMode", serde_json::json!({}))]);
        assert_eq!(last_step(&path).unwrap().asking, "Has a plan ready");
        let path = transcript(
            "other",
            &[call("t4", "WebSearch", serde_json::json!({"query":"x"}))],
        );
        assert_eq!(last_step(&path).unwrap().asking, "Wants to use WebSearch");
    }

    #[test]
    fn a_subagents_calls_are_not_the_sessions_and_a_transcript_without_calls_has_no_step() {
        let mut side = call("s1", "Bash", serde_json::json!({"command":"rm -rf build"}));
        side["isSidechain"] = serde_json::json!(true);
        let path = transcript(
            "side",
            &[
                call("t1", "Grep", serde_json::json!({"pattern":"TODO"})),
                side,
            ],
        );
        assert_eq!(last_step(&path).unwrap().doing, "Searched for TODO");
        let path = transcript(
            "none",
            &[serde_json::json!({"type":"user","message":{"content":"hello"}})],
        );
        assert_eq!(last_step(&path), None);
        assert_eq!(last_step(Path::new("/nonexistent/x.jsonl")), None);
    }
}
