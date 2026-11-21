//! Reading Codex, Grok and OpenCode sessions as a conversation.
//!
//! `conversation.rs` covers Claude's transcripts. These cover the other three
//! agents a phone can watch, one module each, from a fixture in the shape
//! that agent writes. What is left here is shared: the fixture helpers, and
//! what holds across agents.

use std::path::Path;

use ket_core::conversation::{Transcript, TurnKind, codex_transcript, grok_transcript};

#[path = "../common/mod.rs"]
mod common;
use common::Sandbox;

mod codex;
mod grok;
mod opencode;

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

// ---- across agents ----------------------------------------------------------

#[test]
fn only_claude_records_its_queue() {
    let sandbox = Sandbox::new("grok-queue");
    let path = sandbox.path("chat_history.jsonl");
    jsonl(&path, &[]);
    assert!(Transcript::Grok(path.clone()).queued().is_empty());
    assert!(Transcript::Codex(path).queued().is_empty());
}

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
