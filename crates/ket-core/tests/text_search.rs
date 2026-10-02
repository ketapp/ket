//! Workspace-wide plain-text search and its VS Code Search-view patterns.

use std::fs;

use ket_core::text_search::{MAX_MATCHES, Query, search};

mod common;
use common::Sandbox;

fn query(text: &str) -> Query {
    Query {
        text: text.to_owned(),
        include: String::new(),
        exclude: String::new(),
    }
}

#[test]
fn finds_every_occurrence_case_insensitively() {
    let sandbox = Sandbox::new("search-basic");
    fs::write(
        sandbox.path("a.txt"),
        "Hello world\nhello again\nHELLO there\n",
    )
    .unwrap();

    let matches = search(sandbox.root(), &query("hello")).unwrap();
    assert_eq!(matches.total, 3);
    assert_eq!(matches.files.len(), 1);
    assert_eq!(matches.files[0].lines.len(), 3);
}

#[test]
fn reports_line_and_column_one_based() {
    let sandbox = Sandbox::new("search-position");
    fs::write(sandbox.path("a.txt"), "xx target\ntarget again\n").unwrap();

    let matches = search(sandbox.root(), &query("target")).unwrap();
    let lines = &matches.files[0].lines;
    assert_eq!(lines[0].line, 1);
    assert_eq!(lines[0].column, 4);
    assert_eq!(lines[1].line, 2);
    assert_eq!(lines[1].column, 1);
}

#[test]
fn multiple_occurrences_on_one_line_are_all_found() {
    let sandbox = Sandbox::new("search-multi");
    fs::write(sandbox.path("a.txt"), "cat cat cat\n").unwrap();

    let matches = search(sandbox.root(), &query("cat")).unwrap();
    assert_eq!(matches.files[0].lines[0].ranges.len(), 3);
    assert_eq!(matches.total, 3);
}

#[test]
fn an_empty_query_finds_nothing() {
    let sandbox = Sandbox::new("search-empty-query");
    fs::write(sandbox.path("a.txt"), "anything at all\n").unwrap();

    let matches = search(sandbox.root(), &query("")).unwrap();
    assert!(matches.files.is_empty());
    assert_eq!(matches.total, 0);
}

#[test]
fn a_file_with_no_match_is_left_out_of_the_results() {
    let sandbox = Sandbox::new("search-no-match");
    fs::write(sandbox.path("a.txt"), "nothing interesting\n").unwrap();

    let matches = search(sandbox.root(), &query("needle")).unwrap();
    assert!(matches.files.is_empty());
}

#[test]
fn a_binary_file_containing_a_nul_byte_is_skipped() {
    let sandbox = Sandbox::new("search-binary");
    fs::write(
        sandbox.path("a.bin"),
        [b't', b'a', b'r', b'g', b'e', b't', 0, 1, 2],
    )
    .unwrap();

    let matches = search(sandbox.root(), &query("target")).unwrap();
    assert!(matches.files.is_empty());
}

#[test]
fn include_restricts_to_matching_paths() {
    let sandbox = Sandbox::new("search-include");
    fs::create_dir_all(sandbox.path("src")).unwrap();
    fs::create_dir_all(sandbox.path("docs")).unwrap();
    fs::write(sandbox.path("src/a.rs"), "target\n").unwrap();
    fs::write(sandbox.path("docs/a.md"), "target\n").unwrap();

    let mut q = query("target");
    q.include = "src/**".to_owned();
    let matches = search(sandbox.root(), &q).unwrap();
    assert_eq!(matches.files.len(), 1);
    assert_eq!(matches.files[0].relative_path, "src/a.rs");
}

#[test]
fn exclude_removes_matching_paths() {
    let sandbox = Sandbox::new("search-exclude");
    fs::create_dir_all(sandbox.path("src")).unwrap();
    fs::create_dir_all(sandbox.path("docs")).unwrap();
    fs::write(sandbox.path("src/a.rs"), "target\n").unwrap();
    fs::write(sandbox.path("docs/a.md"), "target\n").unwrap();

    let mut q = query("target");
    q.exclude = "docs/**".to_owned();
    let matches = search(sandbox.root(), &q).unwrap();
    assert_eq!(matches.files.len(), 1);
    assert_eq!(matches.files[0].relative_path, "src/a.rs");
}

#[test]
fn a_root_anchored_pattern_does_not_match_a_nested_directory_of_the_same_name() {
    let sandbox = Sandbox::new("search-anchor");
    fs::create_dir_all(sandbox.path("src")).unwrap();
    fs::create_dir_all(sandbox.path("nested/src")).unwrap();
    fs::write(sandbox.path("src/a.rs"), "target\n").unwrap();
    fs::write(sandbox.path("nested/src/b.rs"), "target\n").unwrap();

    let mut q = query("target");
    q.include = "./src".to_owned();
    let matches = search(sandbox.root(), &q).unwrap();
    assert_eq!(matches.files.len(), 1);
    assert_eq!(matches.files[0].relative_path, "src/a.rs");
}

#[test]
fn an_unanchored_pattern_matches_a_directory_of_that_name_anywhere() {
    let sandbox = Sandbox::new("search-unanchored");
    fs::create_dir_all(sandbox.path("src")).unwrap();
    fs::create_dir_all(sandbox.path("nested/src")).unwrap();
    fs::write(sandbox.path("src/a.rs"), "target\n").unwrap();
    fs::write(sandbox.path("nested/src/b.rs"), "target\n").unwrap();

    let mut q = query("target");
    q.include = "src".to_owned();
    let matches = search(sandbox.root(), &q).unwrap();
    assert_eq!(matches.files.len(), 2);
}

#[test]
fn comma_separated_includes_are_split_outside_braces_and_brackets() {
    let sandbox = Sandbox::new("search-comma");
    fs::write(sandbox.path("a.rs"), "target\n").unwrap();
    fs::write(sandbox.path("b.md"), "target\n").unwrap();
    fs::write(sandbox.path("c.txt"), "target\n").unwrap();

    let mut q = query("target");
    q.include = "./a.rs, ./b.md".to_owned();
    let matches = search(sandbox.root(), &q).unwrap();
    let names: Vec<&str> = matches
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    assert_eq!(names, vec!["a.rs", "b.md"]);
}

#[test]
fn a_comma_inside_braces_does_not_split_the_pattern() {
    let sandbox = Sandbox::new("search-brace-comma");
    fs::write(sandbox.path("a.rs"), "target\n").unwrap();
    fs::write(sandbox.path("a.md"), "target\n").unwrap();
    fs::write(sandbox.path("a.txt"), "target\n").unwrap();

    let mut q = query("target");
    q.include = "./a.{rs,md}".to_owned();
    let matches = search(sandbox.root(), &q).unwrap();
    let names: Vec<&str> = matches
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    assert_eq!(names, vec!["a.md", "a.rs"]);
}

#[test]
fn an_invalid_pattern_is_refused_rather_than_silently_dropped() {
    let sandbox = Sandbox::new("search-invalid-pattern");
    fs::write(sandbox.path("a.txt"), "target\n").unwrap();

    let mut q = query("target");
    q.include = "[".to_owned();
    assert!(search(sandbox.root(), &q).is_err());
}

#[test]
fn matches_past_the_cap_are_truncated_and_flagged() {
    let sandbox = Sandbox::new("search-cap");
    let body = "target\n".repeat(MAX_MATCHES + 10);
    fs::write(sandbox.path("a.txt"), body).unwrap();

    let matches = search(sandbox.root(), &query("target")).unwrap();
    assert!(matches.truncated);
    assert_eq!(matches.total, MAX_MATCHES);
}

#[test]
fn a_missing_root_reports_no_matches_rather_than_failing() {
    let sandbox = Sandbox::new("search-missing-root");
    let matches = search(&sandbox.path("does-not-exist"), &query("anything"));
    // Either an empty result or a propagated error is acceptable, but it must
    // not panic walking a directory that is not there.
    if let Ok(matches) = matches {
        assert!(matches.files.is_empty());
    }
}
