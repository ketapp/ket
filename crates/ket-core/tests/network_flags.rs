//! What the network inspector flags in a request, and what it lets pass.

use std::collections::BTreeSet;

use ket_core::network::flags::{Kind, Source, find, sources};
use serde_json::json;

fn kinds(text: &str) -> Vec<Kind> {
    find(text).into_iter().map(|(_, kind)| kind).collect()
}

#[test]
fn an_address_in_prose_is_an_email() {
    assert_eq!(
        kinds("mail jane.doe@example.com about it"),
        vec![Kind::Email]
    );
}

#[test]
fn a_urls_user_and_host_are_not_an_email() {
    // A Sentry DSN, as a tool's grep of a source file carries it.
    let dsn = "dsn: 'https://0123456789abcdef0123456789abcdef@o123456.ingest.example.com/7890'";
    assert_eq!(kinds(dsn), vec![]);
    // A password in a database URL, and a token in a registry's.
    assert_eq!(kinds("postgres://app:hunter2@db.example.com/app"), vec![]);
    assert_eq!(
        kinds("https://x-access-token:abc123@git.example.com/o/r.git"),
        vec![]
    );
}

#[test]
fn an_address_after_a_url_is_still_an_email() {
    assert_eq!(
        kinds("see https://example.com — ask jane@example.com"),
        vec![Kind::Email]
    );
}

#[test]
fn noreply_and_git_remotes_pass() {
    assert_eq!(kinds("From: noreply@example.com"), vec![]);
    assert_eq!(kinds("git@example.com:org/repo.git"), vec![]);
}

fn set(kinds: &[Kind]) -> BTreeSet<Kind> {
    kinds.iter().copied().collect()
}

#[test]
fn each_source_carries_the_kinds_it_sent() {
    // A file read by a tool, and the person's own words.
    let kept = json!({
        "messages": [
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read",
                 "input": {"file_path": "/repo/contacts.csv"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1",
                 "content": "jane@example.com, 203.0.113.7"},
                {"type": "text", "text": "call (415) 555-0100 about it"}
            ]}
        ]
    });
    assert_eq!(
        sources(&kept),
        [
            (
                Source::File("/repo/contacts.csv".to_owned()),
                set(&[Kind::Email, Kind::Ip])
            ),
            (Source::Message, set(&[Kind::Phone])),
        ]
    );
}

#[test]
fn a_responses_message_names_each_text_by_what_it_is() {
    let kept = json!({
        "input": [
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "<environment_context>ip 203.0.113.7</environment_context>"},
                {"type": "input_text", "text": "mail jane@example.com"}
            ]}
        ]
    });
    assert_eq!(
        sources(&kept),
        [
            (Source::Context, set(&[Kind::Ip])),
            (Source::Message, set(&[Kind::Email])),
        ]
    );
}

#[test]
fn a_source_met_twice_is_listed_once_with_every_kind() {
    let kept = json!({
        "messages": [
            {"role": "user", "content": "mail jane@example.com"},
            {"role": "user", "content": "or call (415) 555-0100"}
        ]
    });
    assert_eq!(
        sources(&kept),
        [(Source::Message, set(&[Kind::Phone, Kind::Email]))]
    );
}
