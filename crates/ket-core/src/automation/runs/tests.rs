//! What is kept of a run's record, and for how long.

use super::*;

#[test]
fn old_runs_go_but_a_live_one_stays() {
    let now = 400 * 24 * 60 * 60 * 1000;
    let day_ms = 24 * 60 * 60 * 1000;
    let run = |at_ms: u64, outcome: Outcome| Run {
        automation: "a".to_owned(),
        due_ms: at_ms,
        at_ms,
        outcome,
        worktree: None,
        branch: None,
        ended_ms: None,
        tokens: None,
        command: None,
    };
    let mut runs = Runs::default();
    runs.record(run(now - 200 * day_ms, Outcome::Started));
    runs.record(run(now - 100 * day_ms, Outcome::Done));
    runs.record(run(now - day_ms, Outcome::Done));
    runs.prune(now);
    assert_eq!(runs.runs.len(), 2, "the run past 90 days goes");
    assert!(runs.runs.iter().any(|run| run.outcome.is_live()));

    let mut many = Runs::default();
    for n in 0..(KEEP_EACH as u64 + 10) {
        many.record(run(now - n, Outcome::Done));
    }
    many.prune(now);
    assert_eq!(many.runs.len(), KEEP_EACH);
}

#[test]
fn a_run_ends_once_and_only_while_it_is_going() {
    let started = |due_ms| Run {
        automation: "a".to_owned(),
        due_ms,
        at_ms: due_ms,
        outcome: Outcome::Started,
        worktree: None,
        branch: None,
        ended_ms: None,
        tokens: None,
        command: None,
    };
    let mut runs = Runs::default();
    runs.record(started(1));
    runs.record(started(2));
    let failed = Outcome::Failed {
        why: "exited 1".to_owned(),
    };
    assert!(runs.end("a", 1, failed.clone(), 10));
    assert!(!runs.end("a", 1, Outcome::Done, 11), "already ended");
    assert!(!runs.end("a", 9, Outcome::Done, 11), "no such run");
    let first = runs.runs.iter().find(|run| run.due_ms == 1).unwrap();
    assert_eq!(first.outcome, failed);
    assert_eq!(first.ended_ms, Some(10));
    assert!(runs.live("a").is_some_and(|run| run.due_ms == 2));
}
