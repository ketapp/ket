//! An OpenCode session, read as a conversation from rows in a SQLite
//! database built with the same `sqlite3` binary ket reads it through.

use std::path::PathBuf;
use std::process::Command;

use ket_core::conversation::{Transcript, TurnKind};
use serde_json::json;

use super::{Sandbox, said, shape, tool, user};

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

// ---- OpenCode 2 -------------------------------------------------------------

/// The session every OpenCode 2 fixture below holds.
const SESSION_2: &str = "ses_two";

/// What OpenCode said when Ollama was still reading a long conversation
/// after OpenCode's wait for an answer ran out.
const TIMED_OUT: &str = "Timed out waiting for response headers";

/// An OpenCode 2 database: its one `session_message` table, holding
/// [`SESSION_2`]'s `rows` as `(seq, type, data)`.
fn opencode_2_db(sandbox: &Sandbox, rows: &[(u64, &str, serde_json::Value)]) -> Transcript {
    let database = sandbox.path("opencode.db");
    let mut sql = String::from(
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, \
         type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, \
         time_updated INTEGER NOT NULL, data TEXT NOT NULL);\n",
    );
    for &(seq, kind, ref data) in rows {
        sql.push_str(&insert(seq, kind, data));
    }
    sqlite(&database, &sql);
    Transcript::OpenCode {
        database,
        session: SESSION_2.to_owned(),
    }
}

fn insert(seq: u64, kind: &str, data: &serde_json::Value) -> String {
    let data = data.to_string().replace('\'', "''");
    format!(
        "INSERT INTO session_message VALUES ('m{seq}', '{SESSION_2}', '{kind}', {seq}, {seq}, {seq}, '{data}');\n"
    )
}

fn sqlite(database: &std::path::Path, sql: &str) {
    let out = Command::new("sqlite3")
        .arg(database)
        .arg(sql)
        .output()
        .expect("sqlite3 is installed");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// OpenCode writing to its database between two reads: `rows` added, and
/// each of `changed` given new `data`, as a step moves on in place.
fn write(
    transcript: &Transcript,
    rows: &[(u64, &str, serde_json::Value)],
    changed: &[(u64, serde_json::Value)],
) {
    let Transcript::OpenCode { database, .. } = transcript else {
        panic!("not an OpenCode transcript");
    };
    let mut sql = String::new();
    for &(seq, kind, ref data) in rows {
        sql.push_str(&insert(seq, kind, data));
    }
    for (seq, data) in changed {
        let data = data.to_string().replace('\'', "''");
        sql.push_str(&format!(
            "UPDATE session_message SET data = '{data}' WHERE seq = {seq};\n"
        ));
    }
    sqlite(database, &sql);
}

fn oc2_user(text: &str) -> serde_json::Value {
    json!({"time": {"created": 1}, "text": text})
}

/// A finished step that said `text`.
fn oc2_said(text: &str) -> serde_json::Value {
    json!({"time": {"created": 1, "completed": 2}, "content": [{"type": "text", "text": text}]})
}

/// A step still going, nothing said yet.
fn oc2_going() -> serde_json::Value {
    json!({"time": {"created": 1}, "content": []})
}

/// A step waiting to try its model again, at `attempt`.
fn oc2_waiting(attempt: u64) -> serde_json::Value {
    json!({"time": {"created": 1}, "content": [],
           "retry": {"attempt": attempt, "at": 9,
                     "error": {"type": "provider.transport", "message": TIMED_OUT}}})
}

/// A step that ended on `error`, having said `text` first if any.
fn oc2_failed(text: Option<&str>, error: serde_json::Value) -> serde_json::Value {
    let content: Vec<_> = text
        .map(|text| json!({"type": "text", "text": text}))
        .into_iter()
        .collect();
    json!({"time": {"created": 1, "completed": 2}, "content": content, "error": error})
}

fn retry_line(attempt: u64) -> (TurnKind, String, String) {
    said(&format!("{TIMED_OUT} — trying again (attempt {attempt})"))
}

/// Ollama not running: OpenCode tries again, then gives up. Each poll says
/// what is new once — the retry while it waits, the error when it ends.
#[test]
fn an_opencode_2_step_that_retries_then_fails_says_each_once_as_it_lands() {
    let sandbox = Sandbox::new("opencode2-retry-fail");
    let transcript = opencode_2_db(
        &sandbox,
        &[(1, "user", oc2_user("hi")), (2, "assistant", oc2_going())],
    );
    let first = transcript.read(0).unwrap();
    assert_eq!(shape(&first.turns), [user("hi")]);

    write(&transcript, &[], &[(2, oc2_waiting(2))]);
    let waiting = transcript.read(first.next).unwrap();
    assert!(!waiting.fresh);
    assert_eq!(shape(&waiting.turns), [retry_line(2)]);
    let again = transcript.read(waiting.next).unwrap();
    assert!(again.turns.is_empty(), "{:?}", again.turns);

    let refused = "ConnectionRefused: Unable to connect. Is the computer able to access the url?";
    write(
        &transcript,
        &[(3, "idle", json!({"time": {"created": 3}}))],
        &[(
            2,
            oc2_failed(
                Some("Let me look"),
                json!({"type": "provider.transport", "message": refused}),
            ),
        )],
    );
    let failed = transcript.read(again.next).unwrap();
    // After the step's own words, in the same bubble, as Grok's error is.
    assert_eq!(
        shape(&failed.turns),
        [said(&format!("Let me look\n\n{refused}"))]
    );
    assert_eq!(failed.next, 4, "a plain cursor past the step");
    let after = transcript.read(failed.next).unwrap();
    assert!(after.turns.is_empty(), "{:?}", after.turns);
}

/// The attempt a cursor carries is the step's it waited at: the next step's
/// retry is said even at the attempt the last one reached.
#[test]
fn a_new_opencode_2_step_retrying_is_said_at_the_attempt_the_last_one_reached() {
    let sandbox = Sandbox::new("opencode2-next-step");
    let transcript = opencode_2_db(
        &sandbox,
        &[
            (1, "user", oc2_user("hi")),
            (2, "assistant", oc2_waiting(2)),
        ],
    );
    let first = transcript.read(0).unwrap();
    assert_eq!(shape(&first.turns), [user("hi"), retry_line(2)]);

    write(
        &transcript,
        &[
            (3, "user", oc2_user("and now?")),
            (4, "assistant", oc2_waiting(2)),
        ],
        &[(2, oc2_said("hello"))],
    );
    let next = transcript.read(first.next).unwrap();
    assert_eq!(
        shape(&next.turns),
        [said("hello"), user("and now?"), retry_line(2)]
    );
    let again = transcript.read(next.next).unwrap();
    assert!(again.turns.is_empty(), "{:?}", again.turns);
}

/// A phone that read from a host before retries were said holds a plain
/// `seq`: it reads on from there, and hears a retry once.
#[test]
fn a_cursor_from_an_older_host_reads_on_and_hears_a_retry_once() {
    let sandbox = Sandbox::new("opencode2-old-cursor");
    let transcript = opencode_2_db(
        &sandbox,
        &[
            (1, "user", oc2_user("hi")),
            (2, "assistant", oc2_waiting(3)),
        ],
    );
    let read = transcript.read(2).unwrap();
    assert!(!read.fresh);
    assert_eq!(shape(&read.turns), [retry_line(3)]);
    let again = transcript.read(read.next).unwrap();
    assert!(again.turns.is_empty(), "{:?}", again.turns);
}

/// A cursor from OpenCode 1 — a creation time times 1000 — held across
/// OpenCode moving its database on starts the chat over, and is not read as
/// a `seq` with a retry attempt above it.
#[test]
fn an_opencode_1_cursor_starts_an_opencode_2_session_over() {
    let sandbox = Sandbox::new("opencode2-v1-cursor");
    let transcript = opencode_2_db(
        &sandbox,
        &[
            (1, "user", oc2_user("hi")),
            (2, "assistant", oc2_said("hello")),
        ],
    );
    let read = transcript.read(1_795_058_944_765 * 1000 + 1).unwrap();
    assert!(read.fresh);
    assert_eq!(shape(&read.turns), [user("hi"), said("hello")]);
    assert_eq!(read.next, 3);
}

/// An attempt past what the cursor holds is said in full, then not again.
#[test]
fn an_attempt_past_what_the_cursor_holds_is_said_once() {
    let sandbox = Sandbox::new("opencode2-many-attempts");
    let transcript = opencode_2_db(
        &sandbox,
        &[
            (1, "user", oc2_user("hi")),
            (2, "assistant", oc2_waiting(300)),
        ],
    );
    let first = transcript.read(0).unwrap();
    assert_eq!(shape(&first.turns), [user("hi"), retry_line(300)]);
    for _ in 0..2 {
        assert!(transcript.read(first.next).unwrap().turns.is_empty());
    }
    write(&transcript, &[], &[(2, oc2_waiting(301))]);
    assert!(transcript.read(first.next).unwrap().turns.is_empty());
}

/// An error with no words of its own still ends the chat with a line; an
/// interrupt, which the person did, says nothing.
#[test]
fn an_opencode_2_error_without_a_message_says_the_step_failed() {
    let sandbox = Sandbox::new("opencode2-bare-error");
    let transcript = opencode_2_db(
        &sandbox,
        &[
            (1, "user", oc2_user("hi")),
            (
                2,
                "assistant",
                oc2_failed(None, json!({"type": "provider.internal", "message": "  "})),
            ),
            (3, "user", oc2_user("stop")),
            (
                4,
                "assistant",
                oc2_failed(
                    Some("Start"),
                    json!({"type": "aborted", "message": "Step interrupted"}),
                ),
            ),
        ],
    );
    let read = transcript.read(0).unwrap();
    assert_eq!(
        shape(&read.turns),
        [
            user("hi"),
            said("OpenCode's step ended on an error."),
            user("stop"),
            said("Start"),
        ]
    );
}
