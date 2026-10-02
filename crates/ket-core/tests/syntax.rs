//! The line-at-a-time syntax lexer: language selection and token spans.

use ket_core::syntax::{LANGUAGES, State, Token, highlight_line, language_for, opening_states};

fn rust() -> &'static ket_core::syntax::Language {
    LANGUAGES.iter().find(|l| l.name == "Rust").unwrap()
}

fn python() -> &'static ket_core::syntax::Language {
    LANGUAGES.iter().find(|l| l.name == "Python").unwrap()
}

fn toml() -> &'static ket_core::syntax::Language {
    LANGUAGES.iter().find(|l| l.name == "TOML").unwrap()
}

fn typescript() -> &'static ket_core::syntax::Language {
    LANGUAGES.iter().find(|l| l.name == "TypeScript").unwrap()
}

// ---- language_for -------------------------------------------------------

#[test]
fn an_extension_selects_its_language() {
    assert_eq!(language_for("main.rs").unwrap().name, "Rust");
    assert_eq!(language_for("src/lib.rs").unwrap().name, "Rust");
    assert_eq!(language_for("app.tsx").unwrap().name, "TypeScript");
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(language_for("Main.RS").unwrap().name, "Rust");
    assert_eq!(language_for("MAIN.PY").unwrap().name, "Python");
}

#[test]
fn an_exact_filename_beats_its_extension() {
    // `Cargo.lock` would otherwise fall to no extension rule for `.lock`.
    assert_eq!(language_for("Cargo.lock").unwrap().name, "TOML");
    assert_eq!(language_for(".zshrc").unwrap().name, "Shell");
}

#[test]
fn an_unknown_extension_is_none() {
    assert!(language_for("photo.png").is_none());
    assert!(language_for("noextension").is_none());
}

#[test]
fn a_path_with_directories_uses_only_the_final_component() {
    assert_eq!(language_for("a/b/c/main.py").unwrap().name, "Python");
    assert_eq!(language_for("a\\b\\main.go").unwrap().name, "Go");
}

// ---- highlight_line: keywords, kinds, numbers, punctuation ---------------

#[test]
fn a_keyword_is_recognised_as_a_whole_word() {
    let (spans, state) = highlight_line("let mutable = 1;", State::Code, rust());
    assert_eq!(state, State::Code);
    assert_eq!(spans[0], (0..3, Token::Keyword));
    // `mutable` must not partially match the keyword `mut`.
    assert!(
        !spans
            .iter()
            .any(|(range, token)| *token == Token::Keyword && *range == (4..11))
    );
}

#[test]
fn a_capitalised_word_is_a_kind() {
    let (spans, _) = highlight_line("let x: Option<String> = None;", State::Code, rust());
    assert!(spans.contains(&(7..13, Token::Kind)));
    assert!(spans.contains(&(14..20, Token::Kind)));
}

#[test]
fn a_shared_keyword_is_recognised_alongside_the_languages_own() {
    // `class` is in C_LIKE, shared by TypeScript, not in its own keyword list.
    let (spans, _) = highlight_line("class Foo {}", State::Code, typescript());
    assert_eq!(spans[0], (0..5, Token::Keyword));
}

#[test]
fn a_number_literal_is_taken_as_one_run() {
    let (spans, _) = highlight_line("x = 1_000.5e-2;", State::Code, rust());
    let number = spans.iter().find(|(_, t)| *t == Token::Number).unwrap();
    // The scan stops at the unsigned `-` in the exponent: only alphanumerics,
    // `_` and `.` extend a numeric run.
    assert_eq!(number.0, 4..12);
}

#[test]
fn punctuation_runs_are_grouped() {
    let (spans, _) = highlight_line("a == b", State::Code, rust());
    assert!(spans.contains(&(2..4, Token::Punctuation)));
}

#[test]
fn plain_code_with_no_tokens_yields_no_spans() {
    let (spans, state) = highlight_line("    ", State::Code, rust());
    assert!(spans.is_empty());
    assert_eq!(state, State::Code);
}

// ---- comments --------------------------------------------------------------

#[test]
fn a_line_comment_runs_to_the_end_of_the_line() {
    let (spans, state) = highlight_line("let x = 1; // set x", State::Code, rust());
    let comment = spans.iter().find(|(_, t)| *t == Token::Comment).unwrap();
    assert_eq!(comment.0.start, 11);
    assert_eq!(comment.0.end, 19);
    assert_eq!(state, State::Code);
}

#[test]
fn a_line_comment_beats_anything_after_it() {
    // A string quote inside a `//` comment must not open a string.
    let (spans, _) = highlight_line("// not a \"string\"", State::Code, rust());
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].1, Token::Comment);
}

#[test]
fn a_block_comment_that_closes_on_the_same_line_returns_to_code() {
    let (spans, state) = highlight_line("/* hi */ let x = 1;", State::Code, rust());
    assert_eq!(spans[0], (0..8, Token::Comment));
    assert_eq!(state, State::Code);
}

#[test]
fn an_unclosed_block_comment_carries_into_the_next_line() {
    let (spans, state) = highlight_line("/* start", State::Code, rust());
    assert_eq!(spans[0], (0..8, Token::Comment));
    assert_eq!(state, State::BlockComment(1));

    let (spans2, state2) = highlight_line("still a comment */ code();", state, rust());
    assert_eq!(spans2[0], (0..18, Token::Comment));
    assert_eq!(state2, State::Code);
}

#[test]
fn rust_block_comments_nest() {
    let (_, state) = highlight_line("/* outer /* inner */", State::Code, rust());
    // The inner close only drops the depth to 1; still inside the outer.
    assert_eq!(state, State::BlockComment(1));

    let (spans, state2) = highlight_line("still inside */ after", state, rust());
    assert_eq!(spans[0].1, Token::Comment);
    assert_eq!(state2, State::Code);
}

#[test]
fn a_non_nesting_languages_block_comments_close_on_the_first_close_marker() {
    // TypeScript's block comments do not nest.
    let (_, state) = highlight_line("/* outer /* inner */", State::Code, typescript());
    assert_eq!(state, State::Code);
}

// ---- strings -----------------------------------------------------------

#[test]
fn a_simple_string_is_one_span() {
    let (spans, state) = highlight_line("let s = \"hello\";", State::Code, rust());
    let string = spans.iter().find(|(_, t)| *t == Token::Str).unwrap();
    assert_eq!(string.0, 8..15);
    assert_eq!(state, State::Code);
}

#[test]
fn an_escaped_quote_does_not_close_the_string() {
    let (spans, state) = highlight_line(r#"let s = "a \" b";"#, State::Code, rust());
    let string = spans.iter().find(|(_, t)| *t == Token::Str).unwrap();
    assert_eq!(string.0, 8..16);
    assert_eq!(state, State::Code);
}

#[test]
fn a_string_unterminated_at_end_of_line_closes_there_rather_than_carrying_over() {
    let (spans, state) = highlight_line("let s = \"unterminated", State::Code, rust());
    assert_eq!(spans.last().unwrap().1, Token::Str);
    assert_eq!(
        state,
        State::Code,
        "a single-quote string never carries over"
    );
}

#[test]
fn a_triple_quoted_string_spans_multiple_lines() {
    let (spans, state) = highlight_line("x = \"\"\"start", State::Code, python());
    let opened = spans.iter().find(|(_, t)| *t == Token::Str).unwrap();
    assert_eq!(opened.0, 4..12);
    assert_eq!(state, State::MultilineString(b'"'));

    let (spans2, state2) = highlight_line("middle line, no quotes", state, python());
    assert_eq!(spans2, vec![(0..22, Token::Str)]);
    assert_eq!(state2, State::MultilineString(b'"'));

    let (spans3, state3) = highlight_line("end\"\"\" code", state2, python());
    assert_eq!(spans3[0], (0..6, Token::Str));
    assert_eq!(state3, State::Code);
}

#[test]
fn a_triple_quote_does_not_close_on_a_different_quote_character() {
    let (_, state) = highlight_line("x = \"\"\"start", State::Code, python());
    let (_, state2) = highlight_line("not closed by singles '''", state, python());
    assert_eq!(state2, State::MultilineString(b'"'));
}

#[test]
fn a_language_without_triple_quotes_treats_three_quotes_as_an_empty_string_plus_one() {
    let (spans, state) = highlight_line("x = \"\"\"y", State::Code, rust());
    assert!(spans.iter().any(|(_, t)| *t == Token::Str));
    assert_eq!(state, State::Code);
}

// ---- opening_states ------------------------------------------------------

#[test]
fn opening_states_starts_every_document_on_code() {
    let states = opening_states("a\nb\nc", rust());
    assert_eq!(states[0], State::Code);
    assert_eq!(states.len(), 3);
}

#[test]
fn opening_states_tracks_a_comment_open_across_several_lines() {
    let states = opening_states("/* one\ntwo\nthree */ four", rust());
    assert_eq!(states[0], State::Code);
    assert_eq!(states[1], State::BlockComment(1));
    assert_eq!(states[2], State::BlockComment(1));
}

// ---- unicode safety --------------------------------------------------------

#[test]
fn multi_byte_characters_do_not_panic_the_lexer() {
    let (_, state) = highlight_line(
        "let s = \"héllo wörld → done\"; // 日本語",
        State::Code,
        rust(),
    );
    assert_eq!(state, State::Code);
}

#[test]
fn an_identifier_with_accented_letters_does_not_panic() {
    // Not ASCII alphabetic, so these fall through to the unicode skip path
    // rather than being lexed as a word — the lexer must still not panic.
    let (_, state) = highlight_line("é + 1", State::Code, rust());
    assert_eq!(state, State::Code);
}

// ---- toml / triple-quote language with different comment style ------------

#[test]
fn toml_comments_and_keywords() {
    let (spans, _) = highlight_line("enabled = true # note", State::Code, toml());
    assert!(
        spans
            .iter()
            .any(|(range, t)| *t == Token::Keyword && range.start == 10)
    );
    assert!(spans.iter().any(|(_, t)| *t == Token::Comment));
}
