//! Reading a Claude Code transcript as a conversation: what a person said,
//! what the agent said back, and each tool it used as one line.

use std::path::Path;

use ket_core::conversation::{TurnKind, claude_transcript, queued, read};

mod common;
use common::Sandbox;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn jsonl(records: &[serde_json::Value]) -> String {
    records
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn user(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "user", "message": {"content": text}})
}

fn assistant(text: &str) -> serde_json::Value {
    serde_json::json!({"type": "assistant", "message": {"content": text}})
}

fn tool_use(name: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": name, "input": input}]}
    })
}

// ---- read: basic turns -------------------------------------------------------

#[test]
fn read_extracts_a_user_and_assistant_turn() {
    let sandbox = Sandbox::new("conv-basic");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[user("hello there"), assistant("hi, how can I help?")]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 2);
    assert_eq!(found.turns[0].kind, TurnKind::User);
    assert_eq!(found.turns[0].text, "hello there");
    assert_eq!(found.turns[1].kind, TurnKind::Assistant);
    assert_eq!(found.turns[1].text, "hi, how can I help?");
    assert!(found.fresh);
}

#[test]
fn read_extracts_a_tool_use_as_its_own_turn_with_a_summary_line() {
    let sandbox = Sandbox::new("conv-tool");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[tool_use(
            "Bash",
            serde_json::json!({"command": "cargo test"}),
        )]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].kind, TurnKind::Tool);
    assert_eq!(found.turns[0].tool, "Bash");
    assert_eq!(found.turns[0].text, "$ cargo test");
}

#[test]
fn tool_lines_are_formatted_per_tool() {
    let sandbox = Sandbox::new("conv-tool-lines");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            tool_use("Read", serde_json::json!({"file_path": "/a/b/main.rs"})),
            tool_use("Edit", serde_json::json!({"file_path": "/a/b/lib.rs"})),
            tool_use("Write", serde_json::json!({"file_path": "/a/b/new.rs"})),
            tool_use("Grep", serde_json::json!({"pattern": "TODO"})),
            tool_use("Glob", serde_json::json!({"pattern": "**/*.rs"})),
            tool_use(
                "WebFetch",
                serde_json::json!({"url": "https://example.com"}),
            ),
            tool_use("TodoWrite", serde_json::json!({})),
            tool_use("SomeOtherTool", serde_json::json!({})),
        ]),
    );

    let found = read(&path, 0).unwrap();
    let lines: Vec<&str> = found.turns.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(
        lines,
        vec![
            "Read main.rs",
            "Edited lib.rs",
            "Wrote new.rs",
            "Searched for TODO",
            "Listed **/*.rs",
            "Fetched https://example.com",
            "Updated its plan",
            "SomeOtherTool",
        ]
    );
}

#[test]
fn consecutive_assistant_turns_are_merged_into_one() {
    let sandbox = Sandbox::new("conv-merge");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[assistant("first part"), assistant("second part")]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "first part\n\nsecond part");
}

#[test]
fn an_assistant_turn_does_not_merge_across_a_tool_use() {
    let sandbox = Sandbox::new("conv-no-merge-across-tool");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            assistant("before"),
            tool_use("Bash", serde_json::json!({"command": "ls"})),
            assistant("after"),
        ]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 3);
    assert_eq!(found.turns[0].text, "before");
    assert_eq!(found.turns[1].kind, TurnKind::Tool);
    assert_eq!(found.turns[2].text, "after");
}

// ---- records that are skipped or specially handled --------------------------

#[test]
fn sidechain_meta_and_compact_summary_records_are_skipped() {
    let sandbox = Sandbox::new("conv-skip-flags");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            serde_json::json!({"type": "user", "isSidechain": true, "message": {"content": "hidden"}}),
            serde_json::json!({"type": "user", "isMeta": true, "message": {"content": "hidden"}}),
            serde_json::json!({"type": "assistant", "isCompactSummary": true, "message": {"content": "hidden"}}),
            user("visible"),
        ]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "visible");
}

#[test]
fn a_thinking_block_is_not_a_turn() {
    let sandbox = Sandbox::new("conv-thinking");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "thinking", "thinking": "hmm let me think"}]}
        })]),
    );

    let found = read(&path, 0).unwrap();
    assert!(found.turns.is_empty());
}

#[test]
fn an_unrecognised_record_type_is_ignored() {
    let sandbox = Sandbox::new("conv-unknown-type");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            serde_json::json!({"type": "summary", "summary": "whatever"}),
            user("real"),
        ]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "real");
}

#[test]
fn tool_answers_to_the_agents_own_question_read_as_a_user_turn() {
    let sandbox = Sandbox::new("conv-tool-answers");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[serde_json::json!({
            "type": "user",
            "message": {"content": "ignored because toolUseResult wins"},
            "toolUseResult": {"answers": {"q1": "yes please", "q2": "option b"}}
        })]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].kind, TurnKind::User);
    assert!(found.turns[0].text.contains("yes please"));
    assert!(found.turns[0].text.contains("option b"));
}

#[test]
fn a_local_commands_printed_output_becomes_a_command_tool_turn() {
    let sandbox = Sandbox::new("conv-local-command");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[serde_json::json!({
            "type": "system",
            "subtype": "local_command",
            "content": "<local-command-stdout>All tests passed</local-command-stdout>"
        })]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].kind, TurnKind::Tool);
    assert_eq!(found.turns[0].tool, "Command");
    assert_eq!(found.turns[0].text, "All tests passed");
}

#[test]
fn a_system_record_of_a_different_subtype_is_ignored() {
    let sandbox = Sandbox::new("conv-system-other");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[serde_json::json!({"type": "system", "subtype": "other", "content": "noise"})]),
    );

    let found = read(&path, 0).unwrap();
    assert!(found.turns.is_empty());
}

#[test]
fn a_slash_command_is_read_as_the_typed_form() {
    let sandbox = Sandbox::new("conv-slash");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[user(
            "<command-name>review</command-name><command-args>42</command-args>",
        )]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "/review 42");
}

#[test]
fn a_slash_command_already_spelled_with_a_slash_is_not_doubled() {
    let sandbox = Sandbox::new("conv-slash-doubled");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[user("<command-name>/compact</command-name>")]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns[0].text, "/compact");
}

#[test]
fn text_starting_with_a_tag_other_than_a_command_is_dropped() {
    let sandbox = Sandbox::new("conv-tagged-drop");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            user("<system-reminder>be careful</system-reminder>"),
            user("real prompt"),
        ]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "real prompt");
}

#[test]
fn an_interrupted_notice_is_dropped() {
    let sandbox = Sandbox::new("conv-interrupted");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            assistant("[Request interrupted by user]"),
            assistant("real reply"),
        ]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "real reply");
}

#[test]
fn an_empty_turn_is_dropped() {
    let sandbox = Sandbox::new("conv-empty");
    let path = sandbox.path("t.jsonl");
    write(&path, &jsonl(&[user("   "), user("real")]));

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "real");
}

#[test]
fn a_queued_command_attachment_is_read_as_a_user_turn() {
    let sandbox = Sandbox::new("conv-attachment");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[serde_json::json!({
            "type": "attachment",
            "attachment": {
                "type": "queued_command",
                "commandMode": "prompt",
                "prompt": "typed while the agent was busy",
            }
        })]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].kind, TurnKind::User);
    assert_eq!(found.turns[0].text, "typed while the agent was busy");
}

#[test]
fn an_attachment_that_is_not_a_queued_prompt_is_ignored() {
    let sandbox = Sandbox::new("conv-attachment-other");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[serde_json::json!({
            "type": "attachment",
            "attachment": {"type": "image", "commandMode": "prompt"}
        })]),
    );

    let found = read(&path, 0).unwrap();
    assert!(found.turns.is_empty());
}

// ---- AskUserQuestion / ExitPlanMode spoken through as assistant text --------

#[test]
fn ask_user_question_is_spoken_as_assistant_text_with_its_options() {
    let sandbox = Sandbox::new("conv-ask-user-question");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[tool_use(
            "AskUserQuestion",
            serde_json::json!({
                "questions": [{
                    "question": "Which approach?",
                    "multiSelect": false,
                    "options": [
                        {"label": "A", "description": "the first"},
                        {"label": "B"},
                    ]
                }]
            }),
        )]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].kind, TurnKind::Assistant);
    assert!(found.turns[0].text.contains("Which approach?"));
    assert!(found.turns[0].text.contains("• A — the first"));
    assert!(found.turns[0].text.contains("• B"));
    assert!(!found.turns[0].text.contains("pick any"));
}

#[test]
fn a_multiselect_question_says_pick_any() {
    let sandbox = Sandbox::new("conv-ask-user-question-multi");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[tool_use(
            "AskUserQuestion",
            serde_json::json!({
                "questions": [{"question": "Pick some", "multiSelect": true, "options": []}]
            }),
        )]),
    );

    let found = read(&path, 0).unwrap();
    assert!(found.turns[0].text.contains("(pick any)"));
}

#[test]
fn an_ask_user_question_with_no_questions_falls_back_to_the_tool_line() {
    let sandbox = Sandbox::new("conv-ask-user-question-empty");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[tool_use(
            "AskUserQuestion",
            serde_json::json!({"questions": []}),
        )]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns[0].kind, TurnKind::Tool);
}

#[test]
fn exit_plan_mode_is_spoken_as_assistant_text() {
    let sandbox = Sandbox::new("conv-exit-plan-mode");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[tool_use(
            "ExitPlanMode",
            serde_json::json!({"plan": "1. do this\n2. do that"}),
        )]),
    );

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].kind, TurnKind::Assistant);
    assert_eq!(found.turns[0].text, "1. do this\n2. do that");
}

// ---- clipping, incremental reads, first-read cap -----------------------------

#[test]
fn very_long_text_is_clipped_with_an_ellipsis() {
    let sandbox = Sandbox::new("conv-clip");
    let path = sandbox.path("t.jsonl");
    let long = "x".repeat(9_000);
    write(&path, &jsonl(&[user(&long)]));

    let found = read(&path, 0).unwrap();
    let text = &found.turns[0].text;
    assert!(text.ends_with('…'));
    assert_eq!(text.chars().count(), 8_001);
}

#[test]
fn a_first_read_keeps_only_the_newest_two_hundred_turns() {
    let sandbox = Sandbox::new("conv-cap");
    let path = sandbox.path("t.jsonl");
    let records: Vec<serde_json::Value> = (0..250).map(|i| user(&format!("msg {i}"))).collect();
    write(&path, &jsonl(&records));

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 200);
    assert_eq!(found.turns[0].text, "msg 50");
    assert_eq!(found.turns[199].text, "msg 249");
    assert!(found.fresh);
}

#[test]
fn a_second_read_from_the_returned_offset_only_sees_whats_new() {
    let sandbox = Sandbox::new("conv-incremental");
    let path = sandbox.path("t.jsonl");
    write(&path, &jsonl(&[user("first")]));

    let first = read(&path, 0).unwrap();
    assert_eq!(first.turns.len(), 1);

    let mut contents = std::fs::read_to_string(&path).unwrap();
    contents.push_str(&jsonl(&[user("second")]));
    write(&path, &contents);

    let second = read(&path, first.next).unwrap();
    assert!(!second.fresh);
    assert_eq!(second.turns.len(), 1);
    assert_eq!(second.turns[0].text, "second");
}

#[test]
fn an_offset_past_the_end_of_a_shrunk_file_is_treated_as_fresh() {
    let sandbox = Sandbox::new("conv-shrunk");
    let path = sandbox.path("t.jsonl");
    write(&path, &jsonl(&[user("short")]));

    let found = read(&path, 10_000).unwrap();
    assert!(found.fresh);
    assert_eq!(found.turns.len(), 1);
}

#[test]
fn a_trailing_partial_line_is_not_read_and_not_advanced_past() {
    let sandbox = Sandbox::new("conv-partial-line");
    let path = sandbox.path("t.jsonl");
    let whole = jsonl(&[user("complete")]);
    write(&path, &format!("{whole}{{\"type\": \"user\", \"mess"));

    let found = read(&path, 0).unwrap();
    assert_eq!(found.turns.len(), 1);
    assert_eq!(found.turns[0].text, "complete");
    assert_eq!(found.next as usize, whole.len());
}

#[test]
fn an_incremental_read_is_capped_rather_than_scanning_to_the_end() {
    // A phone's cursor can sit anywhere in a transcript that has since grown
    // very large; one incremental call must not read the whole remaining
    // span in one shot.
    let sandbox = Sandbox::new("conv-incremental-cap");
    let path = sandbox.path("t.jsonl");
    write(&path, &jsonl(&[user("first")]));
    let first = read(&path, 0).unwrap();

    // Comfortably past the 2 MiB incremental cap, in many whole lines.
    let mut rest = String::new();
    let mut total = 0usize;
    while total < 3 * 1024 * 1024 {
        let line = user(&"x".repeat(200)).to_string();
        total += line.len() + 1;
        rest.push_str(&line);
        rest.push('\n');
    }
    let mut contents = std::fs::read_to_string(&path).unwrap();
    contents.push_str(&rest);
    write(&path, &contents);

    let second = read(&path, first.next).unwrap();
    assert!(!second.fresh);
    assert!(
        (second.next as usize) < contents.len(),
        "a single incremental read reached end of file"
    );
    assert!(
        !second.turns.is_empty(),
        "nothing was read within the cap at all"
    );

    // The rest keeps coming on a follow-up read with the returned cursor.
    let mut seen = second.turns.len();
    let mut cursor = second.next;
    while (cursor as usize) < contents.len() {
        let more = read(&path, cursor).unwrap();
        assert!(!more.fresh);
        seen += more.turns.len();
        assert!(more.next > cursor, "the cursor must always move forward");
        cursor = more.next;
    }
    assert_eq!(seen, rest.lines().count());
}

#[test]
fn a_record_bigger_than_the_incremental_cap_still_advances_the_cursor() {
    // One JSONL line exceeding the whole incremental budget has no newline
    // anywhere inside the capped window, so the ordinary "skip the partial
    // line" rule would otherwise leave the cursor stuck here forever.
    let sandbox = Sandbox::new("conv-incremental-giant-record");
    let path = sandbox.path("t.jsonl");
    write(&path, &jsonl(&[user("first")]));
    let first = read(&path, 0).unwrap();

    let giant = user(&"x".repeat(3 * 1024 * 1024)).to_string();
    let mut contents = std::fs::read_to_string(&path).unwrap();
    contents.push_str(&giant);
    contents.push('\n');
    write(&path, &contents);

    let second = read(&path, first.next).unwrap();
    assert!(!second.fresh);
    assert!(second.turns.is_empty(), "no whole line fit in the cap");
    assert_eq!(
        second.next,
        first.next + 2 * 1024 * 1024,
        "the cursor must advance past the examined fragment, not stay put"
    );
}

#[test]
fn reading_a_missing_file_is_an_error() {
    let sandbox = Sandbox::new("conv-missing");
    let path = sandbox.path("does-not-exist.jsonl");
    assert!(read(&path, 0).is_err());
}

// ---- claude_transcript --------------------------------------------------------

#[test]
fn claude_transcript_refuses_an_empty_session_id() {
    assert_eq!(claude_transcript(""), None);
}

#[test]
fn claude_transcript_refuses_a_session_id_with_a_path_separator() {
    assert_eq!(claude_transcript("a/b"), None);
    assert_eq!(claude_transcript("a\\b"), None);
}

#[test]
fn claude_transcript_refuses_a_session_id_that_tries_to_escape() {
    assert_eq!(claude_transcript("../../etc/passwd"), None);
}

// ---- queued -------------------------------------------------------------------

fn queue_op(operation: &str, content: &str) -> serde_json::Value {
    serde_json::json!({"queue-operation": true, "operation": operation, "content": content})
}

#[test]
fn queued_returns_messages_not_yet_dequeued_oldest_first() {
    let sandbox = Sandbox::new("conv-queued-basic");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[queue_op("enqueue", "first"), queue_op("enqueue", "second")]),
    );

    let found = queued(&path).unwrap();
    assert_eq!(found, vec!["first".to_owned(), "second".to_owned()]);
}

#[test]
fn queued_drops_a_message_thats_been_dequeued() {
    let sandbox = Sandbox::new("conv-queued-dequeue");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            queue_op("enqueue", "first"),
            queue_op("enqueue", "second"),
            queue_op("dequeue", ""),
        ]),
    );

    let found = queued(&path).unwrap();
    assert_eq!(found, vec!["second".to_owned()]);
}

#[test]
fn queued_drops_a_message_thats_been_removed_by_content() {
    let sandbox = Sandbox::new("conv-queued-remove");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            queue_op("enqueue", "first"),
            queue_op("enqueue", "second"),
            queue_op("remove", "first"),
        ]),
    );

    let found = queued(&path).unwrap();
    assert_eq!(found, vec!["second".to_owned()]);
}

#[test]
fn queued_filters_out_tag_prefixed_notifications() {
    let sandbox = Sandbox::new("conv-queued-tagged");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[
            queue_op(
                "enqueue",
                "<task-notification>background thing</task-notification>",
            ),
            queue_op("enqueue", "a real typed message"),
        ]),
    );

    let found = queued(&path).unwrap();
    assert_eq!(found, vec!["a real typed message".to_owned()]);
}

#[test]
fn queued_ignores_lines_with_no_queue_operation_marker() {
    let sandbox = Sandbox::new("conv-queued-no-marker");
    let path = sandbox.path("t.jsonl");
    write(
        &path,
        &jsonl(&[user("just an ordinary message, not a queue record")]),
    );

    let found = queued(&path).unwrap();
    assert!(found.is_empty());
}

#[test]
fn queued_on_an_empty_queue_is_empty() {
    let sandbox = Sandbox::new("conv-queued-empty");
    let path = sandbox.path("t.jsonl");
    write(&path, "");

    let found = queued(&path).unwrap();
    assert!(found.is_empty());
}

#[test]
fn queued_on_a_missing_file_is_an_error() {
    let sandbox = Sandbox::new("conv-queued-missing");
    let path = sandbox.path("does-not-exist.jsonl");
    assert!(queued(&path).is_err());
}
