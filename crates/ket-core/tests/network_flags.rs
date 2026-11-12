//! What the network inspector flags in a request, and what it lets pass.

use ket_core::network::flags::{Kind, find};

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
