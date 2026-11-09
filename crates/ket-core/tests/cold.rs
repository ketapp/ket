//! Cold-session detection against real, throwaway transcript files.

use std::fs::{self, File};
use std::io::Write;
use std::time::{Duration, SystemTime};

use ket_core::cold::{claude_last_call, claude_last_call_cached};

mod common;
use common::Sandbox;

fn response(timestamp: &str, tokens: u64) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": timestamp,
        "message": {
            "model": "claude-haiku-4-5-20251001",
            "usage": { "input_tokens": tokens, "output_tokens": 10 }
        }
    })
    .to_string()
}

#[test]
fn a_missing_empty_or_non_transcript_file_has_no_last_call() {
    let sandbox = Sandbox::new("cold-absent");
    let path = sandbox.path("session.jsonl");
    assert_eq!(claude_last_call(&path), None);
    assert_eq!(claude_last_call_cached(&path), None);
    fs::write(&path, "").unwrap();
    assert_eq!(claude_last_call(&path), None);
    fs::write(&path, "not a transcript\n").unwrap();
    assert_eq!(claude_last_call(&path), None);
    assert_eq!(claude_last_call(sandbox.root()), None);
}

#[test]
fn the_last_response_is_read_even_when_the_tail_starts_inside_a_large_record() {
    let sandbox = Sandbox::new("cold-partial-head");
    let path = sandbox.path("session.jsonl");
    let mut file = File::create(&path).unwrap();
    // Seeking to the last 256 KiB lands inside this JSON string.
    writeln!(
        file,
        "{}",
        serde_json::json!({ "type": "user", "message": "x".repeat(300_000) })
    )
    .unwrap();
    // No final newline: an intact final record is still readable.
    write!(file, "{}", response("2026-10-29T12:00:00Z", 150_000)).unwrap();
    let call = claude_last_call(&path).unwrap();
    assert_eq!(call.context_tokens, 150_010);
    assert_eq!(call.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(claude_last_call_cached(&path), Some(call.clone()));
    // Dated model identifiers still resolve to a list price.
    assert_eq!(
        call.cold(call.at_ms + 300_001).unwrap().cost_micros,
        Some(187_513)
    );
}

#[test]
fn a_response_outside_the_tail_is_not_read() {
    let sandbox = Sandbox::new("cold-bounded-tail");
    let path = sandbox.path("session.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            response("2026-10-29T12:00:00Z", 150_000),
            serde_json::json!({ "type": "user", "message": "x".repeat(300_000) })
        ),
    )
    .unwrap();
    assert_eq!(claude_last_call(&path), None);
    assert_eq!(claude_last_call_cached(&path), None);
}

#[test]
fn an_appended_response_replaces_a_cached_miss_and_then_a_cached_call() {
    let sandbox = Sandbox::new("cold-cache-append");
    let path = sandbox.path("session.jsonl");
    fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
    assert_eq!(claude_last_call_cached(&path), None);
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", response("2026-10-29T12:00:00Z", 100_000)).unwrap();
    let first = claude_last_call_cached(&path).unwrap();
    assert_eq!(first.context_tokens, 100_010);
    writeln!(file, "{}", response("2026-10-29T13:00:00Z", 200_000)).unwrap();
    let next = claude_last_call_cached(&path).unwrap();
    assert!(next.at_ms > first.at_ms);
    assert_eq!(next.context_tokens, 200_010);
    assert_eq!(claude_last_call_cached(&path), Some(next));
}

#[test]
fn changing_a_transcript_without_changing_its_length_invalidates_the_cache() {
    let sandbox = Sandbox::new("cold-cache-modified");
    let path = sandbox.path("session.jsonl");
    let old = response("2026-10-29T12:00:00Z", 100_000);
    let new = response("2026-10-29T13:00:00Z", 200_000);
    assert_eq!(old.len(), new.len());
    fs::write(&path, old).unwrap();
    let file = File::options().write(true).open(&path).unwrap();
    let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    file.set_modified(stamp).unwrap();
    assert_eq!(
        claude_last_call_cached(&path).unwrap().context_tokens,
        100_010
    );
    fs::write(&path, new).unwrap();
    file.set_modified(stamp + Duration::from_secs(1)).unwrap();
    assert_eq!(
        claude_last_call_cached(&path).unwrap().context_tokens,
        200_010
    );
}

#[test]
fn a_deleted_or_truncated_transcript_does_not_keep_a_cached_call() {
    let sandbox = Sandbox::new("cold-cache-removed");
    let path = sandbox.path("session.jsonl");
    let record = response("2026-10-29T12:00:00Z", 100_000);
    fs::write(&path, &record).unwrap();
    assert!(claude_last_call_cached(&path).is_some());
    fs::remove_file(&path).unwrap();
    assert_eq!(claude_last_call_cached(&path), None);
    fs::write(&path, &record).unwrap();
    assert!(claude_last_call_cached(&path).is_some());
    fs::write(&path, "").unwrap();
    assert_eq!(claude_last_call_cached(&path), None);
}
