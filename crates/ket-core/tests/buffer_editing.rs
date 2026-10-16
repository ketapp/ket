//! The buffer's selection-extending moves, indentation, and `revise` — the
//! parts `buffer.rs`'s other tests leave out.

use ket_core::buffer::Buffer;

fn at(text: &str, cursor: usize) -> Buffer {
    let mut buffer = Buffer::from_text(text);
    buffer.set_cursor(cursor);
    buffer
}

// ---- extending the selection ----------------------------------------------------

#[test]
fn extending_by_character_and_word_keeps_the_anchor_where_it_was() {
    let mut buffer = at("hello brave world", 6);
    buffer.extend_right();
    assert_eq!(buffer.selection(), 6..7);
    buffer.extend_left();
    buffer.extend_left();
    assert_eq!((buffer.anchor(), buffer.cursor()), (6, 5));
    assert_eq!(buffer.selection(), 5..6);

    let mut words = at("hello brave world", 6);
    words.extend_word_right();
    assert_eq!(words.selection(), 6..11, "to the end of `brave`");
    let mut back = at("hello brave world", 11);
    back.extend_word_left();
    assert_eq!(back.selection(), 6..11);
    assert_eq!(back.anchor(), 11);
}

#[test]
fn extending_to_line_and_document_ends() {
    let text = "first line\nsecond line\nthird";
    let mut home = at(text, 15);
    home.extend_home();
    assert_eq!(home.selection(), 11..15);
    let mut end = at(text, 15);
    end.extend_end();
    assert_eq!(end.selection(), 15..22, "stops before the line break");

    let mut start = at(text, 15);
    start.extend_document_start();
    assert_eq!(start.selection(), 0..15);
    let mut finish = at(text, 15);
    finish.extend_document_end();
    assert_eq!(finish.selection(), 15..text.chars().count());
}

#[test]
fn extending_up_and_down_keeps_the_goal_column_across_a_short_line() {
    let text = "long line here\nab\nanother long line";
    let mut buffer = at(text, 10); // column 10 of line 0
    buffer.extend_down();
    assert_eq!(buffer.cursor(), 17, "clamped to the end of `ab`");
    buffer.extend_down();
    assert_eq!(buffer.cursor(), 18 + 10, "back at column 10");
    assert_eq!(buffer.anchor(), 10);
    buffer.extend_up();
    buffer.extend_up();
    assert_eq!(buffer.cursor(), 10);
    // At the first line, up stops rather than jumping to the start.
    buffer.extend_up();
    assert_eq!(buffer.cursor(), 10);
}

#[test]
fn select_all_takes_the_whole_buffer() {
    let mut buffer = at("a\nb\nc", 2);
    buffer.select_all();
    assert_eq!(buffer.selection(), 0..5);
    assert_eq!(buffer.anchor(), 0);
}

// ---- indentation -----------------------------------------------------------------

#[test]
fn the_indent_unit_is_read_off_the_file() {
    assert_eq!(
        Buffer::from_text("fn a() {\n\tb();\n}\n").indent_unit(),
        "\t"
    );
    assert_eq!(
        Buffer::from_text("a:\n  b:\n    c: 1\n  d: 2\n").indent_unit(),
        "  "
    );
    assert_eq!(
        Buffer::from_text("fn a() {\n    if x {\n        y();\n    }\n}\n").indent_unit(),
        "    "
    );
    // A block comment's one-space lines do not make the step one.
    assert_eq!(
        Buffer::from_text("/**\n * doc\n */\nfn a() {\n    b();\n}\n").indent_unit(),
        "    "
    );
    // Nothing indented at all: four spaces.
    assert_eq!(Buffer::from_text("a\nb\n").indent_unit(), "    ");
    // Only one-space steps: one space.
    assert_eq!(Buffer::from_text("a\n b\n").indent_unit(), " ");
}

#[test]
fn indenting_moves_every_selected_line_and_skips_blank_ones() {
    let mut buffer = Buffer::from_text("one\n\ntwo\nthree");
    buffer.set_selection(0, 8); // "one\n\ntwo"
    buffer.indent_lines("  ");
    assert_eq!(buffer.text(), "  one\n\n  two\nthree");
    // The selection still holds the same lines, from their new indent.
    assert_eq!(buffer.selection(), 0..12);

    // One undo step for the whole block.
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "one\n\ntwo\nthree");
}

#[test]
fn indenting_with_no_selection_moves_the_cursors_line_and_the_cursor_with_it() {
    let mut buffer = at("alpha\nbeta", 8); // inside "beta"
    buffer.indent_lines("\t");
    assert_eq!(buffer.text(), "alpha\n\tbeta");
    assert_eq!(buffer.cursor(), 9);
}

#[test]
fn a_selection_ending_at_a_line_start_does_not_take_that_line_in() {
    let mut buffer = Buffer::from_text("a\nb\nc");
    buffer.set_selection(0, 4); // "a\nb\n" up to the start of "c"
    buffer.indent_lines("  ");
    assert_eq!(buffer.text(), "  a\n  b\nc");
}

#[test]
fn outdenting_takes_one_level_off_whatever_it_is_made_of() {
    let mut buffer = Buffer::from_text("    four\n\ttab\n  two\nnone");
    buffer.select_all();
    buffer.outdent_lines("    ");
    assert_eq!(buffer.text(), "four\ntab\ntwo\nnone");

    // With a tab as the unit, spaces still come off, up to four of them.
    let mut spaces = Buffer::from_text("      six");
    spaces.select_all();
    spaces.outdent_lines("\t");
    assert_eq!(spaces.text(), "  six");
}

#[test]
fn outdenting_a_line_with_no_indent_changes_nothing_and_adds_no_undo_step() {
    let mut buffer = at("flush", 2);
    buffer.outdent_lines("    ");
    assert_eq!(buffer.text(), "flush");
    assert!(!buffer.undo(), "nothing to undo");
    assert!(!buffer.is_dirty());
}

// ---- revise ------------------------------------------------------------------------

#[test]
fn revising_dictated_words_is_one_undo_step_however_often_they_change() {
    let mut buffer = Buffer::from_text("Say: ");
    buffer.set_cursor(5);
    buffer.insert("helo");
    buffer.revise(5..9, "hello");
    buffer.revise(5..10, "hello world");
    assert_eq!(buffer.text(), "Say: hello world");
    assert_eq!(buffer.cursor(), 16);

    assert!(buffer.undo());
    assert_eq!(
        buffer.text(),
        "Say: ",
        "one undo takes all the revisions back"
    );
    assert!(buffer.redo());
    assert_eq!(buffer.text(), "Say: hello world");
}

#[test]
fn revising_text_something_else_wrote_is_its_own_undo_step() {
    let mut buffer = Buffer::from_text("");
    buffer.insert("first");
    buffer.set_cursor(0);
    buffer.insert("X"); // a different edit is now on top
    buffer.revise(1..6, "FIRST");
    assert_eq!(buffer.text(), "XFIRST");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "Xfirst", "only the revision is undone");
}

#[test]
fn an_empty_range_is_never_folded_into_the_edit_before_it() {
    let mut buffer = Buffer::from_text("abc");
    buffer.set_cursor(3);
    buffer.insert("d");
    buffer.revise(4..4, "e");
    assert_eq!(buffer.text(), "abcde");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), "abcd");
}
