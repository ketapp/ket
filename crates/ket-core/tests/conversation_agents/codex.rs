//! A Codex rollout, read as a conversation.

use ket_core::conversation::read_codex;
use serde_json::json;

use super::{Sandbox, append, jsonl, said, shape, tool, user};

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
