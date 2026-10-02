//! A window's connection to the host.
//!
//! One per process, shared by every terminal in it. A reader thread takes
//! frames off the socket and applies them to the terminal they are for;
//! requests go out under a lock. Starting the host when there is none is done
//! here too: see [`connection`].

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use super::wire::{self, Frame, Reply, Request};
use super::{SERVE_ARG, host_error};
use crate::Result;
use crate::agent_status::AgentStatusSnapshot;
use crate::terminal::TerminalSpec;
use crate::terminal::remote::Shared;

/// How long a window waits for the host to answer an open.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a window waits for a host it has just started to listen.
const START_TIMEOUT: Duration = Duration::from_secs(5);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type Opening = SyncSender<std::result::Result<Arc<Shared>, String>>;

/// This process's connection to the host.
pub(crate) struct Connection {
    writer: Mutex<UnixStream>,
    /// Which connection of this process's this is: later ones are higher.
    serial: u64,
    alive: AtomicBool,
    next_req: AtomicU64,
    /// Opens waiting for their reply, with the spec they asked for.
    opening: Mutex<HashMap<u64, (TerminalSpec, Opening)>>,
    /// Terminals this process is showing, by the host's id.
    terminals: Mutex<HashMap<u64, Weak<Shared>>>,
}

static CONNECTION: Mutex<Option<Arc<Connection>>> = Mutex::new(None);

/// Hook reports and status lines the host has passed on, until the window
/// takes them — see [`super::take_hook_reports`].
pub(crate) static HOOK_REPORTS: Mutex<Vec<crate::agent_hooks::HookReport>> = Mutex::new(Vec::new());
/// See [`HOOK_REPORTS`].
pub(crate) static STATUS_LINES: Mutex<Vec<crate::agent_hooks::StatusLine>> = Mutex::new(Vec::new());
/// The host's newest status snapshot, until the window takes it — see
/// [`super::take_agent_status`]. Each one replaces the last, unless it came
/// over a connection older than the one the held snapshot came over: that
/// is the serial beside it.
///
/// A connection whose send failed is replaced while its reader may still be
/// draining what the host sent it first, and a snapshot from there would
/// roll the window back past the new connection's.
pub(crate) static AGENT_STATUS: Mutex<(u64, Option<AgentStatusSnapshot>)> = Mutex::new((0, None));
/// Work phones asked this window to start — see [`super::take_phone_work`].
pub(crate) static PHONE_WORK: Mutex<Vec<super::PhoneWork>> = Mutex::new(Vec::new());
/// Worktrees phones asked this window to merge — see [`super::take_phone_merge`].
pub(crate) static PHONE_MERGES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The serial the next [`Connection`] gets. See [`AGENT_STATUS`].
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);
/// This window's worktrees, as last published — sent again on every new
/// connection, since a host keeps each window's list only while it is
/// connected.
pub(crate) static WORKTREES: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());
/// The agents' plan usage, as last published — like [`WORKTREES`], sent
/// again on every new connection.
pub(crate) static USAGE: Mutex<Vec<crate::rate_limits::ProviderSnapshot>> = Mutex::new(Vec::new());
/// The agents this window can start, as last published — like [`USAGE`].
pub(crate) static AGENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The theme this window is drawn in, as last published — like [`USAGE`].
pub(crate) static THEME: Mutex<Option<crate::theme::Theme>> = Mutex::new(None);

/// The connection to the host, if there is one now. Never starts a host:
/// for telling it something that only matters if it is running.
pub(crate) fn current() -> Option<Arc<Connection>> {
    lock(&CONNECTION)
        .as_ref()
        .filter(|conn| conn.alive())
        .cloned()
}

/// The connection to the host, made — and the host started — if need be.
pub(crate) fn connection() -> Result<Arc<Connection>> {
    let mut slot = lock(&CONNECTION);
    if let Some(conn) = slot.as_ref()
        && conn.alive()
    {
        return Ok(conn.clone());
    }
    let conn = establish()?;
    *slot = Some(conn.clone());
    Ok(conn)
}

/// Connects, starting a host if none answers, and replacing one from another
/// build if it is idle.
fn establish() -> Result<Arc<Connection>> {
    let socket = super::socket_path()?;
    let mine = super::build_id();
    let mut replaced = false;

    for _ in 0..3 {
        let Ok(stream) = UnixStream::connect(&socket) else {
            let exited = start_host()?;
            if !wait_for(&socket, &exited, START_TIMEOUT) {
                return Err(host_error(
                    "the host exited on starting; see host.log beside its lock",
                ));
            }
            continue;
        };
        let (stream, protocol, build, live) = hello(stream, &mine).map_err(host_error)?;
        if protocol != wire::PROTOCOL || (build != mine && live == 0 && !replaced) {
            if live > 0 {
                return Err(host_error(format!(
                    "the running host speaks protocol {protocol}, this build {}; \
                     it has terminals running, so it stays",
                    wire::PROTOCOL
                )));
            }
            // An idle host from another build: ask it to go, and start ours.
            let conn = Connection::start(stream)?;
            conn.request(&Request::Shutdown { force: false })?;
            conn.alive.store(false, Ordering::Release);
            wait_gone(&socket, START_TIMEOUT);
            replaced = true;
            continue;
        }
        let conn = Connection::start(stream)?;
        let worktrees = lock(&WORKTREES).clone();
        if !worktrees.is_empty() {
            // Only for placing Codex reports; a connection that cannot take
            // it will fail on its next request anyway.
            let _ = conn.request(&Request::Worktrees { worktrees });
        }
        let providers = lock(&USAGE).clone();
        if !providers.is_empty() {
            let _ = conn.request(&Request::Usage { providers });
        }
        let names = lock(&AGENTS).clone();
        if !names.is_empty() {
            let _ = conn.request(&Request::Agents { names });
        }
        if let Some(theme) = *lock(&THEME) {
            let _ = conn.request(&Request::Theme {
                theme: Box::new(theme),
            });
        }
        return Ok(conn);
    }
    Err(host_error("could not reach or start the host"))
}

/// Connects to a running host without starting one: the stream, its build
/// and how many terminals it is running.
pub(crate) fn peek() -> Result<Option<(UnixStream, String, usize)>> {
    let Ok(stream) = UnixStream::connect(super::socket_path()?) else {
        return Ok(None);
    };
    let (stream, _, build, live) = hello(stream, &super::build_id()).map_err(host_error)?;
    Ok(Some((stream, build, live)))
}

/// Asks the host on a stream from [`peek`] for a pairing code.
pub(crate) fn pairing_code(stream: UnixStream) -> Result<String> {
    let mut writer = stream.try_clone().map_err(host_error)?;
    wire::send(
        &mut writer,
        &wire::json(&Request::PairingCode).map_err(host_error)?,
    )
    .map_err(host_error)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(host_error)?;
    let mut reader = &stream;
    // Whatever else the host has queued for a new window — hook traffic —
    // comes first, and is not this caller's.
    loop {
        if let Frame::Json(body) = wire::read(&mut reader).map_err(host_error)?
            && let Ok(Reply::PairingCode { code, error }) = serde_json::from_slice(&body)
        {
            return code.ok_or_else(|| host_error(error.unwrap_or_default()));
        }
    }
}

/// Sends `request` down a stream from [`peek`] and waits for the reply
/// `pick` accepts, skipping whatever else the host has queued for a new
/// connection.
pub(crate) fn ask<T>(
    stream: UnixStream,
    request: &Request,
    pick: impl Fn(Reply) -> Option<T>,
) -> Result<T> {
    let mut writer = stream.try_clone().map_err(host_error)?;
    wire::send(&mut writer, &wire::json(request).map_err(host_error)?).map_err(host_error)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(host_error)?;
    let mut reader = &stream;
    loop {
        if let Frame::Json(body) = wire::read(&mut reader).map_err(host_error)?
            && let Ok(reply) = serde_json::from_slice(&body)
            && let Some(answer) = pick(reply)
        {
            return Ok(answer);
        }
    }
}

/// Sends a shutdown request down a stream from [`peek`].
pub(crate) fn shutdown(mut stream: UnixStream, force: bool) -> Result<()> {
    wire::send(
        &mut stream,
        &wire::json(&Request::Shutdown { force }).map_err(host_error)?,
    )
    .map_err(host_error)
}

/// Whether the host at the other end of `stream` goes within `within`: its
/// end of the socket closes when it exits.
///
/// Reads, and drops, whatever the host sends meanwhile. A host greets a new
/// connection with its whole status and hook backlog, and a busy one keeps
/// broadcasting; one left writing to a connection nobody reads can block
/// there instead of reading the request that would end it.
pub(crate) fn exits(stream: &UnixStream, within: Duration) -> bool {
    use std::io::Read;
    let deadline = Instant::now() + within;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        match (&*stream).read(&mut buffer) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return true,
        }
    }
}

/// The process at the other end of `stream`, from the kernel.
#[cfg(target_os = "macos")]
pub(crate) fn peer_pid(stream: &UnixStream) -> Option<i32> {
    use nix::sys::socket::{getsockopt, sockopt::LocalPeerPid};
    getsockopt(stream, LocalPeerPid).ok().filter(|pid| *pid > 0)
}

/// Not asked for off macOS yet; a host that ignores its socket stays up.
#[cfg(not(target_os = "macos"))]
pub(crate) fn peer_pid(_stream: &UnixStream) -> Option<i32> {
    None
}

/// Ends process `pid` with `SIGTERM`, then `SIGKILL` if it is still there
/// after `grace`. Whether it is gone afterwards.
#[cfg(target_os = "macos")]
pub(crate) fn end_process(pid: i32, grace: Duration) -> bool {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let pid = Pid::from_raw(pid);
    let gone = |within: Duration| {
        let deadline = Instant::now() + within;
        loop {
            // Signal 0 checks without sending; `ESRCH` is the process gone.
            if kill(pid, None) == Err(nix::errno::Errno::ESRCH) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let _ = kill(pid, Signal::SIGTERM);
    if gone(grace) {
        return true;
    }
    let _ = kill(pid, Signal::SIGKILL);
    gone(Duration::from_secs(2))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn end_process(_pid: i32, _grace: Duration) -> bool {
    false
}

/// Says hello and reads the welcome.
fn hello(stream: UnixStream, build: &str) -> io::Result<(UnixStream, u32, String, usize)> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut writer = stream.try_clone()?;
    wire::send(
        &mut writer,
        &wire::json(&Request::Hello {
            protocol: wire::PROTOCOL,
            build: build.to_owned(),
        })?,
    )?;
    let mut reader = &stream;
    match wire::read(&mut reader)? {
        Frame::Json(body) => match serde_json::from_slice(&body) {
            Ok(Reply::Welcome {
                protocol,
                build,
                live,
            }) => {
                // Fails with EINVAL on macOS if the host has already hung up —
                // which it does after welcoming a window on another protocol.
                // That window needs the welcome, not this error.
                let _ = stream.set_read_timeout(None);
                Ok((stream, protocol, build, live))
            }
            _ => Err(io::Error::other("expected a welcome")),
        },
        _ => Err(io::Error::other("expected a welcome")),
    }
}

/// Starts this binary as the host, detached from this process.
///
/// Its own process group, so a Ctrl-C meant for a `cargo run` in a terminal
/// does not reach it, and its own process name, `ket-host`; its output goes
/// to a log beside the socket.
///
/// Returns a flag that is set when the process exits, so a window waiting for
/// a host that failed on starting stops waiting straight away.
fn start_host() -> Result<Arc<AtomicBool>> {
    let dir = super::dir()?;
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(host_error)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("host.log"))
        .map_err(host_error)?;
    let exe = std::env::current_exe().map_err(host_error)?;
    let mut child = Command::new(exe)
        // Its own name, so it is not caught by anything aimed at the app:
        // `pkill ket-ui`, or Force Quit on ket in Activity Monitor, would
        // otherwise end every terminal in every project along with the window.
        .arg0("ket-host")
        .arg(SERVE_ARG)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(host_error)?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(host_error)?;
    // Reaped whenever it exits, so it is never left a zombie of this process.
    let exited = Arc::new(AtomicBool::new(false));
    let flag = exited.clone();
    let _ = std::thread::Builder::new()
        .name("ket-host-reap".into())
        .spawn(move || {
            let _ = child.wait();
            flag.store(true, Ordering::Release);
        });
    Ok(exited)
}

/// Waits for the socket to answer. `false` if the host exited instead.
fn wait_for(socket: &std::path::Path, exited: &AtomicBool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            return true;
        }
        if exited.load(Ordering::Acquire) {
            // One more look: a host that lost the race to another exits at
            // once, and the winner may be listening by now.
            return UnixStream::connect(socket).is_ok();
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    true
}

fn wait_gone(socket: &std::path::Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && UnixStream::connect(socket).is_ok() {
        std::thread::sleep(Duration::from_millis(25));
    }
}

impl Connection {
    fn start(stream: UnixStream) -> Result<Arc<Self>> {
        let reader = stream.try_clone().map_err(host_error)?;
        let conn = Arc::new(Self {
            writer: Mutex::new(stream),
            serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed),
            alive: AtomicBool::new(true),
            next_req: AtomicU64::new(1),
            opening: Mutex::new(HashMap::new()),
            terminals: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&conn);
        std::thread::Builder::new()
            .name("ket-host-client".into())
            .spawn(move || read_loop(&weak, reader))
            .map_err(host_error)?;
        Ok(conn)
    }

    pub(crate) fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn send(&self, frame: &[u8]) -> Result<()> {
        if !self.alive() {
            return Err(host_error("the connection to the host is closed"));
        }
        wire::send(&mut *lock(&self.writer), frame).map_err(|error| {
            self.alive.store(false, Ordering::Release);
            host_error(error)
        })
    }

    pub(crate) fn request(&self, request: &Request) -> Result<()> {
        self.send(&wire::json(request).map_err(host_error)?)
    }

    pub(crate) fn input(&self, id: u64, bytes: &[u8]) -> Result<()> {
        self.send(&wire::input(id, bytes))
    }

    /// Starts or adopts a terminal, and waits for it to exist.
    pub(crate) fn open(self: &Arc<Self>, spec: &TerminalSpec) -> Result<Arc<Shared>> {
        let req = self.next_req.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = sync_channel(1);
        lock(&self.opening).insert(req, (spec.clone(), tx));
        if let Err(error) = self.request(&Request::Open {
            req,
            spec: Box::new(spec.clone()),
        }) {
            lock(&self.opening).remove(&req);
            return Err(error);
        }
        match rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(shared)) => Ok(shared),
            Ok(Err(why)) => Err(host_error(why)),
            Err(_) => {
                lock(&self.opening).remove(&req);
                Err(host_error("no answer to an open"))
            }
        }
    }

    /// Stops routing frames for a terminal this process has let go of.
    pub(crate) fn forget(&self, id: u64) {
        lock(&self.terminals).remove(&id);
    }

    fn shared(&self, id: u64) -> Option<Arc<Shared>> {
        lock(&self.terminals).get(&id).and_then(Weak::upgrade)
    }
}

/// Takes frames off the socket until it closes.
fn read_loop(conn: &Weak<Connection>, stream: UnixStream) {
    let mut reader = io::BufReader::new(stream);
    loop {
        let frame = wire::read(&mut reader);
        let Some(conn) = conn.upgrade() else {
            return;
        };
        match frame {
            Ok(Frame::Checkpoint {
                id,
                size,
                scrollback,
                ansi,
            }) => {
                if let Some(shared) = conn.shared(id) {
                    shared.checkpoint(size, scrollback, &ansi);
                }
            }
            Ok(Frame::Output { id, bytes }) => {
                if let Some(shared) = conn.shared(id) {
                    shared.output(&bytes);
                }
            }
            Ok(Frame::Json(body)) => match serde_json::from_slice::<Reply>(&body) {
                Ok(reply) => route(&conn, reply),
                Err(error) => tracing::warn!(%error, "unreadable reply from the ket host"),
            },
            Ok(Frame::Input { .. }) => {}
            Err(error) => {
                // The host is gone. Every terminal it was running for this
                // window is too, as far as the window can tell.
                tracing::warn!(%error, "lost the ket host");
                conn.alive.store(false, Ordering::Release);
                let terminals: Vec<_> = lock(&conn.terminals).drain().collect();
                for (_, shared) in terminals {
                    if let Some(shared) = shared.upgrade() {
                        shared.lost();
                    }
                }
                for (_, (_, tx)) in lock(&conn.opening).drain() {
                    let _ = tx.send(Err("the host went away".into()));
                }
                return;
            }
        }
    }
}

fn route(conn: &Arc<Connection>, reply: Reply) {
    match reply {
        Reply::Opened { req, id, adopted } => {
            let Some((spec, tx)) = lock(&conn.opening).remove(&req) else {
                return;
            };
            // Created and registered here, on the reader thread, before the
            // next frame is read: that frame is this terminal's checkpoint.
            let shared = Shared::new(id, &spec, adopted);
            lock(&conn.terminals).insert(id, Arc::downgrade(&shared));
            let _ = tx.send(Ok(shared));
        }
        Reply::Failed { req, error } => {
            if let Some((_, tx)) = lock(&conn.opening).remove(&req) {
                let _ = tx.send(Err(error));
            }
        }
        Reply::Closed { id, status } => {
            if let Some(shared) = conn.shared(id) {
                shared.closed(status.map(Into::into));
            }
        }
        Reply::Foreground { id, foreground } => {
            if let Some(shared) = conn.shared(id) {
                shared.foreground(foreground.map(Into::into));
            }
        }
        Reply::AgentStatus { snapshot } => {
            let mut slot = lock(&AGENT_STATUS);
            if conn.serial >= slot.0 {
                *slot = (conn.serial, Some(*snapshot));
            }
        }
        Reply::Hook { report } => lock(&HOOK_REPORTS).push(*report),
        Reply::StatusLine { line } => lock(&STATUS_LINES).push(*line),
        Reply::StartWork {
            project,
            prompt,
            agent,
            note,
        } => lock(&PHONE_WORK).push(super::PhoneWork {
            project,
            prompt,
            agent,
            note,
        }),
        Reply::MergeWork { worktree } => lock(&PHONE_MERGES).push(worktree),
        Reply::Welcome { .. }
        | Reply::PairingCode { .. }
        | Reply::Devices { .. }
        | Reply::Revoked { .. }
        | Reply::Phones { .. }
        | Reply::Pairings { .. }
        | Reply::PairingDecided { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn next_frame(reader: &mut impl io::Read) -> Frame {
        wire::read(reader).expect("a frame")
    }

    /// Reads and answers one `Hello`, replying with `Welcome`.
    fn answer_hello(mut stream: UnixStream, build: &str, live: usize) {
        match next_frame(&mut stream) {
            Frame::Json(body) => match serde_json::from_slice::<Request>(&body) {
                Ok(Request::Hello { .. }) => {}
                other => panic!("expected a hello, got {other:?}"),
            },
            other => panic!("expected json, got {other:?}"),
        }
        let frame = wire::json(&Reply::Welcome {
            protocol: wire::PROTOCOL,
            build: build.to_owned(),
            live,
        })
        .expect("an encoded welcome");
        wire::send(&mut stream, &frame).expect("sent");
    }

    #[test]
    fn hello_reads_the_welcome_and_returns_its_fields() {
        let (a, b) = UnixStream::pair().expect("a socket pair");
        let server = std::thread::spawn(move || answer_hello(b, "host-build", 3));

        let (_stream, protocol, build, live) = hello(a, "window-build").expect("a hello");
        assert_eq!(protocol, wire::PROTOCOL);
        assert_eq!(build, "host-build");
        assert_eq!(live, 3);

        server.join().expect("the server thread");
    }

    #[test]
    fn hello_errors_when_the_reply_is_not_a_welcome() {
        let (a, mut b) = UnixStream::pair().expect("a socket pair");
        let server = std::thread::spawn(move || {
            let _ = next_frame(&mut b);
            let frame = wire::json(&Reply::Devices { devices: None }).expect("encoded");
            wire::send(&mut b, &frame).expect("sent");
        });

        let error = hello(a, "window-build").expect_err("not a welcome");
        assert_eq!(error.kind(), io::ErrorKind::Other);

        server.join().expect("the server thread");
    }

    #[test]
    fn ask_skips_whatever_else_is_queued_and_picks_the_matching_reply() {
        let (a, mut b) = UnixStream::pair().expect("a socket pair");
        let server = std::thread::spawn(move || {
            match next_frame(&mut b) {
                Frame::Json(body) => {
                    let request: Request = serde_json::from_slice(&body).expect("a request");
                    assert!(matches!(request, Request::Devices));
                }
                other => panic!("expected json, got {other:?}"),
            }
            // Queued ahead of the answer, as hook traffic would be.
            let hook = wire::json(&Reply::StatusLine {
                line: Box::new(crate::agent_hooks::StatusLine {
                    worktree: None,
                    pane: Some("p1".to_owned()),
                    agent: None,
                    payload: serde_json::json!({}),
                }),
            })
            .expect("encoded");
            wire::send(&mut b, &hook).expect("sent");
            let answer = wire::json(&Reply::Devices {
                devices: Some(Vec::new()),
            })
            .expect("encoded");
            wire::send(&mut b, &answer).expect("sent");
        });

        let devices = ask(a, &Request::Devices, |reply| match reply {
            Reply::Devices { devices } => Some(devices),
            _ => None,
        })
        .expect("an answer");
        assert!(devices.is_some_and(|devices| devices.is_empty()));

        server.join().expect("the server thread");
    }

    #[test]
    fn shutdown_sends_the_force_flag_as_given() {
        let (a, mut b) = UnixStream::pair().expect("a socket pair");
        shutdown(a, true).expect("sent");
        match next_frame(&mut b) {
            Frame::Json(body) => {
                let request: Request = serde_json::from_slice(&body).expect("a request");
                assert!(matches!(request, Request::Shutdown { force: true }));
            }
            other => panic!("expected json, got {other:?}"),
        }
    }

    #[test]
    fn pairing_code_skips_other_frames_and_reports_the_hosts_error() {
        let (a, mut b) = UnixStream::pair().expect("a socket pair");
        let server = std::thread::spawn(move || {
            let _ = next_frame(&mut b);
            let hook = wire::json(&Reply::Devices { devices: None }).expect("encoded");
            wire::send(&mut b, &hook).expect("sent");
            let answer = wire::json(&Reply::PairingCode {
                code: None,
                error: Some("phones are off".to_owned()),
            })
            .expect("encoded");
            wire::send(&mut b, &answer).expect("sent");
        });

        let error = pairing_code(a).expect_err("phones are off");
        assert!(error.to_string().contains("phones are off"));

        server.join().expect("the server thread");
    }

    #[test]
    fn connection_request_writes_the_encoded_frame() {
        let (a, mut b) = UnixStream::pair().expect("a socket pair");
        let conn = Connection::start(a).expect("a connection");

        conn.request(&Request::Devices).expect("sent");
        match next_frame(&mut b) {
            Frame::Json(body) => {
                let request: Request = serde_json::from_slice(&body).expect("a request");
                assert!(matches!(request, Request::Devices));
            }
            other => panic!("expected json, got {other:?}"),
        }
        assert!(conn.alive());
    }

    #[test]
    fn connection_goes_dead_once_its_peer_closes() {
        let (a, b) = UnixStream::pair().expect("a socket pair");
        let conn = Connection::start(a).expect("a connection");
        drop(b);

        let deadline = Instant::now() + Duration::from_secs(5);
        while conn.alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!conn.alive());
    }

    #[test]
    fn peek_reports_none_then_a_listening_hosts_build_and_live_count() {
        if rerun_sandboxed("peek_reports_none_then_a_listening_hosts_build_and_live_count") {
            return;
        }
        assert!(peek().expect("no host yet").is_none());

        let socket = super::super::socket_path().expect("a socket path");
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent).expect("the socket's directory");
        }
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("a listener");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("a connection");
            answer_hello(stream, "sandboxed-build", 2);
        });

        let (_, build, live) = peek().expect("a host now").expect("it is listening");
        assert_eq!(build, "sandboxed-build");
        assert_eq!(live, 2);

        server.join().expect("the server thread");
    }

    /// Set in the child a test runs in.
    const SANDBOXED: &str = "KET_CLIENT_TEST_SANDBOX";

    /// In the parent: runs test `name` again in a child whose data directory
    /// is a temporary one, checks it passed, and returns `true`. In that
    /// child: `false`, and the test goes on.
    fn rerun_sandboxed(name: &str) -> bool {
        if std::env::var_os(SANDBOXED).is_some() {
            assert!(std::env::var_os("XDG_DATA_HOME").is_some_and(|dir| !dir.is_empty()));
            return false;
        }
        let data = std::env::temp_dir().join(format!("ket-client-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).expect("a data directory");
        let out = Command::new(std::env::current_exe().expect("this test binary"))
            .arg(format!("host::client::tests::{name}"))
            .args(["--exact", "--test-threads=1", "--nocapture"])
            .env(SANDBOXED, "1")
            .env("XDG_DATA_HOME", &data)
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
}
