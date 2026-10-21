//! The phone side end to end, lightly: a real relay on loopback, the host's
//! link to it, and a phone speaking the protocol through it — pairing,
//! who is let back in, and what a phone can do to a terminal. Kept to a few
//! tests while phones are a prototype.
//!
//! Each test runs again in a child process with a throwaway
//! `XDG_DATA_HOME`: pairing and revoking write the host's identity and
//! device list to the data directory, and a test cannot point its own
//! process somewhere else (see `crate::paths`).

use std::net::TcpStream;
use std::process::Command;

use ket_relay_protocol::{Control, Frame, Kind, REFUSED_HOST_OFFLINE};
use ket_remote::proto::envelope::Payload;
use ket_remote::{Keypair, Offer};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::{self, WebSocket, stream::MaybeTlsStream};

use super::keys::answer_keys;
use super::relay::broken;
use super::requests::decision;
use super::snapshot::snapshot;
use super::*;
use crate::agent_hooks::{Decision, Prompt};
use crate::terminal::{LocalTerminal, TerminalSize, TerminalSpec};

/// Longest any one step waits before the test calls it stuck.
const WAIT: Duration = Duration::from_secs(10);

/// Set in the child a test runs in.
const SANDBOXED: &str = "KET_PHONES_TEST_SANDBOX";

#[test]
fn picks_tailnet_ipv4_from_interface_addresses() {
    use std::net::Ipv4Addr;

    let addresses = [
        Ipv4Addr::new(192, 168, 1, 4),
        Ipv4Addr::new(100, 63, 255, 255),
        Ipv4Addr::new(100, 64, 0, 1),
        Ipv4Addr::new(100, 127, 255, 254),
    ];
    assert_eq!(
        tailnet_ip_from(addresses),
        Some(Ipv4Addr::new(100, 64, 0, 1))
    );
    assert_eq!(
        tailnet_ip_from([Ipv4Addr::new(100, 127, 255, 254)]),
        Some(Ipv4Addr::new(100, 127, 255, 254))
    );
    assert_eq!(tailnet_ip_from([Ipv4Addr::new(100, 128, 0, 1)]), None);
}

/// In the parent: runs test `name` again in a child whose data directory
/// is a temporary one, checks it passed, and returns `true`. In that
/// child: `false`, and the test goes on.
fn rerun_sandboxed(name: &str) -> bool {
    if std::env::var_os(SANDBOXED).is_some() {
        assert!(std::env::var_os("XDG_DATA_HOME").is_some_and(|dir| !dir.is_empty()));
        return false;
    }
    let data = std::env::temp_dir().join(format!("ket-phones-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).expect("a data directory");
    let out = Command::new(std::env::current_exe().expect("this test binary"))
        .arg(format!("host::phones::tests::{name}"))
        .args(["--exact", "--test-threads=1", "--nocapture"])
        .env(SANDBOXED, "1")
        .env("XDG_DATA_HOME", &data)
        // Snippets and settings are read from the config directory;
        // the sandbox's own, so the person's never are.
        .env("XDG_CONFIG_HOME", data.join("config"))
        .env_remove(super::super::RELAY_ENV)
        .output()
        .expect("the test runs again");
    let _ = std::fs::remove_dir_all(&data);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    true
}

/// A host with phones on, linked to a relay of its own on a free
/// loopback port.
struct Rig {
    host: Arc<Host>,
    phones: Arc<Phones>,
    relay: String,
}

fn rig() -> Rig {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let relay = format!("ws://{}", listener.local_addr().expect("its address"));
    listener
        .set_nonblocking(true)
        .expect("a nonblocking listener");
    let host = Host::for_tests();
    let phones = Arc::new(Phones {
        identity: Mutex::new(ket_remote::Host::generate().expect("a host identity")),
        roles: Mutex::new(HashMap::new()),
        relay: Relay::External(relay.clone()),
        stopped: AtomicBool::new(false),
        stop: tokio::sync::watch::channel(false).0,
        connected: AtomicUsize::new(0),
        devices: Mutex::new(HashMap::new()),
        next_session: AtomicU64::new(1),
        live: Mutex::new(HashMap::new()),
        activity: Mutex::new(Tracker::new(Vec::<String>::new())),
        places: Mutex::new(None),
        figures: Mutex::new(None),
        backlogs: Mutex::new(None),
        last_snapshot: Mutex::new(Vec::new()),
        pending: AtomicUsize::new(0),
        approvals: Mutex::new(Vec::new()),
        next_approval: AtomicU64::new(1),
        claim: None,
        typing: Typing::default(),
    });
    let (serving, linked) = (host.clone(), phones.clone());
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
            tokio::select! {
                () = ket_relay::serve(listener, None) => {}
                () = link_forever(&serving, &linked) => {}
            }
        });
    });
    Rig {
        host,
        phones,
        relay,
    }
}

/// A phone's connection through the relay.
struct PhonePipe {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    connection: u64,
}

impl PhonePipe {
    /// Connects to host `id`. The relay does not acknowledge a host's
    /// registration, so a phone that arrives first is refused; this tries
    /// again until the host is there.
    fn connect(relay: &str, id: [u8; 16]) -> Self {
        let deadline = Instant::now() + WAIT;
        loop {
            let (mut socket, _) = tungstenite::connect(relay).expect("the relay");
            if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
                stream.set_read_timeout(Some(WAIT)).expect("a read timeout");
            }
            let connect = Control::Connect { host: id }
                .frame(0)
                .encode()
                .expect("a frame");
            socket.send(WsMessage::binary(connect)).expect("sent");
            let frame = loop {
                if let WsMessage::Binary(bytes) = socket.read().expect("the relay answers") {
                    break Frame::decode(&bytes).expect("a frame");
                }
            };
            match Control::parse(&frame.body) {
                Ok(Control::Connected) => {
                    return Self {
                        socket,
                        connection: frame.connection,
                    };
                }
                Ok(Control::Refused {
                    reason: REFUSED_HOST_OFFLINE,
                }) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

impl ket_remote::Pipe for PhonePipe {
    fn send(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
        let bytes = Frame::data(self.connection, frame)
            .encode()
            .map_err(std::io::Error::other)?;
        self.socket
            .send(WsMessage::binary(bytes))
            .map_err(std::io::Error::other)
    }

    /// The next data frame; an error once the connection is closed.
    fn recv(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            match self.socket.read().map_err(std::io::Error::other)? {
                WsMessage::Binary(bytes) => {
                    let frame = Frame::decode(&bytes).map_err(std::io::Error::other)?;
                    if frame.kind == Kind::Data {
                        return Ok(frame.body);
                    }
                    if Control::parse(&frame.body) == Ok(Control::Closed) {
                        return Err(broken());
                    }
                }
                WsMessage::Close(_) => return Err(broken()),
                _ => {}
            }
        }
    }
}

type PhoneSession = ket_remote::Session<PhonePipe>;

fn hello(resume_epoch: u64, resume_seq: u64) -> proto::Envelope {
    ket_remote::envelope(
        0,
        Payload::Hello(proto::Hello {
            device_name: "Test phone".into(),
            resume_epoch,
            resume_seq,
            ..Default::default()
        }),
    )
}

/// Pairs `device` with a fresh code from the host and says hello. Not
/// yet approved.
fn pair(rig: &Rig, device: &Keypair) -> PhoneSession {
    let offer = Offer::from_code(&rig.phones.pairing_code()).expect("a pairing code");
    let pipe = PhonePipe::connect(&offer.relay, offer.host_id);
    let mut session = ket_remote::pair(device, &offer, pipe).expect("the handshake");
    session.send_envelope(&hello(0, 0)).expect("hello");
    session
}

/// Reconnects `device` to the rig's host and says `hello`.
fn resume(
    rig: &Rig,
    device: &Keypair,
    hello: &proto::Envelope,
) -> ket_remote::Result<PhoneSession> {
    let (id, public) = {
        let identity = lock(&rig.phones.identity);
        (identity.id, identity.public())
    };
    let pipe = PhonePipe::connect(&rig.relay, id);
    let mut session = ket_remote::resume(device, &id, &public, pipe)?;
    session.send_envelope(hello)?;
    Ok(session)
}

fn next(session: &mut PhoneSession) -> proto::Envelope {
    session.recv_envelope().expect("an envelope from the host")
}

/// Decides the one pairing waiting, once it is, checking it shows the
/// code the phone was given.
fn decide(rig: &Rig, code: &str, approve: bool) {
    let deadline = Instant::now() + WAIT;
    let waiting = loop {
        if let [waiting] = rig.phones.pairings().as_slice() {
            break waiting.clone();
        }
        assert!(Instant::now() < deadline, "no pairing waited for approval");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(waiting.code, code);
    assert_eq!(waiting.name, "Test phone");
    let role = approve.then_some(DeviceRole::Administrator);
    assert!(rig.phones.decide(waiting.id, role));
}

/// The code a phone that has just paired is asked to wait with.
fn pending(session: &mut PhoneSession) -> String {
    match next(session).payload {
        Some(Payload::PairingPending(pending)) => pending.code,
        other => panic!("the phone was not asked to wait for approval: {other:?}"),
    }
}

/// A phone paired and approved, past its welcome and first snapshot,
/// and the sequence number of that snapshot.
fn paired(rig: &Rig, device: &Keypair) -> (PhoneSession, u64) {
    let mut session = pair(rig, device);
    let code = pending(&mut session);
    decide(rig, &code, true);
    assert!(matches!(
        next(&mut session).payload,
        Some(Payload::Welcome(_))
    ));
    let snapshot = next(&mut session);
    assert!(matches!(snapshot.payload, Some(Payload::Snapshot(_))));
    (session, snapshot.seq)
}

/// Sends a request under its own idempotency key and reads the answer.
fn ask(session: &mut PhoneSession, id: u64, payload: Payload) -> proto::Envelope {
    let mut request = ket_remote::envelope(id, payload);
    request.idempotency_key = format!("test-{id}");
    session.send_envelope(&request).expect("the request");
    next(session)
}

fn is_ack(reply: &proto::Envelope) -> bool {
    matches!(reply.payload, Some(Payload::Ack(_)))
}

fn error_code(reply: &proto::Envelope) -> Option<proto::ErrorCode> {
    match &reply.payload {
        Some(Payload::Error(error)) => proto::ErrorCode::try_from(error.code).ok(),
        _ => None,
    }
}

/// `command` in a terminal the rig's host owns.
fn terminal(rig: &Rig, command: &str, args: &[&str]) -> (u64, Arc<LocalTerminal>) {
    let spec = TerminalSpec {
        command: command.into(),
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        cwd: std::env::temp_dir(),
        env: std::collections::BTreeMap::new(),
        env_remove: Vec::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
        adopt_only: false,
    };
    let id = rig
        .host
        .file_for_tests(LocalTerminal::open(&spec).expect("a terminal"));
    (id, rig.host.find(id).expect("filed"))
}

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_phone_pairs_through_the_relay_once_the_desktop_approves() {
    if rerun_sandboxed("a_phone_pairs_through_the_relay_once_the_desktop_approves") {
        return;
    }
    let rig = rig();
    let mut session = pair(&rig, &Keypair::generate().unwrap());
    let code = pending(&mut session);
    // A scanned code is not enough on its own.
    assert!(rig.phones.devices().is_empty());

    decide(&rig, &code, true);
    let Some(Payload::Welcome(welcome)) = next(&mut session).payload else {
        panic!("no welcome");
    };
    assert_eq!(welcome.host_epoch, epoch());
    assert!(!welcome.resumed);
    assert!(matches!(
        next(&mut session).payload,
        Some(Payload::Snapshot(_))
    ));

    let devices = rig.phones.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Test phone");
    assert!(devices[0].connected);
    let saved = crate::host::identity::load()
        .unwrap()
        .expect("a saved identity");
    assert_eq!(saved.grants().len(), 1);
}

#[test]
fn a_declined_pairing_is_turned_away_and_not_granted() {
    if rerun_sandboxed("a_declined_pairing_is_turned_away_and_not_granted") {
        return;
    }
    let rig = rig();
    let device = Keypair::generate().unwrap();
    let mut session = pair(&rig, &device);
    let code = pending(&mut session);
    decide(&rig, &code, false);

    assert_eq!(
        error_code(&next(&mut session)),
        Some(proto::ErrorCode::Declined)
    );
    assert!(session.recv().is_err(), "the connection stayed open");
    assert!(rig.phones.devices().is_empty());
    assert!(matches!(
        resume(&rig, &device, &hello(0, 0)),
        Err(ket_remote::Error::Refused)
    ));
}

#[test]
fn a_phone_the_host_never_granted_is_refused() {
    if rerun_sandboxed("a_phone_the_host_never_granted_is_refused") {
        return;
    }
    let rig = rig();
    assert!(matches!(
        resume(&rig, &Keypair::generate().unwrap(), &hello(0, 0)),
        Err(ket_remote::Error::Refused)
    ));

    // Nor is a code good twice: the first use spends it.
    let offer = Offer::from_code(&rig.phones.pairing_code()).unwrap();
    let first = PhonePipe::connect(&offer.relay, offer.host_id);
    let _first = ket_remote::pair(&Keypair::generate().unwrap(), &offer, first).unwrap();
    let second = PhonePipe::connect(&offer.relay, offer.host_id);
    assert!(matches!(
        ket_remote::pair(&Keypair::generate().unwrap(), &offer, second),
        Err(ket_remote::Error::Refused)
    ));
    assert!(rig.phones.devices().is_empty());
}

#[test]
fn a_granted_phone_reconnects_and_resumes_without_asking_again() {
    if rerun_sandboxed("a_granted_phone_reconnects_and_resumes_without_asking_again") {
        return;
    }
    let rig = rig();
    let device = Keypair::generate().unwrap();
    let (session, seq) = paired(&rig, &device);
    drop(session);
    eventually("the first session did not end", || {
        rig.phones.connected() == 0
    });

    let mut session = resume(&rig, &device, &hello(epoch(), seq)).expect("let back in");
    let Some(Payload::Welcome(welcome)) = next(&mut session).payload else {
        panic!("a reconnect was not welcomed straight away");
    };
    assert!(welcome.resumed);
    assert!(rig.phones.pairings().is_empty());
    assert_eq!(rig.phones.devices().len(), 1);
}

#[test]
fn a_keystroke_from_a_phone_reaches_the_terminal_once_it_has_control() {
    if rerun_sandboxed("a_keystroke_from_a_phone_reaches_the_terminal_once_it_has_control") {
        return;
    }
    let rig = rig();
    let (id, terminal) = terminal(&rig, "/bin/cat", &[]);
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let input = |bytes: &[u8]| {
        Payload::Input(proto::Input {
            terminal_id: id,
            bytes: bytes.to_vec(),
        })
    };

    let refused = ask(&mut session, 1, input(b"ket-too-soon\r"));
    assert_eq!(error_code(&refused), Some(proto::ErrorCode::NotController));

    let lease = Payload::Lease(proto::Lease {
        terminal_id: id,
        action: proto::lease::Action::Acquire as i32,
    });
    assert!(is_ack(&ask(&mut session, 2, lease)));
    assert!(is_ack(&ask(&mut session, 3, input(b"ket-from-a-phone\r"))));

    let screen = || String::from_utf8_lossy(&terminal.checkpoint().ansi).into_owned();
    eventually("the keystrokes never reached the terminal", || {
        screen().contains("ket-from-a-phone")
    });
    assert!(!screen().contains("ket-too-soon"));
    terminal.kill();
}

#[test]
fn a_message_with_files_pastes_its_image_then_types_the_words_and_names_the_rest() {
    if rerun_sandboxed(
        "a_message_with_files_pastes_its_image_then_types_the_words_and_names_the_rest",
    ) {
        return;
    }
    let rig = rig();
    // A program that asks for bracketed paste, as an agent's input does,
    // and keeps what it is given: a pasted path is wider than the screen.
    let typed = std::env::temp_dir().join(format!("ket-typed-{}", std::process::id()));
    let script = format!("printf '\\033[?2004h'; exec cat > '{}'", typed.display());
    let (id, terminal) = terminal(&rig, "/bin/sh", &["-c", &script]);
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let message = Payload::SendFiles(proto::SendFiles {
        terminal_id: id,
        text: "ket-look-at-these".to_owned(),
        attachments: vec![
            proto::Attachment {
                name: "IMG 2041.jpg".to_owned(),
                data: b"not really a jpeg".to_vec(),
            },
            proto::Attachment {
                name: "../console.txt".to_owned(),
                data: b"a log".to_vec(),
            },
        ],
    });
    // Its answer comes once the words are typed, after whatever else the
    // host sends meanwhile.
    let answer = |session: &mut PhoneSession, request: u64| loop {
        let reply = next(session);
        if reply.request_id == request {
            break reply;
        }
    };

    let mut refused = ket_remote::envelope(1, message.clone());
    refused.idempotency_key = "test-1".to_owned();
    session.send_envelope(&refused).expect("the request");
    let refused = answer(&mut session, 1);
    assert_eq!(error_code(&refused), Some(proto::ErrorCode::NotController));
    let uploads = crate::paths::phone_uploads_dir().unwrap();
    assert!(
        !uploads.exists(),
        "files were kept for a phone without control"
    );

    let lease = Payload::Lease(proto::Lease {
        terminal_id: id,
        action: proto::lease::Action::Acquire as i32,
    });
    assert!(is_ack(&ask(&mut session, 2, lease)));
    let mut request = ket_remote::envelope(3, message);
    request.idempotency_key = "test-3".to_owned();
    session.send_envelope(&request).expect("the request");
    let sent = answer(&mut session, 3);
    assert!(is_ack(&sent), "{sent:?}");

    let kept: Vec<std::path::PathBuf> = std::fs::read_dir(&uploads)
        .unwrap()
        .flatten()
        .flat_map(|dir| std::fs::read_dir(dir.path()).unwrap().flatten())
        .map(|file| file.path())
        .collect();
    let named = |name: &str| {
        kept.iter()
            .find(|path| path.file_name().is_some_and(|file| file == name))
            .unwrap_or_else(|| panic!("{name} was not kept: {kept:?}"))
            .clone()
    };
    let image = named("IMG-2041.jpg");
    let log = named("console.txt");
    assert_eq!(std::fs::read(&log).unwrap(), b"a log");

    let read = || std::fs::read_to_string(&typed).unwrap_or_default();
    eventually("the message never reached the terminal", || {
        read().contains("console.txt")
    });
    let given = read();
    let image = format!("\x1b[200~{}\x1b[201~", image.display());
    let image_at = given.find(&image).expect("the image pasted by its path");
    let words_at = given
        .find("\x1b[200~ket-look-at-these")
        .expect("the words pasted");
    assert!(
        image_at < words_at,
        "the words came before the image: {given:?}"
    );
    assert!(
        given.contains(&format!("Attached files:\n- {}", log.display())),
        "{given:?}"
    );
    terminal.kill();
    let _ = std::fs::remove_file(&typed);
}

#[test]
fn what_a_phone_sends_during_a_hand_over_waits_its_turn() {
    if rerun_sandboxed("what_a_phone_sends_during_a_hand_over_waits_its_turn") {
        return;
    }
    let rig = rig();
    let typed = std::env::temp_dir().join(format!("ket-typed-order-{}", std::process::id()));
    let script = format!("printf '\\033[?2004h'; exec cat > '{}'", typed.display());
    let (id, terminal) = terminal(&rig, "/bin/sh", &["-c", &script]);
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let lease = Payload::Lease(proto::Lease {
        terminal_id: id,
        action: proto::lease::Action::Acquire as i32,
    });
    assert!(is_ack(&ask(&mut session, 2, lease)));
    let photo = |name: &str, text: &str| {
        Payload::SendFiles(proto::SendFiles {
            terminal_id: id,
            text: text.to_owned(),
            attachments: vec![proto::Attachment {
                name: name.to_owned(),
                data: b"jpeg".to_vec(),
            }],
        })
    };
    let keys = |bytes: &[u8]| {
        Payload::Input(proto::Input {
            terminal_id: id,
            bytes: bytes.to_vec(),
        })
    };
    // A message with a photo, then — before its answer — a message typed
    // as the phone types one, then another with a photo.
    let requests = [
        (3, photo("first.jpg", "ket-first")),
        (4, keys(b"ket-second")),
        (5, keys(b"\r")),
        (6, photo("third.jpg", "ket-third")),
    ];
    for (request, payload) in requests {
        let mut envelope = ket_remote::envelope(request, payload);
        envelope.idempotency_key = format!("test-{request}");
        session.send_envelope(&envelope).expect("the request");
    }
    let mut answered = HashSet::new();
    while answered.len() < 4 {
        let reply = next(&mut session);
        if (3..=6).contains(&reply.request_id) {
            assert!(is_ack(&reply), "{reply:?}");
            answered.insert(reply.request_id);
        }
    }

    let given = std::fs::read_to_string(&typed).unwrap_or_default();
    let at = |what: &str| {
        given
            .find(what)
            .unwrap_or_else(|| panic!("{what} never arrived: {given:?}"))
    };
    let order = [
        at("first.jpg\x1b[201~"),
        at("\x1b[200~ket-first\x1b[201~\n"),
        at("ket-second\n"),
        at("third.jpg\x1b[201~"),
        at("\x1b[200~ket-third\x1b[201~\n"),
    ];
    assert!(order.is_sorted(), "out of turn: {given:?}");
    terminal.kill();
    let _ = std::fs::remove_file(&typed);
}

#[test]
fn a_terminate_signal_from_a_phone_ends_the_program() {
    if rerun_sandboxed("a_terminate_signal_from_a_phone_ends_the_program") {
        return;
    }
    let rig = rig();
    let (id, terminal) = terminal(&rig, "/bin/sleep", &["30"]);
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    let signal = Payload::Signal(proto::Signal {
        terminal_id: id,
        kind: proto::signal::Kind::Terminate as i32,
    });
    assert!(is_ack(&ask(&mut session, 1, signal)));
    eventually("the program outlived the signal", || terminal.is_closed());
}

#[test]
fn malformed_input_from_a_phone_ends_only_its_own_session() {
    if rerun_sandboxed("malformed_input_from_a_phone_ends_only_its_own_session") {
        return;
    }
    let rig = rig();
    let id = lock(&rig.phones.identity).id;

    // Not a handshake at all: dropped, without a refusal.
    let mut stranger = PhonePipe::connect(&rig.relay, id);
    ket_remote::Pipe::send(&mut stranger, b"not a ket handshake".to_vec()).unwrap();
    assert!(ket_remote::Pipe::recv(&mut stranger).is_err());

    // A paired phone whose message decrypts but is not an envelope.
    let device = Keypair::generate().unwrap();
    let (mut session, _) = paired(&rig, &device);
    session.send(&[0xff; 11]).unwrap();
    assert!(
        session.recv().is_err(),
        "the session outlived a malformed message"
    );
    eventually("the session did not end", || rig.phones.connected() == 0);

    // The host still serves that phone.
    let mut session = resume(&rig, &device, &hello(0, 0)).expect("let back in");
    assert!(matches!(
        next(&mut session).payload,
        Some(Payload::Welcome(_))
    ));
}

#[test]
fn a_revoked_phone_is_cut_off_and_cannot_reconnect() {
    if rerun_sandboxed("a_revoked_phone_is_cut_off_and_cannot_reconnect") {
        return;
    }
    let rig = rig();
    let device = Keypair::generate().unwrap();
    let (mut session, _) = paired(&rig, &device);

    let id = rig.phones.devices()[0].id.clone();
    assert!(rig.phones.revoke(&id).unwrap());
    assert!(
        session.recv().is_err(),
        "the revoked phone's session stayed open"
    );
    assert!(rig.phones.devices().is_empty());
    assert!(matches!(
        resume(&rig, &device, &hello(0, 0)),
        Err(ket_remote::Error::Refused)
    ));
}

#[test]
fn a_response_fits_only_what_was_asked() {
    use crate::agent_hooks::{Asked, Offered, PermissionQuestion};
    let option = |label: &str| Offered {
        label: label.to_owned(),
        description: String::new(),
    };
    let asked = PermissionQuestion {
        tool: "AskUserQuestion".to_owned(),
        subject: None,
        id: 1,
        prompt: Some(Prompt::Ask {
            questions: vec![
                Asked {
                    question: "Colour?".to_owned(),
                    header: String::new(),
                    options: vec![option("Red"), option("Blue")],
                    multi_select: false,
                },
                Asked {
                    question: "Toppings?".to_owned(),
                    header: String::new(),
                    options: vec![option("Cheese"), option("Ham")],
                    multi_select: true,
                },
            ],
        }),
    };
    let pick = |choices: &[u32], other: &str| proto::respond::Pick {
        choices: choices.to_vec(),
        other: other.to_owned(),
    };
    let respond = |picks: Vec<proto::respond::Pick>| proto::Respond {
        picks,
        ..Default::default()
    };

    assert_eq!(
        decision(&asked, &respond(vec![pick(&[1], ""), pick(&[1, 0, 1], "")])),
        Some(Decision::Answers(vec![
            "Blue".to_owned(),
            "Cheese, Ham".to_owned()
        ]))
    );
    // Written instead of chosen: the words are the answer.
    assert_eq!(
        decision(&asked, &respond(vec![pick(&[], " Green "), pick(&[0], "")])),
        Some(Decision::Answers(vec![
            "Green".to_owned(),
            "Cheese".to_owned()
        ]))
    );
    for misfit in [
        vec![pick(&[0], "")],
        vec![pick(&[0, 1], ""), pick(&[0], "")],
        vec![pick(&[], ""), pick(&[0], "")],
        vec![pick(&[2], ""), pick(&[0], "")],
    ] {
        assert_eq!(decision(&asked, &respond(misfit)), None);
    }

    let plan = PermissionQuestion {
        tool: "ExitPlanMode".to_owned(),
        prompt: Some(Prompt::Plan),
        ..asked.clone()
    };
    let verdict = |approve: bool, feedback: &str| proto::Respond {
        approve,
        feedback: feedback.to_owned(),
        ..Default::default()
    };
    assert_eq!(decision(&plan, &verdict(true, "")), Some(Decision::Approve));
    assert_eq!(
        decision(&plan, &verdict(false, "Add tests")),
        Some(Decision::KeepPlanning("Add tests".to_owned()))
    );
    assert_eq!(decision(&plan, &verdict(false, " ")), None);

    let permission = PermissionQuestion {
        tool: "Bash".to_owned(),
        prompt: None,
        ..asked
    };
    assert_eq!(decision(&permission, &verdict(true, "")), None);
}

// ---- what a paired phone asks for -----------------------------------------

/// A project in the sandbox's store with one worktree in it, the
/// worktree holding one uncommitted change. Returns the project's id,
/// the worktree's id, and the repository.
fn place() -> (String, String, std::path::PathBuf) {
    let data = std::env::var_os("XDG_DATA_HOME").expect("sandboxed");
    let root = std::path::PathBuf::from(data);
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "--initial-branch=main", "--quiet"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "ket test"]);
    std::fs::write(repo.join("README.md"), "# fixture\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "initial"]);

    let workspace = crate::workspace::Workspace::with_paths(
        crate::store::Store::open_default().unwrap(),
        root.join("worktrees"),
    );
    let project = workspace.add_project(&repo).unwrap();
    let worktree = workspace
        .create_worktree(&project.id, "phone-work", None, None)
        .unwrap();
    std::fs::write(worktree.path.join("README.md"), "# fixture\nfrom a phone\n").unwrap();
    std::fs::write(worktree.path.join("new.txt"), "one\ntwo\n").unwrap();
    (project.id.to_string(), worktree.id.to_string(), repo)
}

/// A key per request, so each is applied rather than taken as a retry.
fn change(session: &mut PhoneSession, id: u64, payload: Payload) -> proto::Envelope {
    ask(session, id, payload)
}

fn message(reply: &proto::Envelope) -> String {
    match &reply.payload {
        Some(Payload::Error(error)) => error.message.clone(),
        other => panic!("not an error: {other:?}"),
    }
}

fn backlog_of(reply: &proto::Envelope) -> &proto::Backlog {
    match &reply.payload {
        Some(Payload::Backlog(backlog)) => backlog,
        other => panic!("not a backlog: {other:?}"),
    }
}

fn edit(project: &str, note: &str, action: proto::edit_backlog::Action, title: &str) -> Payload {
    Payload::EditBacklog(proto::EditBacklog {
        project_id: project.to_owned(),
        note_id: note.to_owned(),
        action: action as i32,
        title: title.to_owned(),
        ..Default::default()
    })
}

#[test]
fn the_first_snapshot_lists_the_desktops_projects_and_worktrees() {
    if rerun_sandboxed("the_first_snapshot_lists_the_desktops_projects_and_worktrees") {
        return;
    }
    let (project, worktree, _) = place();
    let rig = rig();
    // A phone sees active terminals and idle worktrees it could open one in.
    let spec = TerminalSpec {
        command: "/bin/sleep".into(),
        args: vec!["30".to_owned()],
        cwd: std::env::temp_dir(),
        env: std::collections::BTreeMap::new(),
        env_remove: Vec::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
        adopt_only: false,
    };
    let filed = rig.host.file_keyed_for_tests(
        LocalTerminal::open(&spec).unwrap(),
        Some(format!("{worktree}|shell|1")),
    );
    let mut session = pair(&rig, &Keypair::generate().unwrap());
    let code = pending(&mut session);
    decide(&rig, &code, true);
    assert!(matches!(
        next(&mut session).payload,
        Some(Payload::Welcome(_))
    ));
    let Some(Payload::Snapshot(snapshot)) = next(&mut session).payload else {
        panic!("no snapshot");
    };
    let listed = snapshot
        .projects
        .iter()
        .find(|p| p.id == project)
        .unwrap_or_else(|| panic!("the project is listed: {:?}", snapshot.projects));
    assert_eq!(listed.name, "repo");
    let shown = listed
        .worktrees
        .iter()
        .find(|w| w.id == worktree)
        .expect("the worktree with a terminal is listed");
    assert_eq!(shown.branch, "phone-work");
    assert_eq!(shown.base, "main");
    assert_eq!(shown.terminals.len(), 1);
    assert_eq!(shown.terminals[0].id, filed);
    let primary = listed
        .worktrees
        .iter()
        .find(|worktree| worktree.id == format!("primary-{project}"))
        .expect("the idle primary checkout is listed");
    assert!(primary.terminals.is_empty());
    // Its uncommitted work, counted for the phone's row.
    assert_eq!(shown.files_changed, 2);
    // No window has read GitHub, so there is no pull request to show.
    assert_eq!(shown.pull_request, None);
    rig.host.find(filed).unwrap().kill();
}

/// A pull request with `failed` failing among its checks.
fn open_pull_request(failed: bool) -> crate::github::PullRequestStatus {
    use crate::github::{Check, CheckState, PullRequestStatus, Review};
    PullRequestStatus {
        number: 17,
        url: "https://github.com/o/r/pull/17".to_owned(),
        branch: "phone-work".to_owned(),
        draft: true,
        review: Review::ChangesRequested,
        state: crate::github::PullRequestState::Open,
        base: "main".to_owned(),
        behind: false,
        unresolved_comments: 0,
        merge_methods: crate::github::MergeMethods::default(),
        checks: vec![
            Check {
                name: "rust".to_owned(),
                state: CheckState::Passed,
                url: String::new(),
            },
            Check {
                name: "clippy".to_owned(),
                state: if failed {
                    CheckState::Failed
                } else {
                    CheckState::Pending
                },
                url: "https://github.com/o/r/actions/runs/1/job/2".to_owned(),
            },
        ],
    }
}

#[test]
fn a_worktrees_pull_request_is_sent_with_its_checks_as_a_window_read_them() {
    let rig = rig();
    rig.host
        .pull_requests_for_tests(vec![("wt-1".to_owned(), open_pull_request(true))]);
    let spec = TerminalSpec {
        command: "/bin/sleep".into(),
        args: vec!["30".to_owned()],
        cwd: std::env::temp_dir(),
        env: std::collections::BTreeMap::new(),
        env_remove: Vec::new(),
        size: TerminalSize::new(80, 24),
        key: None,
        adopt: false,
        adopt_only: false,
    };
    let filed = rig.host.file_keyed_for_tests(
        LocalTerminal::open(&spec).unwrap(),
        Some("wt-1|shell|1".to_owned()),
    );
    let snapshot = snapshot(&rig.host, &rig.phones);
    let shown = snapshot
        .projects
        .iter()
        .flat_map(|project| &project.worktrees)
        .find(|worktree| worktree.id == "wt-1")
        .expect("the worktree with a terminal is listed");
    let open = shown.pull_request.as_ref().expect("its pull request");
    assert_eq!(open.number, 17);
    assert!(open.draft);
    assert_eq!(
        open.review,
        proto::pull_request_summary::Review::ChangesRequested as i32
    );
    let states: Vec<(&str, i32)> = open
        .checks
        .iter()
        .map(|check| (check.name.as_str(), check.state))
        .collect();
    assert_eq!(
        states,
        [
            ("rust", proto::check_summary::State::Passed as i32),
            ("clippy", proto::check_summary::State::Failed as i32),
        ]
    );
    assert_eq!(
        open.checks[1].url,
        "https://github.com/o/r/actions/runs/1/job/2"
    );
    assert_eq!(open.state, proto::pull_request_summary::State::Open as i32);
    assert_eq!(open.base, "main");
    assert!(!open.behind);
    assert!(open.merge_commit_allowed);
    assert!(open.squash_merge_allowed);
    assert!(open.rebase_merge_allowed);
    rig.host.find(filed).unwrap().kill();
}

#[test]
fn fixing_checks_is_checked_then_handed_to_a_window() {
    if rerun_sandboxed("fixing_checks_is_checked_then_handed_to_a_window") {
        return;
    }
    let (_, worktree, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let fix = |worktree: &str| {
        Payload::FixChecks(proto::FixChecks {
            worktree_id: worktree.to_owned(),
        })
    };

    assert_eq!(
        error_code(&change(&mut session, 1, fix("nowhere"))),
        Some(proto::ErrorCode::NotFound)
    );
    assert!(
        message(&change(&mut session, 2, fix(&worktree))).contains("has not found a pull request")
    );
    rig.host
        .pull_requests_for_tests(vec![(worktree.clone(), open_pull_request(false))]);
    assert!(message(&change(&mut session, 3, fix(&worktree))).contains("checks have failed"));
    // Something failed, and no window is open to hand it to.
    rig.host
        .pull_requests_for_tests(vec![(worktree.clone(), open_pull_request(true))]);
    assert!(message(&change(&mut session, 4, fix(&worktree))).contains("open ket on your Mac"));
}

#[test]
fn pull_request_actions_are_refused_when_they_do_not_apply() {
    if rerun_sandboxed("pull_request_actions_are_refused_when_they_do_not_apply") {
        return;
    }
    let (_, worktree, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let merge = |worktree: &str| {
        Payload::MergePullRequest(proto::MergePullRequest {
            worktree_id: worktree.to_owned(),
            method: proto::PullRequestMergeMethod::Squash as i32,
        })
    };
    assert_eq!(
        error_code(&change(&mut session, 1, merge("nowhere"))),
        Some(proto::ErrorCode::NotFound)
    );
    assert!(
        message(&change(&mut session, 2, merge(&worktree)))
            .contains("has not found a pull request")
    );
    rig.host
        .pull_requests_for_tests(vec![(worktree.clone(), open_pull_request(false))]);
    // The fixture is a draft, not behind, not closed.
    assert!(message(&change(&mut session, 3, merge(&worktree))).contains("cannot be merged yet"));
    assert!(
        message(&change(
            &mut session,
            4,
            Payload::UpdatePullRequest(proto::UpdatePullRequest {
                worktree_id: worktree.clone(),
            })
        ))
        .contains("not behind")
    );
    assert!(
        message(&change(
            &mut session,
            5,
            Payload::ReopenPullRequest(proto::ReopenPullRequest {
                worktree_id: worktree.clone(),
            })
        ))
        .contains("not closed")
    );
    assert!(
        message(&change(
            &mut session,
            6,
            Payload::AddressComments(proto::AddressComments {
                worktree_id: worktree,
            })
        ))
        .contains("open ket on your Mac")
    );
}

/// A request to fix `worktree`'s checks under `key`.
fn fix(id: u64, key: &str, worktree: &str) -> proto::Envelope {
    let mut request = ket_remote::envelope(
        id,
        Payload::FixChecks(proto::FixChecks {
            worktree_id: worktree.to_owned(),
        }),
    );
    request.idempotency_key = key.to_owned();
    request
}

/// Reads what the host tells `window` until it is told to fix checks: the
/// request to answer, the worktree and the message for its agent. `None`
/// once the window hears nothing for its read timeout.
fn told_to_fix(window: &mut std::os::unix::net::UnixStream) -> Option<(u64, String, String)> {
    use crate::host::wire::{self, Frame, Reply};
    loop {
        let Ok(frame) = wire::read(window) else {
            return None;
        };
        if let Frame::Json(body) = frame
            && let Ok(Reply::FixChecks {
                req,
                worktree,
                prompt,
            }) = serde_json::from_slice(&body)
        {
            return Some((req, worktree, prompt));
        }
    }
}

/// The answer to request `id`, past the snapshots before it.
fn answer_to(session: &mut PhoneSession, id: u64) -> proto::Envelope {
    loop {
        let envelope = next(session);
        if envelope.request_id == id {
            return envelope;
        }
    }
}

/// Whether `reply` says the agent has the checks, and if so whether it is
/// still starting.
fn handed_over(reply: &proto::Envelope) -> Option<bool> {
    match &reply.payload {
        Some(Payload::ChecksHandedOver(handed)) => Some(handed.starting),
        _ => None,
    }
}

/// A rig with a window, and a pull request on `place()`'s worktree whose
/// failed check another service posted — its log is on that service, so no
/// `gh` runs to read it.
fn fixing_rig() -> (Rig, String, std::os::unix::net::UnixStream) {
    let (_, worktree, _) = place();
    let rig = rig();
    let window = rig.host.window_for_tests();
    window.set_read_timeout(Some(WAIT)).expect("a read timeout");
    let mut open = open_pull_request(true);
    open.checks[1].url = "https://circleci.com/gh/o/r/1".to_owned();
    rig.host
        .pull_requests_for_tests(vec![(worktree.clone(), open)]);
    (rig, worktree, window)
}

#[test]
fn fixing_checks_is_answered_once_the_agent_has_them_even_across_a_reconnect() {
    if rerun_sandboxed("fixing_checks_is_answered_once_the_agent_has_them_even_across_a_reconnect")
    {
        return;
    }
    let (rig, worktree, mut window) = fixing_rig();
    let device = Keypair::generate().unwrap();
    let (mut session, _) = paired(&rig, &device);

    // Read off the session's thread, and answered once the agent has them.
    session.send_envelope(&fix(1, "fix-1", &worktree)).unwrap();
    let (req, fixing, prompt) = told_to_fix(&mut window).expect("the window is told");
    assert_eq!(fixing, worktree);
    assert!(
        prompt.starts_with("The checks on pull request #17 failed: clippy."),
        "{prompt}"
    );
    assert!(
        prompt.contains("clippy's log is at https://circleci.com/gh/o/r/1."),
        "{prompt}"
    );
    rig.host.finish_fix_checks(req, None);
    assert_eq!(handed_over(&answer_to(&mut session, 1)), Some(false));

    // The same key again: the same answer, and the agent not told twice.
    session.send_envelope(&fix(2, "fix-1", &worktree)).unwrap();
    assert_eq!(handed_over(&answer_to(&mut session, 2)), Some(false));
    window
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("a read timeout");
    assert!(told_to_fix(&mut window).is_none(), "handed over twice");
    window.set_read_timeout(Some(WAIT)).expect("a read timeout");

    // The connection drops while the window starts the agent. The phone
    // asks again under the same key from its next one, and is answered
    // with how that went — a failure, here.
    session.send_envelope(&fix(3, "fix-3", &worktree)).unwrap();
    let (req, _, _) = told_to_fix(&mut window).expect("the window is told");
    drop(session);
    eventually("the first session did not end", || {
        rig.phones.connected() == 0
    });
    let mut session = resume(&rig, &device, &hello(0, 0)).expect("let back in");
    session.send_envelope(&fix(4, "fix-3", &worktree)).unwrap();
    // Asked for afresh while that is going, it is refused — which also
    // says the ask again is waiting by now.
    session.send_envelope(&fix(5, "fix-5", &worktree)).unwrap();
    assert!(message(&answer_to(&mut session, 5)).contains("already giving"));
    rig.host
        .finish_fix_checks(req, Some("agent is not configured".to_owned()));
    assert!(message(&answer_to(&mut session, 4)).contains("agent is not configured"));
}

#[test]
fn an_agent_slow_to_start_on_fixed_checks_is_still_starting_not_failed() {
    if rerun_sandboxed("an_agent_slow_to_start_on_fixed_checks_is_still_starting_not_failed") {
        return;
    }
    let (rig, worktree, mut window) = fixing_rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    session.send_envelope(&fix(1, "fix-1", &worktree)).unwrap();
    assert!(told_to_fix(&mut window).is_some());
    // The window never says: the host's wait runs out.
    assert_eq!(handed_over(&answer_to(&mut session, 1)), Some(true));
}

#[test]
fn a_change_without_an_idempotency_key_is_refused_and_a_retry_is_not_applied_twice() {
    if rerun_sandboxed(
        "a_change_without_an_idempotency_key_is_refused_and_a_retry_is_not_applied_twice",
    ) {
        return;
    }
    let (project, _, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let save = edit(
        &project,
        "",
        proto::edit_backlog::Action::Save,
        "From the phone",
    );

    session
        .send_envelope(&ket_remote::envelope(1, save.clone()))
        .unwrap();
    assert_eq!(
        error_code(&next(&mut session)),
        Some(proto::ErrorCode::StaleRequest)
    );

    let first = change(&mut session, 2, save.clone());
    assert_eq!(backlog_of(&first).notes.len(), 1);
    // The same key again, as after a reconnect: acknowledged, not redone.
    assert!(is_ack(&change(&mut session, 2, save)));
    let listed = ask(
        &mut session,
        3,
        Payload::GetBacklog(proto::GetBacklog {
            project_id: project,
        }),
    );
    assert_eq!(backlog_of(&listed).notes.len(), 1, "saved once");
}

#[test]
fn a_phone_edits_a_backlog_note_through_its_whole_life() {
    if rerun_sandboxed("a_phone_edits_a_backlog_note_through_its_whole_life") {
        return;
    }
    use proto::edit_backlog::Action;
    let (project, _, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    let saved = change(
        &mut session,
        1,
        edit(&project, "", Action::Save, "Fix the login"),
    );
    let note = backlog_of(&saved).notes[0].id.clone();

    let mut urgent = proto::EditBacklog {
        project_id: project.clone(),
        note_id: note.clone(),
        action: Action::Save as i32,
        title: "Fix the login redirect".to_owned(),
        body: "It loops.".to_owned(),
        priority: proto::BacklogPriority::Urgent as i32,
    };
    let renamed = change(&mut session, 2, Payload::EditBacklog(urgent.clone()));
    let renamed = &backlog_of(&renamed).notes[0];
    assert_eq!(renamed.title, "Fix the login redirect");
    assert_eq!(renamed.body, "It loops.");
    assert_eq!(renamed.priority, proto::BacklogPriority::Urgent as i32);

    // A save that names no priority keeps the note's.
    urgent.priority = proto::BacklogPriority::Unspecified as i32;
    urgent.title = "Fix the login redirect, again".to_owned();
    let kept = change(&mut session, 3, Payload::EditBacklog(urgent));
    assert_eq!(
        backlog_of(&kept).notes[0].priority,
        proto::BacklogPriority::Urgent as i32
    );

    let done = change(&mut session, 4, edit(&project, &note, Action::Done, ""));
    assert!(backlog_of(&done).notes[0].done_ms > 0);
    let reopened = change(&mut session, 5, edit(&project, &note, Action::Reopen, ""));
    assert_eq!(backlog_of(&reopened).notes[0].done_ms, 0);
    let removed = change(&mut session, 6, edit(&project, &note, Action::Remove, ""));
    assert!(backlog_of(&removed).notes.is_empty());
}

#[test]
fn a_backlog_edit_that_does_not_fit_is_refused_with_a_reason() {
    if rerun_sandboxed("a_backlog_edit_that_does_not_fit_is_refused_with_a_reason") {
        return;
    }
    use proto::edit_backlog::Action;
    let (project, _, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    let empty = change(&mut session, 1, edit(&project, "", Action::Save, "  "));
    assert!(message(&empty).contains("write what the work is first"));

    let missing = change(&mut session, 2, edit(&project, "no-such", Action::Done, ""));
    assert_eq!(error_code(&missing), Some(proto::ErrorCode::NotFound));

    let unknown = change(
        &mut session,
        3,
        edit(&project, "", Action::Unspecified, "x"),
    );
    assert_eq!(error_code(&unknown), Some(proto::ErrorCode::NotFound));

    let elsewhere = change(
        &mut session,
        4,
        edit("not-a-project", "", Action::Save, "x"),
    );
    assert!(message(&elsewhere).contains("no such project"));
    let read = ask(
        &mut session,
        5,
        Payload::GetBacklog(proto::GetBacklog {
            project_id: String::new(),
        }),
    );
    assert_eq!(error_code(&read), Some(proto::ErrorCode::NotFound));
}

#[test]
fn starting_work_is_checked_then_handed_to_a_window() {
    if rerun_sandboxed("starting_work_is_checked_then_handed_to_a_window") {
        return;
    }
    use proto::edit_backlog::Action;
    let (project, worktree, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let work = |project: &str, prompt: &str| {
        Payload::StartWork(proto::StartWork {
            project_id: project.to_owned(),
            prompt: prompt.to_owned(),
            agent: String::new(),
            ..Default::default()
        })
    };

    assert!(message(&change(&mut session, 1, work(&project, "  "))).contains("say what"));
    assert!(
        message(&change(&mut session, 2, work("elsewhere", "Fix it"))).contains("no such project")
    );
    // Everything checks out, and no window is open to do it.
    assert!(
        message(&change(&mut session, 3, work(&project, "Fix it")))
            .contains("open ket on your Mac")
    );
    // A note: unknown, done, and then a good one.
    let start = |note: &str| {
        Payload::StartNote(proto::StartNote {
            project_id: project.clone(),
            note_id: note.to_owned(),
        })
    };
    assert_eq!(
        error_code(&change(&mut session, 4, start("no-such"))),
        Some(proto::ErrorCode::NotFound)
    );
    let saved = change(&mut session, 5, edit(&project, "", Action::Save, "A note"));
    let note = backlog_of(&saved).notes[0].id.clone();
    change(&mut session, 6, edit(&project, &note, Action::Done, ""));
    assert!(message(&change(&mut session, 7, start(&note))).contains("already been started"));
    change(&mut session, 8, edit(&project, &note, Action::Reopen, ""));
    assert!(message(&change(&mut session, 9, start(&note))).contains("open ket on your Mac"));
    let (mut fresh, _) = paired(&rig, &Keypair::generate().unwrap());
    let options = ask(
        &mut fresh,
        1,
        Payload::GetStartOptions(proto::GetStartOptions {
            project_id: project.clone(),
        }),
    );
    let Some(Payload::StartOptions(options)) = options.payload else {
        panic!("expected start options");
    };
    assert_eq!(options.default_base, "main");
    assert!(options.local_branches.contains(&"main".to_owned()));
    let rejected = message(&change(
        &mut fresh,
        2,
        Payload::StartWork(proto::StartWork {
            project_id: project.clone(),
            prompt: "Fix it".into(),
            base: "missing".into(),
            ..Default::default()
        }),
    ));
    assert!(
        rejected.contains("base branch no longer exists"),
        "{rejected}"
    );
    assert!(
        message(&change(
            &mut fresh,
            3,
            Payload::StartSession(proto::StartSession {
                worktree_id: worktree,
                ..Default::default()
            })
        ))
        .contains("open ket on your Mac")
    );
}

#[test]
fn a_phone_reads_a_worktrees_changes_and_diffs_and_asks_a_window_to_merge() {
    if rerun_sandboxed("a_phone_reads_a_worktrees_changes_and_diffs_and_asks_a_window_to_merge") {
        return;
    }
    let (project, worktree, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    let Some(Payload::Changes(changes)) = ask(
        &mut session,
        1,
        Payload::GetChanges(proto::GetChanges {
            worktree_id: worktree.clone(),
        }),
    )
    .payload
    else {
        panic!("no changes");
    };
    assert!(changes.error.is_empty(), "{}", changes.error);
    assert_eq!(changes.base, "main");
    let paths: Vec<&str> = changes.files.iter().map(|f| f.path.as_str()).collect();
    assert!(
        paths.contains(&"README.md") && paths.contains(&"new.txt"),
        "{paths:?}"
    );
    let added = changes.files.iter().find(|f| f.path == "new.txt").unwrap();
    assert_eq!(added.added, 2);

    let Some(Payload::Diff(diff)) = ask(
        &mut session,
        2,
        Payload::GetDiff(proto::GetDiff {
            worktree_id: worktree.clone(),
            path: "README.md".to_owned(),
        }),
    )
    .payload
    else {
        panic!("no diff");
    };
    assert!(diff.text.contains("+from a phone"), "{}", diff.text);

    for (n, payload) in [
        Payload::GetChanges(proto::GetChanges {
            worktree_id: "nowhere".to_owned(),
        }),
        Payload::GetDiff(proto::GetDiff {
            worktree_id: "nowhere".to_owned(),
            path: "README.md".to_owned(),
        }),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = ask(&mut session, 10 + n as u64, payload);
        let error = match reply.payload {
            Some(Payload::Changes(c)) => c.error,
            Some(Payload::Diff(d)) => d.error,
            other => panic!("{other:?}"),
        };
        assert!(error.contains("no such worktree"), "{error}");
    }

    let merge = |worktree: &str| {
        Payload::MergeWorktree(proto::MergeWorktree {
            worktree_id: worktree.to_owned(),
        })
    };
    assert_eq!(
        error_code(&change(&mut session, 20, merge("nowhere"))),
        Some(proto::ErrorCode::NotFound)
    );
    assert!(
        message(&change(
            &mut session,
            21,
            merge(&format!("primary-{project}"))
        ))
        .contains("nothing to merge into")
    );
    assert!(message(&change(&mut session, 22, merge(&worktree))).contains("open ket on your Mac"));
}

#[test]
fn expensive_requests_are_rate_limited_per_phone() {
    if rerun_sandboxed("expensive_requests_are_rate_limited_per_phone") {
        return;
    }
    let (_, worktree, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let read = || {
        Payload::GetChanges(proto::GetChanges {
            worktree_id: worktree.clone(),
        })
    };
    // A burst of sixty units at three each is twenty reads.
    let mut refused = None;
    for n in 1..=40 {
        let reply = ask(&mut session, n, read());
        if error_code(&reply) == Some(proto::ErrorCode::Backpressure) {
            refused = Some(n);
            break;
        }
    }
    let refused = refused.expect("the budget ran out");
    assert!((20..=22).contains(&refused), "refused at {refused}");
    // Cheap requests are not budgeted.
    let devices = ask(
        &mut session,
        100,
        Payload::ListDevices(proto::ListDevices {}),
    );
    assert!(matches!(devices.payload, Some(Payload::Devices(_))));
}

#[test]
fn a_phone_lists_the_paired_devices_and_cannot_revoke_one_that_is_not_there() {
    if rerun_sandboxed("a_phone_lists_the_paired_devices_and_cannot_revoke_one_that_is_not_there") {
        return;
    }
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let Some(Payload::Devices(list)) =
        ask(&mut session, 1, Payload::ListDevices(proto::ListDevices {})).payload
    else {
        panic!("no devices");
    };
    assert_eq!(list.devices.len(), 1);
    assert!(list.devices[0].this_device);
    assert!(list.devices[0].connected);
    assert_eq!(list.devices[0].name, "Test phone");

    let revoke = Payload::RevokeDevice(proto::RevokeDevice {
        id: "not-a-device".to_owned(),
    });
    assert_eq!(
        error_code(&change(&mut session, 2, revoke)),
        Some(proto::ErrorCode::NotFound)
    );

    let Some(Payload::Snippets(_)) =
        ask(&mut session, 3, Payload::GetSnippets(proto::GetSnippets {})).payload
    else {
        panic!("no snippets");
    };
}

#[test]
fn a_subscribed_phone_is_sent_the_terminals_screen_and_must_hold_it_to_resize() {
    if rerun_sandboxed("a_subscribed_phone_is_sent_the_terminals_screen_and_must_hold_it_to_resize")
    {
        return;
    }
    let rig = rig();
    let (id, terminal) = terminal(&rig, "/bin/echo", &["ket-on-screen"]);
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    let missing = ask(
        &mut session,
        1,
        Payload::Subscribe(proto::Subscribe { terminal_id: 999 }),
    );
    assert_eq!(error_code(&missing), Some(proto::ErrorCode::NotFound));

    let mut subscribe =
        ket_remote::envelope(2, Payload::Subscribe(proto::Subscribe { terminal_id: id }));
    subscribe.idempotency_key = "test-2".to_owned();
    session.send_envelope(&subscribe).unwrap();
    let deadline = Instant::now() + WAIT;
    let mut screen = String::new();
    while !screen.contains("ket-on-screen") {
        assert!(
            Instant::now() < deadline,
            "the screen never came: {screen:?}"
        );
        match next(&mut session).payload {
            Some(Payload::Checkpoint(checkpoint)) => {
                assert_eq!(checkpoint.terminal_id, id);
                screen.push_str(&String::from_utf8_lossy(&checkpoint.ansi));
            }
            Some(Payload::Chunk(chunk)) => {
                screen.push_str(&String::from_utf8_lossy(&chunk.bytes));
            }
            _ => {}
        }
    }

    // Below the floor on purpose: clamped, not refused. With nobody
    // holding the terminal, the phone watching it may fit it.
    let size = Payload::Resize(proto::Resize {
        terminal_id: id,
        cols: 1,
        rows: 1,
    });
    assert!(is_ack(&reply_to(&mut session, 3, size.clone())));
    let lease = |action: proto::lease::Action| {
        Payload::Lease(proto::Lease {
            terminal_id: id,
            action: action as i32,
        })
    };
    assert!(is_ack(&reply_to(
        &mut session,
        4,
        lease(proto::lease::Action::Acquire)
    )));
    assert!(is_ack(&reply_to(&mut session, 5, size)));
    assert!(is_ack(&reply_to(
        &mut session,
        6,
        lease(proto::lease::Action::Release)
    )));
    let off = Payload::Unsubscribe(proto::Unsubscribe { terminal_id: id });
    session
        .send_envelope(&ket_remote::envelope(7, off))
        .unwrap();
    terminal.kill();
}

/// Sends a keyed request and reads on, past any terminal stream, to the
/// envelope that answers it.
fn reply_to(session: &mut PhoneSession, id: u64, payload: Payload) -> proto::Envelope {
    let mut request = ket_remote::envelope(id, payload);
    request.idempotency_key = format!("test-{id}");
    session.send_envelope(&request).expect("the request");
    let deadline = Instant::now() + WAIT;
    loop {
        let reply = next(session);
        if reply.request_id == id {
            return reply;
        }
        assert!(Instant::now() < deadline, "no answer to request {id}");
    }
}

#[test]
fn answering_a_prompt_that_is_not_there_says_why() {
    if rerun_sandboxed("answering_a_prompt_that_is_not_there_says_why") {
        return;
    }
    let rig = rig();
    let (id, terminal) = terminal(&rig, "/bin/sleep", &["30"]);
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());

    let answer = |terminal_id: u64| {
        Payload::Answer(proto::Answer {
            terminal_id,
            choice: proto::answer::Choice::AllowOnce as i32,
            question_id: 1,
        })
    };
    let gone = change(&mut session, 1, answer(999));
    assert!(message(&gone).contains("closed"), "{}", message(&gone));
    let unasked = change(&mut session, 2, answer(id));
    assert!(matches!(unasked.payload, Some(Payload::Error(_))));

    let Some(Payload::Conversation(conversation)) = ask(
        &mut session,
        3,
        Payload::GetConversation(proto::GetConversation {
            terminal_id: id,
            after: 0,
        }),
    )
    .payload
    else {
        panic!("no conversation");
    };
    assert_eq!(conversation.terminal_id, id);
    assert!(
        !conversation.supported,
        "a plain program has no conversation"
    );
    assert!(conversation.turns.is_empty());
    terminal.kill();
}

#[test]
fn every_role_allows_what_the_roles_below_it_do() {
    use DeviceRole::{Administrator, Controller, Operator, Viewer};
    let ladder = [Viewer, Controller, Operator, Administrator];
    for (i, held) in ladder.iter().enumerate() {
        for (j, needed) in ladder.iter().enumerate() {
            assert_eq!(held.allows(*needed), i >= j, "{held:?} allows {needed:?}");
        }
    }
}

#[test]
fn a_local_agent_is_answered_with_opencodes_keys() {
    use proto::answer::Choice;
    let local = "opencode:ollama/qwen3-coder:30b";
    assert_eq!(
        answer_keys(local, "bash", Choice::AllowOnce),
        answer_keys("opencode", "bash", Choice::AllowOnce)
    );
    assert_eq!(
        answer_keys(local, "bash", Choice::Deny),
        answer_keys("opencode", "bash", Choice::Deny)
    );
    assert!(answer_keys(local, "bash", Choice::AllowOnce).is_some());
}

/// Reads what the host tells `window` until `want` picks something out of
/// a reply. `None` once the window hears nothing for its read timeout.
fn told<T>(
    window: &mut std::os::unix::net::UnixStream,
    want: impl Fn(crate::host::wire::Reply) -> Option<T>,
) -> Option<T> {
    use crate::host::wire::{self, Frame, Reply};
    loop {
        let Ok(frame) = wire::read(window) else {
            return None;
        };
        if let Frame::Json(body) = frame
            && let Ok(reply) = serde_json::from_slice::<Reply>(&body)
            && let Some(found) = want(reply)
        {
            return Some(found);
        }
    }
}

#[test]
fn a_phone_pins_a_worktree_but_never_the_repositorys_own_checkout() {
    if rerun_sandboxed("a_phone_pins_a_worktree_but_never_the_repositorys_own_checkout") {
        return;
    }
    let (project, worktree, _) = place();
    let rig = rig();
    let mut window = rig.host.window_for_tests();
    window.set_read_timeout(Some(WAIT)).expect("a read timeout");
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let pin = |worktree: &str, pinned: bool| {
        Payload::PinWorktree(proto::PinWorktree {
            worktree_id: worktree.to_owned(),
            pinned,
        })
    };
    let pinned = || {
        crate::store::Store::open_default()
            .and_then(|store| store.load())
            .unwrap()
            .worktrees
            .iter()
            .find(|w| w.id.to_string() == worktree)
            .expect("the worktree")
            .pinned
    };

    assert_eq!(
        error_code(&change(&mut session, 1, pin("nowhere", true))),
        Some(proto::ErrorCode::NotFound)
    );
    assert!(
        message(&change(
            &mut session,
            2,
            pin(&format!("primary-{project}"), true)
        ))
        .contains("can't be pinned")
    );

    // Pinned in the store, and every window told to read its sidebar again.
    assert!(is_ack(&change(&mut session, 3, pin(&worktree, true))));
    assert!(pinned());
    assert!(
        told(&mut window, |reply| matches!(
            reply,
            crate::host::wire::Reply::WorktreesChanged
        )
        .then_some(()))
        .is_some(),
        "the window was not told"
    );
    assert!(is_ack(&change(&mut session, 4, pin(&worktree, false))));
    assert!(!pinned());
}

#[test]
fn deleting_a_worktree_is_checked_then_handed_to_a_window_which_may_refuse() {
    if rerun_sandboxed("deleting_a_worktree_is_checked_then_handed_to_a_window_which_may_refuse") {
        return;
    }
    let (project, worktree, _) = place();
    let rig = rig();
    let (mut session, _) = paired(&rig, &Keypair::generate().unwrap());
    let delete = |id: u64, worktree: &str, force: bool| {
        let mut request = ket_remote::envelope(
            id,
            Payload::DeleteWorktree(proto::DeleteWorktree {
                worktree_id: worktree.to_owned(),
                force,
            }),
        );
        request.idempotency_key = format!("delete-{id}");
        request
    };
    let send = |session: &mut PhoneSession, request: proto::Envelope| {
        let id = request.request_id;
        session.send_envelope(&request).expect("the request");
        answer_to(session, id)
    };

    assert_eq!(
        error_code(&send(&mut session, delete(1, "nowhere", false))),
        Some(proto::ErrorCode::NotFound)
    );
    // The repository itself is never a worktree's delete.
    assert!(
        message(&send(
            &mut session,
            delete(2, &format!("primary-{project}"), true)
        ))
        .contains("can't be deleted")
    );
    // No window: nothing forcing would get past.
    let alone = send(&mut session, delete(3, &worktree, false));
    assert_ne!(error_code(&alone), Some(proto::ErrorCode::WouldLoseWork));
    assert!(message(&alone).contains("open ket on your Mac"));

    // With a window: an unforced delete it refuses comes back saying why.
    let mut window = rig.host.window_for_tests();
    window.set_read_timeout(Some(WAIT)).expect("a read timeout");
    let deleting = |window: &mut std::os::unix::net::UnixStream| {
        told(window, |reply| match reply {
            crate::host::wire::Reply::DeleteWork {
                req,
                worktree,
                force,
            } => Some((req, worktree, force)),
            _ => None,
        })
        .expect("the window is told")
    };
    session.send_envelope(&delete(4, &worktree, false)).unwrap();
    let (req, asked, force) = deleting(&mut window);
    assert_eq!((asked.as_str(), force), (worktree.as_str(), false));
    rig.host.finish_delete_work(
        req,
        Err("it has uncommitted changes that would be lost".to_owned()),
    );
    let refused = answer_to(&mut session, 4);
    assert_eq!(error_code(&refused), Some(proto::ErrorCode::WouldLoseWork));
    assert!(message(&refused).contains("uncommitted changes"));

    // Asked again with force, it goes, keeping a branch with unmerged work.
    session.send_envelope(&delete(5, &worktree, true)).unwrap();
    let (req, _, force) = deleting(&mut window);
    assert!(force);
    rig.host
        .finish_delete_work(req, Ok(Some("phone-work".to_owned())));
    match answer_to(&mut session, 5).payload {
        Some(Payload::WorktreeDeleted(deleted)) => assert_eq!(deleted.kept_branch, "phone-work"),
        other => panic!("not deleted: {other:?}"),
    }
}
