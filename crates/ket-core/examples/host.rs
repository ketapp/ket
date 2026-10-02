//! Drives the ket host end to end (Epic 5b.1): a terminal outliving the
//! process that opened it, adoption by the next one, and what the hop through
//! the host costs a keystroke.
//!
//! Refuses to run without a throwaway data directory, because the host lives
//! in it — see AGENTS.md:
//!
//! ```sh
//! export XDG_DATA_HOME="$(mktemp -d)" KET_HOST=1
//! cargo run --release -p ket-core --example host -- first    # opens, starts a loop, exits
//! cargo run --release -p ket-core --example host -- second   # adopts it, checks, kills it
//! cargo run --release -p ket-core --example host -- latency  # echo, in-process vs hosted
//! cargo run --release -p ket-core --example host -- stop     # ends the host and its terminals
//! ```
//!
//! End with `stop`. A host keeps its terminals running with no window
//! attached — that is what it is for — so one left behind in a throwaway
//! directory, with `first`'s loop or a shell still in it, runs until killed.
//!
//! This binary is the host too, when started as one: the first thing `main`
//! does is check.

use std::time::{Duration, Instant};

use ket_core::terminal::alacritty_terminal::grid::Dimensions;
use ket_core::terminal::alacritty_terminal::index::{Column, Line};
use ket_core::terminal::{LocalTerminal, Terminal, TerminalSize, TerminalSpec};

const KEY: &str = "host-example|shell";

fn main() {
    ket_core::host::serve_if_asked();

    if std::env::var_os("XDG_DATA_HOME").is_none() || !ket_core::host::enabled() {
        eprintln!("set XDG_DATA_HOME to a throwaway directory and KET_HOST=1 first");
        std::process::exit(2);
    }
    let result = match std::env::args().nth(1).as_deref() {
        Some("first") => first(),
        Some("second") => second(),
        Some("latency") => latency(),
        Some("flood") => flood(),
        Some("hooks") => hooks(),
        Some("hooks-leave") => hooks_leave(),
        Some("hooks-back") => hooks_back(),
        Some("agent") => agent(),
        Some("identity") => identity(),
        Some("type-in") => type_in(),
        Some("stop") => ket_core::host::stop(true)
            .map(|asked| {
                println!(
                    "host {}",
                    if asked {
                        "asked to stop"
                    } else {
                        "not running"
                    }
                )
            })
            .map_err(|e| e.to_string()),
        _ => Err(
            "usage: host first | second | latency | flood | hooks | hooks-leave | hooks-back | agent | identity | stop"
                .to_owned(),
        ),
    };
    if let Err(error) = result {
        eprintln!("FAILED: {error}");
        std::process::exit(1);
    }
}

fn spec(adopt: bool) -> TerminalSpec {
    let mut spec = TerminalSpec::shell(std::env::temp_dir());
    spec.size = TerminalSize::new(90, 24);
    spec.key = Some(KEY.to_owned());
    spec.adopt = adopt;
    spec
}

/// Opens a hosted shell, starts something long in it, and leaves.
fn first() -> Result<(), String> {
    let terminal = Terminal::open(&spec(false)).map_err(|e| e.to_string())?;
    if !terminal.is_hosted() {
        return Err("the terminal is not hosted".into());
    }
    settle(&terminal, 800);
    send(&terminal, "export SPIKE_MARK=42\r");
    send(
        &terminal,
        "for i in $(seq 1 300); do echo tick $i; sleep 0.05; done\r",
    );
    std::thread::sleep(Duration::from_millis(1200));
    println!("first: last line {:?}", last_line(&terminal));
    println!("first: exiting without killing it");
    Ok(())
}

/// Adopts what `first` left running, checks it is the same shell and the same
/// screen, then kills it.
fn second() -> Result<(), String> {
    let status = ket_core::host::status().map_err(|e| e.to_string())?;
    println!("second: host status before opening: {status:?}");

    let terminal = Terminal::open(&spec(true)).map_err(|e| e.to_string())?;
    println!(
        "second: hosted={} adopted={}",
        terminal.is_hosted(),
        terminal.adopted()
    );
    if !terminal.adopted() {
        return Err("expected to adopt the terminal `first` left".into());
    }
    std::thread::sleep(Duration::from_millis(300));
    let seen = last_line(&terminal);
    println!("second: the loop is still going: {seen:?}");
    if !seen.contains("tick") {
        return Err("the adopted screen shows no ticks".into());
    }

    // Let the loop finish, then prove it is the same shell.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !screen(&terminal).iter().any(|l| l.contains("tick 300")) {
        if Instant::now() > deadline {
            return Err("the loop never finished".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    settle(&terminal, 500);
    send(&terminal, "echo mark=$SPIKE_MARK\r");
    settle(&terminal, 500);
    let same_shell = screen(&terminal).iter().any(|l| l == "mark=42");
    println!("second: same shell (its variable survived): {same_shell}");

    // The screen this window built from a checkpoint and a stream of output,
    // against one built from a fresh checkpoint alone.
    let incremental = cells(&terminal);
    drop(terminal);
    std::thread::sleep(Duration::from_millis(200));
    let again = Terminal::open(&spec(true)).map_err(|e| e.to_string())?;
    if !again.adopted() {
        return Err("could not adopt it a second time".into());
    }
    settle(&again, 300);
    let fresh = cells(&again);
    let differing = incremental
        .iter()
        .zip(&fresh)
        .filter(|(a, b)| a != b)
        .count();
    println!(
        "second: streamed screen vs fresh checkpoint: {} of {} lines differ",
        differing + incremental.len().abs_diff(fresh.len()),
        incremental.len()
    );

    again.kill();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !again.is_closed() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!(
        "second: killed; closed={} status={:?}",
        again.is_closed(),
        again.exit_status()
    );
    drop(again);
    std::thread::sleep(Duration::from_millis(1500));
    println!(
        "second: host status after: {:?}",
        ket_core::host::status().map_err(|e| e.to_string())?
    );

    if !same_shell || differing > 0 {
        return Err("see above".into());
    }
    Ok(())
}

/// Heavy output through the host — enough to overrun the replay buffer, so
/// the window has to be sent fresh checkpoints — then the screen it streamed
/// against one from a fresh checkpoint.
fn flood() -> Result<(), String> {
    let mut spec = spec(false);
    spec.key = Some("host-example|flood".into());
    let terminal = Terminal::open(&spec).map_err(|e| e.to_string())?;
    if !terminal.is_hosted() {
        return Err("not hosted".into());
    }
    settle(&terminal, 500);
    let started = Instant::now();
    send(
        &terminal,
        "seq 1 300000; printf '\\033[1;31mdone\\033[0m\\n'\r",
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while !screen(&terminal).iter().any(|l| l == "done") {
        if Instant::now() > deadline {
            return Err("never finished".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("flood: 300,000 lines shown in {:?}", started.elapsed());
    settle(&terminal, 300);
    let streamed = cells(&terminal);
    drop(terminal);
    std::thread::sleep(Duration::from_millis(200));
    let mut again = spec.clone();
    again.adopt = true;
    let fresh_terminal = Terminal::open(&again).map_err(|e| e.to_string())?;
    settle(&fresh_terminal, 300);
    let fresh = cells(&fresh_terminal);
    let differing = streamed.iter().zip(&fresh).filter(|(a, b)| a != b).count()
        + streamed.len().abs_diff(fresh.len());
    println!(
        "flood: streamed screen vs fresh checkpoint: {differing} of {} lines differ",
        streamed.len()
    );
    fresh_terminal.kill();
    if differing > 0 {
        return Err("see above".into());
    }
    Ok(())
}

/// A shell set up the way a window sets up an agent's: with the window's own
/// hook listener's address, which the host must replace with its own.
fn hook_spec(key: &str, adopt: bool) -> TerminalSpec {
    use ket_core::agent_hooks::{PANE_ENV, PORT_ENV, TOKEN_ENV, VERSION_ENV, WORKTREE_ENV};
    let mut spec = spec(adopt);
    spec.key = Some(key.to_owned());
    for (name, value) in [
        (PORT_ENV, "1"),
        (TOKEN_ENV, "the-window's-token"),
        (WORKTREE_ENV, "spike-worktree"),
        (PANE_ENV, "7"),
        (VERSION_ENV, "1"),
        ("KET_AGENT_NAME", "claude"),
    ] {
        spec.env.insert(name.to_owned(), value.to_owned());
    }
    spec
}

/// What an agent's hook script does, reduced to one prompt report.
fn post_hook(session: &str) -> String {
    post_event("UserPromptSubmit", session)
}

/// What an agent's hook script does for one event.
fn post_event(event: &str, session: &str) -> String {
    format!(
        "printf '{{\"agent\":\"claude\",\"worktree\":\"%s\",\"pane\":\"%s\",\"token\":\"%s\",\"version\":\"1\",\"payload\":{{\"hook_event_name\":\"{event}\",\"session_id\":\"{session}\"}}}}' \
         \"$KET_WORKTREE_ID\" \"$KET_PANE_KEY\" \"$KET_LAUNCH_TOKEN\" \
         | curl -s -o /dev/null --data-binary @- \"http://127.0.0.1:$KET_HOOK_PORT/hook\"; echo posted=$?"
    )
}

fn wait_for_reports(want: usize) -> Vec<ket_core::agent_hooks::HookReport> {
    let mut reports = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while reports.len() < want && Instant::now() < deadline {
        reports.extend(ket_core::host::take_hook_reports());
        std::thread::sleep(Duration::from_millis(50));
    }
    reports
}

/// An agent in a hosted terminal reports to the host, and the window hears it.
fn hooks() -> Result<(), String> {
    let terminal =
        Terminal::open(&hook_spec("host-example|hooks", false)).map_err(|e| e.to_string())?;
    settle(&terminal, 500);
    send(&terminal, "echo port=$KET_HOOK_PORT\r");
    send(&terminal, &format!("{}\r", post_hook("live-session")));
    settle(&terminal, 500);
    let port = screen(&terminal)
        .into_iter()
        .find(|l| l.starts_with("port="))
        .unwrap_or_default();
    println!("hooks: the agent was given {port} (the window asked for port=1)");
    let reports = wait_for_reports(1);
    println!("hooks: reports the window heard: {reports:?}");
    terminal.kill();
    if port == "port=1" || reports.len() != 1 {
        return Err("see above".into());
    }
    Ok(())
}

/// Leaves a hosted "agent" that goes through a turn, four seconds a step:
/// thinking, a permission prompt, then done — for a phone watching with
/// `phone watch` to see its worktree move through each.
///
/// `host agent <worktree id>` files it under a worktree ket's store knows.
fn agent() -> Result<(), String> {
    let worktree = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "spike-worktree".into());
    let key = format!("{worktree}|claude|1");
    let mut spec = hook_spec(&key, false);
    for (name, value) in [
        (ket_core::agent_hooks::PANE_ENV, key.as_str()),
        (ket_core::agent_hooks::WORKTREE_ENV, worktree.as_str()),
    ] {
        spec.env.insert(name.to_owned(), value.to_owned());
    }
    let terminal = Terminal::open(&spec).map_err(|e| e.to_string())?;
    settle(&terminal, 500);
    let steps = ["UserPromptSubmit", "PermissionRequest", "Stop"]
        .map(|event| post_event(event, "agent-example"))
        .join("; sleep 4; ");
    send(&terminal, &format!("sleep 4; {steps}\r"));
    println!("agent: left running; it reports in 4, 8 and 12 seconds");
    Ok(())
}

/// Leaves an agent that reports two seconds later, when no window is there.
fn hooks_leave() -> Result<(), String> {
    let terminal =
        Terminal::open(&hook_spec("host-example|backlog", false)).map_err(|e| e.to_string())?;
    settle(&terminal, 500);
    send(
        &terminal,
        &format!("sleep 2; {}\r", post_hook("while-away")),
    );
    std::thread::sleep(Duration::from_millis(300));
    println!("hooks-leave: leaving before it reports");
    Ok(())
}

/// Comes back, and should hear what was reported while it was away.
fn hooks_back() -> Result<(), String> {
    let terminal =
        Terminal::open(&hook_spec("host-example|backlog", true)).map_err(|e| e.to_string())?;
    let reports = wait_for_reports(1);
    println!(
        "hooks-back: adopted={} reports from while it was away: {reports:?}",
        terminal.adopted()
    );
    terminal.kill();
    if reports.len() != 1 || reports[0].session.as_deref() != Some("while-away") {
        return Err("see above".into());
    }
    Ok(())
}

/// The host's identity for phones: stable across loads, owner-only on disk,
/// and a phone paired before a restart still gets in after one.
fn identity() -> Result<(), String> {
    use ket_core::host::identity;
    use ket_remote::proto::envelope::Payload;
    use std::os::unix::fs::PermissionsExt;

    let first = identity::load_or_create().map_err(|e| e.to_string())?;
    let again = identity::load_or_create().map_err(|e| e.to_string())?;
    let stable = first.id == again.id && first.public() == again.public();
    println!("identity: the same id and key on a second load: {stable}");

    let dir = identity::dir().map_err(|e| e.to_string())?;
    let mode = |path: std::path::PathBuf| {
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0)
    };
    let (dir_mode, file_mode) = (mode(dir.clone()), mode(dir.join("identity.json")));
    println!("identity: directory {dir_mode:o}, identity.json {file_mode:o}");

    // Pair a phone with the loaded host, name it from its hello, save.
    let mut host = first;
    let phone = ket_remote::Keypair::generate().map_err(|e| e.to_string())?;
    let offer = host.offer("wss://relay.invalid/", Duration::from_secs(60));
    let (phone_end, host_end) = ket_remote::memory_pipe();
    let accepting = std::thread::spawn(move || {
        let result = host.accept(host_end).map(|(_, peer)| peer);
        (host, result)
    });
    let mut session = ket_remote::pair(&phone, &offer, phone_end).map_err(|e| e.to_string())?;
    session
        .send_envelope(&ket_remote::envelope(
            1,
            Payload::Hello(ket_remote::proto::Hello {
                device_name: "My test phone".into(),
                ..Default::default()
            }),
        ))
        .map_err(|e| e.to_string())?;
    let (mut host, peer) = accepting.join().map_err(|_| "host thread")?;
    let peer = peer.map_err(|e| e.to_string())?;
    let hello = <ket_remote::proto::Envelope as prost::Message>::decode(peer.first.as_slice())
        .map_err(|e| e.to_string())?;
    if let Some(Payload::Hello(hello)) = hello.payload {
        host.name_device(&peer.device, &hello.device_name);
    }
    identity::save(&host).map_err(|e| e.to_string())?;
    drop(session);

    // "Restart": load it back, and reconnect as the same phone.
    let mut reloaded = identity::load_or_create().map_err(|e| e.to_string())?;
    let remembered = reloaded
        .grants()
        .iter()
        .any(|g| g.device == phone.public && g.name == "My test phone");
    println!("identity: the paired phone and its name survive a reload: {remembered}");
    let (id, public) = (reloaded.id, reloaded.public());
    let (phone_end, host_end) = ket_remote::memory_pipe();
    let accepting = std::thread::spawn(move || reloaded.accept(host_end).map(|_| ()));
    let resumed = ket_remote::resume(&phone, &id, &public, phone_end).and_then(|mut s| {
        s.send(b"hello again")?;
        Ok(())
    });
    let accepted = accepting.join().map_err(|_| "host thread")?;
    let reconnects = resumed.is_ok() && accepted.is_ok();
    println!("identity: it reconnects after the reload: {reconnects}");

    if !(stable && dir_mode == 0o700 && file_mode == 0o600 && remembered && reconnects) {
        return Err("see above".into());
    }
    Ok(())
}

/// Types into the terminal `first` left, as a window would — which takes
/// control of it back from any phone.
fn type_in() -> Result<(), String> {
    let text = std::env::args().nth(2).unwrap_or_default();
    let terminal = Terminal::open(&spec(true)).map_err(|e| e.to_string())?;
    if !terminal.adopted() {
        return Err("no terminal to type into".into());
    }
    send(&terminal, &text);
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

/// Keystroke to screen, in-process and through the host.
///
/// `sleep` is the program, so the echo is the kernel's line discipline and the
/// only thing measured is ket: the write, the read, the parse, and for the
/// hosted one the two trips across the socket.
fn latency() -> Result<(), String> {
    let mut spec = spec(false);
    spec.command = "/bin/sleep".into();
    spec.args = vec!["60".into()];
    spec.key = None;

    let local = LocalTerminal::open(&spec).map_err(|e| e.to_string())?;
    let local_times = measure(
        |b| local.send_input(b).is_ok(),
        |col| local.with_term(|t| t.grid().cursor.point.column.0 >= col),
    );
    local.kill();

    let hosted = Terminal::open(&spec).map_err(|e| e.to_string())?;
    if !hosted.is_hosted() {
        return Err("not hosted".into());
    }
    let hosted_times = measure(
        |b| hosted.send_input(b).is_ok(),
        |col| hosted.with_term(|t| t.grid().cursor.point.column.0 >= col),
    );
    hosted.kill();

    println!("echo latency, in-process: {}", summary(local_times));
    println!("echo latency, hosted:     {}", summary(hosted_times));
    Ok(())
}

fn measure(send: impl Fn(Vec<u8>) -> bool, reached: impl Fn(usize) -> bool) -> Vec<Duration> {
    std::thread::sleep(Duration::from_millis(300));
    let mut times = Vec::new();
    for n in 1..=60 {
        let started = Instant::now();
        if !send(b"x".to_vec()) {
            break;
        }
        while !reached(n) {
            if started.elapsed() > Duration::from_secs(1) {
                break;
            }
            std::hint::spin_loop();
        }
        times.push(started.elapsed());
        std::thread::sleep(Duration::from_millis(20));
    }
    times
}

fn summary(mut times: Vec<Duration>) -> String {
    times.sort_unstable();
    let at = |q: f64| {
        times
            .get(((times.len() as f64 - 1.0) * q) as usize)
            .copied()
    };
    format!(
        "median {:?}, p90 {:?}, max {:?} ({} samples)",
        at(0.5).unwrap_or_default(),
        at(0.9).unwrap_or_default(),
        times.last().copied().unwrap_or_default(),
        times.len()
    )
}

fn send(terminal: &Terminal, text: &str) {
    let _ = terminal.send_input(text.as_bytes().to_vec());
}

/// Waits until no frame has arrived for `ms`.
fn settle(terminal: &Terminal, ms: u64) {
    let mut last = terminal.frame();
    let mut quiet = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        let now = terminal.frame();
        if now != last {
            last = now;
            quiet = Instant::now();
        } else if quiet.elapsed() >= Duration::from_millis(ms) {
            return;
        }
    }
}

fn screen(terminal: &Terminal) -> Vec<String> {
    terminal.with_term(|term| {
        let grid = term.grid();
        (0..grid.screen_lines() as i32)
            .map(|line| {
                let row = &grid[Line(line)];
                (0..grid.columns())
                    .map(|col| row[Column(col)].c)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    })
}

fn last_line(terminal: &Terminal) -> String {
    screen(terminal)
        .into_iter()
        .rev()
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// Every line of the grid, scrollback included, with every cell's attributes.
fn cells(terminal: &Terminal) -> Vec<String> {
    terminal.with_term(|term| {
        let grid = term.grid();
        (grid.topmost_line().0..=grid.bottommost_line().0)
            .map(|line| {
                let row = &grid[Line(line)];
                (0..grid.columns())
                    .map(|col| {
                        let cell = &row[Column(col)];
                        format!("{}{:?}{:?}{:?}", cell.c, cell.fg, cell.bg, cell.flags)
                    })
                    .collect::<String>()
            })
            .collect()
    })
}
