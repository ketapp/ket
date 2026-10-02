//! A stand-in for the phone app (Epic 5b.3), to drive the whole path — host,
//! relay, phone — before there is a phone app.
//!
//! It keeps its key in the data directory, so point `XDG_DATA_HOME` at a
//! throwaway directory, like everything else here:
//!
//! ```sh
//! cargo run -p ket-relay -- 127.0.0.1:7878 &
//! export XDG_DATA_HOME="$(mktemp -d)" KET_HOST=1 KET_RELAY_URL=ws://127.0.0.1:7878
//! cargo run -p ket-core --example host -- first          # a hosted shell, left running
//! code=$(cargo run -p ket-cli -- host pair)
//! cargo run -p ket-core --example phone -- pair "$code"  # pairs, lists terminals
//! cargo run -p ket-core --example phone -- attach 1 'echo hi from the phone'
//! cargo run -p ket-cli -- host stop --force              # ends the host, shell and all
//! ```

use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ket_core::terminal::TerminalSize;
use ket_core::terminal::alacritty_terminal::event::VoidListener;
use ket_core::terminal::alacritty_terminal::grid::Dimensions;
use ket_core::terminal::alacritty_terminal::index::{Column, Line};
use ket_core::terminal::alacritty_terminal::term::{Config, Term};
use ket_core::terminal::alacritty_terminal::vte::ansi::Processor;
use ket_relay_protocol::{Control, Frame, Kind};
use ket_remote::proto::{self, envelope::Payload};
use prost::Message as _;
use tokio_tungstenite::tungstenite::stream::MaybeTlsStream;
use tokio_tungstenite::tungstenite::{Message, WebSocket};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if std::env::var_os("XDG_DATA_HOME").is_none() {
        eprintln!("set XDG_DATA_HOME to a throwaway directory first");
        std::process::exit(2);
    }
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["pair", code] => pair(code),
        ["list"] => list(),
        ["watch", seconds] => watch(seconds.parse().unwrap_or(15)),
        ["attach", id, text] => attach(id.parse().unwrap_or(0), Some(text)),
        ["attach", id] => attach(id.parse().unwrap_or(0), None),
        ["control", id] => control(id.parse().unwrap_or(0)),
        ["interrupt", id] => interrupt(id.parse().unwrap_or(0)),
        _ => Err(
            "usage: phone pair <code> | list | watch <seconds> | attach <terminal> [text] | control <terminal> | interrupt <terminal>"
                .into(),
        ),
    };
    if let Err(error) = result {
        eprintln!("FAILED: {error}");
        std::process::exit(1);
    }
}

type Error = Box<dyn std::error::Error>;

/// What the phone remembers about the host it paired with.
#[derive(serde::Serialize, serde::Deserialize)]
struct Remembered {
    private: String,
    public: String,
    host_id: String,
    host_public: String,
    relay: String,
    /// Where the phone was when it last disconnected: the host's epoch and
    /// the last envelope it applied.
    #[serde(default)]
    cursor: (u64, u64),
}

fn remembered_path() -> Result<std::path::PathBuf, Error> {
    Ok(ket_core::paths::data_dir()?
        .join("remote")
        .join("dev-phone.json"))
}

/// The relay connection, as a pipe for the Noise session.
struct PhonePipe {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    connection: u64,
}

impl PhonePipe {
    /// Dials the relay and asks for the host.
    fn connect(relay: &str, host: [u8; 16]) -> Result<Self, Error> {
        let (mut socket, _) = tokio_tungstenite::tungstenite::connect(relay)?;
        socket.send(Message::Binary(
            Control::Connect { host }.frame(0).encode()?.into(),
        ))?;
        loop {
            let Message::Binary(bytes) = socket.read()? else {
                continue;
            };
            let frame = Frame::decode(&bytes)?;
            match (frame.kind, Control::parse(&frame.body)) {
                (Kind::Control, Ok(Control::Connected)) => {
                    return Ok(Self {
                        socket,
                        connection: frame.connection,
                    });
                }
                (Kind::Control, Ok(Control::Refused { reason })) => {
                    return Err(format!(
                        "the relay refused: reason {reason} (is the host connected?)"
                    )
                    .into());
                }
                _ => {}
            }
        }
    }

    /// Makes reads give up after `timeout`, so a phone can stop listening.
    fn read_timeout(&mut self, timeout: Option<Duration>) {
        if let MaybeTlsStream::Plain(stream) = self.socket.get_mut() {
            let _ = stream.set_read_timeout(timeout);
        }
    }
}

impl ket_remote::Pipe for PhonePipe {
    fn send(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
        let frame = Frame::data(self.connection, frame)
            .encode()
            .map_err(std::io::Error::other)?;
        self.socket
            .send(Message::Binary(frame.into()))
            .map_err(std::io::Error::other)
    }

    fn recv(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            let message = self.socket.read().map_err(|error| match error {
                tokio_tungstenite::tungstenite::Error::Io(io) => io,
                other => std::io::Error::other(other),
            })?;
            let Message::Binary(bytes) = message else {
                continue;
            };
            let frame = Frame::decode(&bytes).map_err(std::io::Error::other)?;
            match frame.kind {
                Kind::Data => return Ok(frame.body),
                Kind::Control if Control::parse(&frame.body) == Ok(Control::Closed) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "the relay closed the connection",
                    ));
                }
                Kind::Control => {}
            }
        }
    }
}

type Session = ket_remote::Session<PhonePipe>;

fn hello(session: &mut Session) -> Result<proto::HostSnapshot, Error> {
    session.send_envelope(&ket_remote::envelope(
        1,
        Payload::Hello(proto::Hello {
            device_name: "phone example".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        }),
    ))?;
    let mut snapshot = None;
    while snapshot.is_none() {
        match session.recv_envelope()?.payload {
            Some(Payload::Welcome(welcome)) => {
                println!(
                    "welcome from {:?}, epoch {}",
                    welcome.host_name, welcome.host_epoch
                );
            }
            Some(Payload::Snapshot(s)) => snapshot = Some(s),
            Some(Payload::Error(e)) => return Err(e.message.into()),
            _ => {}
        }
    }
    Ok(snapshot.unwrap_or_default())
}

fn print_snapshot(snapshot: &proto::HostSnapshot) {
    for project in &snapshot.projects {
        println!("{}", project.name);
        for worktree in &project.worktrees {
            println!(
                "  {}  [{:?}] {} {}",
                if worktree.name.is_empty() {
                    "(no worktree)"
                } else {
                    &worktree.name
                },
                worktree.activity(),
                worktree.agent,
                worktree.agent_state,
            );
            for terminal in &worktree.terminals {
                println!(
                    "    [{}] {}{}",
                    terminal.id,
                    terminal.title,
                    if terminal.closed { " (closed)" } else { "" }
                );
            }
        }
    }
}

fn pair(code: &str) -> Result<(), Error> {
    let offer = ket_remote::Offer::from_code(code)?;
    let keys = ket_remote::Keypair::generate()?;
    let pipe = PhonePipe::connect(&offer.relay, offer.host_id)?;
    let mut session = ket_remote::pair(&keys, &offer, pipe)?;
    let snapshot = hello(&mut session)?;
    println!("paired");
    print_snapshot(&snapshot);

    let path = remembered_path()?;
    std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&Remembered {
            private: B64.encode(keys.private_for_storage()),
            public: B64.encode(&keys.public),
            host_id: B64.encode(offer.host_id),
            host_public: B64.encode(offer.host_public),
            relay: offer.relay.clone(),
            cursor: (0, 0),
        })?,
    )?;
    Ok(())
}

fn reconnect() -> Result<Session, Error> {
    let remembered: Remembered = serde_json::from_slice(&std::fs::read(remembered_path()?)?)?;
    let keys = ket_remote::Keypair::from_parts(
        B64.decode(&remembered.private)?,
        B64.decode(&remembered.public)?,
    )?;
    let host_id: [u8; 16] = B64
        .decode(&remembered.host_id)?
        .try_into()
        .map_err(|_| "host id")?;
    let host_public: [u8; 32] = B64
        .decode(&remembered.host_public)?
        .try_into()
        .map_err(|_| "host key")?;
    let pipe = PhonePipe::connect(&remembered.relay, host_id)?;
    Ok(ket_remote::resume(&keys, &host_id, &host_public, pipe)?)
}

fn list() -> Result<(), Error> {
    let mut session = reconnect()?;
    print_snapshot(&hello(&mut session)?);
    Ok(())
}

/// Attaches to a terminal: rebuilds its screen from the checkpoint and the
/// output after it, optionally types a line, and prints what it shows.
/// Prints every snapshot the host sends for `seconds`: what a phone's list
/// would redraw from as terminals come and go and agents change state.
fn watch(seconds: u64) -> Result<(), Error> {
    let mut session = reconnect()?;
    print_snapshot(&hello(&mut session)?);
    session
        .pipe_mut()
        .read_timeout(Some(Duration::from_millis(300)));
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        if let Some(envelope) = next(&mut session)?
            && let Some(Payload::Snapshot(snapshot)) = &envelope.payload
        {
            println!("--- {:.1}s left", (until - Instant::now()).as_secs_f32());
            print_snapshot(snapshot);
        }
    }
    Ok(())
}

/// Interrupts a terminal's program, the way the app's stop button does.
fn interrupt(id: u64) -> Result<(), Error> {
    let mut session = reconnect()?;
    hello(&mut session)?;
    session
        .pipe_mut()
        .read_timeout(Some(Duration::from_millis(300)));
    let reply = ask(
        &mut session,
        &mut View::default(),
        ket_remote::envelope(
            2,
            Payload::Signal(proto::Signal {
                terminal_id: id,
                kind: proto::signal::Kind::Interrupt as i32,
            }),
        ),
    )?;
    println!("interrupt: acked={}", acked(&reply));
    Ok(())
}

fn attach(id: u64, text: Option<&str>) -> Result<(), Error> {
    let mut session = reconnect()?;
    hello(&mut session)?;
    session.send_envelope(&ket_remote::envelope(
        2,
        Payload::Subscribe(proto::Subscribe { terminal_id: id }),
    ))?;

    let mut screen: Option<(Term<VoidListener>, Processor)> = None;
    let mut typed = text.is_none();
    let deadline = Instant::now() + Duration::from_secs(4);
    session
        .pipe_mut()
        .read_timeout(Some(Duration::from_millis(300)));
    let started = Instant::now();
    while Instant::now() < deadline {
        if !typed && screen.is_some() && started.elapsed() > Duration::from_millis(800) {
            session.send_envelope(&ket_remote::envelope(
                9,
                Payload::Lease(proto::Lease {
                    terminal_id: id,
                    action: proto::lease::Action::Acquire as i32,
                }),
            ))?;
            let line = format!("{}\r", text.unwrap_or_default());
            session.send_envelope(&ket_remote::envelope(
                3,
                Payload::Input(proto::Input {
                    terminal_id: id,
                    bytes: line.into_bytes(),
                }),
            ))?;
            typed = true;
        }
        let envelope = match session.recv() {
            Ok(message) => proto::Envelope::decode(message.as_slice())?,
            Err(ket_remote::Error::Pipe(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        match envelope.payload {
            Some(Payload::Checkpoint(checkpoint)) => {
                let size = TerminalSize::new(checkpoint.cols as u16, checkpoint.rows as u16);
                let config = Config {
                    scrolling_history: checkpoint.scrollback as usize,
                    ..Config::default()
                };
                let mut term = Term::new(config, &size, VoidListener);
                let mut parser = Processor::new();
                parser.advance(&mut term, &checkpoint.ansi);
                println!(
                    "checkpoint: {}x{}, {} bytes",
                    checkpoint.cols,
                    checkpoint.rows,
                    checkpoint.ansi.len()
                );
                screen = Some((term, parser));
            }
            Some(Payload::Chunk(chunk)) => {
                if let Some((term, parser)) = screen.as_mut() {
                    parser.advance(term, &chunk.bytes);
                }
            }
            Some(Payload::Reset(_)) => screen = None,
            Some(Payload::Closed(_)) => {
                println!("the terminal closed");
                break;
            }
            Some(Payload::Error(e)) => return Err(e.message.into()),
            _ => {}
        }
    }

    let Some((term, _)) = screen else {
        return Err("no checkpoint arrived".into());
    };
    let grid = term.grid();
    println!("--- what the phone shows ---");
    let lines: Vec<String> = (0..grid.screen_lines() as i32)
        .map(|line| {
            let row = &grid[Line(line)];
            (0..grid.columns())
                .map(|col| row[Column(col)].c)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    let last = lines.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
    for line in &lines[..=last] {
        println!("{line}");
    }
    Ok(())
}

/// A terminal's screen as the phone rebuilds it, and the last envelope it
/// applied.
#[derive(Default)]
struct View {
    screen: Option<(Term<VoidListener>, Processor)>,
    seq: u64,
}

impl View {
    fn apply(&mut self, envelope: &proto::Envelope) {
        self.seq = self.seq.max(envelope.seq);
        match &envelope.payload {
            Some(Payload::Checkpoint(checkpoint)) => {
                let size = TerminalSize::new(checkpoint.cols as u16, checkpoint.rows as u16);
                let config = Config {
                    scrolling_history: checkpoint.scrollback as usize,
                    ..Config::default()
                };
                let mut term = Term::new(config, &size, VoidListener);
                let mut parser = Processor::new();
                parser.advance(&mut term, &checkpoint.ansi);
                self.screen = Some((term, parser));
            }
            Some(Payload::Chunk(chunk)) => {
                if let Some((term, parser)) = self.screen.as_mut() {
                    parser.advance(term, &chunk.bytes);
                }
            }
            Some(Payload::Reset(_)) => self.screen = None,
            _ => {}
        }
    }

    fn lines(&self) -> Vec<String> {
        let Some((term, _)) = &self.screen else {
            return Vec::new();
        };
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
    }
}

/// The next envelope, or `None` if nothing arrives for a moment.
fn next(session: &mut Session) -> Result<Option<proto::Envelope>, Error> {
    match session.recv() {
        Ok(message) => Ok(Some(proto::Envelope::decode(message.as_slice())?)),
        Err(ket_remote::Error::Pipe(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// Sends a request and waits for the envelope answering it, applying
/// whatever else arrives meanwhile.
fn ask(
    session: &mut Session,
    view: &mut View,
    envelope: proto::Envelope,
) -> Result<proto::Envelope, Error> {
    let id = envelope.request_id;
    session.send_envelope(&envelope)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(reply) = next(session)? {
            view.apply(&reply);
            if reply.request_id == id {
                return Ok(reply);
            }
        }
    }
    Err(format!("no answer to request {id}").into())
}

/// Reads for `how_long`, applying everything.
fn drain(session: &mut Session, view: &mut View, how_long: Duration) -> Result<(), Error> {
    let until = Instant::now() + how_long;
    while Instant::now() < until {
        if let Some(envelope) = next(session)? {
            view.apply(&envelope);
        }
    }
    Ok(())
}

fn refused_for_control(reply: &proto::Envelope) -> bool {
    matches!(&reply.payload, Some(Payload::Error(e)) if e.code == proto::ErrorCode::NotController as i32)
}

fn acked(reply: &proto::Envelope) -> bool {
    matches!(reply.payload, Some(Payload::Ack(_)))
}

/// Leases, idempotency and resuming, against a live host through the relay.
fn control(id: u64) -> Result<(), Error> {
    let mut failures = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {name}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };
    let mut session = reconnect()?;
    let mut view = View::default();
    session.send_envelope(&ket_remote::envelope(
        1,
        Payload::Hello(proto::Hello {
            device_name: "phone example".into(),
            ..Default::default()
        }),
    ))?;
    session
        .pipe_mut()
        .read_timeout(Some(Duration::from_millis(200)));
    let mut epoch = 0;
    while epoch == 0 {
        let Some(envelope) = next(&mut session)? else {
            continue;
        };
        view.apply(&envelope);
        if let Some(Payload::Welcome(welcome)) = &envelope.payload {
            epoch = welcome.host_epoch;
        }
    }
    let request = |n: u64, payload: Payload| ket_remote::envelope(n, payload);
    let input = |n: u64, text: &str, key: &str| {
        let mut envelope = request(
            n,
            Payload::Input(proto::Input {
                terminal_id: id,
                bytes: text.as_bytes().to_vec(),
            }),
        );
        envelope.idempotency_key = key.to_owned();
        envelope
    };

    session.send_envelope(&request(
        2,
        Payload::Subscribe(proto::Subscribe { terminal_id: id }),
    ))?;
    while view.screen.is_none() {
        drain(&mut session, &mut view, Duration::from_millis(100))?;
    }

    let reply = ask(&mut session, &mut view, input(10, "echo sneaky\r", ""))?;
    check(
        "typing without the lease is refused",
        refused_for_control(&reply),
    );

    let reply = ask(
        &mut session,
        &mut view,
        request(
            11,
            Payload::Lease(proto::Lease {
                terminal_id: id,
                action: proto::lease::Action::Acquire as i32,
            }),
        ),
    )?;
    check("the lease is granted", acked(&reply));

    let key = format!("once-{}", ket_core::now_ms());
    let first = ask(
        &mut session,
        &mut view,
        input(12, "echo once-$((40+2))\r", &key),
    )?;
    let retry = ask(
        &mut session,
        &mut view,
        input(13, "echo once-$((40+2))\r", &key),
    )?;
    drain(&mut session, &mut view, Duration::from_millis(800))?;
    let ran = view
        .lines()
        .iter()
        .filter(|l| l.as_str() == "once-42")
        .count();
    check(
        "a keystroke and its retry are both acknowledged",
        acked(&first) && acked(&retry),
    );
    check(
        &format!("the retry was not applied again (ran {ran} time(s))"),
        ran == 1,
    );

    // The desktop types into the same terminal, and takes control back.
    let host = std::env::current_exe()?.with_file_name("host");
    let desk = std::process::Command::new(host)
        .args(["type-in", "echo from the desk\r"])
        .status()?;
    drain(&mut session, &mut view, Duration::from_millis(600))?;
    let reply = ask(&mut session, &mut view, input(14, "echo after\r", ""))?;
    check(
        "a desktop keystroke takes control back",
        desk.success() && refused_for_control(&reply),
    );

    // Leave, and come back with the cursor.
    let cursor = (epoch, view.seq);
    drop(session);
    let mut session = reconnect()?;
    session.send_envelope(&ket_remote::envelope(
        1,
        Payload::Hello(proto::Hello {
            device_name: "phone example".into(),
            resume_epoch: cursor.0,
            resume_seq: cursor.1,
            ..Default::default()
        }),
    ))?;
    let welcome = loop {
        if let Payload::Welcome(welcome) = session
            .recv_envelope()?
            .payload
            .unwrap_or(Payload::Ack(proto::Ack {}))
        {
            break welcome;
        }
    };
    check(
        &format!(
            "reconnecting with its cursor ({}, {}) resumes",
            cursor.0, cursor.1
        ),
        welcome.resumed,
    );

    println!();
    if failures > 0 {
        return Err(format!("{failures} check(s) failed").into());
    }
    println!("all checks passed");
    Ok(())
}
