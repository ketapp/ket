//! What a worktree's `Activity` means to the eye (`Signal`), and how
//! `Tracker` reconciles every source: an agent's own hook reports (via
//! `Tracker::report`, built on the same `StatusStore` `tests/agent_status.rs`
//! covers directly), driven sessions, an observed terminal foreground, and
//! what is recorded on disk.

use ket_core::activity::{Activity, Foreground, REPORT_LINGERS_MS, Signal, Tracker, roll_up};
use ket_core::agent::{Session, SessionStatus};
use ket_core::agent_hooks::{HookEvent, HookReport};
use ket_core::event::{AgentState, SessionOutcome};
use ket_core::id::{ProjectId, SessionId, WorktreeId};
use ket_core::sessions::DiscoveredSession;
use ket_core::subagents::{Subagent, SubagentState};

fn hook_report(event: HookEvent, pane: &str, worktree: &str, at_ms: u64) -> HookReport {
    HookReport {
        agent: "claude".to_owned(),
        event,
        worktree: Some(worktree.to_owned()),
        cwd: None,
        pane: Some(pane.to_owned()),
        session: Some("s1".to_owned()),
        tool_name: None,
        note: None,
        mutating_git: false,
        at_ms,
        subagent: None,
        subagent_type: None,
        subagent_description: None,
        subagent_model: None,
        tool_use_id: None,
        call: None,
        teammate: None,
        fresh_session: false,
        background: None,
        question: None,
    }
}

fn known_any(_: &str) -> bool {
    true
}

fn worktree(name: &str) -> WorktreeId {
    WorktreeId::new(name)
}

fn session(worktree: &WorktreeId, status: SessionStatus, started_at_ms: u64) -> Session {
    Session {
        id: SessionId::new("s1"),
        project_id: ProjectId::new("p1"),
        worktree_id: worktree.clone(),
        agent: "claude".to_owned(),
        status,
        driver_pid: 1,
        started_at_ms,
        heartbeat_ms: started_at_ms,
    }
}

fn discovered(agent: &str, id: &str, title: Option<&str>, updated_at_ms: u64) -> DiscoveredSession {
    DiscoveredSession {
        agent: agent.to_owned(),
        id: id.to_owned(),
        title: title.map(str::to_owned),
        explicit_title: None,
        directory: std::path::PathBuf::from("/wherever"),
        updated_at_ms,
    }
}

// ---- Activity::signal / helpers ---------------------------------------------

#[test]
fn idle_and_recorded_are_quiet() {
    assert_eq!(Activity::Idle.signal(), Signal::Quiet);
    assert_eq!(
        Activity::Recorded {
            agent: "claude".to_owned(),
            session: "s1".to_owned(),
            title: None,
            updated_at_ms: 0,
        }
        .signal(),
        Signal::Quiet
    );
}

#[test]
fn a_mutating_busy_command_is_merging_others_are_running() {
    assert_eq!(
        Activity::Busy {
            command: "committing".to_owned(),
            mutating: true,
        }
        .signal(),
        Signal::Merging
    );
    assert_eq!(
        Activity::Busy {
            command: "cargo test".to_owned(),
            mutating: false,
        }
        .signal(),
        Signal::Running
    );
    assert_eq!(
        Activity::Observed {
            agent: "claude".to_owned()
        }
        .signal(),
        Signal::Running
    );
}

#[test]
fn a_session_executing_a_mutating_git_tool_is_merging() {
    let session = Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state: AgentState::ExecutingTool,
        note: None,
        mutating_git: true,
    };
    assert_eq!(session.signal(), Signal::Merging);
}

#[test]
fn session_states_map_to_the_right_signal() {
    let with = |state| Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state,
        note: None,
        mutating_git: false,
    };

    assert_eq!(with(AgentState::Idle).signal(), Signal::Quiet);
    assert_eq!(with(AgentState::Starting).signal(), Signal::Running);
    assert_eq!(with(AgentState::Authenticating).signal(), Signal::Running);
    assert_eq!(with(AgentState::Thinking).signal(), Signal::Working);
    assert_eq!(with(AgentState::ExecutingTool).signal(), Signal::Working);
    assert_eq!(
        with(AgentState::AwaitingPermission).signal(),
        Signal::Blocked
    );
}

#[test]
fn ended_signal_depends_on_the_outcome() {
    let with = |outcome| Activity::Ended {
        agent: "claude".to_owned(),
        outcome,
    };
    assert_eq!(
        with(SessionOutcome::Failed {
            why: "boom".to_owned()
        })
        .signal(),
        Signal::Failed
    );
    assert_eq!(with(SessionOutcome::Cancelled).signal(), Signal::Quiet);
    assert_eq!(with(SessionOutcome::Completed).signal(), Signal::Quiet);
}

#[test]
fn is_working_is_true_for_anything_with_a_live_process() {
    assert!(!Activity::Idle.is_working());
    assert!(
        Activity::Session {
            session_id: SessionId::new("s1"),
            agent: "claude".to_owned(),
            state: AgentState::Idle,
            note: None,
            mutating_git: false,
        }
        .is_working()
    );
    assert!(
        Activity::Observed {
            agent: "claude".to_owned()
        }
        .is_working()
    );
    assert!(
        Activity::Busy {
            command: "cargo test".to_owned(),
            mutating: false,
        }
        .is_working()
    );
    assert!(
        !Activity::Recorded {
            agent: "claude".to_owned(),
            session: "s1".to_owned(),
            title: None,
            updated_at_ms: 0,
        }
        .is_working()
    );
}

#[test]
fn needs_attention_is_only_true_while_awaiting_permission() {
    let awaiting = Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state: AgentState::AwaitingPermission,
        note: None,
        mutating_git: false,
    };
    assert!(awaiting.needs_attention());

    let thinking = Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state: AgentState::Thinking,
        note: None,
        mutating_git: false,
    };
    assert!(!thinking.needs_attention());
    assert!(!Activity::Idle.needs_attention());
}

#[test]
fn failed_is_only_true_for_a_failed_outcome() {
    assert!(
        Activity::Ended {
            agent: "claude".to_owned(),
            outcome: SessionOutcome::Failed {
                why: "boom".to_owned()
            },
        }
        .failed()
    );
    assert!(
        !Activity::Ended {
            agent: "claude".to_owned(),
            outcome: SessionOutcome::Completed,
        }
        .failed()
    );
    assert!(!Activity::Idle.failed());
}

#[test]
fn only_a_session_is_controllable() {
    assert!(
        Activity::Session {
            session_id: SessionId::new("s1"),
            agent: "claude".to_owned(),
            state: AgentState::Idle,
            note: None,
            mutating_git: false,
        }
        .is_controllable()
    );
    assert!(
        !Activity::Observed {
            agent: "claude".to_owned()
        }
        .is_controllable()
    );
    assert!(!Activity::Idle.is_controllable());
}

#[test]
fn agent_name_is_available_where_there_is_one() {
    assert_eq!(
        Activity::Observed {
            agent: "codex".to_owned()
        }
        .agent(),
        Some("codex")
    );
    assert_eq!(Activity::Idle.agent(), None);
    assert_eq!(
        Activity::Busy {
            command: "cargo".to_owned(),
            mutating: false
        }
        .agent(),
        None
    );
}

#[test]
fn detail_prefers_the_agents_own_note_for_tool_and_idle_states() {
    let with_note = Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state: AgentState::ExecutingTool,
        note: Some("running the test suite".to_owned()),
        mutating_git: false,
    };
    assert_eq!(with_note.detail(), "running the test suite");

    let thinking_with_note = Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state: AgentState::Thinking,
        // A note is only honoured for ExecutingTool and Idle.
        note: Some("ignored".to_owned()),
        mutating_git: false,
    };
    assert_eq!(thinking_with_note.detail(), "thinking");
}

#[test]
fn label_combines_the_agent_and_the_detail() {
    let session = Activity::Session {
        session_id: SessionId::new("s1"),
        agent: "claude".to_owned(),
        state: AgentState::Thinking,
        note: None,
        mutating_git: false,
    };
    assert_eq!(session.label(), "claude · thinking");

    assert_eq!(Activity::Idle.label(), "");
    assert_eq!(
        Activity::Ended {
            agent: "codex".to_owned(),
            outcome: SessionOutcome::Failed {
                why: "boom".to_owned()
            },
        }
        .label(),
        "codex · failed"
    );
}

// ---- roll_up -----------------------------------------------------------------

fn subagent(state: SubagentState) -> Subagent {
    Subagent {
        id: "sub1".to_owned(),
        label: "doing something".to_owned(),
        model: None,
        state,
        started_at_ms: 0,
        pane: None,
    }
}

#[test]
fn a_blocked_subagent_always_wins() {
    let subs = vec![subagent(SubagentState::Blocked)];
    assert_eq!(roll_up(Signal::Quiet, &subs), Signal::Blocked);
    assert_eq!(roll_up(Signal::Working, &subs), Signal::Blocked);
}

#[test]
fn a_working_subagent_raises_a_quiet_or_running_lead_but_not_others() {
    let subs = vec![subagent(SubagentState::Working)];
    assert_eq!(roll_up(Signal::Quiet, &subs), Signal::Working);
    assert_eq!(roll_up(Signal::Running, &subs), Signal::Working);
    // A lead already working, merging or failed keeps its own signal.
    assert_eq!(roll_up(Signal::Merging, &subs), Signal::Merging);
    assert_eq!(roll_up(Signal::Failed, &subs), Signal::Failed);
}

#[test]
fn an_idle_subagent_changes_nothing() {
    let subs = vec![subagent(SubagentState::Idle)];
    assert_eq!(roll_up(Signal::Quiet, &subs), Signal::Quiet);
}

#[test]
fn no_subagents_leaves_the_lead_alone() {
    assert_eq!(roll_up(Signal::Quiet, &[]), Signal::Quiet);
    assert_eq!(roll_up(Signal::Working, &[]), Signal::Working);
}

// ---- Tracker::inferred / activity / signal (non-report sources) -------------

#[test]
fn an_untouched_worktree_is_idle() {
    let tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    assert_eq!(tracker.activity(&wt, 0), Activity::Idle);
    assert_eq!(tracker.signal(&wt, 0), Signal::Quiet);
}

#[test]
fn a_live_driven_session_beats_everything_else() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.sync_sessions([session(&wt, SessionStatus::Live(AgentState::Thinking), 0)]);
    tracker.observe(&wt, Some(Foreground::Shell));
    tracker.record(
        &wt,
        Some(discovered("claude", "old-session", Some("old"), 0)),
    );

    let activity = tracker.activity(&wt, 0);
    assert!(matches!(
        activity,
        Activity::Session {
            state: AgentState::Thinking,
            ..
        }
    ));
}

#[test]
fn a_recently_ended_session_is_reported_as_ended() {
    use ket_core::activity::ENDED_LINGERS_MS;

    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.sync_sessions([session(
        &wt,
        SessionStatus::Ended(SessionOutcome::Completed),
        1_000,
    )]);

    let activity = tracker.activity(&wt, 1_000 + ENDED_LINGERS_MS - 1);
    assert_eq!(
        activity,
        Activity::Ended {
            agent: "claude".to_owned(),
            outcome: SessionOutcome::Completed,
        }
    );
}

#[test]
fn an_ended_session_stops_being_news_once_it_has_lingered_long_enough() {
    use ket_core::activity::ENDED_LINGERS_MS;

    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.sync_sessions([session(
        &wt,
        SessionStatus::Ended(SessionOutcome::Completed),
        1_000,
    )]);

    let activity = tracker.activity(&wt, 1_000 + ENDED_LINGERS_MS);
    assert_eq!(activity, Activity::Idle);
}

#[test]
fn an_observed_agent_in_the_foreground_is_reported_as_observed() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(&wt, Some(Foreground::Running("claude".to_owned())));

    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Observed {
            agent: "claude".to_owned()
        }
    );
}

#[test]
fn an_agent_run_via_a_full_path_or_npm_wrapper_is_still_recognised() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(
        &wt,
        Some(Foreground::Running(
            "node /usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js".to_owned(),
        )),
    );

    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Observed {
            agent: "claude".to_owned()
        }
    );
}

#[test]
fn a_git_command_that_mutates_the_worktree_is_reported_by_its_verb() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(&wt, Some(Foreground::Running("git rebase main".to_owned())));

    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Busy {
            command: "rebasing".to_owned(),
            mutating: true,
        }
    );
}

#[test]
fn a_read_only_git_command_is_not_mutating() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(&wt, Some(Foreground::Running("git status".to_owned())));

    // `status` is not a recognised mutating verb, so this falls back to the
    // plain command name rather than a git-specific phrase.
    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Busy {
            command: "git".to_owned(),
            mutating: false,
        }
    );
}

#[test]
fn an_unrecognised_foreground_command_falls_back_to_its_short_name() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(
        &wt,
        Some(Foreground::Running(
            "/opt/homebrew/bin/cargo test --all".to_owned(),
        )),
    );

    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Busy {
            command: "cargo".to_owned(),
            mutating: false,
        }
    );
}

#[test]
fn a_shell_at_its_prompt_falls_back_to_a_recorded_session() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(&wt, Some(Foreground::Shell));
    tracker.record(
        &wt,
        Some(discovered("claude", "sess-1", Some("fix the bug"), 42)),
    );

    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Recorded {
            agent: "claude".to_owned(),
            session: "sess-1".to_owned(),
            title: Some("fix the bug".to_owned()),
            updated_at_ms: 42,
        }
    );
}

#[test]
fn no_terminal_at_all_also_falls_back_to_a_recorded_session() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.record(&wt, Some(discovered("claude", "sess-1", None, 42)));

    assert_eq!(
        tracker.activity(&wt, 0),
        Activity::Recorded {
            agent: "claude".to_owned(),
            session: "sess-1".to_owned(),
            title: None,
            updated_at_ms: 42,
        }
    );
}

#[test]
fn clearing_the_observed_foreground_removes_it() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(&wt, Some(Foreground::Running("claude".to_owned())));
    assert!(matches!(
        tracker.activity(&wt, 0),
        Activity::Observed { .. }
    ));

    tracker.observe(&wt, None);
    assert_eq!(tracker.activity(&wt, 0), Activity::Idle);
}

#[test]
fn clearing_a_recorded_session_with_none_removes_it() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.record(&wt, Some(discovered("claude", "sess-1", None, 1)));
    assert!(matches!(
        tracker.activity(&wt, 0),
        Activity::Recorded { .. }
    ));

    tracker.record(&wt, None);
    assert_eq!(tracker.activity(&wt, 0), Activity::Idle);
}

#[test]
fn sync_sessions_keeps_the_most_recently_started_per_worktree() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");

    let mut older = session(&wt, SessionStatus::Live(AgentState::Idle), 100);
    older.agent = "codex".to_owned();
    let newer = session(&wt, SessionStatus::Live(AgentState::Thinking), 200);

    // Order in the iterator must not matter: newest-started wins either way.
    tracker.sync_sessions([newer.clone(), older]);
    let activity = tracker.activity(&wt, 0);
    assert!(matches!(
        activity,
        Activity::Session {
            agent,
            state: AgentState::Thinking,
            ..
        } if agent == "claude"
    ));
}

#[test]
fn sync_sessions_replaces_the_whole_set() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt1 = worktree("w1");
    let wt2 = worktree("w2");

    tracker.sync_sessions([session(&wt1, SessionStatus::Live(AgentState::Thinking), 0)]);
    assert!(matches!(
        tracker.activity(&wt1, 0),
        Activity::Session { .. }
    ));

    // A second sync that mentions only wt2 must clear wt1's entry, not merge.
    tracker.sync_sessions([session(&wt2, SessionStatus::Live(AgentState::Thinking), 0)]);
    assert_eq!(tracker.activity(&wt1, 0), Activity::Idle);
    assert!(matches!(
        tracker.activity(&wt2, 0),
        Activity::Session { .. }
    ));
}

#[test]
fn sync_records_replaces_the_whole_set_and_marks_it_read() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt1 = worktree("w1");
    let wt2 = worktree("w2");

    assert!(tracker.wants_records(0));
    tracker.sync_records(
        1_000,
        [
            (wt1.clone(), discovered("claude", "s1", None, 1)),
            (wt2.clone(), discovered("claude", "s2", None, 2)),
        ],
    );
    assert!(matches!(
        tracker.activity(&wt1, 0),
        Activity::Recorded { .. }
    ));
    assert!(matches!(
        tracker.activity(&wt2, 0),
        Activity::Recorded { .. }
    ));

    // A later sync that mentions only wt1 clears wt2's record.
    tracker.sync_records(2_000, [(wt1.clone(), discovered("claude", "s1", None, 1))]);
    assert!(matches!(
        tracker.activity(&wt1, 0),
        Activity::Recorded { .. }
    ));
    assert_eq!(tracker.activity(&wt2, 0), Activity::Idle);
}

#[test]
fn wants_records_respects_the_poll_interval() {
    use ket_core::activity::RECORDS_EVERY_MS;

    let mut tracker = Tracker::new(["claude".to_owned()]);
    assert!(tracker.wants_records(0), "never read before, so due now");

    tracker.sync_records(1_000, []);
    assert!(!tracker.wants_records(1_000 + RECORDS_EVERY_MS - 1));
    assert!(tracker.wants_records(1_000 + RECORDS_EVERY_MS));
}

#[test]
fn working_lists_only_non_idle_worktrees_sorted_by_id() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let busy = worktree("zzz-busy");
    let idle = worktree("aaa-idle-but-recorded-as-nothing");
    let recorded = worktree("mmm-recorded");

    tracker.observe(&busy, Some(Foreground::Running("claude".to_owned())));
    tracker.observe(&idle, Some(Foreground::Shell));
    tracker.record(&recorded, Some(discovered("claude", "s1", None, 1)));

    let working = tracker.working(0);
    let ids: Vec<&str> = working.iter().map(|(id, _)| id.as_str()).collect();

    assert!(!ids.contains(&idle.as_str()), "idle worktrees are excluded");
    assert_eq!(ids, vec![recorded.as_str(), busy.as_str()], "sorted by id");
}

#[test]
fn signal_falls_back_to_inferred_when_no_pane_has_reported() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");
    tracker.observe(&wt, Some(Foreground::Running("git commit".to_owned())));

    assert_eq!(tracker.signal(&wt, 0), Signal::Merging);
}

// ---- Tracker::report (the agent-self-report path) ---------------------------

#[test]
fn a_reported_pane_beats_a_driven_session() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");

    tracker.sync_sessions([session(&wt, SessionStatus::Live(AgentState::Idle), 0)]);
    let changed = tracker.report(
        &hook_report(HookEvent::UserPrompt, "p1", "w1", 0),
        known_any,
    );
    assert!(changed);

    let activity = tracker.activity(&wt, 0);
    assert!(
        matches!(
            &activity,
            Activity::Session { session_id, state: AgentState::Thinking, .. }
                if session_id.as_str() == "hook:w1"
        ),
        "an agent reporting on itself beats the driven session it started as: {activity:?}"
    );
}

#[test]
fn pane_is_none_until_something_reports_and_some_once_it_has() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    assert!(tracker.pane("p1", 0).is_none());

    tracker.report(
        &hook_report(HookEvent::UserPrompt, "p1", "w1", 0),
        known_any,
    );
    let (activity, signal) = tracker.pane("p1", 0).expect("pane reported");
    assert_eq!(signal, Signal::Working);
    assert!(matches!(
        activity,
        Activity::Session {
            state: AgentState::Thinking,
            ..
        }
    ));
}

#[test]
fn mutating_git_on_a_report_earns_the_merging_signal() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");

    let mut r = hook_report(HookEvent::PreTool, "p1", "w1", 0);
    r.mutating_git = true;
    tracker.report(&r, known_any);

    assert_eq!(tracker.signal(&wt, 0), Signal::Merging);
    assert!(matches!(
        tracker.activity(&wt, 0),
        Activity::Session {
            state: AgentState::ExecutingTool,
            mutating_git: true,
            ..
        }
    ));
}

#[test]
fn an_idle_reports_renewal_needs_the_same_agent_still_in_the_foreground() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");

    tracker.report(&hook_report(HookEvent::Stop, "p1", "w1", 0), known_any);
    assert!(matches!(
        tracker.activity(&wt, 0),
        Activity::Session {
            state: AgentState::Idle,
            ..
        }
    ));

    let far_later = REPORT_LINGERS_MS + 1;

    // No foreground evidence: the linger has run out, so the row falls back
    // to whatever the other sources know, which here is nothing.
    assert_eq!(tracker.activity(&wt, far_later), Activity::Idle);

    // The same agent is still sitting in that worktree's terminal: the report
    // is renewed rather than discarded.
    tracker.observe(&wt, Some(Foreground::Running("claude".to_owned())));
    assert!(matches!(
        tracker.activity(&wt, far_later),
        Activity::Session {
            state: AgentState::Idle,
            ..
        }
    ));

    // A different agent now owns that terminal: the old report is not about
    // it and must not be renewed. `inferred` then reads the foreground fresh,
    // the same as it would for any agent ket never heard a report from.
    tracker.observe(&wt, Some(Foreground::Running("codex".to_owned())));
    assert_eq!(
        tracker.activity(&wt, far_later),
        Activity::Busy {
            command: "codex".to_owned(),
            mutating: false,
        }
    );
}

#[test]
fn subagents_filed_under_a_reported_pane_are_readable_and_roll_up_the_signal() {
    let mut tracker = Tracker::new(["claude".to_owned()]);
    let wt = worktree("w1");

    let mut child = hook_report(HookEvent::SubagentStart, "p1", "w1", 0);
    child.subagent = Some("child-1".to_owned());
    child.subagent_type = Some("Explore".to_owned());
    tracker.report(&child, known_any);

    let subs = tracker.subagents(&wt, 0);
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].id, "child-1");

    // The lead goes quiet, but its `Stop` names the subagent as still running
    // in its background-task inventory — the one way a `Stop` does not clear
    // it (an inventory-less `Stop`, which is what Codex always sends, clears
    // every child on the assumption that nothing vouches for them anymore).
    let mut stop = hook_report(HookEvent::Stop, "p1", "w1", 1);
    stop.background = Some(vec![ket_core::agent_hooks::BackgroundTask {
        id: "child-1".to_owned(),
        agent_type: Some("Explore".to_owned()),
        description: None,
        running: true,
        teammate: false,
    }]);
    tracker.report(&stop, known_any);
    assert!(matches!(
        tracker.activity(&wt, 1),
        Activity::Session {
            state: AgentState::Idle,
            ..
        }
    ));
    assert_eq!(
        tracker.subagents(&wt, 1).len(),
        1,
        "the subagent survives its lead's Stop"
    );

    // A quiet lead with a working subagent still reads as working: only
    // `roll_up` explains a row that is both `Idle` and `Signal::Working`.
    assert_eq!(tracker.signal(&wt, 1), Signal::Working);
}
