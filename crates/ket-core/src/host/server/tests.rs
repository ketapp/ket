//! The host's own tests: terminals, windows, phones' requests through it.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use super::*;
use crate::terminal::{LocalTerminal, TerminalSize, TerminalSpec};

fn spec(command: &str, args: &[&str]) -> TerminalSpec {
    TerminalSpec {
        command: command.to_owned(),
        args: args.iter().map(|&a| a.to_owned()).collect(),
        cwd: std::env::temp_dir(),
        env: std::collections::BTreeMap::new(),
        env_remove: Vec::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
        adopt_only: false,
    }
}

fn keyed_spec(key: &str) -> TerminalSpec {
    TerminalSpec {
        key: Some(key.to_owned()),
        ..spec("sh", &["-c", "sleep 30"])
    }
}

/// An `Out` a test can `handle()` against, and the socket end a test
/// reads its replies from.
fn out_pair() -> (Arc<Out>, UnixStream) {
    let (a, b) = UnixStream::pair().expect("a socket pair");
    let out = Arc::new(Out {
        stream: Mutex::new(a),
        alive: AtomicBool::new(true),
        greeted: AtomicBool::new(false),
        window: true,
    });
    (out, b)
}

fn next_frame(reader: &mut impl Read) -> Frame {
    wire::read(reader).expect("a frame")
}

fn next_reply(reader: &mut impl Read) -> Reply {
    match next_frame(reader) {
        Frame::Json(body) => serde_json::from_slice(&body).expect("a reply"),
        other => panic!("expected json, got {other:?}"),
    }
}

fn wait_until(limit: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if ready() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn hello_replies_with_a_welcome_then_the_status_snapshot() {
    let host = Host::for_tests();
    let (out, mut reader) = out_pair();

    let still_open = handle(
        &host,
        1,
        &out,
        Request::Hello {
            protocol: wire::PROTOCOL,
            build: "some-window-build".to_owned(),
        },
    );
    assert!(still_open);
    assert!(out.greeted.load(Ordering::Acquire));

    match next_reply(&mut reader) {
        Reply::Welcome {
            protocol,
            build,
            live,
        } => {
            assert_eq!(protocol, wire::PROTOCOL);
            assert_eq!(build, host.build);
            assert_eq!(live, 0);
        }
        other => panic!("expected a welcome, got {other:?}"),
    }
    match next_reply(&mut reader) {
        Reply::AgentStatus { .. } => {}
        other => panic!("expected the status snapshot, got {other:?}"),
    }
}

/// A CLI connected while no window is open — `ket host devices` — once
/// took the backlog and counted as listening, losing reports a window
/// wanted for usage.
#[test]
fn only_a_window_takes_hook_traffic() {
    let host = Host::for_tests();
    let hello = || Request::Hello {
        protocol: wire::PROTOCOL,
        build: String::new(),
    };
    lock(&host.backlog).0.push_back(prompt_report("before"));

    let (stream, mut cli_reader) = UnixStream::pair().expect("a socket pair");
    let cli = Arc::new(Out {
        stream: Mutex::new(stream),
        alive: AtomicBool::new(true),
        greeted: AtomicBool::new(false),
        window: false,
    });
    lock(&host.outs).push((1, cli.clone()));
    assert!(handle(&host, 1, &cli, hello()));
    assert!(matches!(next_reply(&mut cli_reader), Reply::Welcome { .. }));
    assert!(matches!(
        next_reply(&mut cli_reader),
        Reply::AgentStatus { .. }
    ));
    pass_on_hooks(&host, vec![prompt_report("during")], Vec::new());
    assert_eq!(lock(&host.backlog).0.len(), 2, "kept for a window");
    cli_reader.set_nonblocking(true).expect("nonblocking");
    let mut byte = [0u8];
    assert!(
        matches!(cli_reader.read(&mut byte), Err(e) if e.kind() == io::ErrorKind::WouldBlock),
        "the CLI is sent no hook traffic"
    );

    let (window, mut reader) = out_pair();
    lock(&host.outs).push((2, window.clone()));
    assert!(handle(&host, 2, &window, hello()));
    assert!(matches!(next_reply(&mut reader), Reply::Welcome { .. }));
    assert!(matches!(next_reply(&mut reader), Reply::AgentStatus { .. }));
    let panes: Vec<_> = (0..2)
        .map(|_| match next_reply(&mut reader) {
            Reply::Hook { report } => report.pane,
            other => panic!("expected a backlogged report, got {other:?}"),
        })
        .collect();
    assert_eq!(panes, [Some("before".into()), Some("during".into())]);
    assert!(lock(&host.backlog).0.is_empty());

    pass_on_hooks(&host, vec![prompt_report("after")], Vec::new());
    match next_reply(&mut reader) {
        Reply::Hook { report } => assert_eq!(report.pane.as_deref(), Some("after")),
        other => panic!("expected a live report, got {other:?}"),
    }
    assert!(lock(&host.backlog).0.is_empty());
}

#[test]
fn devices_and_pairing_requests_answer_honestly_with_no_phone_side() {
    let host = Host::for_tests();

    let (out, mut reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::Devices));
    match next_reply(&mut reader) {
        Reply::Devices { devices, hosted } => {
            assert!(devices.is_none());
            assert!(hosted.is_none());
        }
        other => panic!("expected devices, got {other:?}"),
    }

    let (out, mut reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::Pairings));
    match next_reply(&mut reader) {
        Reply::Pairings { pairings, .. } => assert!(pairings.is_empty()),
        other => panic!("expected pairings, got {other:?}"),
    }

    let (out, mut reader) = out_pair();
    assert!(handle(
        &host,
        1,
        &out,
        Request::DecidePairingRole {
            id: 1,
            role: Some(wire::DeviceRole::Controller)
        }
    ));
    match next_reply(&mut reader) {
        Reply::PairingDecided { found } => assert!(!found),
        other => panic!("expected a pairing decision, got {other:?}"),
    }

    let (out, mut reader) = out_pair();
    assert!(handle(
        &host,
        1,
        &out,
        Request::Revoke {
            id: "anything".to_owned()
        }
    ));
    match next_reply(&mut reader) {
        Reply::Revoked { found, error } => {
            assert!(!found);
            assert!(error.is_some());
        }
        other => panic!("expected a revoked reply, got {other:?}"),
    }

    let (out, mut reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::PairingCode));
    match next_reply(&mut reader) {
        Reply::PairingCode { code, error } => {
            assert!(code.is_none());
            assert!(error.is_some());
        }
        other => panic!("expected a pairing code reply, got {other:?}"),
    }
}

#[test]
fn worktrees_and_usage_requests_are_stored() {
    let host = Host::for_tests();
    let (out, _reader) = out_pair();

    let worktrees = vec![("w1".to_owned(), PathBuf::from("/tmp/w1"))];
    assert!(handle(
        &host,
        1,
        &out,
        Request::Worktrees {
            worktrees: worktrees.clone(),
        }
    ));

    let providers = vec![crate::rate_limits::ProviderSnapshot::unavailable(
        crate::rate_limits::Provider::Claude,
        "no key",
    )];
    assert!(handle(
        &host,
        1,
        &out,
        Request::Usage {
            providers: providers.clone(),
        }
    ));
    assert_eq!(host.usage(), providers);
}

#[test]
fn interrupt_on_an_unknown_pane_changes_nothing_and_broadcasts_nothing() {
    let host = Host::for_tests();
    let (out, mut reader) = out_pair();

    assert!(handle(
        &host,
        1,
        &out,
        Request::Interrupt {
            pane: "no-such-pane".to_owned(),
            cancel: true,
        }
    ));

    // No broadcast means nothing more was ever written to this window.
    reader
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("a read timeout");
    let mut buf = [0u8; 1];
    let error = reader.read(&mut buf).expect_err("nothing arrives");
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
}

#[test]
fn open_starts_a_real_terminal_and_a_checkpoint_follows_it() {
    let host = Host::for_tests();
    let (out, mut reader) = out_pair();

    assert!(handle(
        &host,
        1,
        &out,
        Request::Open {
            req: 7,
            spec: Box::new(spec("sh", &["-c", "sleep 30"])),
        }
    ));

    let id = match next_reply(&mut reader) {
        Reply::Opened {
            req, id, adopted, ..
        } => {
            assert_eq!(req, 7);
            assert!(!adopted);
            id
        }
        other => panic!("expected opened, got {other:?}"),
    };

    match next_frame(&mut reader) {
        Frame::Checkpoint {
            id: checkpoint_id, ..
        } => assert_eq!(checkpoint_id, id),
        other => panic!("expected a checkpoint, got {other:?}"),
    }

    host.find(id).expect("filed").kill();
}

#[test]
fn open_with_adopt_reuses_a_terminal_no_one_is_watching() {
    let host = Host::for_tests();

    let (out1, mut reader1) = out_pair();
    assert!(handle(
        &host,
        1,
        &out1,
        Request::Open {
            req: 1,
            spec: Box::new(keyed_spec("k1")),
        }
    ));
    let id1 = match next_reply(&mut reader1) {
        Reply::Opened { id, .. } => id,
        other => panic!("expected opened, got {other:?}"),
    };

    // Not adoptable yet: connection 1 is still watching it.
    assert!(adoptable(&host, Some("k1")).is_none());

    assert!(handle(&host, 1, &out1, Request::Detach { id: id1 }));
    assert!(adoptable(&host, Some("k1")).is_some());

    let (out2, mut reader2) = out_pair();
    assert!(handle(
        &host,
        2,
        &out2,
        Request::Open {
            req: 2,
            spec: Box::new(TerminalSpec {
                adopt: true,
                adopt_only: true,
                ..keyed_spec("k1")
            }),
        }
    ));
    match next_reply(&mut reader2) {
        Reply::Opened { id, adopted, .. } => {
            assert!(adopted);
            assert_eq!(id, id1);
        }
        other => panic!("expected an adopted open, got {other:?}"),
    }

    host.find(id1).expect("filed").kill();
}

#[test]
fn resize_is_deferred_while_a_phone_has_the_terminal_fitted() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));
    let desktop_size = host.find(id).expect("filed").size();

    assert!(host.fit(id, 999, TerminalSize::new(40, 20)));
    assert_eq!(
        host.find(id).expect("filed").size(),
        TerminalSize::new(40, 20)
    );

    let (out, _reader) = out_pair();
    assert!(handle(
        &host,
        1,
        &out,
        Request::Resize {
            id,
            size: TerminalSize::new(120, 50),
        }
    ));
    assert_eq!(
        host.find(id).expect("filed").size(),
        TerminalSize::new(40, 20),
        "a fitted terminal keeps the phone's size"
    );

    host.unfit(id, 999);
    assert_eq!(
        host.find(id).expect("filed").size(),
        TerminalSize::new(120, 50),
        "unfitting restores the desktop's most recent size"
    );
    let _ = desktop_size;

    host.find(id).expect("filed").kill();
}

#[test]
fn resize_applies_immediately_when_nothing_has_it_fitted() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));

    let (out, _reader) = out_pair();
    assert!(handle(
        &host,
        1,
        &out,
        Request::Resize {
            id,
            size: TerminalSize::new(100, 40),
        }
    ));
    assert_eq!(
        host.find(id).expect("filed").size(),
        TerminalSize::new(100, 40)
    );

    host.find(id).expect("filed").kill();
}

#[test]
fn kill_marks_the_entry_killed_and_ends_the_process() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));

    let (out, _reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::Kill { id }));

    assert!(
        wait_until(Duration::from_secs(5), || host
            .find(id)
            .is_some_and(|t| t.is_closed())),
        "a killed terminal closes"
    );
}

#[test]
fn clear_is_applied_to_the_named_terminal() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));

    let (out, _reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::Clear { id }));
    assert!(!host.find(id).expect("filed").is_closed());

    host.find(id).expect("filed").kill();
}

#[test]
fn detach_only_releases_this_connections_watcher() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));

    let (out_a, _reader_a) = out_pair();
    let detached_a = Arc::new(AtomicBool::new(false));
    let (out_b, _reader_b) = out_pair();
    let detached_b = Arc::new(AtomicBool::new(false));
    lock(&host.terminals)
        .get_mut(&id)
        .unwrap()
        .watchers
        .extend([
            Watcher {
                out: out_a,
                detached: detached_a.clone(),
                conn: 1,
            },
            Watcher {
                out: out_b,
                detached: detached_b.clone(),
                conn: 2,
            },
        ]);

    let (out, _reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::Detach { id }));

    assert!(detached_a.load(Ordering::Acquire));
    assert!(!detached_b.load(Ordering::Acquire));

    host.find(id).expect("filed").kill();
}

#[test]
fn shutdown_without_force_keeps_running_while_a_terminal_is_live() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));

    let (out, _reader) = out_pair();
    // `force: true`, and `force: false` while nothing is live, both exit
    // this process — never exercised here.
    let still_open = handle(&host, 1, &out, Request::Shutdown { force: false });
    assert!(still_open, "a live terminal keeps the host up");

    host.find(id).expect("filed").kill();
}

#[test]
fn refuse_outsider_refuses_only_the_window_only_requests() {
    let (out, mut reader) = out_pair();
    assert!(refuse_outsider(
        &out,
        &Request::DecidePairingRole {
            id: 1,
            role: Some(wire::DeviceRole::Controller),
        }
    ));
    match next_reply(&mut reader) {
        Reply::PairingDecided { found } => assert!(!found),
        other => panic!("expected a pairing decision, got {other:?}"),
    }

    let (out, mut reader) = out_pair();
    assert!(refuse_outsider(&out, &Request::Phones { enabled: true }));
    match next_reply(&mut reader) {
        Reply::Phones { serving, error } => {
            assert!(!serving);
            assert!(error.is_some());
        }
        other => panic!("expected a phones reply, got {other:?}"),
    }

    let (out, _reader) = out_pair();
    assert!(
        !refuse_outsider(&out, &Request::Devices),
        "an ordinary request is not refused"
    );
}

#[test]
fn acquire_grants_a_lease_and_refuses_a_conflicting_holder() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));

    assert!(host.acquire(id, 1));
    assert!(host.controls(id, 1));
    assert!(!host.controls(id, 2));
    assert!(
        !host.acquire(id, 2),
        "a live lease refuses a different holder"
    );
    assert!(host.acquire(id, 1), "the same holder renews its own lease");

    host.release(id, 1);
    assert!(!host.controls(id, 1));
    assert!(host.acquire(id, 2), "a released lease is free");

    host.find(id).expect("filed").kill();
}

#[test]
fn stop_refuses_a_phone_lease_then_accepts_after_release() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));
    let (out, mut reader) = out_pair();
    assert!(host.acquire(id, 1));
    assert!(handle(
        &host,
        1,
        &out,
        Request::Stop {
            req: 7,
            id,
            grace_ms: 10
        }
    ));
    assert!(matches!(
        next_reply(&mut reader),
        Reply::Stopped {
            req: 7,
            outcome: StopOutcome::Controlled
        }
    ));
    assert!(!host.find(id).expect("filed").is_closed());
    host.release(id, 1);
    assert!(handle(
        &host,
        1,
        &out,
        Request::Stop {
            req: 8,
            id,
            grace_ms: 10
        }
    ));
    assert!(matches!(
        next_reply(&mut reader),
        Reply::Stopped {
            req: 8,
            outcome: StopOutcome::Stopped
        }
    ));
    assert!(handle(
        &host,
        1,
        &out,
        Request::Stop {
            req: 9,
            id,
            grace_ms: 10
        }
    ));
    assert!(matches!(
        next_reply(&mut reader),
        Reply::Stopped {
            req: 9,
            outcome: StopOutcome::Gone
        }
    ));
}

#[test]
fn adopt_only_never_starts_a_missing_terminal() {
    let host = Host::for_tests();
    let (out, mut reader) = out_pair();
    let mut spec = keyed_spec("missing-pane");
    spec.adopt = true;
    spec.adopt_only = true;
    assert!(handle(
        &host,
        1,
        &out,
        Request::Open {
            req: 10,
            spec: Box::new(spec)
        }
    ));
    assert!(matches!(
        next_reply(&mut reader),
        Reply::Failed { req: 10, .. }
    ));
    assert!(lock(&host.terminals).is_empty());
}

#[test]
fn listing_excludes_killed_terminals() {
    let host = Host::for_tests();
    let id = host
        .file_for_tests(LocalTerminal::open(&spec("sh", &["-c", "sleep 30"])).expect("a terminal"));
    assert_eq!(host.listing().len(), 1);

    let (out, _reader) = out_pair();
    assert!(handle(&host, 1, &out, Request::Kill { id }));
    assert!(host.listing().is_empty());
}

#[test]
#[cfg(target_os = "macos")]
fn same_program_recognises_its_own_process() {
    let (a, _b) = UnixStream::pair().expect("a socket pair");
    assert!(same_program(&a));
}

/// A hook report from pane `key` of worktree `wt`, asking nothing yet.
fn prompt_report(key: &str) -> agent_hooks::HookReport {
    agent_hooks::HookReport {
        failure: None,
        agent: "claude".to_owned(),
        event: agent_hooks::HookEvent::PermissionRequest,
        worktree: Some("wt".to_owned()),
        cwd: None,
        pane: Some(key.to_owned()),
        session: None,
        tool_name: None,
        note: None,
        mutating_git: false,
        command: None,
        at_ms: crate::now_ms(),
        subagent: None,
        subagent_type: None,
        subagent_description: None,
        subagent_model: None,
        tool_use_id: None,
        teammate: None,
        fresh_session: false,
        background: None,
        question: None,
        call: None,
        ruling: None,
    }
}

/// A greeted window, past its welcome and first status snapshot.
pub(super) fn window(host: &Arc<Host>) -> (Arc<Out>, UnixStream) {
    let (out, mut reader) = out_pair();
    lock(&host.outs).push((1, out.clone()));
    assert!(handle(
        host,
        1,
        &out,
        Request::Hello {
            protocol: wire::PROTOCOL,
            build: "a-window".to_owned(),
        },
    ));
    assert!(matches!(next_reply(&mut reader), Reply::Welcome { .. }));
    assert!(matches!(next_reply(&mut reader), Reply::AgentStatus { .. }));
    reader
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read timeout");
    (out, reader)
}

mod automations;
pub(super) mod handoffs;
