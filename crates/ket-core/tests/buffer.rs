//! The tweak editor's text buffer, checked against the cases a rope, a cursor,
//! and hand-written undo bookkeeping are most likely to get wrong: text that
//! is not ASCII, files with unusual line endings, and undo history that must
//! coalesce typing without ever coalescing something it shouldn't.

mod common;
use common::Sandbox;

use ket_core::KetError;
use ket_core::buffer::{Buffer, Case, LineCol, LineEnding, MAX_FIND_MATCHES};

// ---------------------------------------------------------------------------
// Construction and basic content
// ---------------------------------------------------------------------------

#[test]
fn a_new_buffer_is_empty_and_not_dirty() {
    let buffer = Buffer::new();
    assert_eq!(buffer.text(), "");
    assert_eq!(buffer.len_chars(), 0);
    assert_eq!(buffer.line_count(), 1);
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.path(), None);
}

#[test]
fn from_text_preserves_plain_ascii_content() {
    let buffer = Buffer::from_text("hello world");
    assert_eq!(buffer.text(), "hello world");
    assert_eq!(buffer.line_ending(), LineEnding::Lf);
}

// ---------------------------------------------------------------------------
// UTF-8 that is not ASCII
// ---------------------------------------------------------------------------

#[test]
fn inserting_a_multi_byte_character_does_not_split_it() {
    let mut buffer = Buffer::from_text("");
    buffer.insert("😀"); // 1 char, 4 bytes.
    assert_eq!(buffer.len_chars(), 1);
    assert_eq!(buffer.text(), "😀");

    // The byte offset just past the character is the full width of its UTF-8
    // encoding, never a position inside it.
    assert_eq!(buffer.char_to_byte(1), 4);
    assert_eq!(buffer.byte_to_char(4), 1);

    // Backspacing removes the whole character, not one of its bytes.
    buffer.backspace();
    assert_eq!(buffer.text(), "");
}

#[test]
fn column_arithmetic_counts_characters_not_bytes() {
    // "héllo": 5 characters, but "é" is 2 bytes in UTF-8, so the line is 6
    // bytes long. A column computed from bytes would put the end of the line
    // at column 7, not 6.
    let buffer = Buffer::from_text("héllo");
    assert_eq!(buffer.len_chars(), 5);
    assert_ne!(
        buffer.char_to_byte(5),
        5,
        "the line must contain a multi-byte character"
    );

    let end = buffer.line_col_at(5);
    assert_eq!(end, LineCol { line: 1, column: 6 });
}

#[test]
fn word_movement_stops_correctly_around_multi_byte_characters() {
    let mut buffer = Buffer::from_text("héllo wörld");
    buffer.move_document_start();
    buffer.move_word_right();
    assert_eq!(buffer.cursor(), 5); // end of "héllo", in characters.
    buffer.move_word_right();
    assert_eq!(buffer.cursor(), 11); // end of "wörld".
}

// ---------------------------------------------------------------------------
// CRLF files
// ---------------------------------------------------------------------------

#[test]
fn loading_a_crlf_file_normalizes_line_endings_to_lf_internally() {
    let sandbox = Sandbox::new("buffer-crlf-load");
    let path = sandbox.path("file.txt");
    std::fs::write(&path, b"one\r\ntwo\r\nthree").unwrap();

    let buffer = Buffer::load(&path).unwrap();
    assert_eq!(buffer.line_ending(), LineEnding::Crlf);
    assert_eq!(buffer.text(), "one\ntwo\nthree");
    assert_eq!(buffer.line_count(), 3);
}

#[test]
fn saving_a_crlf_buffer_restores_crlf_line_endings_byte_for_byte() {
    let sandbox = Sandbox::new("buffer-crlf-save");
    let path = sandbox.path("file.txt");
    std::fs::write(&path, b"one\r\ntwo\r\n").unwrap();

    let mut buffer = Buffer::load(&path).unwrap();
    buffer.move_document_end();
    buffer.insert("three");
    buffer.save().unwrap();

    let saved = std::fs::read(&path).unwrap();
    assert_eq!(saved, b"one\r\ntwo\r\nthree");
}

#[test]
fn a_lone_carriage_return_that_is_not_part_of_crlf_is_left_alone() {
    // Not a required-to-preserve line ending in this module's model, but it
    // must not be silently eaten either: it is content.
    let buffer = Buffer::from_text("a\rb");
    assert_eq!(buffer.text(), "a\rb");
    assert_eq!(buffer.line_ending(), LineEnding::Lf);
    assert_eq!(buffer.line_count(), 1);
}

// ---------------------------------------------------------------------------
// A file with no trailing newline
// ---------------------------------------------------------------------------

#[test]
fn a_file_with_no_trailing_newline_still_counts_its_last_line() {
    let buffer = Buffer::from_text("line1\nline2");
    assert_eq!(buffer.line_count(), 2);
    assert_eq!(buffer.line_text(1), "line2");
}

#[test]
fn move_end_on_the_last_line_without_a_trailing_newline_reaches_the_end_of_the_buffer() {
    let mut buffer = Buffer::from_text("line1\nline2");
    buffer.move_document_end();
    buffer.move_home();
    buffer.move_end();
    assert_eq!(buffer.cursor(), buffer.len_chars());
}

#[test]
fn saving_a_buffer_with_no_trailing_newline_does_not_add_one() {
    let sandbox = Sandbox::new("buffer-no-trailing-newline");
    let path = sandbox.path("file.txt");
    std::fs::write(&path, b"abc").unwrap();

    let mut buffer = Buffer::load(&path).unwrap();
    buffer.save_as(&sandbox.path("out.txt")).unwrap();

    assert_eq!(std::fs::read(sandbox.path("out.txt")).unwrap(), b"abc");
}

// ---------------------------------------------------------------------------
// Undo/redo, including coalesced runs
// ---------------------------------------------------------------------------

#[test]
fn typing_several_characters_then_undoing_once_reverts_all_of_them() {
    let mut buffer = Buffer::new();
    for ch in "hello".chars() {
        buffer.insert(&ch.to_string());
    }
    assert_eq!(buffer.text(), "hello");

    assert!(buffer.undo());
    assert_eq!(
        buffer.text(),
        "",
        "one undo should revert the whole typed run"
    );
    assert!(!buffer.can_undo());
}

#[test]
fn moving_the_cursor_between_keystrokes_splits_the_undo_run() {
    let mut buffer = Buffer::new();
    buffer.insert("a");
    buffer.insert("b");
    buffer.move_left();
    buffer.insert("c");
    assert_eq!(buffer.text(), "acb");

    assert!(buffer.undo());
    assert_eq!(
        buffer.text(),
        "ab",
        "the move should have closed the first run"
    );
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "");
}

#[test]
fn backspacing_a_run_does_not_coalesce_with_the_insertion_that_preceded_it() {
    let mut buffer = Buffer::new();
    buffer.insert("a");
    buffer.insert("b");
    buffer.insert("c");
    buffer.backspace();
    buffer.backspace();
    assert_eq!(buffer.text(), "a");

    // First undo reverts only the two backspaces (a different kind of edit
    // than the insertions, so it never merged with them).
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "abc");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "");
}

#[test]
fn backspacing_across_a_line_break_closes_the_undo_run() {
    let mut buffer = Buffer::from_text("ab\ncd");
    buffer.move_document_end();
    buffer.backspace(); // remove 'd'
    buffer.backspace(); // remove 'c'
    buffer.backspace(); // remove '\n'
    buffer.backspace(); // remove 'b' -- crosses the line break just removed
    assert_eq!(buffer.text(), "a");

    // The newline the third backspace removed is now part of the merged
    // entry's text, so the fourth backspace could not extend it further.
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "ab", "only the last backspace should revert");
    assert!(buffer.undo());
    assert_eq!(
        buffer.text(),
        "ab\ncd",
        "the rest reverts as the earlier run"
    );
}

#[test]
fn pressing_enter_is_always_its_own_undo_step() {
    let mut buffer = Buffer::new();
    buffer.insert("a");
    buffer.newline();
    buffer.insert("b");
    assert_eq!(buffer.text(), "a\nb");

    assert!(buffer.undo());
    assert_eq!(
        buffer.text(),
        "a\n",
        "typing after Enter must not merge with it"
    );
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "a", "Enter itself is a standalone step");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "");
}

#[test]
fn a_multi_character_insert_is_always_its_own_undo_step() {
    let mut buffer = Buffer::new();
    buffer.insert("a");
    buffer.insert("pasted");
    buffer.insert("b");
    assert_eq!(buffer.text(), "apastedb");

    assert!(buffer.undo());
    assert_eq!(buffer.text(), "apasted");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "a");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "");
}

#[test]
fn replace_selection_never_coalesces_even_for_a_single_character() {
    let mut buffer = Buffer::from_text("cat");
    buffer.set_selection(0, 3);
    buffer.replace_selection("x");
    buffer.insert("y");
    assert_eq!(buffer.text(), "xy");

    assert!(buffer.undo());
    assert_eq!(
        buffer.text(),
        "x",
        "the typed 'y' must not merge with the replacement"
    );
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "cat");
}

#[test]
fn redo_reapplies_an_undone_run_exactly() {
    let mut buffer = Buffer::new();
    buffer.insert("a");
    buffer.insert("b");
    buffer.insert("c");
    buffer.undo();
    assert_eq!(buffer.text(), "");
    assert!(buffer.redo());
    assert_eq!(buffer.text(), "abc");
    assert!(!buffer.can_redo());
}

#[test]
fn a_new_edit_after_undo_clears_the_redo_stack() {
    let mut buffer = Buffer::new();
    buffer.insert("a");
    buffer.undo();
    assert!(buffer.can_redo());

    buffer.insert("z");
    assert!(
        !buffer.can_redo(),
        "a fresh edit should discard the old redo history"
    );
}

#[test]
fn undo_and_redo_restore_the_selection_around_the_edit() {
    let mut buffer = Buffer::from_text("hello world");
    buffer.set_selection(0, 5);
    buffer.replace_selection("bye");
    assert_eq!(buffer.text(), "bye world");

    buffer.undo();
    assert_eq!(buffer.selection(), 0..5);
    assert_eq!(buffer.text(), "hello world");

    buffer.redo();
    assert_eq!(buffer.cursor(), 3);
    assert_eq!(buffer.text(), "bye world");
}

// ---------------------------------------------------------------------------
// Find
// ---------------------------------------------------------------------------

#[test]
fn find_all_reports_every_match_and_its_count() {
    let buffer = Buffer::from_text("the cat sat on the mat");
    let matches = buffer.find_all("at", Case::Sensitive);
    assert_eq!(matches.total, 3);
    assert_eq!(matches.ranges.len(), 3);
    assert!(!matches.truncated);
}

#[test]
fn find_all_returns_nothing_for_an_empty_needle() {
    let buffer = Buffer::from_text("anything");
    let matches = buffer.find_all("", Case::Sensitive);
    assert_eq!(matches.total, 0);
    assert!(matches.ranges.is_empty());
}

#[test]
fn find_is_case_sensitive_by_default_and_insensitive_on_request() {
    let buffer = Buffer::from_text("Cat cat CAT");
    assert_eq!(buffer.find_all("cat", Case::Sensitive).total, 1);
    assert_eq!(buffer.find_all("cat", Case::Insensitive).total, 3);
}

#[test]
fn find_next_wraps_from_the_end_of_the_buffer_to_the_start() {
    let buffer = Buffer::from_text("cat dog");
    // Searching forward from past the only match finds nothing ahead, and
    // must wrap around to find it at the start instead.
    let found = buffer.find_next("cat", Case::Sensitive, 5).unwrap();
    assert_eq!(found, 0..3);
}

#[test]
fn find_previous_wraps_from_the_start_of_the_buffer_to_the_end() {
    let buffer = Buffer::from_text("dog cat");
    let found = buffer.find_previous("cat", Case::Sensitive, 0).unwrap();
    assert_eq!(found, 4..7);
}

#[test]
fn find_next_advances_past_the_current_match_instead_of_repeating_it() {
    let buffer = Buffer::from_text("cat cat cat");
    let first = buffer.find_next("cat", Case::Sensitive, 0).unwrap();
    assert_eq!(first, 0..3);
    let second = buffer.find_next("cat", Case::Sensitive, first.end).unwrap();
    assert_eq!(second, 4..7);
}

#[test]
fn find_next_and_previous_return_none_when_the_needle_never_occurs() {
    let buffer = Buffer::from_text("no match here");
    assert_eq!(buffer.find_next("zzz", Case::Sensitive, 0), None);
    assert_eq!(buffer.find_previous("zzz", Case::Sensitive, 0), None);
}

#[test]
fn find_all_caps_the_kept_matches_but_still_counts_every_one() {
    let text = "x".repeat(MAX_FIND_MATCHES + 50);
    let buffer = Buffer::from_text(&text);
    let matches = buffer.find_all("x", Case::Sensitive);
    assert_eq!(matches.total, MAX_FIND_MATCHES + 50);
    assert_eq!(matches.ranges.len(), MAX_FIND_MATCHES);
    assert!(matches.truncated);
}

// ---------------------------------------------------------------------------
// Line/column <-> byte-offset conversion
// ---------------------------------------------------------------------------

#[test]
fn line_col_at_reports_one_based_positions() {
    let buffer = Buffer::from_text("abc\ndef\nghi");
    assert_eq!(buffer.line_col_at(0), LineCol { line: 1, column: 1 });
    assert_eq!(buffer.line_col_at(4), LineCol { line: 2, column: 1 });
    assert_eq!(buffer.line_col_at(6), LineCol { line: 2, column: 3 });
}

#[test]
fn char_at_line_col_is_the_inverse_of_line_col_at() {
    let buffer = Buffer::from_text("abc\ndef\nghi");
    for char_idx in 0..=buffer.len_chars() {
        let at = buffer.line_col_at(char_idx);
        assert_eq!(
            buffer.char_at_line_col(at),
            char_idx,
            "round trip for char {char_idx}"
        );
    }
}

#[test]
fn byte_and_char_line_col_conversions_agree_on_a_multi_byte_line() {
    let buffer = Buffer::from_text("é\nworld");
    // "é" is 2 bytes but 1 character, so char index 2 (start of "world")
    // sits at byte offset 3 (2 for "é" + 1 for the newline).
    let at_char = buffer.line_col_at(2);
    assert_eq!(at_char, LineCol { line: 2, column: 1 });
    assert_eq!(buffer.line_col_at_byte(3), at_char);
    assert_eq!(buffer.byte_at_line_col(at_char), 3);
}

#[test]
fn char_at_line_col_clamps_a_column_past_the_end_of_a_shorter_line() {
    let buffer = Buffer::from_text("ab\nc");
    let clamped = buffer.char_at_line_col(LineCol {
        line: 1,
        column: 99,
    });
    assert_eq!(clamped, 2, "clamped to the end of \"ab\"");
}

// ---------------------------------------------------------------------------
// Cursor and selection movement
// ---------------------------------------------------------------------------

#[test]
fn move_left_and_right_walk_one_character_at_a_time_and_stop_at_the_ends() {
    let mut buffer = Buffer::from_text("ab");
    buffer.move_left();
    assert_eq!(buffer.cursor(), 0, "already at the start");
    buffer.move_right();
    buffer.move_right();
    assert_eq!(buffer.cursor(), 2);
    buffer.move_right();
    assert_eq!(buffer.cursor(), 2, "already at the end");
}

#[test]
fn an_arrow_key_with_a_selection_collapses_to_the_near_edge_first() {
    let mut buffer = Buffer::from_text("hello world");
    buffer.set_selection(2, 7);
    buffer.move_left();
    assert_eq!(buffer.cursor(), 2);
    assert!(!buffer.has_selection());

    buffer.set_selection(2, 7);
    buffer.move_right();
    assert_eq!(buffer.cursor(), 7);
    assert!(!buffer.has_selection());
}

#[test]
fn extend_variants_grow_the_selection_without_moving_the_anchor() {
    let mut buffer = Buffer::from_text("hello world");
    buffer.set_cursor(2);
    buffer.extend_right();
    buffer.extend_right();
    buffer.extend_right();
    assert_eq!(buffer.anchor(), 2);
    assert_eq!(buffer.cursor(), 5);
    assert_eq!(buffer.selected_text(), "llo");
}

#[test]
fn word_movement_skips_whitespace_and_stops_at_word_boundaries() {
    let mut buffer = Buffer::from_text("  foo bar-baz");
    buffer.move_document_start();
    buffer.move_word_right();
    assert_eq!(buffer.cursor(), 5, "past leading spaces and \"foo\"");
    buffer.move_word_right();
    assert_eq!(buffer.cursor(), 9, "stops at the punctuation boundary");
    buffer.move_word_left();
    assert_eq!(buffer.cursor(), 6);
}

#[test]
fn home_and_end_move_to_the_boundaries_of_the_current_line() {
    let mut buffer = Buffer::from_text("first\nsecond line\nthird");
    buffer.set_cursor(10); // somewhere inside "second line"
    buffer.move_home();
    assert_eq!(buffer.cursor(), 6);
    buffer.move_end();
    assert_eq!(buffer.cursor(), 17);
}

#[test]
fn document_start_and_end_move_to_the_ends_of_the_whole_buffer() {
    let mut buffer = Buffer::from_text("first\nsecond\nthird");
    buffer.set_cursor(8);
    buffer.move_document_start();
    assert_eq!(buffer.cursor(), 0);
    buffer.move_document_end();
    assert_eq!(buffer.cursor(), buffer.len_chars());
}

#[test]
fn vertical_movement_preserves_the_goal_column_through_a_shorter_line() {
    let mut buffer = Buffer::from_text("long line\nhi\nlong line");
    buffer.set_cursor(7); // column 8 on the first line
    buffer.move_down(); // lands clamped on the short "hi" line
    assert_eq!(buffer.cursor(), 12, "clamped to the end of \"hi\"");
    buffer.move_down(); // back onto a line as long as the first
    assert_eq!(buffer.cursor(), 20, "column 8 restored on the third line");
}

#[test]
fn moving_up_from_the_first_line_or_down_from_the_last_is_a_no_op() {
    let mut buffer = Buffer::from_text("only line");
    buffer.set_cursor(3);
    buffer.move_up();
    assert_eq!(buffer.cursor(), 3);
    buffer.move_down();
    assert_eq!(buffer.cursor(), 3);
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

#[test]
fn insert_replaces_an_active_selection() {
    let mut buffer = Buffer::from_text("hello world");
    buffer.set_selection(0, 5);
    buffer.insert("goodbye");
    assert_eq!(buffer.text(), "goodbye world");
    assert!(!buffer.has_selection());
}

#[test]
fn delete_removes_the_character_after_the_cursor() {
    let mut buffer = Buffer::from_text("hello");
    buffer.set_cursor(0);
    buffer.delete();
    assert_eq!(buffer.text(), "ello");
    assert_eq!(buffer.cursor(), 0);
}

#[test]
fn delete_with_no_character_after_the_cursor_does_nothing() {
    let mut buffer = Buffer::from_text("hello");
    buffer.move_document_end();
    buffer.delete();
    assert_eq!(buffer.text(), "hello");
}

#[test]
fn backspace_removes_the_character_before_the_cursor() {
    let mut buffer = Buffer::from_text("hello");
    buffer.move_document_end();
    buffer.backspace();
    assert_eq!(buffer.text(), "hell");
    assert_eq!(buffer.cursor(), 4);
}

#[test]
fn delete_and_backspace_remove_a_selection_instead_of_one_character() {
    let mut buffer = Buffer::from_text("hello world");
    buffer.set_selection(5, 11);
    buffer.delete();
    assert_eq!(buffer.text(), "hello");

    let mut buffer = Buffer::from_text("hello world");
    buffer.set_selection(5, 11);
    buffer.backspace();
    assert_eq!(buffer.text(), "hello");
}

#[test]
fn newline_inserts_a_line_break_at_the_cursor() {
    let mut buffer = Buffer::from_text("helloworld");
    buffer.set_cursor(5);
    buffer.newline();
    assert_eq!(buffer.text(), "hello\nworld");
    assert_eq!(buffer.cursor(), 6);
}

#[test]
fn replace_selection_substitutes_the_selected_text() {
    let mut buffer = Buffer::from_text("the cat sat");
    buffer.set_selection(4, 7);
    buffer.replace_selection("dog");
    assert_eq!(buffer.text(), "the dog sat");
}

#[test]
fn replace_selection_with_no_active_selection_inserts_at_the_cursor() {
    let mut buffer = Buffer::from_text("ac");
    buffer.set_cursor(1);
    buffer.replace_selection("b");
    assert_eq!(buffer.text(), "abc");
}

// ---------------------------------------------------------------------------
// Dirty flag
// ---------------------------------------------------------------------------

#[test]
fn a_freshly_loaded_buffer_is_not_dirty() {
    let sandbox = Sandbox::new("buffer-dirty-load");
    let path = sandbox.path("file.txt");
    std::fs::write(&path, "content").unwrap();
    let buffer = Buffer::load(&path).unwrap();
    assert!(!buffer.is_dirty());
}

#[test]
fn editing_marks_the_buffer_dirty() {
    let mut buffer = Buffer::from_text("content");
    assert!(!buffer.is_dirty());
    buffer.insert("!");
    assert!(buffer.is_dirty());
}

#[test]
fn saving_clears_the_dirty_flag() {
    let sandbox = Sandbox::new("buffer-dirty-save");
    let path = sandbox.path("file.txt");
    let mut buffer = Buffer::from_text("content");
    buffer.insert("!");
    assert!(buffer.is_dirty());
    buffer.save_as(&path).unwrap();
    assert!(!buffer.is_dirty());
}

#[test]
fn undoing_back_to_saved_text_still_reports_dirty() {
    // Documents the deliberate simplification in the module doc: dirty means
    // "changed since the last save", not "differs from disk right now".
    let mut buffer = Buffer::from_text("content");
    buffer.insert("!");
    buffer.undo();
    assert_eq!(buffer.text(), "content");
    assert!(buffer.is_dirty());
}

#[test]
fn saving_without_a_path_fails_with_a_clear_error() {
    let mut buffer = Buffer::new();
    let err = buffer.save().unwrap_err();
    assert!(matches!(err, KetError::Path { .. }), "{err:?}");
}

#[test]
fn save_as_adopts_the_path_for_later_plain_saves() {
    let sandbox = Sandbox::new("buffer-save-as");
    let path = sandbox.path("file.txt");
    let mut buffer = Buffer::new();
    buffer.insert("first");
    buffer.save_as(&path).unwrap();

    buffer.insert(" second");
    buffer.save().unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first second");
    assert_eq!(buffer.path(), Some(path.as_path()));
}

#[test]
fn loading_a_file_that_is_not_valid_utf8_is_an_error_not_a_panic() {
    let sandbox = Sandbox::new("buffer-invalid-utf8");
    let path = sandbox.path("file.txt");
    std::fs::write(&path, [0xff, 0xfe, 0x00, 0x01]).unwrap();

    let err = Buffer::load(&path).unwrap_err();
    assert!(matches!(err, KetError::Io { .. }), "{err:?}");
}
