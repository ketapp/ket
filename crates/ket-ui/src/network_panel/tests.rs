use super::*;

fn rows(line: &str, columns: usize) -> Vec<&str> {
    let mut out = Vec::new();
    push_segments(0, line, columns, &mut out);
    out.into_iter().map(|(_, range)| &line[range]).collect()
}

#[test]
fn a_line_that_fits_is_one_row() {
    assert_eq!(rows("short", 10), ["short"]);
    assert_eq!(rows("", 10), [""]);
}

#[test]
fn a_long_line_breaks_after_the_last_space_that_fits() {
    assert_eq!(
        rows("one two three four", 9),
        ["one two ", "three ", "four"]
    );
}

#[test]
fn a_word_wider_than_the_body_is_cut() {
    assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
}

#[test]
fn columns_count_characters_not_bytes() {
    assert_eq!(rows("ééééé", 2), ["éé", "éé", "é"]);
}

#[test]
fn every_byte_of_the_line_is_in_exactly_one_row() {
    let line = "{\"content\": \"we need to set up the email address\\n\\n<pasted>\"}";
    assert_eq!(rows(line, 7).concat(), line);
}

#[test]
fn a_flag_deep_in_a_long_line_is_found_in_its_own_row() {
    let mut state = RequestState {
        lines: Arc::new(vec!["{".into(), "aaaa bbbb cccc dddd".into(), "}".into()]),
        columns: Some(5),
        ..Default::default()
    };
    // Line 1 wraps as "aaaa " "bbbb " "cccc " "dddd": rows 1 to 4.
    assert_eq!(state.row_of(1, 0, true), 1);
    assert_eq!(state.row_of(1, 11, true), 3);
    assert_eq!(state.row_of(1, 18, true), 4);
    assert_eq!(state.row_of(2, 0, true), 5);
    assert_eq!(state.row_of(1, 11, false), 1);
}

#[test]
fn unwrapped_lines_are_cut_at_the_cap() {
    let long = SharedString::from("x".repeat(LINE_CAP + 5));
    let (text, shown) = cut_line(&long);
    assert_eq!(shown, LINE_CAP);
    assert!(text.ends_with(" … 5 more characters on this line"));
}

#[test]
fn a_double_click_takes_the_run_under_the_pointer() {
    let line = "dsn: 'https://6f00ab@o45.example.com/'";
    let word = |at: usize| &line[word_at(line, at)];
    assert_eq!(word(0), "dsn");
    assert_eq!(word(2), "dsn");
    assert_eq!(word(3), ":");
    assert_eq!(word(5), "'");
    assert_eq!(word(16), "6f00ab");
    assert_eq!(word(11), "://");
    // Past the end: the last character's run.
    assert_eq!(word(line.len()), "/'");
    assert_eq!(word_at("", 0), 0..0);
}

#[test]
fn marks_and_the_pick_become_runs_that_do_not_overlap() {
    let mark = HighlightStyle {
        color: Some(gpui::red()),
        background_color: Some(gpui::yellow()),
        ..Default::default()
    };
    let pick = HighlightStyle {
        background_color: Some(gpui::blue()),
        ..Default::default()
    };
    let out = runs(std::slice::from_ref(&(2..6)), Some(&(4..9)), mark, pick);
    let ranges: Vec<Range<usize>> = out.iter().map(|(range, _)| range.clone()).collect();
    assert_eq!(ranges, [2..4, 4..6, 6..9]);
    assert_eq!(out[0].1, mark);
    // Marked text in the pick keeps its colour on the pick's ground.
    assert_eq!(out[1].1.color, mark.color);
    assert_eq!(out[1].1.background_color, pick.background_color);
    assert_eq!(out[2].1, pick);

    let apart = runs(std::slice::from_ref(&(0..2)), Some(&(5..7)), mark, pick);
    let ranges: Vec<Range<usize>> = apart.iter().map(|(range, _)| range.clone()).collect();
    assert_eq!(ranges, [0..2, 5..7]);
    assert!(runs(&[], None, mark, pick).is_empty());
}

#[test]
fn the_text_picked_runs_across_lines_either_way_round() {
    let state = RequestState {
        lines: Arc::new(vec!["ab".into(), "cd".into(), "ef".into()]),
        pick: Some(Pick {
            anchor: ((0, 1), (0, 1)),
            head: (2, 1),
        }),
        ..Default::default()
    };
    assert_eq!(state.picked_text().as_deref(), Some("b\ncd\ne"));
    let backwards = RequestState {
        pick: Some(Pick {
            anchor: ((2, 1), (2, 1)),
            head: (0, 1),
        }),
        ..state
    };
    assert_eq!(backwards.picked_text().as_deref(), Some("b\ncd\ne"));
    let nothing = RequestState {
        pick: Some(Pick {
            anchor: ((1, 1), (1, 1)),
            head: (1, 1),
        }),
        ..backwards
    };
    assert_eq!(nothing.picked_text(), None);
    assert!(nothing.pick.unwrap().is_empty());
}

#[test]
fn mute_names_one_kind_and_counts_several() {
    assert_eq!(mute_label(&[Kind::Email]), "Mute emails");
    assert_eq!(mute_label(&[Kind::Secret]), "Mute API keys");
    assert_eq!(mute_label(&[Kind::Email, Kind::Ip]), "Mute both");
    assert_eq!(
        mute_label(&[Kind::Secret, Kind::Email, Kind::Ip]),
        "Mute all 3"
    );
    assert_eq!(muted_sentence(&[Kind::Email]), "Emails won't be flagged");
    assert_eq!(
        muted_sentence(&[Kind::Secret, Kind::Email, Kind::Ip]),
        "API keys, emails and IP addresses won't be flagged"
    );
}

#[test]
fn muting_takes_a_kind_out_of_an_alert_and_the_requests_that_sent_only_it() {
    let counted = |id: &str, kinds: &[Kind]| Counted {
        key: RequestKey {
            worktree: WorktreeId::new("w"),
            id: id.to_owned(),
        },
        to: "Anthropic".to_owned(),
        kinds: kinds.iter().copied().collect(),
    };
    let mut tally = Tally {
        requests: vec![
            counted("a", &[Kind::Email]),
            counted("b", &[Kind::Email, Kind::Ip]),
            counted("c", &[Kind::Email]),
        ],
    };
    assert_eq!(tally.kinds(), [Kind::Email, Kind::Ip]);
    tally.forget(&[Kind::Email].into_iter().collect());
    assert_eq!(tally.kinds(), [Kind::Ip]);
    let left: Vec<&str> = tally.requests.iter().map(|c| c.key.id.as_str()).collect();
    assert_eq!(left, ["b"]);
    tally.forget(&[Kind::Ip].into_iter().collect());
    assert!(tally.requests.is_empty());
}

#[test]
fn a_muted_kind_is_left_out_of_the_flags() {
    let flags = [
        Flag {
            kind: Kind::Secret,
            count: 1,
        },
        Flag {
            kind: Kind::Email,
            count: 4,
        },
    ];
    let muted: BTreeSet<Kind> = [Kind::Email].into_iter().collect();
    assert_eq!(heard(&flags, &muted), [flags[0]]);
    assert!(any_heard(&flags, &muted));
    let all: BTreeSet<Kind> = Kind::ALL.into_iter().collect();
    assert!(heard(&flags, &all).is_empty());
    assert!(!any_heard(&flags, &all));
    assert_eq!(heard(&flags, &BTreeSet::new()), flags);
}

#[test]
fn a_rows_part_of_the_pick_stays_within_the_row() {
    // Line 1 is wrapped as bytes 0..10, 10..20 and 20..30.
    let pick = Some(((1, 3), (1, 12)));
    assert_eq!(row_pick(1, &(0..10), pick), Some(3..10));
    assert_eq!(row_pick(1, &(10..20), pick), Some(0..2));
    // A later row of the line the pick ends on, which it never reaches.
    assert_eq!(row_pick(1, &(20..30), pick), None);
    // An earlier row of the line it starts on.
    let late = Some(((1, 25), (2, 4)));
    assert_eq!(row_pick(1, &(0..10), late), None);
    assert_eq!(row_pick(1, &(20..30), late), Some(5..10));
    assert_eq!(row_pick(2, &(0..8), late), Some(0..4));
    assert_eq!(row_pick(0, &(0..8), late), None);
    assert_eq!(row_pick(3, &(0..8), late), None);
    assert_eq!(row_pick(1, &(0..8), None), None);
}

#[test]
fn a_word_picked_stays_whole_when_dragged_back_past_it() {
    let pick = Pick {
        anchor: ((0, 4), (0, 8)),
        head: (0, 1),
    };
    assert_eq!(pick.range(), ((0, 1), (0, 8)));
}
