//! `Note`/`Backlog`'s pure logic: id generation, blankness, counting, the
//! serde shape, tags, search, the brief and the order notes are shown in, and
//! the file-backed operations via the `_in(root, ...)` seam (mirroring
//! `crate::shell::ensure_wrappers_in`) rather than the real XDG backlog
//! directory. The store kept in a repository is tested beside its code, in
//! `ket_core::backlog`, since nothing outside reaches it without the real
//! state file.

use std::fs;

use ket_core::backlog::{
    Backlog, Done, MAX_TAG_CHARS, MAX_TAGS, Note, Priority, attachment_path, normalised_tags,
};
use ket_core::id::ProjectId;
use ket_core::id::WorktreeId;

mod common;
use common::Sandbox;

fn note(title: &str, body: &str) -> Note {
    Note {
        title: title.to_owned(),
        body: body.to_owned(),
        ..Note::new(&[])
    }
}

#[test]
fn a_new_note_gets_a_unique_id_and_matching_timestamps() {
    let note = Note::new(&[]);
    assert!(!note.id.is_empty());
    assert_eq!(note.created_ms, note.updated_ms);
    assert!(note.title.is_empty());
    assert!(note.is_blank());
}

#[test]
fn a_new_note_avoids_colliding_with_an_existing_id() {
    let first = Note::new(&[]);
    let second = Note::new(std::slice::from_ref(&first));
    assert_ne!(first.id, second.id);
}

#[test]
fn is_blank_is_false_once_anything_is_filled_in() {
    assert!(!note("Title", "").is_blank());
    assert!(!note("", "Body").is_blank());

    let mut with_attachment = Note::new(&[]);
    with_attachment.attachments.push("file.png".to_owned());
    assert!(!with_attachment.is_blank());
}

#[test]
fn is_blank_ignores_surrounding_whitespace() {
    assert!(note("   ", "  \n  ").is_blank());
}

#[test]
fn open_count_excludes_done_notes() {
    let mut backlog = Backlog::default();
    backlog.notes.push(note("open one", "body"));
    backlog.notes.push(note("open two", "body"));
    let mut done = note("done one", "body");
    done.done = Some(Done {
        at_ms: 1,
        worktree: Some(WorktreeId::new("w1")),
        branch: Some("feature".to_owned()),
    });
    backlog.notes.push(done);

    assert_eq!(backlog.open_count(), 2);
}

#[test]
fn open_count_of_an_empty_backlog_is_zero() {
    assert_eq!(Backlog::default().open_count(), 0);
}

#[test]
fn note_and_backlog_round_trip_through_json() {
    let mut backlog = Backlog::default();
    let mut n = note("Title", "Body");
    n.attachments.push("shot.png".to_owned());
    n.done = Some(Done {
        at_ms: 42,
        worktree: Some(WorktreeId::new("w1")),
        branch: Some("feature".to_owned()),
    });
    backlog.notes.push(n);

    let text = serde_json::to_string(&backlog).unwrap();
    let restored: Backlog = serde_json::from_str(&text).unwrap();
    assert_eq!(restored, backlog);
}

#[test]
fn attachment_path_refuses_an_id_with_a_path_separator() {
    let project = ProjectId::new("p1");
    assert!(attachment_path(&project, "../escape", "file.png").is_err());
}

#[test]
fn attachment_path_refuses_an_empty_id() {
    let project = ProjectId::new("p1");
    assert!(attachment_path(&project, "", "file.png").is_err());
}

#[test]
fn attachment_path_accepts_kets_own_base36_id_shape() {
    let project = ProjectId::new("p1");
    let generated = Note::new(&[]).id;
    // ket's own ids are base36 — alphanumeric — which is exactly what the
    // validation accepts; this only fails if that assumption ever drifts.
    assert!(attachment_path(&project, &generated, "file.png").is_ok());
}

// ---- file-backed operations, via the `_in(root, ...)` seam ------------------

#[test]
fn loading_a_backlog_that_was_never_saved_is_empty() {
    let sandbox = Sandbox::new("backlog-load-missing");
    let project = ProjectId::new("p1");
    let backlog = Backlog::load_in(&sandbox.path("root"), &project).unwrap();
    assert_eq!(backlog, Backlog::default());
}

#[test]
fn loading_a_malformed_file_is_an_error_not_a_silent_empty_backlog() {
    let sandbox = Sandbox::new("backlog-load-malformed");
    let root = sandbox.path("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("p1.json"), "not json").unwrap();

    let project = ProjectId::new("p1");
    assert!(Backlog::load_in(&root, &project).is_err());
}

#[test]
fn save_note_adds_a_new_note_at_the_top() {
    let sandbox = Sandbox::new("backlog-save-new");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");

    let first = note("first", "body");
    Backlog::save_note_in(&root, &project, &first).unwrap();
    // `note()` builds an id from the clock with no knowledge of `first`'s, so
    // two calls close enough together would otherwise collide and this would
    // edit the same note in place instead of adding a second one.
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = note("second", "body");
    let backlog = Backlog::save_note_in(&root, &project, &second).unwrap();

    assert_eq!(backlog.notes.len(), 2);
    assert_eq!(backlog.notes[0].title, "second", "newest goes on top");
    assert_eq!(backlog.notes[1].title, "first");
}

#[test]
fn save_note_on_an_existing_id_edits_it_in_place_rather_than_duplicating() {
    let sandbox = Sandbox::new("backlog-save-edit");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");

    let original = note("first", "body");
    Backlog::save_note_in(&root, &project, &original).unwrap();

    let mut edited = original.clone();
    edited.title = "edited".to_owned();
    let backlog = Backlog::save_note_in(&root, &project, &edited).unwrap();

    assert_eq!(backlog.notes.len(), 1);
    assert_eq!(backlog.notes[0].title, "edited");
}

#[test]
fn save_note_persists_across_a_reload() {
    let sandbox = Sandbox::new("backlog-persist");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");

    Backlog::save_note_in(&root, &project, &note("title", "body")).unwrap();
    let reloaded = Backlog::load_in(&root, &project).unwrap();

    assert_eq!(reloaded.notes.len(), 1);
    assert_eq!(reloaded.notes[0].title, "title");
}

#[test]
fn update_reads_the_latest_file_before_applying_the_change() {
    // Simulates two windows: one saves a note, then `update` from "another
    // window" must see it rather than clobbering it with a stale in-memory copy.
    let sandbox = Sandbox::new("backlog-update-fresh");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");

    Backlog::save_note_in(&root, &project, &note("existing", "body")).unwrap();
    let backlog = Backlog::update_in(&root, &project, |backlog| {
        backlog.notes.push(note("added by update", "body"));
    })
    .unwrap();

    assert_eq!(backlog.notes.len(), 2);
}

#[test]
fn remove_deletes_the_note_and_its_attachment_directory() {
    let sandbox = Sandbox::new("backlog-remove");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("to remove", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    let source = sandbox.path("source.png");
    fs::write(&source, b"fake image").unwrap();
    Backlog::attach_in(&root, &project, &id, &source).unwrap();
    assert!(root.join(project.as_str()).join(&id).is_dir());

    let backlog = Backlog::remove_in(&root, &project, &id).unwrap();
    assert!(backlog.notes.is_empty());
    assert!(!root.join(project.as_str()).join(&id).exists());
}

#[test]
fn removing_a_note_with_no_attachment_directory_is_not_an_error() {
    let sandbox = Sandbox::new("backlog-remove-bare");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("bare", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    let backlog = Backlog::remove_in(&root, &project, &id).unwrap();
    assert!(backlog.notes.is_empty());
}

#[test]
fn attach_copies_the_source_file_and_records_the_attachment() {
    let sandbox = Sandbox::new("backlog-attach");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("has files", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    let source = sandbox.path("shot.png");
    fs::write(&source, b"fake image").unwrap();
    let backlog = Backlog::attach_in(&root, &project, &id, &source).unwrap();

    assert_eq!(backlog.notes[0].attachments, vec!["shot.png".to_owned()]);
    let copied = ket_core::backlog::attachment_path_in(&root, &project, &id, "shot.png").unwrap();
    assert_eq!(fs::read(copied).unwrap(), b"fake image");
}

#[test]
fn attaching_a_second_file_of_the_same_name_gets_a_numbered_suffix() {
    let sandbox = Sandbox::new("backlog-attach-collision");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("has files", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    let source = sandbox.path("shot.png");
    fs::write(&source, b"one").unwrap();
    Backlog::attach_in(&root, &project, &id, &source).unwrap();
    fs::write(&source, b"two").unwrap();
    let backlog = Backlog::attach_in(&root, &project, &id, &source).unwrap();

    assert_eq!(
        backlog.notes[0].attachments,
        vec!["shot.png".to_owned(), "shot-2.png".to_owned()]
    );
}

#[test]
fn attach_refuses_a_source_with_no_file_name() {
    let sandbox = Sandbox::new("backlog-attach-no-name");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("has files", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    assert!(Backlog::attach_in(&root, &project, &id, std::path::Path::new("/")).is_err());
}

#[test]
fn detach_removes_the_record_and_deletes_the_file() {
    let sandbox = Sandbox::new("backlog-detach");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("has files", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();
    let source = sandbox.path("shot.png");
    fs::write(&source, b"fake image").unwrap();
    Backlog::attach_in(&root, &project, &id, &source).unwrap();

    let backlog = Backlog::detach_in(&root, &project, &id, "shot.png").unwrap();
    assert!(backlog.notes[0].attachments.is_empty());
    let path = ket_core::backlog::attachment_path_in(&root, &project, &id, "shot.png").unwrap();
    assert!(!path.exists());
}

#[test]
fn detaching_a_file_that_was_already_gone_is_not_an_error() {
    let sandbox = Sandbox::new("backlog-detach-missing");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("has files", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    let backlog = Backlog::detach_in(&root, &project, &id, "never-existed.png").unwrap();
    assert!(backlog.notes[0].attachments.is_empty());
}

#[test]
fn mark_done_records_where_the_note_went() {
    let sandbox = Sandbox::new("backlog-mark-done");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("to start", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();

    let done = Done {
        at_ms: 123,
        worktree: Some(WorktreeId::new("w1")),
        branch: Some("feature".to_owned()),
    };
    let backlog = Backlog::mark_done_in(&root, &project, &id, done.clone()).unwrap();

    assert_eq!(backlog.notes[0].done, Some(done));
    assert_eq!(backlog.open_count(), 0);
}

#[test]
fn reopen_clears_the_done_state() {
    let sandbox = Sandbox::new("backlog-reopen");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let n = note("to start", "body");
    let id = n.id.clone();
    Backlog::save_note_in(&root, &project, &n).unwrap();
    Backlog::mark_done_in(
        &root,
        &project,
        &id,
        Done {
            at_ms: 1,
            worktree: Some(WorktreeId::new("w1")),
            branch: Some("feature".to_owned()),
        },
    )
    .unwrap();

    let backlog = Backlog::reopen_in(&root, &project, &id).unwrap();
    assert_eq!(backlog.notes[0].done, None);
    assert_eq!(backlog.open_count(), 1);
}

#[test]
fn operations_on_different_projects_do_not_collide() {
    let sandbox = Sandbox::new("backlog-projects-isolated");
    let root = sandbox.path("root");
    let p1 = ProjectId::new("p1");
    let p2 = ProjectId::new("p2");

    Backlog::save_note_in(&root, &p1, &note("in p1", "body")).unwrap();
    let backlog_p2 = Backlog::load_in(&root, &p2).unwrap();

    assert!(backlog_p2.notes.is_empty(), "a sibling project stays empty");
    assert_eq!(Backlog::load_in(&root, &p1).unwrap().notes.len(), 1);
}

// ---- tags --------------------------------------------------------------------

#[test]
fn tags_are_trimmed_unhashed_and_joined_across_spaces() {
    assert_eq!(
        normalised_tags(["  #ui ", "needs review", "##relay"]),
        ["ui", "needs-review", "relay"]
    );
}

#[test]
fn tags_that_differ_only_in_case_are_one_tag_spelt_the_first_way() {
    assert_eq!(normalised_tags(["Phone", "phone", "PHONE"]), ["Phone"]);
}

#[test]
fn blank_tags_are_dropped() {
    assert!(normalised_tags(["", "   ", "#"]).is_empty());
}

#[test]
fn tags_are_capped_in_number_and_in_length() {
    let many: Vec<String> = (0..MAX_TAGS + 5).map(|n| format!("t{n}")).collect();
    assert_eq!(normalised_tags(&many).len(), MAX_TAGS);

    let long = "x".repeat(MAX_TAG_CHARS + 10);
    assert_eq!(normalised_tags([long])[0].chars().count(), MAX_TAG_CHARS);
}

// ---- search ------------------------------------------------------------------

fn tagged(title: &str, body: &str, tags: &[&str]) -> Note {
    Note {
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        ..note(title, body)
    }
}

#[test]
fn a_blank_search_matches_every_note() {
    let n = tagged("Anything", "", &[]);
    assert!(n.matches(""));
    assert!(n.matches("   "));
}

#[test]
fn every_word_of_a_search_has_to_be_found_ignoring_case() {
    let n = tagged("Relay drops phones", "after a host restart", &[]);
    assert!(n.matches("relay RESTART"));
    assert!(!n.matches("relay windows"));
}

#[test]
fn a_hash_word_matches_a_tag_exactly() {
    let n = tagged("Fix it", "", &["phone", "ui"]);
    assert!(n.matches("#phone"));
    assert!(n.matches("#PHONE"));
    assert!(!n.matches("#pho"));
    assert!(!n.matches("#windows"));
}

#[test]
fn a_plain_word_also_finds_part_of_a_tag() {
    assert!(tagged("Fix it", "", &["phone"]).matches("pho"));
}

#[test]
fn a_lone_hash_typed_on_the_way_to_a_tag_hides_nothing() {
    assert!(tagged("Fix it", "", &[]).matches("#"));
}

// ---- the brief ---------------------------------------------------------------

#[test]
fn the_brief_is_the_title_and_the_description_a_blank_line_apart() {
    assert_eq!(
        note("  Title ", "\nBody\n").brief().as_deref(),
        Some("Title\n\nBody")
    );
}

#[test]
fn a_brief_without_a_title_is_the_description_alone() {
    assert_eq!(note("", "Body").brief().as_deref(), Some("Body"));
}

#[test]
fn a_note_with_no_words_has_no_brief() {
    assert_eq!(note(" ", "\n").brief(), None);
}

// ---- the order notes are shown in --------------------------------------------

fn ranked(id: &str, priority: Priority, rank: f64, created_ms: u64) -> Note {
    Note {
        id: id.to_owned(),
        title: id.to_owned(),
        priority,
        rank,
        created_ms,
        updated_ms: created_ms,
        ..Note::default()
    }
}

fn ids(notes: &[&Note]) -> Vec<String> {
    notes.iter().map(|note| note.id.clone()).collect()
}

#[test]
fn open_notes_show_the_most_pressing_first_then_by_rank_then_newest() {
    let backlog = Backlog {
        notes: vec![
            ranked("low", Priority::Low, 0.0, 5),
            ranked("mediumlater", Priority::Medium, 1.0, 9),
            ranked("mediumfirst", Priority::Medium, 0.0, 1),
            ranked("urgent", Priority::Urgent, 5.0, 2),
            ranked("mediumtie", Priority::Medium, 1.0, 10),
        ],
    };
    assert_eq!(
        ids(&backlog.sorted_open()),
        ["urgent", "mediumfirst", "mediumtie", "mediumlater", "low"]
    );
}

#[test]
fn done_notes_are_left_out_of_the_open_order_and_listed_latest_first() {
    let finished = |id: &str, at_ms: u64| Note {
        done: Some(Done {
            at_ms,
            worktree: None,
            branch: None,
        }),
        ..ranked(id, Priority::Medium, 0.0, 1)
    };
    let backlog = Backlog {
        notes: vec![
            finished("first", 10),
            ranked("open", Priority::Low, 0.0, 1),
            finished("second", 20),
        ],
    };
    assert_eq!(ids(&backlog.sorted_open()), ["open"]);
    assert_eq!(ids(&backlog.sorted_done()), ["second", "first"]);
}

#[test]
fn a_new_note_goes_to_the_top_of_its_priority() {
    let sandbox = Sandbox::new("backlog-rank-new");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");

    let first = Note::new(&[]);
    Backlog::save_note_in(&root, &project, &note_like(&first, "first")).unwrap();
    let second = Note::new(std::slice::from_ref(&first));
    let backlog = Backlog::save_note_in(&root, &project, &note_like(&second, "second")).unwrap();

    assert_eq!(ids(&backlog.sorted_open()), [second.id, first.id]);
}

/// `id`'s note, with a title.
fn note_like(id: &Note, title: &str) -> Note {
    Note {
        title: title.to_owned(),
        ..id.clone()
    }
}

#[test]
fn saving_a_note_keeps_its_tags_tidy() {
    let sandbox = Sandbox::new("backlog-save-tags");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");

    let n = tagged("Tagged", "", &["#UI", "ui", " needs review "]);
    let backlog = Backlog::save_note_in(&root, &project, &n).unwrap();
    assert_eq!(backlog.notes[0].tags, ["UI", "needs-review"]);
}

#[test]
fn a_file_written_before_tags_and_ranks_still_loads() {
    let sandbox = Sandbox::new("backlog-old-file");
    let root = sandbox.path("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("p1.json"),
        r#"{"notes":[{"id":"old1","title":"Old","body":"b","attachments":[],"created_ms":5,"updated_ms":5}]}"#,
    )
    .unwrap();

    let backlog = Backlog::load_in(&root, &ProjectId::new("p1")).unwrap();
    let old = &backlog.notes[0];
    assert!(old.tags.is_empty());
    assert_eq!(old.rank, 0.0);
    assert_eq!(old.priority, Priority::Medium);
}

#[test]
fn a_note_without_tags_writes_no_tags_field() {
    let text = serde_json::to_string(&note("Plain", "")).unwrap();
    assert!(!text.contains("tags"), "{text}");
}

// ---- moving a note -------------------------------------------------------------

/// Three medium notes saved one after another, so they show newest first:
/// returned in the order they show.
fn three(root: &std::path::Path, project: &ProjectId) -> [String; 3] {
    let a = Note::new(&[]);
    let b = Note::new(std::slice::from_ref(&a));
    let c = Note::new(&[a.clone(), b.clone()]);
    for (n, title) in [(&a, "a"), (&b, "b"), (&c, "c")] {
        Backlog::save_note_in(root, project, &note_like(n, title)).unwrap();
    }
    [c.id, b.id, a.id]
}

fn shown(root: &std::path::Path, project: &ProjectId) -> Vec<String> {
    ids(&Backlog::load_in(root, project).unwrap().sorted_open())
}

#[test]
fn a_note_moves_to_just_before_another() {
    let sandbox = Sandbox::new("backlog-reorder-before");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let [c, b, a] = three(&root, &project);

    Backlog::reorder_in(&root, &project, &a, Some(&c)).unwrap();
    assert_eq!(shown(&root, &project), [a, c, b]);
}

#[test]
fn a_note_moved_before_nothing_goes_to_the_end_of_its_priority() {
    let sandbox = Sandbox::new("backlog-reorder-end");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let [c, b, a] = three(&root, &project);

    Backlog::reorder_in(&root, &project, &c, None).unwrap();
    assert_eq!(shown(&root, &project), [b, a, c]);
}

#[test]
fn a_note_moved_before_one_of_another_priority_takes_that_priority() {
    let sandbox = Sandbox::new("backlog-reorder-priority");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let [c, b, a] = three(&root, &project);
    let existing = Backlog::load_in(&root, &project).unwrap().notes;
    let urgent = Note {
        title: "urgent".to_owned(),
        priority: Priority::Urgent,
        ..Note::new(&existing)
    };
    Backlog::save_note_in(&root, &project, &urgent).unwrap();
    let before = Backlog::load_in(&root, &project).unwrap();
    let was = before.notes.iter().find(|n| n.id == a).unwrap().updated_ms;
    std::thread::sleep(std::time::Duration::from_millis(2));

    let backlog = Backlog::reorder_in(&root, &project, &a, Some(&urgent.id)).unwrap();
    let moved = backlog.notes.iter().find(|n| n.id == a).unwrap();
    assert_eq!(moved.priority, Priority::Urgent);
    assert!(moved.updated_ms > was, "a new priority is an edit");
    assert_eq!(shown(&root, &project), [a, urgent.id, c, b]);
}

#[test]
fn a_move_within_its_priority_is_not_an_edit() {
    let sandbox = Sandbox::new("backlog-reorder-quiet");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let [c, _, a] = three(&root, &project);
    let was = Backlog::load_in(&root, &project)
        .unwrap()
        .notes
        .into_iter()
        .find(|n| n.id == a)
        .unwrap()
        .updated_ms;

    let backlog = Backlog::reorder_in(&root, &project, &a, Some(&c)).unwrap();
    let moved = backlog.notes.iter().find(|n| n.id == a).unwrap();
    assert_eq!(moved.updated_ms, was);
}

#[test]
fn a_done_note_or_an_unknown_one_cannot_be_moved() {
    let sandbox = Sandbox::new("backlog-reorder-refused");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    let [c, b, _] = three(&root, &project);
    Backlog::mark_done_in(&root, &project, &b, Done::by_hand()).unwrap();

    assert!(Backlog::reorder_in(&root, &project, &b, Some(&c)).is_err());
    assert!(Backlog::reorder_in(&root, &project, "nope", Some(&c)).is_err());
    assert!(
        Backlog::reorder_in(&root, &project, &c, Some(&b)).is_err(),
        "nor dropped before a done note"
    );
}

#[test]
fn notes_from_before_ranks_are_numbered_afresh_when_one_moves_between_two() {
    let sandbox = Sandbox::new("backlog-reorder-renumber");
    let root = sandbox.path("root");
    let project = ProjectId::new("p1");
    // Every rank 0, as a file written before ranks existed reads: shown
    // newest first.
    Backlog::update_in(&root, &project, |backlog| {
        backlog.notes = vec![
            ranked("n3", Priority::Medium, 0.0, 3),
            ranked("n2", Priority::Medium, 0.0, 2),
            ranked("n1", Priority::Medium, 0.0, 1),
        ];
    })
    .unwrap();
    assert_eq!(shown(&root, &project), ["n3", "n2", "n1"]);

    let backlog = Backlog::reorder_in(&root, &project, "n1", Some("n2")).unwrap();
    assert_eq!(shown(&root, &project), ["n3", "n1", "n2"]);
    let mut ranks: Vec<f64> = backlog.notes.iter().map(|n| n.rank).collect();
    ranks.sort_by(f64::total_cmp);
    assert_eq!(ranks, [0.0, 1.0, 2.0]);
}
