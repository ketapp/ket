//! What passes between phones and windows through the host: prompts and
//! their answers, work, merges, `ket browser`, asleep tabs and their waking,
//! fresh starts — each answered only by the window asked.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::*;

// ---- prompts, work and merges handed between phones and windows -----------

/// `cat` filed under pane `key`, so what is typed at it can be read.
fn pane(host: &Arc<Host>, key: &str) -> (u64, Arc<LocalTerminal>) {
    let terminal = LocalTerminal::open(&spec("/bin/cat", &[])).expect("a terminal");
    let id = host.file_keyed_for_tests(terminal, Some(key.to_owned()));
    (id, host.find(id).expect("filed"))
}

/// Reports that pane `key` of worktree `wt` is asking `tool`'s permission
/// — as its hook would — and returns the prompt's id.
fn ask_permission(host: &Host, key: &str, tool: &str, prompt: Option<agent_hooks::Prompt>) -> u64 {
    let report = agent_hooks::HookReport {
        tool_name: Some(tool.to_owned()),
        question: Some(Box::new(agent_hooks::PermissionQuestion {
            tool: tool.to_owned(),
            subject: Some("cargo test".to_owned()),
            id: 0,
            prompt,
        })),
        ..prompt_report(key)
    };
    assert!(lock(&host.status).report(&report, |_| true));
    lock(&host.status)
        .snapshot()
        .panes
        .into_iter()
        .find(|p| p.pane == key)
        .and_then(|p| p.question)
        .map(|q| q.id)
        .expect("the prompt has an id")
}

fn allow_once(agent: &str, tool: &str) -> Option<&'static [u8]> {
    (agent == "claude" && tool == "Bash").then_some(b"allowed-from-phone\n".as_slice())
}

fn screen(terminal: &LocalTerminal) -> String {
    String::from_utf8_lossy(&terminal.checkpoint().ansi).into_owned()
}

#[test]
fn a_phones_answer_is_typed_into_the_terminal_showing_that_prompt() {
    let host = Host::for_tests();
    let (id, terminal) = pane(&host, "wt|claude|1");
    let question = ask_permission(&host, "wt|claude|1", "Bash", None);

    assert_eq!(host.answer_prompt(id, question, allow_once), Ok(()));
    assert!(
        wait_until(Duration::from_secs(5), || screen(&terminal)
            .contains("allowed-from-phone")),
        "{}",
        screen(&terminal)
    );
    terminal.kill();
}

#[test]
fn an_answer_to_a_prompt_that_moved_on_is_refused_and_types_nothing() {
    let host = Host::for_tests();
    let (id, terminal) = pane(&host, "wt|claude|1");
    let question = ask_permission(&host, "wt|claude|1", "Bash", None);

    assert_eq!(
        host.answer_prompt(999, question, allow_once),
        Err(AnswerRefused::Gone)
    );
    assert_eq!(
        host.answer_prompt(id, 0, allow_once),
        Err(AnswerRefused::Unnamed)
    );
    assert_eq!(
        host.answer_prompt(id, question + 1, allow_once),
        Err(AnswerRefused::Changed)
    );
    assert_eq!(
        host.answer_prompt(id, question, |_, _| None),
        Err(AnswerRefused::Unknown)
    );

    // Typed at the desktop after the prompt: that may have been the answer.
    lock(&host.terminals).get_mut(&id).unwrap().typed_at_ms = crate::now_ms() + 1;
    assert_eq!(
        host.answer_prompt(id, question, allow_once),
        Err(AnswerRefused::Answered)
    );

    std::thread::sleep(Duration::from_millis(100));
    assert!(!screen(&terminal).contains("allowed-from-phone"));
    terminal.kill();
}

#[test]
fn a_terminal_with_no_pane_or_no_prompt_cannot_be_answered() {
    let host = Host::for_tests();
    let unkeyed = host.file_for_tests(LocalTerminal::open(&spec("/bin/cat", &[])).unwrap());
    assert_eq!(
        host.answer_prompt(unkeyed, 1, allow_once),
        Err(AnswerRefused::Gone)
    );

    let (quiet, terminal) = pane(&host, "wt|claude|2");
    assert_eq!(
        host.answer_prompt(quiet, 1, allow_once),
        Err(AnswerRefused::Answered)
    );
    terminal.kill();
    host.find(unkeyed).unwrap().kill();
}

#[test]
fn declining_from_a_phone_presses_escape_in_the_terminal() {
    let host = Host::for_tests();
    let (id, terminal) = pane(&host, "wt|claude|1");
    let question = ask_permission(&host, "wt|claude|1", "Bash", None);

    assert_eq!(
        host.answer_prompt(id, question, |_, _| Some(b"declined\x1b".as_slice())),
        Ok(())
    );
    assert!(
        wait_until(Duration::from_secs(5), || screen(&terminal)
            .contains("declined")),
        "{}",
        screen(&terminal)
    );
    terminal.kill();
}

#[test]
fn answering_the_same_prompt_twice_types_it_once() {
    let host = Host::for_tests();
    let (id, terminal) = pane(&host, "wt|claude|1");
    let question = ask_permission(&host, "wt|claude|1", "Bash", None);

    assert_eq!(host.answer_prompt(id, question, allow_once), Ok(()));
    assert!(
        wait_until(Duration::from_secs(5), || screen(&terminal)
            .contains("allowed-from-phone")),
        "{}",
        screen(&terminal)
    );
    std::thread::sleep(Duration::from_millis(100));
    // Echoed by the tty and again by `cat`: however many that is, the
    // second tap adds none. The key is pressed, the pane has moved on,
    // and there is nothing left to answer.
    let typed = screen(&terminal).matches("allowed-from-phone").count();
    assert_eq!(
        host.answer_prompt(id, question, allow_once),
        Err(AnswerRefused::Answered)
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        screen(&terminal).matches("allowed-from-phone").count(),
        typed
    );
    terminal.kill();
}

#[test]
fn a_phones_allow_moves_the_pane_to_running_and_a_deny_to_idle() {
    let host = Host::for_tests();
    let (id, terminal) = pane(&host, "wt|claude|1");
    let key = "wt|claude|1";
    let pane_of = |host: &Host| {
        host.agent_status()
            .panes
            .into_iter()
            .find(|p| p.pane == key)
            .expect("the pane")
    };

    let question = ask_permission(&host, key, "Bash", None);
    assert_eq!(host.answer_prompt(id, question, allow_once), Ok(()));
    let pane = pane_of(&host);
    assert_eq!(pane.state, crate::event::AgentState::ExecutingTool);
    assert!(pane.question.is_none());
    let answered = pane.answered.expect("remembered");
    assert!(answered.allowed);
    assert_eq!(answered.question.id, question);

    let question = ask_permission(&host, key, "Bash", None);
    assert_eq!(
        host.answer_prompt(id, question, |_, _| Some(b"\x1b".as_slice())),
        Ok(())
    );
    let pane = pane_of(&host);
    assert_eq!(pane.state, crate::event::AgentState::Idle);
    assert!(pane.question.is_none());
    assert!(!pane.answered.expect("remembered").allowed);
    terminal.kill();
}

/// A Codex pane keyed `key` whose screen shows `shows` at its foot, as
/// Codex draws its dialogs, then echoes whatever is typed.
fn codex_pane(host: &Arc<Host>, key: &str, shows: &str) -> (u64, Arc<LocalTerminal>) {
    let foot = format!("{}{shows}", "\n".repeat(23 - shows.lines().count()));
    let terminal = LocalTerminal::open(&spec(
        "/bin/sh",
        &["-c", "printf '%s\\n' \"$1\"; exec cat", "sh", &foot],
    ))
    .expect("a terminal");
    let last = shows
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned();
    let id = host.file_keyed_for_tests(terminal, Some(key.to_owned()));
    let terminal = host.find(id).expect("filed");
    assert!(
        wait_until(Duration::from_secs(5), || terminal
            .with_term(|term| crate::screen::bottom_rows(term, 24))
            .iter()
            .any(|row| row.contains(&last))),
        "{}",
        screen(&terminal)
    );
    (id, terminal)
}

/// `event` from Codex in pane `key`, asking `question` if anything.
fn codex_report(
    key: &str,
    event: agent_hooks::HookEvent,
    question: Option<agent_hooks::PermissionQuestion>,
) -> agent_hooks::HookReport {
    agent_hooks::HookReport {
        agent: "codex".to_owned(),
        event,
        tool_name: question.as_ref().map(|q| q.tool.clone()),
        question: question.map(Box::new),
        ..prompt_report(key)
    }
}

fn question_id(host: &Host, key: &str) -> Option<u64> {
    lock(&host.status)
        .snapshot()
        .panes
        .into_iter()
        .find(|p| p.pane == key)
        .and_then(|p| p.question)
        .map(|q| q.id)
}

#[test]
fn codex_folder_access_is_answerable_before_any_session_hook() {
    let host = Host::for_tests();
    let key = "wt|codex|1";
    let (id, terminal) = codex_pane(
        &host,
        key,
        "Folder access\n/tmp/new-worktree\n\n\
         Trust this folder? Your trust decision will be saved.\n\n\
         › 1. Trust and continue\n\
         \x20 2. Back to Agent Command Center\n\n\
         enter continue · esc back\n\n\n\n\n",
    );
    assert!(lock(&host.status).get(key).is_none());
    assert!(host.read_codex_dialogs());
    let pane = lock(&host.status).get(key).unwrap().clone();
    assert_eq!(pane.worktree, "wt");
    assert_eq!(pane.agent, "codex");
    assert!(pane.session.is_none());
    assert_eq!(pane.state, crate::event::AgentState::AwaitingPermission);
    let question = pane.question.expect("folder access on the phone");
    assert!(
        question
            .subject
            .as_deref()
            .unwrap()
            .contains("/tmp/new-worktree")
    );
    assert!(!host.read_codex_dialogs(), "the same question stays stable");
    // The same choices for a different folder must not grant trust.
    let mut stale = terminal
        .with_term(|term| {
            crate::screen::codex_dialog(&crate::screen::bottom_rows(term, usize::MAX))
        })
        .unwrap();
    stale.question = stale
        .question
        .replace("/tmp/new-worktree", "/tmp/other-folder");
    assert!(lock(&host.status).screen_dialog(key, Some(stale), crate::now_ms()));
    let stale_id = question_id(&host, key).unwrap();
    assert_eq!(
        host.respond_prompt(id, stale_id, |_| Some(agent_hooks::Decision::Pick(0))),
        Err(AnswerRefused::Changed)
    );
    assert!(host.read_codex_dialogs());
    let question = lock(&host.status)
        .get(key)
        .unwrap()
        .question
        .clone()
        .unwrap();
    assert_eq!(
        host.respond_prompt(id, question.id, |_| Some(agent_hooks::Decision::Pick(0))),
        Ok(())
    );
    assert_eq!(
        host.respond_prompt(id, question.id, |_| Some(agent_hooks::Decision::Pick(0))),
        Err(AnswerRefused::Answered)
    );
    assert!(wait_until(Duration::from_secs(5), || terminal
        .with_term(|term| crate::screen::bottom_rows(term, 24))
        .iter()
        .any(|row| row == "1")));

    // The first real hook takes over the pane created for folder trust,
    // even before the terminal has redrawn the old question away.
    let mut started = codex_report(key, agent_hooks::HookEvent::SessionStart, None);
    started.session = Some("new-session".to_owned());
    assert!(lock(&host.status).report(&started, |_| true));
    let pane = lock(&host.status).get(key).unwrap().clone();
    assert!(pane.question.is_none());
    assert_eq!(pane.state, crate::event::AgentState::Starting);
    assert_eq!(pane.session.as_deref(), Some("new-session"));
    assert!(!host.read_codex_dialogs());
    assert_eq!(
        host.respond_prompt(id, question.id, |_| Some(agent_hooks::Decision::Pick(0))),
        Err(AnswerRefused::Answered)
    );
    terminal.clear();
    terminal.kill();
}

#[test]
fn codex_hook_review_is_answerable_before_any_session_hook() {
    let host = Host::for_tests();
    let key = "wt|codex|1";
    let (id, terminal) = codex_pane(
        &host,
        key,
        "Hooks need review\n\
         1 hook is new or changed.\n\
         Hooks can run outside the sandbox after you trust them.\n\n\
         › 1. Review hooks\n\
         \x20 2. Trust all and continue\n\
         \x20 3. Continue without trusting (hooks won't run)\n\n\
         enter confirm · esc skip\n\n\n\n",
    );
    assert!(lock(&host.status).get(key).is_none());
    assert!(host.read_codex_dialogs());
    let pane = lock(&host.status).get(key).unwrap().clone();
    assert_eq!(pane.state, crate::event::AgentState::AwaitingPermission);
    let question = pane.question.expect("hook review on the phone");
    assert_eq!(
        question.subject.as_deref(),
        Some("1 hook is new or changed.\nHooks can run outside the sandbox after you trust them.")
    );
    // Trust all: Codex's own 2, then Enter. Its Review hooks is not offered.
    assert_eq!(
        host.respond_prompt(id, question.id, |_| Some(agent_hooks::Decision::Pick(0))),
        Ok(())
    );
    assert!(wait_until(Duration::from_secs(5), || terminal
        .with_term(|term| crate::screen::bottom_rows(term, 24))
        .iter()
        .any(|row| row == "2")));
    terminal.clear();
    terminal.kill();
}

#[test]
fn codexs_question_form_is_answered_with_each_options_number() {
    let host = Host::for_tests();
    let key = "wt|codex|1";
    let (id, terminal) = pane(&host, key);
    let asked = |question: &str, options: &[&str]| agent_hooks::Asked {
        question: question.to_owned(),
        header: String::new(),
        options: options
            .iter()
            .map(|&label| agent_hooks::Offered {
                label: label.to_owned(),
                description: String::new(),
            })
            .collect(),
        multi_select: false,
    };
    let form = agent_hooks::PermissionQuestion {
        tool: agent_hooks::CODEX_ASK_TOOL.to_owned(),
        subject: Some("Which crate?".to_owned()),
        id: 0,
        prompt: Some(agent_hooks::Prompt::Ask {
            questions: vec![
                asked("Which crate?", &["ket-core", "ket-ui"]),
                asked("Tests too?", &["Yes", "No"]),
            ],
        }),
    };
    let report = codex_report(key, agent_hooks::HookEvent::PreTool, Some(form));
    assert!(lock(&host.status).report(&report, |_| true));
    let question = question_id(&host, key).expect("the form");
    let picks =
        |_: &agent_hooks::PermissionQuestion| Some(agent_hooks::Decision::Picks(vec![1, 0]));

    // The form is not on screen: numbers would land somewhere else.
    assert_eq!(
        host.respond_prompt(id, question, picks),
        Err(AnswerRefused::Changed)
    );
    // An answer for the wrong number of questions does not fit.
    terminal
        .send_input(b"Question 1/2 (2 unanswered)\n".to_vec())
        .unwrap();
    assert!(wait_until(Duration::from_secs(5), || terminal
        .with_term(|term| crate::screen::bottom_rows(term, 24))
        .iter()
        .any(|row| row.starts_with("Question 1/2"))));
    assert_eq!(
        host.respond_prompt(id, question, |_| Some(agent_hooks::Decision::Pick(0))),
        Err(AnswerRefused::Unfit)
    );
    assert_eq!(host.respond_prompt(id, question, picks), Ok(()));
    assert!(
        wait_until(Duration::from_secs(5), || screen(&terminal).contains("21")),
        "{}",
        screen(&terminal)
    );
    // A second phone, before Codex has said it has the answer: the
    // screen still shows the form, but the keys are not typed again.
    assert_eq!(
        host.respond_prompt(id, question, picks),
        Err(AnswerRefused::Answered)
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(!screen(&terminal).contains("2121"), "{}", screen(&terminal));
    terminal.kill();
}

#[test]
fn codexs_plan_question_after_its_turn_is_answered_with_a_bare_number() {
    let host = Host::for_tests();
    let key = "wt|codex|1";
    let (id, terminal) = codex_pane(
        &host,
        key,
        "  Implement this plan?\n\n\
         › 1. Yes, implement this plan          Switch to Default and start coding\n\
         \x20 2. Yes, clear context and implement  Fresh thread with this plan\n\
         \x20 3. No, stay in Plan mode             Continue planning with the model\n\n\
         \x20 Press enter to confirm or esc to go back",
    );
    let stop = codex_report(key, agent_hooks::HookEvent::Stop, None);
    assert!(lock(&host.status).report(&stop, |_| true));

    assert!(host.read_codex_dialogs());
    let pane = lock(&host.status)
        .snapshot()
        .panes
        .into_iter()
        .find(|p| p.pane == key)
        .expect("the pane");
    assert_eq!(pane.state, crate::event::AgentState::AwaitingPermission);
    let question = pane.question.expect("the plan question");
    assert_eq!(question.tool, crate::screen::DIALOG_TOOL);

    assert_eq!(
        host.respond_prompt(id, question.id, |_| Some(agent_hooks::Decision::Pick(0))),
        Ok(())
    );
    assert_eq!(
        host.respond_prompt(id, question.id, |_| Some(agent_hooks::Decision::Pick(2))),
        Err(AnswerRefused::Answered)
    );
    assert!(
        wait_until(Duration::from_secs(5), || terminal
            .with_term(|term| crate::screen::bottom_rows(term, 24))
            .iter()
            .any(|row| row == "1")),
        "{}",
        screen(&terminal)
    );
    terminal.kill();
}

#[test]
fn a_respond_needs_a_question_with_a_prompt_and_a_hook_still_held() {
    let host = Host::for_tests();
    let (id, terminal) = pane(&host, "wt|claude|1");

    // A plain permission prompt is answered with keys, not a Respond.
    let plain = ask_permission(&host, "wt|claude|1", "Bash", None);
    assert_eq!(
        host.respond_prompt(id, plain, |_| Some(agent_hooks::Decision::Approve)),
        Err(AnswerRefused::Unknown)
    );

    let plan = ask_permission(
        &host,
        "wt|claude|1",
        "ExitPlanMode",
        Some(agent_hooks::Prompt::Plan),
    );
    // An answer that does not fit what was asked.
    assert_eq!(
        host.respond_prompt(id, plan, |_| None),
        Err(AnswerRefused::Unfit)
    );
    // It fits, but no hook is being held for it to answer.
    assert_eq!(
        host.respond_prompt(id, plan, |_| Some(agent_hooks::Decision::Approve)),
        Err(AnswerRefused::Unknown)
    );
    assert!(!host.prompt_held(
        "wt|claude|1",
        &agent_hooks::PermissionQuestion {
            tool: "ExitPlanMode".to_owned(),
            subject: None,
            id: plan,
            prompt: Some(agent_hooks::Prompt::Plan),
        }
    ));
    terminal.kill();
}

#[test]
fn work_merges_and_fixes_from_a_phone_go_to_a_window_and_only_when_one_is_open() {
    let host = Host::for_tests();
    assert!(!host.start_work(
        "p".to_owned(),
        "Fix it".to_owned(),
        None,
        None,
        WorkSource::Phone
    ));
    assert!(!host.merge_work("wt".to_owned()));
    assert!(!host.has_window());
    assert!(matches!(
        host.fix_checks("wt".to_owned(), "Fix it".to_owned()),
        Handed::Failed(_)
    ));

    let (out, mut reader) = window(&host);
    assert!(host.has_window());
    assert!(host.start_work(
        "proj".to_owned(),
        "Fix the login".to_owned(),
        Some("codex".to_owned()),
        Some("note-1".to_owned()),
        WorkSource::Phone,
    ));
    match next_reply(&mut reader) {
        Reply::StartWork {
            project,
            prompt,
            agent,
            note,
            ..
        } => {
            assert_eq!(project, "proj");
            assert_eq!(prompt, "Fix the login");
            assert_eq!(agent.as_deref(), Some("codex"));
            assert_eq!(note.as_deref(), Some("note-1"));
        }
        other => panic!("{other:?}"),
    }
    assert!(host.merge_work("wt-7".to_owned()));
    match next_reply(&mut reader) {
        Reply::MergeWork { worktree } => assert_eq!(worktree, "wt-7"),
        other => panic!("{other:?}"),
    }
    let fixing = std::thread::spawn({
        let host = host.clone();
        move || host.fix_checks("wt-7".to_owned(), "The checks failed".to_owned())
    });
    match next_reply(&mut reader) {
        Reply::FixChecks {
            req,
            worktree,
            prompt,
        } => {
            assert_eq!(worktree, "wt-7");
            assert_eq!(prompt, "The checks failed");
            assert!(!fixing.is_finished(), "the phone still waits for the agent");
            host.finish_fix_checks(req, &out, None);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fixing.join().unwrap(), Handed::Given);

    let failing = std::thread::spawn({
        let host = host.clone();
        move || host.fix_checks("wt-7".to_owned(), "Try again".to_owned())
    });
    match next_reply(&mut reader) {
        Reply::FixChecks { req, .. } => {
            host.finish_fix_checks(req, &out, Some("agent is not configured".to_owned()));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        failing.join().unwrap(),
        Handed::Failed("agent is not configured".to_owned())
    );

    // A window slow to start the agent: still starting, not failed.
    let slow = std::thread::spawn({
        let host = host.clone();
        move || host.fix_checks("wt-7".to_owned(), "Slowly".to_owned())
    });
    assert!(matches!(next_reply(&mut reader), Reply::FixChecks { .. }));
    assert_eq!(slow.join().unwrap(), Handed::Starting);

    // A window that goes while starting it: failed, at once.
    let started = Instant::now();
    let gone = std::thread::spawn({
        let host = host.clone();
        move || host.fix_checks("wt-7".to_owned(), "Then close".to_owned())
    });
    assert!(matches!(next_reply(&mut reader), Reply::FixChecks { .. }));
    out.alive.store(false, Ordering::Release);
    assert!(matches!(gone.join().unwrap(), Handed::Failed(why) if why.contains("closed")));
    assert!(
        started.elapsed() < FIX_CHECKS_WAIT,
        "it did not wait the window out"
    );
}

/// A connection from `ket browser`: the CLI, which says hello as a
/// window does without being one — or, run from the host's own
/// executable, while passing for one.
fn cli(host: &Arc<Host>, conn: u64, window: bool) -> (Arc<Out>, UnixStream) {
    let (stream, mut reader) = UnixStream::pair().expect("a socket pair");
    let out = Arc::new(Out {
        stream: Mutex::new(stream),
        alive: AtomicBool::new(true),
        greeted: AtomicBool::new(false),
        window,
    });
    lock(&host.outs).push((conn, out.clone()));
    assert!(handle(
        host,
        conn,
        &out,
        Request::Hello {
            protocol: wire::PROTOCOL,
            build: String::new(),
        },
    ));
    assert!(matches!(next_reply(&mut reader), Reply::Welcome { .. }));
    assert!(matches!(next_reply(&mut reader), Reply::AgentStatus { .. }));
    reader
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read timeout");
    (out, reader)
}

/// Has `ket browser` ask `action` of `worktree` over connection `conn`,
/// on a thread of its own, as its connection's reader would.
fn ask_browser(
    host: &Arc<Host>,
    conn: u64,
    out: &Arc<Out>,
    action: wire::BrowserAction,
) -> std::thread::JoinHandle<bool> {
    let (host, out) = (host.clone(), out.clone());
    std::thread::spawn(move || {
        handle(
            &host,
            conn,
            &out,
            Request::Browser {
                req: 7,
                worktree: "wt-1".to_owned(),
                action,
            },
        )
    })
}

fn browser_answer(reader: &mut UnixStream) -> (u64, Browsed) {
    match next_reply(reader) {
        Reply::BrowserAnswer { req, result } => (req, result),
        other => panic!("expected the browser's answer, got {other:?}"),
    }
}

#[test]
fn ket_browser_goes_to_a_window_and_its_answer_comes_back() {
    let host = Host::for_tests();

    // No window, or only the asker passing for one: nobody to ask.
    let (alone, mut alone_reader) = cli(&host, 9, true);
    assert!(
        ask_browser(&host, 9, &alone, wire::BrowserAction::Tabs)
            .join()
            .unwrap()
    );
    assert_eq!(
        browser_answer(&mut alone_reader),
        (
            7,
            Err("no ket window is open to run the browser in".to_owned())
        )
    );
    lock(&host.outs).retain(|(conn, _)| *conn != 9);

    let (window, mut window_reader) = window(&host);
    let (asker, mut asker_reader) = cli(&host, 2, false);
    let open = wire::BrowserAction::Open {
        url: Some("localhost:3000".to_owned()),
    };
    let asking = ask_browser(&host, 2, &asker, open.clone());
    let req = match next_reply(&mut window_reader) {
        Reply::BrowserAction {
            req,
            worktree,
            action,
        } => {
            assert_eq!(worktree, "wt-1");
            assert_eq!(action, open);
            req
        }
        other => panic!("expected the browser action, got {other:?}"),
    };
    assert!(!asking.is_finished(), "the CLI waits for the page");
    let done = wire::BrowserAnswer::Done {
        url: "http://localhost:3000/".to_owned(),
        title: "Sign in".to_owned(),
    };
    assert!(handle(
        &host,
        1,
        &window,
        Request::BrowserResult {
            req,
            result: Ok(done.clone()),
        },
    ));
    assert!(asking.join().unwrap());
    assert_eq!(browser_answer(&mut asker_reader), (7, Ok(done)));

    // The window's own words for what went wrong reach the CLI as they are.
    let asking = ask_browser(&host, 2, &asker, wire::BrowserAction::Back);
    let Reply::BrowserAction { req, .. } = next_reply(&mut window_reader) else {
        panic!("expected the browser action");
    };
    host.finish_browser(
        req,
        &window,
        Err("there's no page to go back to".to_owned()),
    );
    assert!(asking.join().unwrap());
    assert_eq!(
        browser_answer(&mut asker_reader),
        (7, Err("there's no page to go back to".to_owned()))
    );

    // A window that goes meanwhile: said at once, not waited out.
    let started = Instant::now();
    let asking = ask_browser(&host, 2, &asker, wire::BrowserAction::Reload);
    assert!(matches!(
        next_reply(&mut window_reader),
        Reply::BrowserAction { .. }
    ));
    window.alive.store(false, Ordering::Release);
    assert!(asking.join().unwrap());
    assert!(matches!(
        browser_answer(&mut asker_reader),
        (7, Err(why)) if why.contains("closed")
    ));
    assert!(
        started.elapsed() < BROWSER_WAIT,
        "it did not wait the window out"
    );
    assert!(lock(&host.browser_waiting).is_empty());
}

/// Any program that says hello could once answer a pending `ket browser`
/// request, the numbers being easy to guess: the terminal printed what
/// it made up, and the real window's answer was dropped.
#[test]
fn ket_browser_hears_only_the_window_it_asked() {
    let host = Host::for_tests();
    let (window, mut window_reader) = window(&host);
    let (asker, mut asker_reader) = cli(&host, 2, false);
    let (forger, _forger_reader) = cli(&host, 3, false);
    let (other_window, _other_reader) = cli(&host, 4, true);

    let asking = ask_browser(&host, 2, &asker, wire::BrowserAction::Tabs);
    let Reply::BrowserAction { req, .. } = next_reply(&mut window_reader) else {
        panic!("expected the browser action");
    };
    let forged = wire::BrowserAnswer::Done {
        url: "https://forged.example/".to_owned(),
        title: "Forged".to_owned(),
    };
    for (conn, out) in [(3, &forger), (4, &other_window), (2, &asker)] {
        assert!(handle(
            &host,
            conn,
            out,
            Request::BrowserResult {
                req,
                result: Ok(forged.clone()),
            },
        ));
        assert!(
            lock(&host.browser_waiting).contains_key(&req),
            "connection {conn} was not asked, so its answer does not count"
        );
    }
    assert!(!asking.is_finished(), "the CLI still waits for the window");

    let done = wire::BrowserAnswer::Done {
        url: "http://localhost:3000/".to_owned(),
        title: "Sign in".to_owned(),
    };
    assert!(handle(
        &host,
        1,
        &window,
        Request::BrowserResult {
            req,
            result: Ok(done.clone()),
        },
    ));
    assert!(asking.join().unwrap());
    assert_eq!(browser_answer(&mut asker_reader), (7, Ok(done)));
    assert!(lock(&host.browser_waiting).is_empty());
}

/// Reports that pane `key` has started a turn, as its prompt hook would.
fn working(host: &Host, key: &str) {
    let mut report = agent_hooks::HookReport {
        event: agent_hooks::HookEvent::UserPrompt,
        question: None,
        tool_name: None,
        at_ms: crate::now_ms(),
        ..prompt_report(key)
    };
    report.pane = Some(key.to_owned());
    assert!(lock(&host.status).report(&report, |_| true));
}

/// Whether `state` is no longer a turn in progress.
fn stopped_state(state: crate::event::AgentState) -> bool {
    !matches!(
        state,
        crate::event::AgentState::Thinking | crate::event::AgentState::ExecutingTool
    )
}

fn state_of(reply: Reply, key: &str) -> crate::event::AgentState {
    match reply {
        Reply::AgentStatus { snapshot } => {
            snapshot
                .panes
                .iter()
                .find(|p| p.pane == key)
                .expect("the pane")
                .state
        }
        other => panic!("the windows were not told: {other:?}"),
    }
}

#[test]
fn a_phones_interrupt_or_escape_stops_a_working_turn_and_the_windows_are_told() {
    let host = Host::for_tests();
    let (_out, mut reader) = window(&host);
    let (cancelled, first) = pane(&host, "wt|claude|1");
    let (escaped, second) = pane(&host, "wt|claude|2");
    working(&host, "wt|claude|1");
    working(&host, "wt|claude|2");

    host.cancel_terminal(cancelled, crate::agent_status::CancelSource::Phone);
    assert!(stopped_state(state_of(
        next_reply(&mut reader),
        "wt|claude|1"
    )));
    host.escape_terminal(escaped);
    assert!(stopped_state(state_of(
        next_reply(&mut reader),
        "wt|claude|2"
    )));

    // A terminal with no pane has nothing to stop, and says nothing.
    let unkeyed = host.file_for_tests(LocalTerminal::open(&spec("/bin/cat", &[])).unwrap());
    host.cancel_terminal(unkeyed, crate::agent_status::CancelSource::Phone);
    host.escape_terminal(unkeyed);
    reader
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert!(wire::read(&mut reader).is_err(), "nothing more was sent");
    for terminal in [first, second] {
        terminal.kill();
    }
    host.find(unkeyed).unwrap().kill();
}

#[test]
fn the_pane_keys_are_those_of_terminals_still_running() {
    let host = Host::for_tests();
    let (live, live_terminal) = pane(&host, "wt|claude|1");
    let (dead, dead_terminal) = pane(&host, "wt|claude|2");
    dead_terminal.kill();
    assert!(wait_until(Duration::from_secs(5), || dead_terminal.is_closed()));
    assert_eq!(host.key(live).as_deref(), Some("wt|claude|1"));
    assert_eq!(host.key(dead).as_deref(), Some("wt|claude|2"));
    let keys = host.pane_keys();
    assert!(keys.contains("wt|claude|1"));
    assert!(!keys.contains("wt|claude|2"));
    live_terminal.kill();
}

// ---- asleep tabs and their waking -----------------------------------------

fn asleep_tab(key: &str) -> wire::AsleepTab {
    wire::AsleepTab {
        key: key.to_owned(),
        worktree: "wt".to_owned(),
        agent: "claude".to_owned(),
        title: "claude".to_owned(),
        since_ms: 1,
        freed_bytes: 2,
    }
}

#[test]
fn asleep_tabs_are_merged_across_windows_less_any_running_again() {
    let host = Host::for_tests();
    let (out, _reader) = out_pair();
    assert!(handle(
        &host,
        1,
        &out,
        Request::Asleep {
            tabs: vec![asleep_tab("wt|claude|a"), asleep_tab("wt|claude|b")],
        }
    ));
    assert!(handle(
        &host,
        2,
        &out,
        Request::Asleep {
            tabs: vec![asleep_tab("wt|claude|b"), asleep_tab("wt|claude|c")],
        }
    ));
    // `b` woke: a terminal runs under its key again.
    let (_id, _terminal) = pane(&host, "wt|claude|b");
    let mut listed: Vec<(u64, String)> = host
        .asleep_tabs()
        .into_iter()
        .map(|(conn, tab)| (conn, tab.key))
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        vec![(1, "wt|claude|a".to_owned()), (2, "wt|claude|c".to_owned())]
    );
}

#[test]
fn a_wake_is_asked_of_the_window_that_reported_the_tab_and_resolves_when_its_terminal_opens() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    assert!(handle(
        &host,
        1,
        &out,
        Request::Asleep {
            tabs: vec![asleep_tab("wt|claude|z")],
        }
    ));
    let waking = {
        let host = host.clone();
        std::thread::spawn(move || host.wake("wt|claude|z"))
    };
    let req = match next_reply(&mut reader) {
        Reply::Wake { req, key } => {
            assert_eq!(key, "wt|claude|z");
            req
        }
        other => panic!("expected a wake, got {other:?}"),
    };
    assert!(req > 0);
    assert!(handle(
        &host,
        1,
        &out,
        Request::Open {
            req: 5,
            spec: Box::new(keyed_spec("wt|claude|z")),
        }
    ));
    let id = match next_reply(&mut reader) {
        Reply::Opened { id, .. } => id,
        other => panic!("expected the terminal, got {other:?}"),
    };
    assert_eq!(waking.join().unwrap(), Ok(id));
}

#[test]
fn a_wake_the_window_refuses_says_why() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    assert!(handle(
        &host,
        1,
        &out,
        Request::Asleep {
            tabs: vec![asleep_tab("wt|claude|y")],
        }
    ));
    let waking = {
        let host = host.clone();
        std::thread::spawn(move || host.wake("wt|claude|y"))
    };
    let Reply::Wake { req, .. } = next_reply(&mut reader) else {
        panic!("expected a wake");
    };
    assert!(handle(
        &host,
        1,
        &out,
        Request::WakeResult {
            req,
            error: Some("This agent is unavailable".to_owned()),
        }
    ));
    assert_eq!(
        waking.join().unwrap(),
        Err("This agent is unavailable".to_owned())
    );
}

/// The next reply that is not a terminal's output: a window watching a
/// terminal is sent its screen between replies.
fn next_json_reply(reader: &mut impl Read) -> Reply {
    loop {
        if let Frame::Json(body) = next_frame(reader) {
            return serde_json::from_slice(&body).expect("a reply");
        }
    }
}

/// Opens a terminal under `key` from the window on `out`, which then
/// shows it, and returns its id.
fn opened(host: &Arc<Host>, out: &Arc<Out>, reader: &mut UnixStream, req: u64, key: &str) -> u64 {
    assert!(handle(
        host,
        1,
        out,
        Request::Open {
            req,
            spec: Box::new(keyed_spec(key)),
        }
    ));
    match next_json_reply(reader) {
        Reply::Opened { id, .. } => id,
        other => panic!("expected the terminal, got {other:?}"),
    }
}

#[test]
fn a_fresh_start_is_asked_of_the_window_showing_the_tab_and_resolves_when_the_fresh_one_opens() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|claude|cold");
    let starting = {
        let host = host.clone();
        std::thread::spawn(move || host.start_fresh("wt|claude|cold", "s", 1))
    };
    let req = match next_json_reply(&mut reader) {
        Reply::StartFresh {
            req,
            key,
            session,
            since_ms,
        } => {
            assert_eq!(key, "wt|claude|cold");
            assert_eq!((session.as_str(), since_ms), ("s", 1));
            req
        }
        other => panic!("expected a fresh start, got {other:?}"),
    };
    // The window names the tab it opened, then its terminal opens.
    assert!(handle(
        &host,
        1,
        &out,
        Request::FreshResult {
            req,
            key: Some("wt|claude|fresh".to_owned()),
            error: None,
        }
    ));
    let fresh = opened(&host, &out, &mut reader, 2, "wt|claude|fresh");
    assert_eq!(starting.join().unwrap(), Ok(fresh));
}

#[test]
fn a_fresh_start_the_window_refuses_says_why() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|claude|busy");
    let starting = {
        let host = host.clone();
        std::thread::spawn(move || host.start_fresh("wt|claude|busy", "s", 1))
    };
    let Reply::StartFresh { req, .. } = next_json_reply(&mut reader) else {
        panic!("expected a fresh start");
    };
    assert!(handle(
        &host,
        1,
        &out,
        Request::FreshResult {
            req,
            key: None,
            error: Some("it's working".to_owned()),
        }
    ));
    assert_eq!(starting.join().unwrap(), Err("it's working".to_owned()));
}

#[test]
fn a_fresh_start_for_a_tab_no_window_shows_is_refused_at_once() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    opened(&host, &out, &mut reader, 1, "wt|claude|shown");
    // Another tab in the same worktree, which no window is showing.
    assert_eq!(
        host.start_fresh("wt|claude|elsewhere", "s", 1),
        Err("that tab isn't open in a ket window on your Mac".to_owned())
    );
}

#[test]
fn a_fresh_start_does_not_keep_waiting_when_its_window_closes() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    let id = opened(&host, &out, &mut reader, 1, "wt|claude|cold");
    let starting = {
        let host = host.clone();
        std::thread::spawn(move || host.start_fresh("wt|claude|cold", "s", 1))
    };
    assert!(matches!(
        next_json_reply(&mut reader),
        Reply::StartFresh { .. }
    ));
    out.alive.store(false, Ordering::Release);
    assert_eq!(
        starting.join().unwrap(),
        Err("the ket window closed before it started fresh".to_owned())
    );
    assert!(lock(&host.fresh_waiting).is_empty());
    assert!(!host.find(id).unwrap().is_closed());
    host.find(id).unwrap().kill();
}

#[test]
fn a_fresh_start_without_a_tab_or_error_gets_a_useful_failure() {
    let host = Host::for_tests();
    let (out, mut reader) = window(&host);
    let id = opened(&host, &out, &mut reader, 1, "wt|claude|cold");
    let starting = {
        let host = host.clone();
        std::thread::spawn(move || host.start_fresh("wt|claude|cold", "s", 1))
    };
    let Reply::StartFresh { req, .. } = next_json_reply(&mut reader) else {
        panic!("expected a fresh start");
    };
    assert!(handle(
        &host,
        1,
        &out,
        Request::FreshResult {
            req,
            key: None,
            error: None
        }
    ));
    assert_eq!(
        starting.join().unwrap(),
        Err("ket on your Mac didn't start fresh".to_owned())
    );
    assert!(lock(&host.fresh_waiting).is_empty());
    host.find(id).unwrap().kill();
}

#[test]
fn a_wake_without_its_window_is_refused_at_once() {
    let host = Host::for_tests();
    let (out, _reader) = out_pair();
    // Reported by a connection no window is on.
    assert!(handle(
        &host,
        9,
        &out,
        Request::Asleep {
            tabs: vec![asleep_tab("wt|claude|x")],
        }
    ));
    let refused = Err("open ket on your Mac to wake this tab".to_owned());
    assert_eq!(host.wake("wt|claude|x"), refused);
    assert_eq!(host.wake("wt|claude|none"), refused);
}

// ---- a phone's requests answered only by the window asked -----------------
//
// Any program that says hello could once answer a phone's pending wake,
// fix, review or delete, the numbers counting up from 1: the phone was
// told what it made up, and the real window's answer was dropped.

/// Has a program that said hello, and a CLI passing for a window — neither
/// asked anything — each send every request `forged` makes, and checks
/// after each that request `req` still waits in `waiting`.
fn strangers_answer<V>(
    host: &Arc<Host>,
    waiting: &Mutex<BTreeMap<u64, V>>,
    req: u64,
    forged: impl Fn() -> Vec<Request>,
) {
    let (forger, _forger_reader) = cli(host, 3, false);
    let (other_window, _other_reader) = cli(host, 4, true);
    for (conn, out) in [(3, &forger), (4, &other_window)] {
        for request in forged() {
            assert!(handle(host, conn, out, request));
            assert!(
                lock(waiting).contains_key(&req),
                "connection {conn} was not asked, so its answer does not count"
            );
        }
    }
}

#[test]
fn a_phones_wake_hears_only_the_window_it_asked() {
    let host = Host::for_tests();
    let (window, mut reader) = window(&host);
    let tabs = vec![asleep_tab("wt|claude|w")];
    assert!(handle(&host, 1, &window, Request::Asleep { tabs }));
    let waking = {
        let host = host.clone();
        std::thread::spawn(move || host.wake("wt|claude|w"))
    };
    let Reply::Wake { req, .. } = next_reply(&mut reader) else {
        panic!("expected a wake");
    };
    // Nor does a terminal opened under the tab's key: any can name it.
    strangers_answer(&host, &host.wake_waiting, req, || {
        vec![
            Request::WakeResult {
                req,
                error: Some("forged".to_owned()),
            },
            Request::Open {
                req: 1,
                spec: Box::new(keyed_spec("wt|claude|w")),
            },
        ]
    });
    assert!(handle(
        &host,
        1,
        &window,
        Request::Open {
            req: 5,
            spec: Box::new(keyed_spec("wt|claude|w")),
        }
    ));
    let id = loop {
        if let Reply::Opened { id, .. } = next_reply(&mut reader) {
            break id;
        }
    };
    assert_eq!(waking.join().unwrap(), Ok(id));
    for entry in lock(&host.terminals).values() {
        entry.terminal.kill();
    }
}

#[test]
fn a_phones_fix_hears_only_the_window_it_asked() {
    let host = Host::for_tests();
    let (window, mut reader) = window(&host);
    let fixing = {
        let host = host.clone();
        std::thread::spawn(move || host.fix_checks("wt".to_owned(), "Fix it".to_owned()))
    };
    let Reply::FixChecks { req, .. } = next_reply(&mut reader) else {
        panic!("expected the checks");
    };
    strangers_answer(&host, &host.fix_checks_waiting, req, || {
        vec![Request::FixChecksResult {
            req,
            error: Some("forged".to_owned()),
        }]
    });
    let given = Request::FixChecksResult { req, error: None };
    assert!(handle(&host, 1, &window, given));
    assert_eq!(fixing.join().unwrap(), Handed::Given);
}

#[test]
fn a_phones_review_hears_only_the_window_it_asked() {
    let host = Host::for_tests();
    let (window, mut reader) = window(&host);
    let reviewing = {
        let host = host.clone();
        std::thread::spawn(move || host.send_review("wt".to_owned(), None, Vec::new()))
    };
    let Reply::SendReview { req, .. } = next_reply(&mut reader) else {
        panic!("expected the review");
    };
    strangers_answer(&host, &host.review_waiting, req, || {
        vec![Request::SendReviewResult { req, error: None }]
    });
    let busy = Request::SendReviewResult {
        req,
        error: Some("The agent is busy".to_owned()),
    };
    assert!(handle(&host, 1, &window, busy));
    assert_eq!(
        reviewing.join().unwrap(),
        Err("The agent is busy".to_owned())
    );
}

#[test]
fn a_phones_delete_hears_only_the_window_it_asked() {
    let host = Host::for_tests();
    let (window, mut reader) = window(&host);
    let deleting = {
        let host = host.clone();
        std::thread::spawn(move || host.delete_work("wt".to_owned(), false))
    };
    let Reply::DeleteWork { req, .. } = next_reply(&mut reader) else {
        panic!("expected the delete");
    };
    strangers_answer(&host, &host.delete_waiting, req, || {
        vec![
            Request::DeleteWorkResult {
                req,
                error: None,
                kept_branch: Some("forged".to_owned()),
            },
            Request::DeleteWorkResult {
                req,
                error: Some("forged".to_owned()),
                kept_branch: None,
            },
        ]
    });
    let deleted = Request::DeleteWorkResult {
        req,
        error: None,
        kept_branch: Some("phone-work".to_owned()),
    };
    assert!(handle(&host, 1, &window, deleted));
    assert_eq!(deleting.join().unwrap(), Ok(Some("phone-work".to_owned())));
}

#[test]
fn the_mouse_passing_over_a_terminal_is_not_typing_at_the_desktop() {
    // SGR motion and wheel reports, legacy motion, focus in and out.
    assert!(only_passive_reports(b"\x1b[<35;10;5M"));
    assert!(only_passive_reports(b"\x1b[<64;10;5M\x1b[<65;10;5M"));
    assert!(only_passive_reports(b"\x1b[M\x43\x2a\x25"));
    assert!(only_passive_reports(b"\x1b[I"));
    assert!(only_passive_reports(b"\x1b[O\x1b[<35;1;1M"));
    // A click, a release, a key, a paste and nothing at all are not.
    assert!(!only_passive_reports(b"\x1b[<0;10;5M"));
    assert!(!only_passive_reports(b"\x1b[<0;10;5m"));
    assert!(!only_passive_reports(b"\x1b[M\x20\x2a\x25"));
    assert!(!only_passive_reports(b"y"));
    assert!(!only_passive_reports(b"\x1b[<35;10;5My"));
    assert!(!only_passive_reports(b"\x1b[<35;10;5"));
    assert!(!only_passive_reports(b""));
}
