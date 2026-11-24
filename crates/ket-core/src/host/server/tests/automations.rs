//! The host's clock for automations, against a throwaway state file and
//! automations directory: a start due goes to a window once, and is recorded;
//! one with no window waits; a missed one, a busy one and one whose project
//! has gone are recorded as such.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::*;
use crate::automation::{Automation, Automations, Clock as Time, Outcome, Run, Runs, Task, When};
use crate::host::server::housekeeping::automations::Clock;
use crate::id::ProjectId;
use crate::project::Project;

/// A directory of its own for one test: an automations directory, a state
/// file naming one project, and that project's (empty) repository.
struct Place {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    project: ProjectId,
}

impl Place {
    fn new(tag: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("ket-host-automations-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(init.success());
        let state = base.join("state.json");
        let project = ProjectId::new("proj");
        crate::store::Store::at(state.clone())
            .update(|state| {
                state.projects.push(Project {
                    id: project.clone(),
                    name: "proj".to_owned(),
                    root: repo.clone(),
                    default_base: "main".to_owned(),
                    preferred_agent: None,
                    last_opened_ms: 0,
                });
                Ok(())
            })
            .unwrap();
        Self {
            root: base.join("automations"),
            base,
            state,
            project,
        }
    }

    fn clock(&self) -> Clock {
        Clock::at(Some(self.root.clone()), Some(self.state.clone()))
    }

    /// Saves `automation`, made an hour ago.
    fn add(&self, mut automation: Automation) -> String {
        automation.created_ms = crate::now_ms() - 60 * 60 * 1000;
        let id = automation.id.clone();
        Automations::update_in(&self.root, |all| {
            all.automations.push(automation);
            Ok(())
        })
        .unwrap();
        id
    }

    fn runs(&self, id: &str) -> Vec<Run> {
        Runs::load_in(&self.root).unwrap().of(id).cloned().collect()
    }

    fn handled(&self, id: &str) -> bool {
        Automations::load_in(&self.root)
            .unwrap()
            .get(id)
            .unwrap()
            .handled_ms
            .is_some()
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Every day at the minute it is `ago` before now, on this Mac's clock.
fn daily_at(ago: SignedDuration) -> When {
    let at = Timestamp::now()
        .checked_sub(ago)
        .unwrap()
        .to_zoned(TimeZone::system());
    let time = Time::new(
        u8::try_from(at.hour()).unwrap(),
        u8::try_from(at.minute()).unwrap(),
    )
    .unwrap();
    When::Daily { time }
}

fn prompt(text: &str) -> Task {
    Task::Prompt {
        text: text.to_owned(),
        agent: Some("claude".to_owned()),
        model: None,
        effort: None,
    }
}

fn nothing_more(reader: &mut UnixStream) {
    reader
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    assert!(wire::read(reader).is_err(), "nothing more is sent");
}

#[test]
fn a_start_due_goes_to_a_window_once_and_is_recorded() {
    let place = Place::new("due");
    let automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit dependencies",
        prompt("Audit the dependencies."),
        daily_at(SignedDuration::ZERO),
    );
    let id = place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    let mut clock = place.clock();
    clock.tick(&host);
    match next_reply(&mut reader) {
        Reply::StartWork {
            project,
            prompt,
            agent,
            from,
            shell,
            automation: Some(start),
            ..
        } => {
            assert_eq!(project, "proj");
            assert_eq!(prompt, "Audit the dependencies.");
            assert_eq!(agent.as_deref(), Some("claude"));
            assert_eq!(from, WorkSource::Cli);
            assert!(!shell);
            assert_eq!(start.id, id);
            assert!(
                start.branch.starts_with("audit-dependencies-"),
                "{}",
                start.branch
            );
        }
        other => panic!("{other:?}"),
    }
    let runs = place.runs(&id);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome, Outcome::Started);
    assert!(runs[0].branch.is_some());
    assert!(place.handled(&id));

    clock.tick(&host);
    nothing_more(&mut reader);
    assert_eq!(place.runs(&id).len(), 1, "started once");
}

#[test]
fn a_command_goes_to_a_window_as_a_terminal() {
    let place = Place::new("command");
    let automation = Automation::new(
        &[],
        place.project.clone(),
        "Run the e2e suite",
        Task::Command {
            line: "npm run e2e".to_owned(),
        },
        daily_at(SignedDuration::ZERO),
    );
    place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    match next_reply(&mut reader) {
        Reply::StartWork {
            prompt,
            shell,
            agent,
            ..
        } => {
            assert_eq!(prompt, "npm run e2e");
            assert!(shell);
            assert!(agent.is_none());
        }
        other => panic!("{other:?}"),
    }
    // Recorded with the line it was started with: what `ket automation
    // exec` runs, whatever the automation says by then.
    let runs = Runs::load_in(&place.root).unwrap();
    assert_eq!(runs.runs[0].command.as_deref(), Some("npm run e2e"));
}

#[test]
fn a_start_with_no_window_waits_for_one_and_keeps_the_host_up() {
    let place = Place::new("no-window");
    let automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::ZERO),
    );
    let id = place.add(automation);
    let host = Host::for_tests();
    let mut clock = place.clock();
    clock.tick(&host);
    assert!(place.runs(&id).is_empty());
    assert!(!place.handled(&id));
    assert!(clock.keeps_alive());

    // A window comes: the start waiting goes to it.
    let (_out, mut reader) = window(&host);
    clock.tick(&host);
    assert!(matches!(next_reply(&mut reader), Reply::StartWork { .. }));
    assert!(place.handled(&id));
}

#[test]
fn a_start_too_long_ago_is_recorded_as_missed() {
    let place = Place::new("missed");
    let automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::from_mins(50)),
    );
    let mut automation = automation;
    automation.catch_up_minutes = Some(10);
    let id = place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    nothing_more(&mut reader);
    let runs = place.runs(&id);
    assert!(
        matches!(runs[0].outcome, Outcome::Missed { .. }),
        "{runs:?}"
    );
    assert!(place.handled(&id));
}

#[test]
fn a_start_while_the_last_run_works_is_skipped() {
    let place = Place::new("busy");
    let automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::ZERO),
    );
    let id = place.add(automation);
    Runs::update_in(&place.root, |runs| {
        runs.record(Run {
            automation: id.clone(),
            due_ms: 1,
            at_ms: crate::now_ms(),
            outcome: Outcome::Started,
            worktree: None,
            branch: Some("audit-yesterday".to_owned()),
            ended_ms: None,
            tokens: None,
            command: None,
        });
    })
    .unwrap();
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    nothing_more(&mut reader);
    let runs = place.runs(&id);
    assert!(
        matches!(runs[0].outcome, Outcome::Skipped { .. }),
        "{runs:?}"
    );
    assert!(place.handled(&id));
}

#[test]
fn a_start_whose_project_has_gone_cannot_start() {
    let place = Place::new("gone");
    let automation = Automation::new(
        &[],
        ProjectId::new("removed"),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::ZERO),
    );
    let id = place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    nothing_more(&mut reader);
    let runs = place.runs(&id);
    assert!(
        matches!(runs[0].outcome, Outcome::CouldNotStart { .. }),
        "{runs:?}"
    );
}

#[test]
fn run_now_starts_one_whatever_its_schedule_and_is_asked_once() {
    let place = Place::new("run-now");
    let mut automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::from_hours(5)),
    );
    automation.enabled = false;
    automation.ask_to_run();
    let id = place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    let mut clock = place.clock();
    clock.tick(&host);
    assert!(matches!(
        next_reply(&mut reader),
        Reply::StartWork {
            automation: Some(_),
            ..
        }
    ));
    let saved = Automations::load_in(&place.root).unwrap();
    let saved = saved.get(&id).unwrap();
    assert!(saved.run_now_ms.is_none(), "asked once");
    assert!(saved.handled_ms.is_none(), "its schedule is untouched");
    assert_eq!(place.runs(&id)[0].outcome, Outcome::Started);
    clock.tick(&host);
    nothing_more(&mut reader);
}

/// A worktree of `project` called `id`, as ket's state keeps one.
fn worktree(id: &str, project: &str) -> crate::worktree::Worktree {
    crate::worktree::Worktree {
        agent: None,
        id: crate::id::WorktreeId::new(id),
        project_id: ProjectId::new(project),
        branch: id.to_owned(),
        name: None,
        path: PathBuf::from(format!("/tmp/{id}")),
        base: "main".to_owned(),
        base_commit: None,
        created_at_ms: 0,
        provisioned_at_ms: None,
        provisioned_paths: Vec::new(),
        token_reduction: crate::worktree::default_token_reduction(),
        token_reduction_id: None,
        token_reduction_pack_id: None,
        pinned: false,
        pull_request: None,
    }
}

#[test]
fn a_worktree_of_another_project_is_never_where_one_runs() {
    let place = Place::new("other-project");
    crate::store::Store::at(place.state.clone())
        .update(|state| {
            state.worktrees.push(worktree("theirs", "other"));
            Ok(())
        })
        .unwrap();
    let mut automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::ZERO),
    );
    automation.place = crate::automation::Place::Worktree {
        id: crate::id::WorktreeId::new("theirs"),
    };
    let id = place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    nothing_more(&mut reader);
    let runs = place.runs(&id);
    assert!(
        matches!(runs[0].outcome, Outcome::CouldNotStart { .. }),
        "{runs:?}"
    );
}

#[test]
fn a_start_tells_the_window_which_run_it_is() {
    let place = Place::new("due-sent");
    let mut automation = Automation::new(
        &[],
        place.project.clone(),
        "Audit",
        prompt("Audit."),
        daily_at(SignedDuration::from_hours(5)),
    );
    automation.ask_to_run();
    let id = place.add(automation);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    let Reply::StartWork {
        automation: Some(start),
        ..
    } = next_reply(&mut reader)
    else {
        panic!("not an automation's start");
    };
    let runs = place.runs(&id);
    assert_eq!(
        start.due_ms, runs[0].due_ms,
        "the window ends the run by this"
    );
    assert!(start.due_ms > 0);
}

#[test]
fn two_starts_sent_together_never_share_a_branch() {
    let place = Place::new("reserve");
    let made = |taken: &[Automation]| {
        Automation::new(
            taken,
            place.project.clone(),
            "Audit",
            prompt("Audit."),
            daily_at(SignedDuration::ZERO),
        )
    };
    let first = made(&[]);
    let second = made(std::slice::from_ref(&first));
    place.add(first);
    place.add(second);
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    place.clock().tick(&host);
    let branch = |reply: Reply| match reply {
        Reply::StartWork {
            automation: Some(start),
            ..
        } => start.branch,
        other => panic!("{other:?}"),
    };
    let a = branch(next_reply(&mut reader));
    let b = branch(next_reply(&mut reader));
    assert_ne!(a, b, "the second is numbered while the first is being made");
}

#[test]
fn a_one_off_made_after_its_moment_does_not_keep_the_host_up() {
    let place = Place::new("keeps-alive");
    let today = Timestamp::now().to_zoned(TimeZone::system()).date();
    let mut gone = Automation::new(
        &[],
        place.project.clone(),
        "Gone",
        prompt("x"),
        When::Once {
            day: crate::automation::Day::new(today.yesterday().unwrap()),
            time: Time::new(9, 0).unwrap(),
        },
    );
    gone.created_ms = crate::now_ms();
    Automations::update_in(&place.root, |all| {
        all.automations.push(gone);
        Ok(())
    })
    .unwrap();
    let host = Host::for_tests();
    let mut clock = place.clock();
    clock.tick(&host);
    assert!(!clock.keeps_alive());
    place.add(Automation::new(
        &[],
        place.project.clone(),
        "Daily",
        prompt("x"),
        daily_at(SignedDuration::from_hours(5)),
    ));
    clock.tick(&host);
    assert!(clock.keeps_alive());
}

#[test]
fn an_agent_that_has_finished_its_turn_is_not_still_working() {
    use crate::host::server::housekeeping::automations::working;
    let host = Host::for_tests();
    // The agent's own program in its tab's foreground, as it is while it
    // sits at its prompt, and a shell running a command.
    let busy =
        || LocalTerminal::open(&spec("sh", &["-c", "set -m; sleep 30; true"])).expect("a terminal");
    let agent = host.file_keyed_for_tests(busy(), Some("wt-agent|claude|1".to_owned()));
    let shell = host.file_keyed_for_tests(busy(), Some("wt-shell|shell|1".to_owned()));
    let running = |id| {
        host.find(id).is_some_and(|t| {
            matches!(
                t.foreground(),
                Some(crate::activity::Foreground::Running(_))
            )
        })
    };
    assert!(wait_until(Duration::from_secs(5), || running(agent) && running(shell)));
    assert!(
        !working(&host, "wt-agent"),
        "its agent says nothing of being at work"
    );
    assert!(working(&host, "wt-shell"), "its command is running");
    for id in [agent, shell] {
        if let Some(terminal) = host.find(id) {
            terminal.kill();
        }
    }
}
