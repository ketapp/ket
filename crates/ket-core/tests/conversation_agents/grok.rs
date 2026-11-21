//! A Grok `chat_history.jsonl`, read as a conversation, and the error a turn
//! ended on, which Grok keeps beside it in `updates.jsonl`.

use ket_core::conversation::Transcript;
use serde_json::json;

use super::{Sandbox, append, jsonl, said, shape, tool, user};

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

/// One record of a Grok session's `updates.jsonl`.
fn grok_update(update: serde_json::Value) -> serde_json::Value {
    json!({"method": "_x.ai/session/update", "params": {"sessionId": "s", "update": update}})
}

const LIMIT: &str = "API error (status 402 Payment Required): Grok Build usage balance exhausted";

/// How Grok 1.0.88 records a turn that hit its usage limit: the retry giving
/// up, the turn ending on the error, and ket's own `StopFailure` hook.
fn grok_limit() -> [serde_json::Value; 3] {
    [
        grok_update(json!({"sessionUpdate": "retry_state", "type": "failed", "message": LIMIT})),
        grok_update(
            json!({"sessionUpdate": "turn_completed", "stop_reason": "error",
                           "agent_result": LIMIT,
                           "error": {"code": -32603, "message": "Internal error",
                                     "data": {"message": LIMIT, "http_status": 402}}}),
        ),
        grok_update(json!({"sessionUpdate": "hook_execution", "event_name": "stop_failure"})),
    ]
}

#[test]
fn a_grok_turn_that_ended_on_an_error_says_so_once() {
    let sandbox = Sandbox::new("grok-error");
    let path = sandbox.path("chat_history.jsonl");
    let updates = sandbox.path("updates.jsonl");
    jsonl(
        &path,
        &[
            json!({"type": "user", "prompt_index": 0, "content": "<user_query>Keep going</user_query>"}),
        ],
    );
    jsonl(
        &updates,
        &[grok_update(json!({"sessionUpdate": "user_message_chunk"}))],
    );
    let before = Transcript::Grok(path.clone()).read(0).unwrap();
    assert_eq!(shape(&before.turns), [user("Keep going")]);

    // Grok writes the error beside the transcript, not in it.
    append(&updates, &grok_limit());
    let error = Transcript::Grok(path.clone()).read(before.next).unwrap();
    assert!(!error.fresh);
    assert_eq!(shape(&error.turns), [said(LIMIT)]);

    // Asked again from there, it is not said twice.
    let again = Transcript::Grok(path.clone()).read(error.next).unwrap();
    assert!(again.turns.is_empty());
    assert_eq!(again.next, error.next);

    // A first read ends on it too.
    let first = Transcript::Grok(path.clone()).read(0).unwrap();
    assert_eq!(shape(&first.turns), [user("Keep going"), said(LIMIT)]);

    // Once the person carries on, the next read is only what they said.
    append(
        &path,
        &[
            json!({"type": "user", "prompt_index": 1, "content": "<user_query>Try again</user_query>"}),
        ],
    );
    append(
        &updates,
        &[grok_update(json!({"sessionUpdate": "user_message_chunk"}))],
    );
    let after = Transcript::Grok(path.clone()).read(error.next).unwrap();
    assert!(!after.fresh);
    assert_eq!(shape(&after.turns), [user("Try again")]);
    let quiet = Transcript::Grok(path).read(after.next).unwrap();
    assert!(quiet.turns.is_empty());
}

#[test]
fn a_grok_turn_that_ended_well_or_carried_on_says_nothing_more() {
    let sandbox = Sandbox::new("grok-no-error");
    let path = sandbox.path("chat_history.jsonl");
    let updates = sandbox.path("updates.jsonl");
    jsonl(&path, &[json!({"type": "assistant", "content": "Done."})]);
    let read = || shape(&Transcript::Grok(path.clone()).read(0).unwrap().turns);

    // No `updates.jsonl` at all.
    assert_eq!(read(), [said("Done.")]);

    for ended in ["end_turn", "cancelled"] {
        jsonl(
            &updates,
            &[grok_update(
                json!({"sessionUpdate": "turn_completed", "stop_reason": ended}),
            )],
        );
        assert_eq!(read(), [said("Done.")], "{ended}");
    }

    // A retry under way after the error.
    let mut records = grok_limit().to_vec();
    records.push(grok_update(json!({"sessionUpdate": "agent_thought_chunk"})));
    jsonl(&updates, &records);
    assert_eq!(read(), [said("Done.")]);
}
