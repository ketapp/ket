//! OpenCode's side of [`super`]: a session read from OpenCode's SQLite
//! database, and its tools by Claude's names.

use std::path::Path;

use super::{FIRST_TURNS, Read_, Turn, TurnKind, push_text, spoken, tool_line};
use crate::{KetError, Result};

/// Whether `session` looks like an OpenCode session id — `ses_` and letters
/// and digits. Checked before it goes into a statement.
pub(crate) fn id(session: &str) -> bool {
    session.len() > 4
        && session.starts_with("ses_")
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// How many of a session's newest messages a first OpenCode read looks at:
/// enough for [`FIRST_TURNS`] turns, since most steps are a tool or two.
const FIRST_MESSAGES: usize = 400;

/// Most rows (message/part pairs, so several per message) one incremental
/// read may ask `sqlite3` for. The fresh path above is already bounded by
/// its own `OFFSET`; a read from an arbitrary cursor has nothing else
/// capping how far back it could start, so this is that cap.
const INCREMENTAL_ROWS: u64 = 2_000;

/// Reads OpenCode session `session` from cursor `after`: from OpenCode 2's
/// `session_message`, or OpenCode 1's `message` and `part` in a database
/// that has not been moved on yet. See [`read_2`] and
/// [`read_1`].
pub(super) fn read(database: &Path, session: &str, after: u64) -> Result<Read_> {
    match read_2(database, session, after) {
        Err(KetError::Agent { why, .. }) if why.contains("no such table: session_message") => {
            read_1(database, session, after)
        }
        read => read,
    }
}

/// The lowest cursor an OpenCode 1 read hands out — a creation time in
/// milliseconds, times 1000 — and far above any OpenCode 2 `seq`. A phone
/// still holding one after OpenCode moved its database on starts over.
const VERSION_1_CURSORS: u64 = 1 << 40;

/// Where an OpenCode 2 cursor keeps the retry attempt a read already showed
/// for the step it waits at, in bits 32–39 above the `seq` — see
/// [`retrying`]. Below [`VERSION_1_CURSORS`], so it is never taken for one.
const RETRY_SHIFT: u32 = 32;

/// Reads OpenCode 2 session `session` from cursor `after`.
///
/// Through the `sqlite3` binary, one read-only statement naming columns of
/// `session_message` only — the way [`crate::sessions::OpencodeSessions`]
/// reads the same database, which also holds OpenCode's credentials.
///
/// Each row is one message: `user`, `assistant` with its text and tool calls
/// in `content`, or OpenCode's own bookkeeping (`idle`, `synthetic`,
/// `compaction`), which says nothing here. The cursor is the next message's
/// `seq`, which only ever grows within a session. As with OpenCode 1, the
/// read stops at an assistant message still streaming when it is the last.
///
/// A step that ended on an error, or is waiting to try its model again,
/// says so in OpenCode's TUI and nowhere in the conversation, so the chat
/// just stopped: both are added as the agent's words, as Codex's and Grok's
/// errors are — see [`failed`] and [`retrying`].
fn read_2(database: &Path, session: &str, after: u64) -> Result<Read_> {
    let fresh = after == 0 || after >= VERSION_1_CURSORS;
    let (after, shown) = match fresh {
        true => (after, 0),
        false => (after & ((1 << RETRY_SHIFT) - 1), after >> RETRY_SHIFT),
    };
    let from = match fresh {
        true => format!(
            "COALESCE((SELECT seq FROM session_message WHERE session_id = '{session}' \
             ORDER BY seq DESC LIMIT 1 OFFSET {}), 0)",
            FIRST_MESSAGES - 1
        ),
        false => after.to_string(),
    };
    let output = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg("-json")
        .arg(database)
        .arg(format!(
            "SELECT seq, type, data FROM session_message \
             WHERE session_id = '{session}' AND seq >= {from} ORDER BY seq {};",
            if fresh {
                String::new()
            } else {
                format!("LIMIT {INCREMENTAL_ROWS}")
            }
        ))
        .output()
        .map_err(|e| KetError::io(database, e))?;
    if !output.status.success() {
        return Err(KetError::Agent {
            agent: "opencode".to_owned(),
            why: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    // No rows prints nothing at all rather than `[]`.
    let rows: Vec<serde_json::Value> = match output.stdout.iter().all(u8::is_ascii_whitespace) {
        true => Vec::new(),
        false => serde_json::from_slice(&output.stdout).map_err(|e| KetError::Agent {
            agent: "opencode".to_owned(),
            why: e.to_string(),
        })?,
    };

    let mut next = if fresh { 0 } else { after };
    let mut turns = Vec::new();
    // The retry attempt shown for the step this read stops at.
    let mut waiting = 0;
    let count = rows.len();
    for (index, row) in rows.iter().enumerate() {
        let Some(seq) = row.get("seq").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let kind = row.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        let data = row
            .get("data")
            .and_then(|d| d.as_str())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
            .unwrap_or_default();
        let finished = kind != "assistant"
            || data
                .get("time")
                .and_then(|t| t.get("completed"))
                .is_some_and(|c| !c.is_null());
        if index + 1 == count && !finished {
            // The cursor's attempt is this step's only when the step is the
            // first row read, the one the cursor already waited at.
            waiting = if index == 0 { shown } else { 0 };
            if let Some((attempt, said)) = retrying(&data)
                && attempt > waiting
                && next > 0
            {
                push_text(&mut turns, TurnKind::Assistant, &said);
                waiting = attempt;
            }
            break;
        }
        match kind {
            "user" => {
                if let Some(text) = data.get("text").and_then(|t| t.as_str()) {
                    push_text(&mut turns, TurnKind::User, text);
                }
            }
            "assistant" => {
                for part in data
                    .get("content")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                push_text(&mut turns, TurnKind::Assistant, text);
                            }
                        }
                        Some("tool") => push_tool(
                            part.get("name").and_then(|t| t.as_str()).unwrap_or("Tool"),
                            part.get("state").and_then(|s| s.get("input")),
                            &mut turns,
                        ),
                        _ => {}
                    }
                }
                if let Some(said) = failed(&data) {
                    push_text(&mut turns, TurnKind::Assistant, &said);
                }
            }
            _ => {}
        }
        next = seq + 1;
    }
    if fresh && turns.len() > FIRST_TURNS {
        turns.drain(..turns.len() - FIRST_TURNS);
    }
    if next > 0 {
        next |= waiting << RETRY_SHIFT;
    }
    Ok(Read_ { turns, next, fresh })
}

/// The error an OpenCode 2 step ended on — the model refusing the
/// connection, a usage limit — in OpenCode's words. `None` for a step that
/// ended cleanly, or one the person interrupted (`aborted`).
fn failed(data: &serde_json::Value) -> Option<String> {
    let error = data.get("error").filter(|e| e.is_object())?;
    if error.get("type").and_then(|t| t.as_str()) == Some("aborted") {
        return None;
    }
    let said = error
        .get("message")
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("OpenCode's step ended on an error.");
    Some(said.to_owned())
}

/// A step waiting to try its model again, and which attempt that will be:
/// OpenCode keeps `retry` on the step while it waits and clears it when the
/// step goes ahead. A local model still reading a long conversation outlasts
/// OpenCode's wait for an answer, which looks like nothing happening at all.
/// The attempt handed back is kept to what [`RETRY_SHIFT`]'s bits hold; the
/// words say the real one.
fn retrying(data: &serde_json::Value) -> Option<(u64, String)> {
    let retry = data.get("retry").filter(|r| r.is_object())?;
    let attempt = retry
        .get("attempt")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
        .max(1);
    let why = retry
        .pointer("/error/message")
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("The model didn't answer");
    Some((
        attempt.min(0xff),
        format!("{why} — trying again (attempt {attempt})"),
    ))
}

/// Reads OpenCode 1 session `session` from cursor `after`.
///
/// Through the `sqlite3` binary, one read-only statement naming columns of
/// `message` and `part` only — the way [`crate::sessions::OpencodeSessions`]
/// reads the same database, which also holds OpenCode's credentials.
///
/// The cursor is a message's creation time in milliseconds times 1000, plus
/// how many messages at that same millisecond were already read: creation
/// times never change, where a row's `rowid` might if OpenCode rewrote it,
/// and two messages created in one millisecond are still read once each.
///
/// Only finished messages are read, and the read stops at the first one that
/// is not — an assistant step still streaming, or the person's message before
/// its words are written — so a turn is never sent half-said and then again.
/// A message with any after it is finished whatever it says: OpenCode works
/// one step at a time, and a step it abandoned may never be marked complete.
fn read_1(database: &Path, session: &str, after: u64) -> Result<Read_> {
    let fresh = after == 0;
    let (since, skip) = (after / 1000, after % 1000);
    let from = match fresh {
        true => format!(
            "COALESCE((SELECT time_created FROM message WHERE session_id = '{session}' \
             ORDER BY time_created DESC, id DESC LIMIT 1 OFFSET {}), 0)",
            FIRST_MESSAGES - 1
        ),
        false => since.to_string(),
    };
    let output = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg("-json")
        .arg(database)
        .arg(format!(
            "SELECT m.time_created AS at, m.data AS message, p.data AS part \
             FROM message m LEFT JOIN part p ON p.message_id = m.id \
             WHERE m.session_id = '{session}' AND m.time_created >= {from} \
             ORDER BY m.time_created, m.id, p.id {};",
            if fresh {
                String::new()
            } else {
                format!("LIMIT {INCREMENTAL_ROWS}")
            }
        ))
        .output()
        .map_err(|e| KetError::io(database, e))?;
    if !output.status.success() {
        return Err(KetError::Agent {
            agent: "opencode".to_owned(),
            why: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    // No rows prints nothing at all rather than `[]`.
    let rows: Vec<serde_json::Value> = match output.stdout.iter().all(u8::is_ascii_whitespace) {
        true => Vec::new(),
        false => serde_json::from_slice(&output.stdout).map_err(|e| KetError::Agent {
            agent: "opencode".to_owned(),
            why: e.to_string(),
        })?,
    };

    // Rows are a message's columns once per part; one entry per message.
    let mut messages: Vec<(u64, serde_json::Value, Vec<serde_json::Value>)> = Vec::new();
    let json = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(|v| v.as_str())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
    };
    for row in &rows {
        let at = row
            .get("at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let Some(message) = json(row, "message") else {
            continue;
        };
        let part = json(row, "part");
        let same = messages.last().is_some_and(|(last_at, last, _)| {
            *last_at == at && last.get("id") == message.get("id")
        });
        if !same {
            messages.push((at, message, Vec::new()));
        }
        if let (Some(part), Some((_, _, parts))) = (part, messages.last_mut()) {
            parts.push(part);
        }
    }

    let (mut next_at, mut next_skip) = (since, skip);
    let mut skipped = 0;
    let mut turns = Vec::new();
    let count = messages.len();
    for (index, (at, message, parts)) in messages.iter().enumerate() {
        if !fresh && *at == since && skipped < skip {
            skipped += 1;
            continue;
        }
        let last = index + 1 == count;
        let finished = match message.get("role").and_then(|r| r.as_str()) {
            Some("user") => !parts.is_empty(),
            _ => message
                .get("time")
                .and_then(|t| t.get("completed"))
                .is_some_and(|c| !c.is_null()),
        };
        if last && !finished {
            break;
        }
        turns_of(message, parts, &mut turns);
        if *at == next_at {
            next_skip += 1;
        } else {
            (next_at, next_skip) = (*at, 1);
        }
    }
    if fresh && turns.len() > FIRST_TURNS {
        turns.drain(..turns.len() - FIRST_TURNS);
    }
    Ok(Read_ {
        turns,
        next: match next_at {
            0 => 0,
            at => at * 1000 + next_skip.min(999),
        },
        fresh,
    })
}

/// The turns one OpenCode message holds, from its parts in order.
///
/// The person's words are the text parts of a user message, less the ones
/// OpenCode adds itself (`synthetic`). An assistant message is one step: its
/// text parts are what it said, and each tool part a tool — its `question`
/// asked out in full, as Claude's `AskUserQuestion` is. Thinking, the step
/// markers, snapshots and patches are not the conversation.
fn turns_of(message: &serde_json::Value, parts: &[serde_json::Value], turns: &mut Vec<Turn>) {
    let kind = match message.get("role").and_then(|r| r.as_str()) {
        Some("user") => TurnKind::User,
        Some("assistant") => TurnKind::Assistant,
        _ => return,
    };
    for part in parts {
        let synthetic = part.get("synthetic").and_then(serde_json::Value::as_bool) == Some(true);
        match part.get("type").and_then(|t| t.as_str()) {
            Some("text") if !synthetic => {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    push_text(turns, kind, text);
                }
            }
            Some("tool") if kind == TurnKind::Assistant => push_tool(
                part.get("tool").and_then(|t| t.as_str()).unwrap_or("Tool"),
                part.get("state").and_then(|s| s.get("input")),
                turns,
            ),
            Some("subtask") => {
                let input = serde_json::json!({ "description": part.get("description") });
                turns.push(Turn {
                    kind: TurnKind::Tool,
                    text: tool_line("Task", Some(&input)),
                    tool: "Task".to_owned(),
                    images: 0,
                });
            }
            _ => {}
        }
    }
}

/// OpenCode tool call `name` with `input`: what it said, for a tool that
/// speaks, or its one line.
fn push_tool(name: &str, input: Option<&serde_json::Value>, turns: &mut Vec<Turn>) {
    let (name, input) = tool(name, input);
    if let Some(said) = spoken(name, input.as_ref()) {
        push_text(turns, TurnKind::Assistant, &said);
    } else {
        turns.push(Turn {
            kind: TurnKind::Tool,
            text: tool_line(name, input.as_ref()),
            tool: name.to_owned(),
            images: 0,
        });
    }
}

/// An OpenCode tool by Claude's name, with its input keyed the way
/// [`tool_line`] and [`spoken`] read Claude's. The plugin ket installs in
/// OpenCode maps the same names for its hook reports — see `OPENCODE_PLUGIN`
/// in [`crate::agent_hooks`] — so a tool reads the same in both.
fn tool<'a>(
    name: &'a str,
    input: Option<&serde_json::Value>,
) -> (&'a str, Option<serde_json::Value>) {
    let claude = match name {
        // OpenCode 2 calls it `shell`; a subagent, `subagent`.
        "bash" | "shell" => "Bash",
        "read" => "Read",
        "edit" | "multiedit" | "patch" => "Edit",
        "write" => "Write",
        "grep" => "Grep",
        "glob" => "Glob",
        "list" => "LS",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "task" | "subagent" => "Task",
        "skill" => "Skill",
        "todowrite" => "TodoWrite",
        "question" => "AskUserQuestion",
        other => other,
    };
    let mut input = input.cloned();
    // A file's path: `filePath` in OpenCode 1, `path` in OpenCode 2 — where
    // Grep's `path` is a folder to search, as Claude's is.
    let file = matches!(claude, "Read" | "Edit" | "Write");
    if let Some(object) = input.as_mut().and_then(|i| i.as_object_mut())
        && !object.contains_key("file_path")
        && let Some(path) = object
            .get("filePath")
            .or_else(|| object.get("path").filter(|_| file))
            .cloned()
    {
        object.insert("file_path".to_owned(), path);
    }
    (claude, input)
}

#[cfg(test)]
mod opencode_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn opencode_2s_tools_read_as_claudes() {
        let (name, input) = tool("shell", Some(&serde_json::json!({"command": "ls"})));
        assert_eq!(name, "Bash");
        assert_eq!(input.unwrap()["command"], "ls");
        assert_eq!(tool("subagent", None).0, "Task");
        assert_eq!(tool("skill", None).0, "Skill");
        let (name, input) = tool("read", Some(&serde_json::json!({"path": "/a/b.rs"})));
        assert_eq!(name, "Read");
        assert_eq!(input.unwrap()["file_path"], "/a/b.rs");
        // Grep's path is a folder to search, as Claude's is: not a file.
        let (_, input) = tool(
            "grep",
            Some(&serde_json::json!({"pattern": "x", "path": "/a"})),
        );
        assert!(input.unwrap().get("file_path").is_none());
        // OpenCode 1's own key still works.
        let (_, input) = tool("edit", Some(&serde_json::json!({"filePath": "/c.rs"})));
        assert_eq!(input.unwrap()["file_path"], "/c.rs");
    }

    /// A throwaway OpenCode 2 database holding one session's messages, each
    /// `(seq, type, data)`.
    fn database(name: &str, rows: &[(u64, &str, serde_json::Value)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ket-opencode-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.db");
        let _ = std::fs::remove_file(&path);
        let mut sql = String::from(
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, \
             type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, \
             time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
        );
        for (seq, kind, data) in rows {
            let data = data.to_string().replace('\'', "''");
            sql.push_str(&format!(
                "INSERT INTO session_message VALUES ('m{seq}', 'ses_one', '{kind}', {seq}, {seq}, {seq}, '{data}');"
            ));
        }
        let status = std::process::Command::new("sqlite3")
            .arg(&path)
            .arg(sql)
            .status()
            .expect("sqlite3");
        assert!(status.success());
        path
    }

    #[test]
    fn an_opencode_2_session_is_read_by_seq_and_waits_for_a_step_still_streaming() {
        let db = database(
            "read",
            &[
                (
                    1,
                    "user",
                    serde_json::json!({"time": {"created": 1}, "text": "hello"}),
                ),
                (
                    2,
                    "assistant",
                    serde_json::json!({"time": {"created": 2, "completed": 2}, "content": [
                        {"type": "reasoning", "text": "hmm"},
                        {"type": "tool", "name": "shell", "state": {"input": {"command": "ls"}}},
                        {"type": "text", "text": "done"}
                    ]}),
                ),
                (
                    3,
                    "idle",
                    serde_json::json!({"time": {"created": 3}, "outcome": "succeeded"}),
                ),
                (
                    4,
                    "assistant",
                    serde_json::json!({"time": {"created": 4}, "content": [{"type": "text", "text": "still going"}]}),
                ),
            ],
        );
        let first = read(&db, "ses_one", 0).unwrap();
        assert!(first.fresh);
        let said: Vec<(TurnKind, String)> = first
            .turns
            .iter()
            .map(|turn| (turn.kind, turn.text.clone()))
            .collect();
        assert_eq!(said[0], (TurnKind::User, "hello".to_owned()));
        assert_eq!(said[1].0, TurnKind::Tool);
        assert!(said[1].1.contains("ls"), "{}", said[1].1);
        assert_eq!(said[2], (TurnKind::Assistant, "done".to_owned()));
        assert_eq!(said.len(), 3, "the step still streaming waits");
        assert_eq!(first.next, 4, "read again from the unfinished step");
        // Nothing new: the unfinished step still holds the cursor.
        let again = read(&db, "ses_one", first.next).unwrap();
        assert!(!again.fresh);
        assert!(again.turns.is_empty());
        assert_eq!(again.next, 4);
    }

    /// Replaces the `data` of message `seq` in a database from [`database`].
    fn rewrite(db: &Path, seq: u64, data: &serde_json::Value) {
        let data = data.to_string().replace('\'', "''");
        let status = std::process::Command::new("sqlite3")
            .arg(db)
            .arg(format!(
                "UPDATE session_message SET data = '{data}' WHERE seq = {seq};"
            ))
            .status()
            .expect("sqlite3");
        assert!(status.success());
    }

    fn said(read: &Read_) -> Vec<String> {
        read.turns.iter().map(|turn| turn.text.clone()).collect()
    }

    #[test]
    fn an_opencode_2_step_that_ended_on_an_error_says_so_unless_interrupted() {
        let db = database(
            "failed",
            &[
                (1, "user", serde_json::json!({"text": "hi"})),
                (
                    2,
                    "assistant",
                    serde_json::json!({"time": {"created": 2, "completed": 3}, "content": [],
                        "error": {"type": "provider.transport",
                                  "message": "ConnectionRefused: Unable to connect."}}),
                ),
                (3, "user", serde_json::json!({"text": "again"})),
                (
                    4,
                    "assistant",
                    serde_json::json!({"time": {"created": 4, "completed": 5}, "content": [],
                        "error": {"type": "aborted", "message": "Step interrupted"}}),
                ),
            ],
        );
        let read = read(&db, "ses_one", 0).unwrap();
        assert_eq!(
            said(&read),
            ["hi", "ConnectionRefused: Unable to connect.", "again"]
        );
        assert_eq!(read.turns[1].kind, TurnKind::Assistant);
        assert_eq!(read.next, 5);
    }

    #[test]
    fn an_opencode_2_step_waiting_to_retry_says_so_once_an_attempt() {
        let retrying = |attempt: u64| {
            serde_json::json!({"time": {"created": 2}, "content": [],
                "retry": {"attempt": attempt, "at": 9,
                          "error": {"type": "provider.transport",
                                    "message": "Timed out waiting for response headers"}}})
        };
        let db = database(
            "retry",
            &[
                (1, "user", serde_json::json!({"text": "hi"})),
                (2, "assistant", retrying(2)),
            ],
        );
        let first = read(&db, "ses_one", 0).unwrap();
        assert_eq!(
            said(&first),
            [
                "hi",
                "Timed out waiting for response headers — trying again (attempt 2)"
            ]
        );
        assert_eq!(first.next & 0xffff_ffff, 2, "still waiting at the step");
        assert!(first.next < VERSION_1_CURSORS);

        // The same attempt is not said twice, nor once the step goes ahead.
        let again = read(&db, "ses_one", first.next).unwrap();
        assert!(again.turns.is_empty());
        assert_eq!(again.next, first.next);
        rewrite(
            &db,
            2,
            &serde_json::json!({"time": {"created": 2}, "content": []}),
        );
        let going = read(&db, "ses_one", again.next).unwrap();
        assert!(going.turns.is_empty());
        assert_eq!(going.next, first.next);

        // A later attempt is.
        rewrite(&db, 2, &retrying(3));
        let third = read(&db, "ses_one", going.next).unwrap();
        assert_eq!(
            said(&third),
            ["Timed out waiting for response headers — trying again (attempt 3)"]
        );

        // The answer, and a plain cursor after it.
        rewrite(
            &db,
            2,
            &serde_json::json!({"time": {"created": 2, "completed": 3},
                "content": [{"type": "text", "text": "hello"}]}),
        );
        let answered = read(&db, "ses_one", third.next).unwrap();
        assert_eq!(said(&answered), ["hello"]);
        assert_eq!(answered.next, 3);
    }
}
