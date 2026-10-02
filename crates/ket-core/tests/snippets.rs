//! Prompt snippets: loading, saving and validating `snippets.toml`.

use ket_core::snippets::{Snippet, Snippets};

mod common;
use common::Sandbox;

fn snippet(name: &str, body: &str) -> Snippet {
    Snippet {
        name: name.to_owned(),
        body: body.to_owned(),
    }
}

#[test]
fn loading_a_missing_file_is_an_empty_set() {
    let sandbox = Sandbox::new("snippets-missing");
    let snippets = Snippets::load_from(&sandbox.path("snippets.toml")).unwrap();
    assert!(snippets.items.is_empty());
}

#[test]
fn a_malformed_file_is_an_error() {
    let sandbox = Sandbox::new("snippets-malformed");
    let path = sandbox.path("snippets.toml");
    std::fs::write(&path, "not valid toml {{{").unwrap();

    assert!(Snippets::load_from(&path).is_err());
}

#[test]
fn saving_and_loading_round_trips() {
    let sandbox = Sandbox::new("snippets-roundtrip");
    let path = sandbox.path("snippets.toml");

    let snippets = Snippets {
        items: vec![
            snippet("Review", "Review the diff for correctness."),
            snippet("Summarise", "Summarise what changed."),
        ],
    };
    snippets.save_to(&path).unwrap();

    let loaded = Snippets::load_from(&path).unwrap();
    assert_eq!(loaded, snippets);
}

#[test]
fn saving_refuses_a_duplicate_name_and_writes_nothing() {
    let sandbox = Sandbox::new("snippets-save-duplicate");
    let path = sandbox.path("snippets.toml");

    let snippets = Snippets {
        items: vec![snippet("Review", "one"), snippet("review", "two")],
    };
    assert!(snippets.save_to(&path).is_err());
    assert!(!path.exists());
}

#[test]
fn check_refuses_a_blank_name() {
    let err = Snippets::check(&[], None, "  ", "body").unwrap_err();
    assert!(err.contains("name"));
}

#[test]
fn check_refuses_a_blank_body() {
    let err = Snippets::check(&[], None, "Review", "   ").unwrap_err();
    assert!(err.contains("nothing"));
}

#[test]
fn check_refuses_a_name_already_taken_case_insensitively() {
    let items = vec![snippet("Review", "existing")];
    let err = Snippets::check(&items, None, "review", "new body").unwrap_err();
    assert!(err.contains("already"));
}

#[test]
fn check_allows_a_snippet_to_keep_its_own_name_when_editing_in_place() {
    let items = vec![snippet("Review", "existing")];
    // Editing slot 0 with the same name it already has must not collide with
    // itself.
    assert!(Snippets::check(&items, Some(0), "Review", "new body").is_ok());
}

#[test]
fn check_still_refuses_another_slots_name_while_editing() {
    let items = vec![snippet("Review", "one"), snippet("Summarise", "two")];
    // Editing slot 0 into a name slot 1 already has is still a collision.
    let err = Snippets::check(&items, Some(0), "Summarise", "new body").unwrap_err();
    assert!(err.contains("already"));
}

#[test]
fn check_allows_a_brand_new_unique_name() {
    let items = vec![snippet("Review", "one")];
    assert!(Snippets::check(&items, None, "Summarise", "two").is_ok());
}
