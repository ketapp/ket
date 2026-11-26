//! What a `StopFailure` says went wrong.
//!
//! A turn that ends badly used to reach ket as the bare fact of it: the
//! payload's account of why was never read, so a plan running out, a login
//! expiring and a dropped connection were one event. [`failure`] reads the
//! account, for [`crate::limits::classify`] to tell the first from the rest.

use crate::limits::Failure;

/// The most of an agent's error message kept. It is a sentence or two; a
/// payload that puts a stack trace there is cut, not carried.
const MESSAGE_MAX: usize = 600;

/// The failure a `StopFailure` payload describes, or `None` when it says
/// nothing about it.
///
/// Read loosely, because three agents send this event and none documents the
/// shape as a contract:
///
/// - Claude Code names the error in `error` — `rate_limit`, `server_error`,
///   `authentication_failed` — and gives the sentence it showed the person
///   in `last_assistant_message`, with the API's own text in `error_details`.
/// - ket's OpenCode plugin passes the error OpenCode's bus gave it, an
///   object with a `message` and usually a `type` or `name`.
/// - Grok sends the event with no account at all; its is read from the
///   session's `updates.jsonl` instead.
pub(super) fn failure(payload: &serde_json::Value) -> Option<Failure> {
    fn text(value: Option<&serde_json::Value>) -> Option<&str> {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|said| !said.is_empty())
    }
    let error = payload.get("error");
    let named = text(error)
        .or_else(|| text(payload.get("error_type")))
        .or_else(|| {
            ["type", "name", "code"]
                .iter()
                .find_map(|key| text(error.and_then(|error| error.get(key))))
        });
    // A name is one word. An `error` that is a sentence is the message.
    let (code, sentence) = match named {
        Some(named) if named.contains(char::is_whitespace) => (None, Some(named)),
        named => (named, None),
    };
    let message = text(payload.get("last_assistant_message"))
        .or_else(|| text(error.and_then(|error| error.get("message"))))
        .or_else(|| text(error.and_then(|error| error.pointer("/data/message"))))
        .or_else(|| text(payload.get("error_details")))
        .or_else(|| text(payload.get("error_message")))
        .or_else(|| text(payload.get("message")))
        .or(sentence);
    if code.is_none() && message.is_none() {
        return None;
    }
    Some(Failure {
        code: code.map(str::to_owned),
        message: message.map(clipped).unwrap_or_default(),
    })
}

/// `said`, cut to [`MESSAGE_MAX`] characters.
fn clipped(said: &str) -> String {
    match said.char_indices().nth(MESSAGE_MAX) {
        Some((end, _)) => format!("{}…", &said[..end]),
        None => said.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claudes_name_for_the_error_and_the_sentence_it_showed_are_both_kept() {
        let payload = json!({
            "hook_event_name": "StopFailure",
            "session_id": "s1",
            "error": "rate_limit",
            "error_details": "429 {\"type\":\"error\"}",
            "last_assistant_message": "You've hit your session limit · resets 10:40pm",
        });
        assert_eq!(
            failure(&payload),
            Some(Failure::coded(
                "rate_limit",
                "You've hit your session limit · resets 10:40pm"
            ))
        );
    }

    #[test]
    fn the_apis_own_text_stands_in_when_no_sentence_was_shown() {
        let payload = json!({ "error": "server_error", "error_details": "529 overloaded" });
        assert_eq!(
            failure(&payload),
            Some(Failure::coded("server_error", "529 overloaded"))
        );
    }

    #[test]
    fn an_error_object_gives_its_type_and_message() {
        let payload = json!({
            "session_id": "s1",
            "error": { "type": "provider.transport", "message": " ConnectionRefused " },
        });
        assert_eq!(
            failure(&payload),
            Some(Failure::coded("provider.transport", "ConnectionRefused"))
        );
        let nested = json!({ "error": { "data": { "message": "usage balance exhausted" } } });
        assert_eq!(
            failure(&nested),
            Some(Failure::said("usage balance exhausted"))
        );
    }

    #[test]
    fn an_error_that_is_a_sentence_is_the_message_not_a_name() {
        let payload = json!({ "error": "You've hit your usage limit." });
        assert_eq!(
            failure(&payload),
            Some(Failure::said("You've hit your usage limit."))
        );
    }

    #[test]
    fn a_payload_that_says_nothing_about_it_is_no_failure() {
        assert_eq!(failure(&json!({ "session_id": "s1" })), None);
        assert_eq!(failure(&json!({ "error": "", "message": "  " })), None);
        assert_eq!(failure(&json!({ "error": null })), None);
    }

    /// The whole way in: a hook's envelope decoded into a report.
    fn decoded(agent: &str, payload: serde_json::Value) -> super::super::HookReport {
        let envelope = json!({
            "agent": agent, "worktree": "wt-1", "pane": "pane-1", "token": "t",
            "payload": payload,
        });
        let mut tools = super::super::decode::GrokTools::new();
        match super::super::decode::decode_envelope(&envelope, false, &mut tools) {
            Some(super::super::listener::Post::Report(report)) => *report,
            _ => panic!("a report"),
        }
    }

    #[test]
    fn a_stop_failure_carries_its_account_and_no_other_event_does() {
        let failed = decoded(
            "claude",
            json!({
                "hook_event_name": "StopFailure",
                "session_id": "s1",
                "error": "rate_limit",
                "last_assistant_message": "You've hit your weekly limit",
            }),
        );
        assert_eq!(
            failed.failure.as_deref(),
            Some(&Failure::coded(
                "rate_limit",
                "You've hit your weekly limit"
            ))
        );

        // A clean stop's last words are its note, never a failure.
        let stopped = decoded(
            "claude",
            json!({
                "hook_event_name": "Stop",
                "session_id": "s1",
                "error": "rate_limit",
                "last_assistant_message": "Done.",
            }),
        );
        assert_eq!(stopped.failure, None);

        // OpenCode's plugin, with the error its bus gave.
        let opencode = decoded(
            "opencode",
            json!({
                "hook_event_name": "StopFailure",
                "session_id": "s2",
                "error": { "name": "APIError", "message": "429 Too Many Requests" },
            }),
        );
        assert_eq!(
            opencode.failure.as_deref(),
            Some(&Failure::coded("APIError", "429 Too Many Requests"))
        );

        // And one that says nothing: still a failure of the turn, with no account.
        let bare = decoded(
            "opencode",
            json!({ "hook_event_name": "StopFailure", "session_id": "s2" }),
        );
        assert_eq!(bare.failure, None);
    }

    #[test]
    fn a_long_message_is_cut() {
        let payload = json!({ "error": "unknown", "message": "é".repeat(MESSAGE_MAX + 50) });
        let read = failure(&payload).unwrap();
        assert_eq!(read.message.chars().count(), MESSAGE_MAX + 1);
        assert!(read.message.ends_with('…'));
    }
}
