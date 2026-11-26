//! How a session's last turn ended, read from the end of what each agent
//! keeps: the records below are in the shapes Claude Code 2.1, Codex 0.170
//! and Grok 1.0.88 write when a plan runs out.

use ket_core::conversation::Transcript;
use ket_core::limits::Failure;
use serde_json::json;

use super::{Sandbox, append, jsonl};

const CODEX_LIMIT: &str = "You’ve hit your usage limit. Upgrade to Pro \
                           (https://chatgpt.com/explore/pro) or try again at 4:37 PM.";

fn codex_event(at: &str, payload: serde_json::Value) -> serde_json::Value {
    json!({"timestamp": at, "type": "event_msg", "payload": payload})
}

fn codex_limit(at: &str) -> serde_json::Value {
    codex_event(
        at,
        json!({"type": "task_complete", "turn_id": "t1", "last_agent_message": null,
               "error": {"message": CODEX_LIMIT, "codex_error_info": "usage_limit_exceeded"}}),
    )
}

#[test]
fn a_codex_turn_that_ran_out_of_plan_ends_on_its_own_account_of_it() {
    let sandbox = Sandbox::new("ended-codex");
    let path = sandbox.path("rollout.jsonl");
    jsonl(
        &path,
        &[
            codex_event("2026-11-17T02:00:00.000Z", json!({"type": "task_started"})),
            codex_event(
                "2026-11-17T02:01:00.000Z",
                json!({"type": "item_completed", "item": {"type": "AgentMessage"}}),
            ),
            codex_event("2026-11-17T02:02:02.826Z", json!({"type": "token_count"})),
            codex_limit("2026-11-17T02:02:02.829Z"),
        ],
    );
    let rollout = Transcript::Codex(path.clone());
    let ended = rollout.ended_on().unwrap();
    assert_eq!(
        ended.failure,
        Failure::coded("usage_limit_exceeded", CODEX_LIMIT)
    );
    assert_eq!(ended.at_ms, Some(1_794_880_922_829));
    assert!(ended.closed, "the turn's own end");

    // The stream dropping afterwards is the lesser news: the turn's own wins.
    append(
        &path,
        &[codex_event(
            "2026-11-17T02:02:03.000Z",
            json!({"type": "error", "message": "stream disconnected"}),
        )],
    );
    assert_eq!(
        rollout.ended_on().unwrap().failure.code.as_deref(),
        Some("usage_limit_exceeded")
    );

    // The next turn beginning: this one's end is no longer the session's.
    append(
        &path,
        &[codex_event(
            "2026-11-17T14:40:13.407Z",
            json!({"type": "task_started"}),
        )],
    );
    assert_eq!(rollout.ended_on(), None);
}

#[test]
fn a_codex_turn_that_finished_or_is_still_going_ended_on_nothing() {
    let sandbox = Sandbox::new("ended-codex-clean");
    let path = sandbox.path("rollout.jsonl");
    jsonl(
        &path,
        &[
            codex_limit("2026-11-17T02:02:02.829Z"),
            codex_event("2026-11-17T03:00:00.000Z", json!({"type": "task_started"})),
            codex_event(
                "2026-11-17T03:00:05.000Z",
                json!({"type": "item_completed", "item": {"type": "AgentMessage"}}),
            ),
        ],
    );
    let rollout = Transcript::Codex(path.clone());
    assert_eq!(rollout.ended_on(), None, "at work");
    append(
        &path,
        &[codex_event(
            "2026-11-17T03:00:09.000Z",
            json!({"type": "task_complete", "turn_id": "t2", "last_agent_message": "Done."}),
        )],
    );
    assert_eq!(rollout.ended_on(), None, "finished");

    // A dropped stream with no turn end to say more is what it ended on.
    append(
        &path,
        &[
            codex_event("2026-11-17T03:10:00.000Z", json!({"type": "task_started"})),
            codex_event(
                "2026-11-17T03:10:04.000Z",
                json!({"type": "error", "message": " stream disconnected "}),
            ),
        ],
    );
    let dropped = rollout.ended_on().unwrap();
    assert_eq!(dropped.failure, Failure::said("stream disconnected"));
    assert!(!dropped.closed, "Codex may yet retry it");
    assert_eq!(
        Transcript::Codex(sandbox.path("absent.jsonl")).ended_on(),
        None
    );
}

#[test]
fn a_codex_rollout_says_how_its_plan_stood_as_of_its_last_request() {
    let sandbox = Sandbox::new("ended-codex-quota");
    let path = sandbox.path("rollout.jsonl");
    let counted = |primary: f64, secondary: f64| {
        codex_event(
            "2026-11-17T02:02:02.826Z",
            json!({"type": "token_count", "info": null, "rate_limits": {
                "limit_id": "codex",
                "primary": {"used_percent": primary, "window_minutes": 300,
                            "resets_at": 1_795_111_120_u64},
                "secondary": {"used_percent": secondary, "window_minutes": 10080,
                              "resets_at": 1_795_630_015_u64},
                "plan_type": "plus"}}),
        )
    };
    jsonl(&path, &[counted(9.0, 16.0), counted(100.0, 16.5)]);
    let rollout = Transcript::Codex(path.clone());
    let windows = rollout.quota();
    assert_eq!(
        windows
            .iter()
            .map(|w| (w.name.as_str(), w.used_percent, w.resets_at_ms))
            .collect::<Vec<_>>(),
        [
            ("Session", 100.0, Some(1_795_111_120_000)),
            ("Weekly", 16.5, Some(1_795_630_015_000)),
        ]
    );
    assert_eq!(
        rollout.written(),
        Some(std::fs::metadata(&path).unwrap().len())
    );

    // A rollout with none, and the agents that keep theirs elsewhere.
    jsonl(&path, &[codex_limit("2026-11-17T02:02:02.829Z")]);
    assert!(rollout.quota().is_empty());
    assert!(Transcript::Claude(path).quota().is_empty());
}

fn claude_said(text: &str) -> serde_json::Value {
    json!({"type": "assistant", "timestamp": "2026-11-17T02:00:00.000Z",
           "message": {"content": [{"type": "text", "text": text}]}})
}

fn claude_refused(error: &str, text: &str) -> serde_json::Value {
    json!({"type": "assistant", "timestamp": "2026-11-17T02:02:02.829Z",
           "isApiErrorMessage": true, "error": error, "apiErrorStatus": 429,
           "message": {"content": [{"type": "text", "text": text}]}})
}

#[test]
fn a_claude_turn_the_api_refused_ends_on_the_record_written_in_its_place() {
    const SESSION: &str = "You've hit your session limit · resets 10:40pm (America/New_York)";
    let sandbox = Sandbox::new("ended-claude");
    let path = sandbox.path("session.jsonl");
    jsonl(
        &path,
        &[
            json!({"type": "user", "message": {"content": "Fix the build"}}),
            claude_said("Looking."),
            claude_refused("rate_limit", SESSION),
            // What Claude Code writes after any turn, and a subagent's own
            // words, which are not this conversation's.
            json!({"type": "system", "subtype": "turn_duration", "durationMs": 1200}),
            json!({"type": "assistant", "isSidechain": true,
                   "message": {"content": [{"type": "text", "text": "aside"}]}}),
            json!({"type": "user", "isMeta": true, "message": {"content": "<caveat/>"}}),
        ],
    );
    let transcript = Transcript::Claude(path.clone());
    let ended = transcript.ended_on().unwrap();
    assert_eq!(ended.failure, Failure::coded("rate_limit", SESSION));
    assert_eq!(ended.at_ms, Some(1_794_880_922_829));

    append(
        &path,
        &[json!({"type": "user", "message": {"content": "try again"}})],
    );
    assert_eq!(transcript.ended_on(), None, "a new prompt: moved on");
    append(&path, &[claude_said("Done.")]);
    assert_eq!(transcript.ended_on(), None, "its own last word");
}

#[test]
fn a_grok_turn_that_ended_on_an_error_gives_groks_words_undated() {
    let sandbox = Sandbox::new("ended-grok");
    let path = sandbox.path("chat_history.jsonl");
    jsonl(&path, &[]);
    let update = |update: serde_json::Value| {
        json!({"method": "_x.ai/session/update",
               "params": {"sessionId": "s", "update": update}})
    };
    jsonl(
        &sandbox.path("updates.jsonl"),
        &[update(
            json!({"sessionUpdate": "turn_completed", "stop_reason": "error",
                        "agent_result": "Grok Build usage balance exhausted"}),
        )],
    );
    let ended = Transcript::Grok(path).ended_on().unwrap();
    assert_eq!(
        ended.failure,
        Failure::said("Grok Build usage balance exhausted")
    );
    assert_eq!(ended.at_ms, None);
}
