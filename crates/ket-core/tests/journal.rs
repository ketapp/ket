//! The event journal — how one `ket` process sees another's events.
//!
//! The bus is a `tokio::sync::broadcast` channel, which never leaves the
//! process that owns it. Everything here exists because `ket events --follow`
//! runs somewhere else entirely.

use std::fs;

use ket_core::event::{DEFAULT_CAPACITY, Event, EventBus};
use ket_core::id::WorktreeId;
use ket_core::journal::{Journal, MAX_JOURNAL_BYTES};

mod common;
use common::Sandbox;

/// An event cheap to construct and easy to recognise on the way out.
fn event(name: &str) -> Event {
    Event::WorktreeRemoved {
        worktree_id: WorktreeId::new(name),
    }
}

#[test]
fn a_journalled_bus_writes_what_it_publishes() {
    let sandbox = Sandbox::new("journal-write");
    let path = sandbox.path("events.jsonl");

    let bus = EventBus::with_journal(DEFAULT_CAPACITY, Journal::at(&path));
    bus.publish(event("one"));
    bus.publish(event("two"));

    let read = Journal::at(&path).read_all();
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].seq, 1);
    assert_eq!(read[1].seq, 2);
}

#[test]
fn a_bus_without_a_journal_writes_nothing() {
    let sandbox = Sandbox::new("journal-none");
    let path = sandbox.path("events.jsonl");

    let bus = EventBus::new(DEFAULT_CAPACITY);
    bus.publish(event("one"));

    // Tests and sandboxed workspaces must never touch the real journal, which
    // is why journalling is opt-in rather than the default.
    assert!(!path.exists());
}

#[test]
fn one_journal_carries_events_from_separate_buses() {
    let sandbox = Sandbox::new("journal-shared");
    let path = sandbox.path("events.jsonl");

    // Two buses on one file stand in for two `ket` processes: a `ket run` in
    // one terminal and another somewhere else, both feeding one `--follow`.
    let first = EventBus::with_journal(DEFAULT_CAPACITY, Journal::at(&path));
    let second = EventBus::with_journal(DEFAULT_CAPACITY, Journal::at(&path));

    first.publish(event("from-first"));
    second.publish(event("from-second"));
    first.publish(event("from-first-again"));

    let read = Journal::at(&path).read_all();
    assert_eq!(read.len(), 3);

    // Append order is preserved, and sequence numbers are not an ordering
    // across processes — both buses start at 1.
    assert_eq!(read[0].seq, 1);
    assert_eq!(read[1].seq, 1);
    assert_eq!(read[2].seq, 2);
}

#[test]
fn a_partial_final_line_does_not_cost_the_rest() {
    let sandbox = Sandbox::new("journal-partial");
    let path = sandbox.path("events.jsonl");

    let bus = EventBus::with_journal(DEFAULT_CAPACITY, Journal::at(&path));
    bus.publish(event("complete"));

    // Something is mid-append. Reading must not throw away the good lines.
    let mut text = fs::read_to_string(&path).expect("read");
    text.push_str("{\"seq\":2,\"atMs\":1,\"type\":\"worktr");
    fs::write(&path, text).expect("write");

    let read = Journal::at(&path).read_all();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].seq, 1);
}

#[test]
fn an_unparseable_line_is_skipped_rather_than_fatal() {
    let sandbox = Sandbox::new("journal-garbage");
    let path = sandbox.path("events.jsonl");

    fs::write(&path, "not json at all\n").expect("write");
    let bus = EventBus::with_journal(DEFAULT_CAPACITY, Journal::at(&path));
    bus.publish(event("after-the-garbage"));

    let read = Journal::at(&path).read_all();
    assert_eq!(read.len(), 1);
}

#[test]
fn the_journal_rotates_once_it_outgrows_its_bound() {
    let sandbox = Sandbox::new("journal-rotate");
    let path = sandbox.path("events.jsonl");
    let journal = Journal::at(&path);

    // Pre-fill rather than publishing 8 MiB of events one at a time.
    fs::write(&path, vec![b'\n'; (MAX_JOURNAL_BYTES + 1) as usize]).expect("fill");

    let bus = EventBus::with_journal(DEFAULT_CAPACITY, journal.clone());
    bus.publish(event("the-straw"));

    // The oversized generation is set aside and a fresh file starts, so the
    // journal cannot grow for as long as ket is ever used.
    assert!(journal.rotated_path().exists(), "previous generation kept");
    let current = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    assert!(
        current < MAX_JOURNAL_BYTES,
        "current file is {current} bytes"
    );
}

#[test]
fn a_journal_in_a_directory_that_does_not_exist_yet_is_created() {
    let sandbox = Sandbox::new("journal-mkdir");
    let path = sandbox.path("nested/deeper/events.jsonl");

    let bus = EventBus::with_journal(DEFAULT_CAPACITY, Journal::at(&path));
    bus.publish(event("one"));

    assert_eq!(Journal::at(&path).read_all().len(), 1);
}

#[test]
fn reading_a_journal_that_does_not_exist_is_empty_rather_than_an_error() {
    let sandbox = Sandbox::new("journal-missing");
    let journal = Journal::at(sandbox.path("never-written.jsonl"));

    // `ket events` on a machine where nothing has run yet is an ordinary state,
    // not a failure.
    assert!(journal.read_all().is_empty());
}
