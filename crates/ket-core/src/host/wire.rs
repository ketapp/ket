//! What goes over the host's socket.
//!
//! Length-prefixed frames: a big-endian `u32` length, a kind byte, and a
//! payload. Control messages are JSON, because they are small, rare and worth
//! being able to read in a hex dump. Terminal bytes — output, input and
//! checkpoints — are raw, because they are neither: a firehose of output
//! would pay for JSON-escaping every byte of it.
//!
//! This is the local protocol between a window and the host on one machine,
//! not the phone's. Epic 5b.2 defines that one, encrypted and versioned for
//! clients that update on their own schedule; this one only ever talks to a
//! build of ket, and [`PROTOCOL`] is bumped whenever it changes.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::activity::Foreground;
use crate::agent_hooks::{HookReport, StatusLine};
use crate::agent_status::AgentStatusSnapshot;
use crate::rate_limits::ProviderSnapshot;
use crate::terminal::{TerminalSize, TerminalSpec};

/// Bumped on every incompatible change. A window and a host that disagree do
/// not talk; hosted terminal opens fail rather than silently losing their
/// crash-survival guarantee.
///
/// A new request is not an incompatible change: a host logs and skips one it
/// cannot read, so a window asking an older host for it just gets no answer.
pub const PROTOCOL: u32 = 5;

/// Authority granted to a paired phone. Roles are hierarchical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    /// Observe projects, terminals, conversations and changes.
    Viewer,
    /// Viewer access plus terminal input, signals and prompt answers.
    Controller,
    /// Controller access plus creating and merging work.
    Operator,
    /// Operator access plus listing and revoking paired devices.
    Administrator,
}

impl DeviceRole {
    /// Whether this role includes `required`.
    pub fn allows(self, required: Self) -> bool {
        let rank = |role| match role {
            Self::Viewer => 0,
            Self::Controller => 1,
            Self::Operator => 2,
            Self::Administrator => 3,
        };
        rank(self) >= rank(required)
    }
}

/// Largest frame either side accepts. A checkpoint of a full scrollback in
/// which every cell has its own colour is under 4 MB; this is headroom, not a
/// target.
const MAX_FRAME: usize = 64 * 1024 * 1024;

const KIND_JSON: u8 = 1;
const KIND_OUTPUT: u8 = 2;
const KIND_CHECKPOINT: u8 = 3;
const KIND_INPUT: u8 = 4;

/// A window asking the host for something.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// The first thing a window says.
    Hello {
        /// See [`PROTOCOL`].
        protocol: u32,
        /// Which build of ket this is — see [`super::build_id`].
        build: String,
    },
    /// Start a terminal, or adopt one — see [`TerminalSpec::adopt`].
    Open {
        /// Echoed in the reply, to match it up.
        req: u64,
        /// What to run.
        spec: Box<TerminalSpec>,
    },
    /// Resize a terminal.
    Resize {
        /// Which terminal.
        id: u64,
        /// Its new size.
        size: TerminalSize,
    },
    /// Kill a terminal: the pane showing it was closed.
    Kill {
        /// Which terminal.
        id: u64,
    },
    /// Empty a terminal's screen and scrollback.
    Clear {
        /// Which terminal.
        id: u64,
    },
    /// Stop showing a terminal without ending it: the window is going away.
    Detach {
        /// Which terminal.
        id: u64,
    },
    /// Exit, if no terminal is running. A newer build asks this of an older
    /// host it finds idle, so the code that runs terminals keeps up with the
    /// code that shows them.
    ///
    /// `force` ends the running terminals first and exits regardless — how
    /// a sandbox's host is torn down, and how a script ends the owner's host
    /// from a shell whose signals do not reach it. A field rather than a new
    /// request so either side may be the older: a host that predates it
    /// ignores it, and one that has it reads its absence as `false`.
    Shutdown {
        /// End the terminals and exit, rather than exit only if idle.
        #[serde(default)]
        force: bool,
    },
    /// A one-time code for pairing a phone. See `super::phones`.
    PairingCode,
    /// The phones paired with this host.
    Devices,
    /// Stop accepting a phone, and end its session.
    Revoke {
        /// The phone's id, as [`Reply::Devices`] gives it.
        id: String,
    },
    /// Phones that have paired and are waiting for a person to approve them.
    /// Answered with [`Reply::Pairings`].
    Pairings,
    /// Approves or declines a waiting pairing. Answered with
    /// [`Reply::PairingDecided`].
    DecidePairing {
        /// As [`PendingPairing::id`] gives it.
        id: u64,
        /// Whether to approve. Retained for protocol-5 host compatibility.
        approve: bool,
    },
    /// Approves a pairing with scoped authority, or declines it.
    DecidePairingRole {
        /// As [`PendingPairing::id`] gives it.
        id: u64,
        /// The role to grant; `None` declines it.
        role: Option<DeviceRole>,
    },
    /// Where this window's worktrees are, so the host can place a Codex
    /// report by its working directory — see
    /// [`crate::agent_status::StatusStore::route`]. Sent on connecting and
    /// whenever the list changes; the host merges every window's.
    Worktrees {
        /// Each worktree's id and path.
        worktrees: Vec<(String, PathBuf)>,
    },
    /// Phones turned on or off, in place — see
    /// [`crate::config::PhonesConfig`]. Answered with [`Reply::Phones`].
    Phones {
        /// Whether the host should serve phones.
        enabled: bool,
    },
    /// The agents' plan usage, as this window's usage popover has it, for
    /// the host to pass on to phones. Sent on connecting and whenever it
    /// changes; the newest from any window wins.
    Usage {
        /// One per provider, in [`crate::rate_limits::Provider::ALL`] order.
        providers: Vec<ProviderSnapshot>,
    },
    /// The agents this window can start — enabled in Settings and installed —
    /// for a phone to offer when it starts work. Sent on connecting and
    /// whenever the list changes; the newest from any window wins.
    Agents {
        /// Each agent's name, in the window's order.
        names: Vec<String>,
    },
    /// The theme this window is drawn in, for phones to draw in too. Sent on
    /// connecting and whenever it changes; the newest from any window wins.
    Theme {
        /// The whole theme, as the window resolved it.
        theme: Box<crate::theme::Theme>,
    },
    /// An interrupt typed into one of the host's terminals.
    Interrupt {
        /// The terminal's pane key.
        pane: String,
        /// `Ctrl-C`, a latched verdict, rather than Escape's unlatched
        /// guess — see [`crate::agent_status`].
        cancel: bool,
    },
}

/// A phone waiting for a person to approve its pairing, on the local wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPairing {
    /// Which one, for [`Request::DecidePairing`].
    pub id: u64,
    /// What it calls itself.
    pub name: String,
    /// The six digits the phone shows too.
    pub code: String,
    /// Seconds left to decide.
    pub expires_in: u64,
}

/// A paired phone, on the local wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Its public key, base64url.
    pub id: String,
    /// What it called itself.
    pub name: String,
    /// Unix seconds.
    pub paired_at: u64,
    /// Whether it is connected now.
    pub connected: bool,
    /// Its stored authority. `None` for a device paired before roles
    /// existed, which is given full access when it next connects.
    #[serde(default)]
    pub role: Option<DeviceRole>,
}

/// The host answering, or telling a window something happened.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    /// The answer to [`Request::Hello`].
    Welcome {
        /// See [`PROTOCOL`].
        protocol: u32,
        /// Which build of ket the host is.
        build: String,
        /// Terminals running now.
        live: usize,
    },
    /// A terminal was started or adopted. Its checkpoint follows.
    Opened {
        /// From the request.
        req: u64,
        /// The host's name for the terminal from now on.
        id: u64,
        /// Whether this is a terminal that was already running.
        adopted: bool,
    },
    /// A terminal could not be started.
    Failed {
        /// From the request.
        req: u64,
        /// Why, for a person.
        error: String,
    },
    /// A terminal's program has finished and its output is all sent.
    Closed {
        /// Which terminal.
        id: u64,
        /// How the program exited, if it has been reaped.
        status: Option<Exit>,
    },
    /// An agent in one of the host's terminals reported through its hook.
    ///
    /// Agents the host starts report to the host's own listener, because the
    /// window that launched them may be long gone; every window hears them.
    /// Not for status — the host applies them to its own store and sends
    /// [`Reply::AgentStatus`] — but for what is not: usage and rollouts.
    Hook {
        /// The report, as the listener parsed it.
        report: Box<HookReport>,
    },
    /// Every pane's agent status, as the host now holds it. Sent after the
    /// welcome and again whenever it changes; each replaces the last.
    AgentStatus {
        /// See [`AgentStatusSnapshot`].
        snapshot: Box<AgentStatusSnapshot>,
    },
    /// An agent in one of the host's terminals drew its status line.
    StatusLine {
        /// The payload and where it came from.
        line: Box<StatusLine>,
    },
    /// The answer to [`Request::Devices`]; `None` when the host has no
    /// phone side.
    Devices {
        /// The paired phones.
        devices: Option<Vec<DeviceEntry>>,
    },
    /// The answer to [`Request::Revoke`].
    Revoked {
        /// Whether a phone by that id was paired.
        found: bool,
        /// Why it could not be done.
        error: Option<String>,
    },
    /// A phone asked for new work: a worktree in `project` whose agent
    /// starts on `prompt`. Sent to one window, which makes the worktree the
    /// way its own dialog does — see `ket_ui`'s `start_phone_work`.
    StartWork {
        /// The project's id.
        project: String,
        /// What the agent is to do.
        prompt: String,
        /// Which agent, by name; `None` for the project's own.
        agent: Option<String>,
        /// The backlog note this is, when a phone started one: the window
        /// builds the work from the note — its files too — and marks it done
        /// once the worktree exists. A window from before it ignores it and
        /// starts `prompt`, which the host writes from the note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A phone asked for a worktree to be merged, as ket's Merge does. Sent to
    /// one window, which runs its own merge on it.
    MergeWork {
        /// The worktree's id.
        worktree: String,
    },
    /// The answer to [`Request::Pairings`]; empty when phones are off.
    Pairings {
        /// Waiting, oldest first.
        pairings: Vec<PendingPairing>,
        /// Whether this host accepts scoped pairing decisions.
        #[serde(default)]
        scoped_roles: bool,
    },
    /// The answer to [`Request::DecidePairing`].
    PairingDecided {
        /// Whether a pairing by that id was waiting.
        found: bool,
    },
    /// The answer to [`Request::PairingCode`].
    PairingCode {
        /// The `ket://pair/…` text for a QR code.
        code: Option<String>,
        /// Why there is none.
        error: Option<String>,
    },
    /// The answer to [`Request::Phones`].
    Phones {
        /// Whether phones are on now.
        serving: bool,
        /// Why the switch did not take.
        error: Option<String>,
    },
    /// What has a terminal's foreground changed.
    Foreground {
        /// Which terminal.
        id: u64,
        /// See [`Foreground`].
        foreground: Option<Running>,
    },
}

/// A program's exit status, on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exit {
    /// The exit code.
    pub code: u32,
    /// The signal that ended it, by name, if one did.
    pub signal: Option<String>,
}

impl From<&portable_pty::ExitStatus> for Exit {
    fn from(status: &portable_pty::ExitStatus) -> Self {
        Self {
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        }
    }
}

impl From<Exit> for portable_pty::ExitStatus {
    fn from(exit: Exit) -> Self {
        match exit.signal {
            Some(signal) => Self::with_signal(&signal),
            None => Self::with_exit_code(exit.code),
        }
    }
}

/// [`Foreground`], on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Running {
    /// The shell is at a prompt.
    Shell,
    /// Something else has the terminal.
    Program(String),
}

impl From<Foreground> for Running {
    fn from(foreground: Foreground) -> Self {
        match foreground {
            Foreground::Shell => Self::Shell,
            Foreground::Running(name) => Self::Program(name),
        }
    }
}

impl From<Running> for Foreground {
    fn from(running: Running) -> Self {
        match running {
            Running::Shell => Self::Shell,
            Running::Program(name) => Self::Running(name),
        }
    }
}

/// One frame off the socket.
#[derive(Debug)]
pub enum Frame {
    /// A [`Request`] or a [`Reply`], still encoded.
    Json(Vec<u8>),
    /// Output from a terminal, host to window.
    Output {
        /// Which terminal.
        id: u64,
        /// Raw pty output.
        bytes: Vec<u8>,
    },
    /// A checkpoint, host to window: replace the screen with this.
    Checkpoint {
        /// Which terminal.
        id: u64,
        /// The size to give the screen first.
        size: TerminalSize,
        /// Scrollback lines to keep.
        scrollback: usize,
        /// The escape sequences.
        ansi: Vec<u8>,
    },
    /// Keystrokes, window to host.
    Input {
        /// Which terminal.
        id: u64,
        /// Raw bytes for the pty.
        bytes: Vec<u8>,
    },
}

/// Encodes a control message.
pub fn json(message: &impl Serialize) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    Ok(frame(KIND_JSON, &[], &body))
}

/// Encodes terminal output.
pub fn output(id: u64, bytes: &[u8]) -> Vec<u8> {
    frame(KIND_OUTPUT, &id.to_be_bytes(), bytes)
}

/// Encodes keystrokes.
pub fn input(id: u64, bytes: &[u8]) -> Vec<u8> {
    frame(KIND_INPUT, &id.to_be_bytes(), bytes)
}

/// Encodes a checkpoint.
pub fn checkpoint(id: u64, size: TerminalSize, scrollback: usize, ansi: &[u8]) -> Vec<u8> {
    let mut head = Vec::with_capacity(16);
    head.extend_from_slice(&id.to_be_bytes());
    head.extend_from_slice(&size.cols.to_be_bytes());
    head.extend_from_slice(&size.rows.to_be_bytes());
    head.extend_from_slice(&(scrollback.min(u32::MAX as usize) as u32).to_be_bytes());
    frame(KIND_CHECKPOINT, &head, ansi)
}

/// One whole frame, ready for a single `write_all`: writers share a socket,
/// and a frame written in two calls could be split by another writer's.
fn frame(kind: u8, head: &[u8], body: &[u8]) -> Vec<u8> {
    let len = 1 + head.len() + body.len();
    let mut out = Vec::with_capacity(4 + len);
    out.extend_from_slice(&(len as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(head);
    out.extend_from_slice(body);
    out
}

/// Reads one frame, blocking until it has all arrived.
pub fn read(reader: &mut impl Read) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let kind = body[0];
    let body = &body[1..];

    let short = || io::Error::new(io::ErrorKind::InvalidData, "short frame");
    let id = |body: &[u8]| -> io::Result<u64> {
        Ok(u64::from_be_bytes(
            body.get(..8)
                .ok_or_else(short)?
                .try_into()
                .map_err(|_| short())?,
        ))
    };
    Ok(match kind {
        KIND_JSON => Frame::Json(body.to_vec()),
        KIND_OUTPUT => Frame::Output {
            id: id(body)?,
            bytes: body[8..].to_vec(),
        },
        KIND_INPUT => Frame::Input {
            id: id(body)?,
            bytes: body[8..].to_vec(),
        },
        KIND_CHECKPOINT => {
            if body.len() < 16 {
                return Err(short());
            }
            let cols = u16::from_be_bytes([body[8], body[9]]);
            let rows = u16::from_be_bytes([body[10], body[11]]);
            let scrollback = u32::from_be_bytes([body[12], body[13], body[14], body[15]]);
            Frame::Checkpoint {
                id: id(body)?,
                size: TerminalSize::new(cols, rows),
                scrollback: scrollback as usize,
                ansi: body[16..].to_vec(),
            }
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame kind {other}"),
            ));
        }
    })
}

/// Writes one encoded frame.
pub fn send(writer: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    writer.write_all(frame)?;
    writer.flush()
}
