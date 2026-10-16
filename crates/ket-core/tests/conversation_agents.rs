//! Reading Codex, Grok and OpenCode sessions as a conversation.
//!
//! `conversation.rs` covers Claude's transcripts. These cover the other three
//! agents a phone can watch, each from a fixture in the shape that agent
//! writes: a Codex rollout and a Grok `chat_history.jsonl` as JSONL files, and
//! an OpenCode session as rows in a SQLite database built with the same
//! `sqlite3` binary ket reads it through.

use std::path::{Path, PathBuf};
use std::process::Command;

use ket_core::conversation::{Transcript, TurnKind, codex_transcript, grok_transcript, read_codex};
use serde_json::json;

mod common;
use common::Sandbox;

fn jsonl(path: &Path, records: &[serde_json::Value]) {
    let text: String = records.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(path, text).unwrap();
}

fn append(path: &Path, records: &[serde_json::Value]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for record in records {
        writeln!(file, "{record}").unwrap();
    }
}

/// `(kind, text, tool)` for each turn, for comparing in one assertion.
fn shape(turns: &[ket_core::conversation::Turn]) -> Vec<(TurnKind, String, String)> {
    turns
        .iter()
        .map(|t| (t.kind, t.text.clone(), t.tool.clone()))
        .collect()
}

fn user(text: &str) -> (TurnKind, String, String) {
    (TurnKind::User, text.to_owned(), String::new())
}

fn said(text: &str) -> (TurnKind, String, String) {
    (TurnKind::Assistant, text.to_owned(), String::new())
}

fn tool(tool: &str, text: &str) -> (TurnKind, String, String) {
    (TurnKind::Tool, text.to_owned(), tool.to_owned())
}

// ---- Codex ------------------------------------------------------------------

fn codex_item(item: serde_json::Value) -> serde_json::Value {
    json!({"type": "event_msg", "payload": {"type": "item_completed", "item": item}})
}

fn codex_words(kind: &str, text: &str) -> serde_json::Value {
    codex_item(json!({"type": kind, "content": [{"type": "text", "text": text}]}))
}

#[test]
fn a_codex_rollout_reads_as_messages_commands_and_edits() {
    let sandbox = Sandbox::new("codex-read");
    let path = sandbox.path("rollout.jsonl");
    jsonl(
        &path,
        &[
            // Bookkeeping Codex writes around the conversation.
            json!({"type": "session_meta", "payload": {"id": "abc"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "<environment_context/>"}]}}),
            codex_words("UserMessage", "Fix the flaky test"),
            codex_words("AgentMessage", "Looking at it."),
            codex_item(json!({"type": "CommandExecution",
                              "command": ["/bin/zsh", "-lc", "cargo test -p core"]})),
            codex_item(json!({"type": "CommandExecution", "command": "git status"})),
            codex_item(json!({"type": "FileChange",
                              "changes": {"src/poll.rs": {}, "tests/poll.rs": {}}})),
            codex_item(json!({"type": "Extension", "kind": "web.search",
                              "query": "tokio interval drift"})),
            // A search with nothing to say is not a turn.
            codex_item(json!({"type": "Extension", "kind": "web.search", "query": "  "})),
            codex_words("AgentMessage", "Fixed."),
        ],
    );

    let read = read_codex(&path, 0).unwrap();
    assert!(read.fresh);
    assert_eq!(read.next, std::fs::metadata(&path).unwrap().len());
    assert_eq!(
        shape(&read.turns),
        [
            user("Fix the flaky test"),
            said("Looking at it."),
            tool("Bash", "$ cargo test -p core"),
            tool("Bash", "$ git status"),
            tool("Edit", "Edited poll.rs"),
            tool("Edit", "Edited poll.rs"),
            tool("WebSearch", "Searched the web for tokio interval drift"),
            said("Fixed."),
        ]
    );
}

#[test]
fn a_codex_turn_that_ended_on_an_error_says_so() {
    let sandbox = Sandbox::new("codex-error");
    let path = sandbox.path("rollout.jsonl");
    jsonl(
        &path,
        &[
            codex_words("UserMessage", "Keep going"),
            json!({"type": "event_msg", "payload": {"type": "task_complete",
                   "error": {"message": "You've hit your usage limit."}}}),
            json!({"type": "event_msg", "payload": {"type": "error",
                   "message": "stream disconnected"}}),
            // A task that completed cleanly says nothing.
            json!({"type": "event_msg", "payload": {"type": "task_complete"}}),
        ],
    );
    // Two in a row are one assistant turn, as consecutive replies are.
    assert_eq!(
        shape(&read_codex(&path, 0).unwrap().turns),
        [
            user("Keep going"),
            said("You've hit your usage limit.\n\nstream disconnected"),
        ]
    );
}

#[test]
fn a_codex_read_from_the_cursor_sees_only_what_was_added() {
    let sandbox = Sandbox::new("codex-incremental");
    let path = sandbox.path("rollout.jsonl");
    jsonl(&path, &[codex_words("UserMessage", "first")]);
    let first = read_codex(&path, 0).unwrap();

    append(&path, &[codex_words("AgentMessage", "second")]);
    let second = read_codex(&path, first.next).unwrap();
    assert!(!second.fresh);
    assert_eq!(shape(&second.turns), [said("second")]);

    // Nothing new: nothing read, and the cursor stays.
    let third = read_codex(&path, second.next).unwrap();
    assert!(third.turns.is_empty());
    assert_eq!(third.next, second.next);
}

#[test]
fn a_codex_record_still_being_written_is_left_for_the_next_read() {
    let sandbox = Sandbox::new("codex-partial");
    let path = sandbox.path("rollout.jsonl");
    let whole = format!("{}\n", codex_words("UserMessage", "done"));
    let partial = r#"{"type": "event_msg", "payload": {"type": "item_comp"#;
    std::fs::write(&path, format!("{whole}{partial}")).unwrap();

    let read = read_codex(&path, 0).unwrap();
    assert_eq!(shape(&read.turns), [user("done")]);
    assert_eq!(
        read.next,
        whole.len() as u64,
        "stops before the partial line"
    );
}

#[test]
fn a_rollout_that_shrank_under_the_cursor_is_read_afresh() {
    let sandbox = Sandbox::new("codex-shrunk");
    let path = sandbox.path("rollout.jsonl");
    jsonl(&path, &[codex_words("UserMessage", "after a rewrite")]);
    let read = read_codex(&path, 1_000_000).unwrap();
    assert!(read.fresh);
    assert_eq!(shape(&read.turns), [user("after a rewrite")]);
}

#[test]
fn a_missing_rollout_is_an_error() {
    let sandbox = Sandbox::new("codex-missing");
    assert!(read_codex(&sandbox.path("nope.jsonl"), 0).is_err());
}

// ---- Grok -------------------------------------------------------------------

#[test]
fn a_grok_history_reads_typed_words_replies_and_tools_by_claudes_names() {
    let sandbox = Sandbox::new("grok-read");
    let path = sandbox.path("chat_history.jsonl");
    jsonl(
        &path,
        &[
            // Context Grok feeds the model as a user record, with no index.
            json!({"type": "user", "content": "<project_instructions>be brief</project_instructions>"}),
            json!({"type": "user", "prompt_index": 0,
                   "content": "<user_query>Add a retry</user_query>"}),
            json!({"type": "user", "prompt_index": 1,
                   "content": [{"type": "text", "text": "and log it"},
                               {"type": "image", "url": "x"}]}),
            json!({"type": "assistant", "content": "On it.",
            "tool_calls": [
                {"name": "run_terminal_command",
                 "arguments": "{\"command\": \"cargo build\"}"},
                {"name": "search_replace",
                 "arguments": "{\"target_file\": \"src/net/retry.rs\"}"},
                {"name": "list_dir",
                 "arguments": "{\"target_directory\": \"src\"}"},
                {"name": "some_new_tool", "arguments": "not json"},
                {"arguments": "{}"}
            ]}),
            json!({"type": "tool", "content": "build output"}),
        ],
    );

    let read = Transcript::Grok(path).read(0).unwrap();
    assert_eq!(
        shape(&read.turns),
        [
            user("Add a retry"),
            user("and log it"),
            said("On it."),
            tool("Bash", "$ cargo build"),
            tool("Edit", "Edited retry.rs"),
            tool("Glob", "Listed src"),
            tool("some_new_tool", "some_new_tool"),
        ]
    );
}

#[test]
fn only_claude_records_its_queue() {
    let sandbox = Sandbox::new("grok-queue");
    let path = sandbox.path("chat_history.jsonl");
    jsonl(&path, &[]);
    assert!(Transcript::Grok(path.clone()).queued().is_empty());
    assert!(Transcript::Codex(path).queued().is_empty());
}

// ---- OpenCode ---------------------------------------------------------------

/// An OpenCode database with the two tables ket reads, holding `messages`
/// as `(id, session, created_ms, message_json, parts_json)`.
fn opencode_db(
    sandbox: &Sandbox,
    messages: &[(&str, &str, u64, serde_json::Value, Vec<serde_json::Value>)],
) -> PathBuf {
    let db = sandbox.path("opencode.db");
    let quote = |v: &serde_json::Value| v.to_string().replace('\'', "''");
    let mut sql = String::from(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);\n\
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT);\n",
    );
    for (id, session, at, message, parts) in messages {
        sql.push_str(&format!(
            "INSERT INTO message VALUES ('{id}', '{session}', {at}, '{}');\n",
            quote(message)
        ));
        for (n, part) in parts.iter().enumerate() {
            sql.push_str(&format!(
                "INSERT INTO part VALUES ('{id}-{n:02}', '{id}', '{}');\n",
                quote(part)
            ));
        }
    }
    let out = Command::new("sqlite3")
        .arg(&db)
        .arg(sql)
        .output()
        .expect("sqlite3 is installed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    db
}

fn oc_user(id: &str) -> serde_json::Value {
    json!({"id": id, "role": "user"})
}

fn oc_assistant(id: &str, completed: bool) -> serde_json::Value {
    json!({"id": id, "role": "assistant",
           "time": {"completed": if completed { json!(1) } else { json!(null) }}})
}

fn oc_text(text: &str) -> serde_json::Value {
    json!({"type": "text", "text": text})
}

fn oc_tool(name: &str, input: serde_json::Value) -> serde_json::Value {
    json!({"type": "tool", "tool": name, "state": {"input": input}})
}

#[test]
fn an_opencode_session_reads_as_turns_with_tools_by_claudes_names() {
    let sandbox = Sandbox::new("opencode-read");
    let session = "ses_abc123";
    let db = opencode_db(
        &sandbox,
        &[
            (
                "m1",
                session,
                1000,
                oc_user("m1"),
                vec![
                    oc_text("Rename the module"),
                    json!({"type": "text", "text": "context ket adds", "synthetic": true}),
                ],
            ),
            (
                "m2",
                session,
                2000,
                oc_assistant("m2", true),
                vec![
                    json!({"type": "step-start"}),
                    oc_text("Renaming."),
                    oc_tool("bash", json!({"command": "git mv a.rs b.rs"})),
                    oc_tool("edit", json!({"filePath": "/repo/src/lib.rs"})),
                    oc_tool("read", json!({"filePath": "/repo/src/b.rs"})),
                    json!({"type": "subtask", "description": "check the callers"}),
                    json!({"type": "reasoning", "text": "thinking"}),
                ],
            ),
            // Another session's message, in the same database.
            (
                "x1",
                "ses_other",
                1500,
                oc_user("x1"),
                vec![oc_text("not mine")],
            ),
        ],
    );

    let read = Transcript::OpenCode {
        database: db,
        session: session.to_owned(),
    }
    .read(0)
    .unwrap();
    assert!(read.fresh);
    assert_eq!(
        shape(&read.turns),
        [
            user("Rename the module"),
            said("Renaming."),
            tool("Bash", "$ git mv a.rs b.rs"),
            tool("Edit", "Edited lib.rs"),
            tool("Read", "Read b.rs"),
            tool("Task", "Started a subagent: check the callers"),
        ]
    );
    assert_eq!(
        read.next,
        2000 * 1000 + 1,
        "cursor is the last message read"
    );
}

#[test]
fn an_opencode_step_still_streaming_is_not_read_until_it_finishes() {
    let sandbox = Sandbox::new("opencode-streaming");
    let session = "ses_live";
    let rows = |done: bool| {
        vec![
            ("m1", session, 1000, oc_user("m1"), vec![oc_text("Go")]),
            (
                "m2",
                session,
                2000,
                oc_assistant("m2", done),
                vec![oc_text("Half a thou")],
            ),
        ]
    };
    let db = opencode_db(&sandbox, &rows(false));
    let transcript = Transcript::OpenCode {
        database: db.clone(),
        session: session.to_owned(),
    };
    let first = transcript.read(0).unwrap();
    assert_eq!(shape(&first.turns), [user("Go")]);
    assert_eq!(first.next, 1000 * 1000 + 1);

    std::fs::remove_file(&db).unwrap();
    opencode_db(&sandbox, &rows(true));
    let second = transcript.read(first.next).unwrap();
    assert!(!second.fresh);
    assert_eq!(shape(&second.turns), [said("Half a thou")]);
}

#[test]
fn opencode_messages_in_the_same_millisecond_are_each_read_once() {
    let sandbox = Sandbox::new("opencode-same-ms");
    let session = "ses_tie";
    let db = opencode_db(
        &sandbox,
        &[
            ("a", session, 5000, oc_user("a"), vec![oc_text("one")]),
            ("b", session, 5000, oc_user("b"), vec![oc_text("two")]),
        ],
    );
    let transcript = Transcript::OpenCode {
        database: db.clone(),
        session: session.to_owned(),
    };
    let first = transcript.read(0).unwrap();
    assert_eq!(shape(&first.turns), [user("one"), user("two")]);
    assert_eq!(first.next, 5000 * 1000 + 2);

    // Read again from there: both were counted, so neither comes back.
    let again = transcript.read(first.next).unwrap();
    assert!(again.turns.is_empty(), "{:?}", again.turns);
}

#[test]
fn an_empty_opencode_session_reads_as_nothing() {
    let sandbox = Sandbox::new("opencode-empty");
    let db = opencode_db(&sandbox, &[]);
    let read = Transcript::OpenCode {
        database: db,
        session: "ses_none".to_owned(),
    }
    .read(0)
    .unwrap();
    assert!(read.turns.is_empty());
    assert_eq!(read.next, 0);
}

#[test]
fn a_database_that_is_not_opencodes_is_an_agent_error() {
    let sandbox = Sandbox::new("opencode-bad-db");
    let db = sandbox.path("not-a.db");
    std::fs::write(&db, "plain text, not sqlite").unwrap();
    let err = Transcript::OpenCode {
        database: db,
        session: "ses_x".to_owned(),
    }
    .read(0)
    .unwrap_err();
    assert!(err.to_string().contains("opencode"), "{err}");
}

// ---- finding them -------------------------------------------------------------

#[test]
fn a_session_id_that_could_climb_out_of_the_store_is_never_looked_up() {
    for bad in ["", "../etc", "a/b", r"a\b"] {
        assert!(codex_transcript(bad).is_none(), "{bad:?}");
        assert!(grok_transcript(bad).is_none(), "{bad:?}");
    }
    // An OpenCode id goes into a statement, so anything that is not
    // `ses_` and word characters is refused before it gets there.
    for bad in ["ses_", "ses_x'; DROP TABLE message;--", "abc"] {
        assert!(Transcript::locate("opencode", bad).is_none(), "{bad:?}");
    }
}

#[test]
fn only_the_four_known_agents_are_readable() {
    for agent in ["claude", "Codex", "GROK", "opencode"] {
        assert!(Transcript::readable(agent), "{agent}");
    }
    for agent in ["gemini", "", "ollama"] {
        assert!(!Transcript::readable(agent), "{agent}");
        assert!(Transcript::locate(agent, "anything").is_none());
    }
}
