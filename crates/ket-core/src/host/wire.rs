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
//! build of ket. A window talks to whatever host is running, whichever build
//! it is: each side logs and skips a message it cannot read, so a message one
//! side doesn't know costs that feature, never the connection.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::activity::Foreground;
use crate::agent_hooks::{HookReport, StatusLine};
use crate::agent_status::AgentStatusSnapshot;
use crate::rate_limits::ProviderSnapshot;
use crate::terminal::{StopOutcome, TerminalSize, TerminalSpec};

/// Sent in [`Request::Hello`] and [`Reply::Welcome`] and checked by nothing.
///
/// Never change it: hosts built before the check was removed drop a window
/// whose hello carries anything but 6, and they live as long as their
/// terminals do.
pub const PROTOCOL: u32 = 6;

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

/// An agent tab a window has put to sleep — see [`Request::Asleep`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsleepTab {
    /// The tab's key, `worktree|agent|nonce`: what a wake names, and what
    /// the woken terminal is opened under.
    pub key: String,
    /// The worktree's id.
    pub worktree: String,
    /// The agent it ran.
    pub agent: String,
    /// The tab's title.
    pub title: String,
    /// When it was put to sleep, in milliseconds since the epoch.
    pub since_ms: u64,
    /// How much memory stopping it freed, as the window measured it.
    pub freed_bytes: u64,
}

/// A relay elsewhere the host links to beside its own, for devices away from
/// this Mac's networks — see [`crate::config::PhonesConfig::relay`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedRelay {
    /// Where it is, as configured.
    pub url: String,
    /// Whether the host is registered with it now.
    pub linked: bool,
    /// Why the last attempt to link failed, while it is not linked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

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
    /// Ask an agent to exit, then force its process group to end after a grace period.
    Stop {
        /// Echoed in the reply.
        req: u64,
        /// Which terminal.
        id: u64,
        /// How long to allow for session data to be written.
        grace_ms: u64,
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
    /// The pull requests this window's worktrees are open as, with their
    /// checks, for the host to pass on to phones. Sent on connecting and
    /// whenever a read of GitHub changes them; the newest from any window
    /// wins.
    PullRequests {
        /// Each worktree's id, and its pull request.
        pull_requests: Vec<(String, crate::github::PullRequestStatus)>,
    },
    /// The agent tabs this window has put to sleep — stopped, their sessions
    /// kept — for the host to list to phones, which see no terminal for
    /// them otherwise. Sent on connecting and whenever the set changes; the
    /// host keeps each window's while it is connected, so it knows which
    /// window to ask to wake one.
    Asleep {
        /// Every asleep tab, in no order.
        tabs: Vec<AsleepTab>,
    },
    /// The result of waking a tab a phone asked to wake, when it could not
    /// be woken: the host learns a wake that worked from the terminal the
    /// window opens under the tab's key.
    WakeResult {
        /// The request named in [`Reply::Wake`].
        req: u64,
        /// Why the tab stays asleep, as the desktop's tab says it, or `None`
        /// once the window has started waking it.
        error: Option<String>,
    },
    /// The answer to [`Reply::StartFresh`]: the key of the tab opened beside
    /// the cold one, sent once its agent has reported itself started, or
    /// why none was. The host then finds that tab's terminal to answer the
    /// phone with.
    FreshResult {
        /// The request named in [`Reply::StartFresh`].
        req: u64,
        /// The fresh tab's pane key, once it is opening.
        key: Option<String>,
        /// Why there is none, as the desktop would say it.
        error: Option<String>,
    },
    /// The result of handing a phone's failed checks to an agent. The host
    /// waits for this before answering the phone.
    FixChecksResult {
        /// The request named in [`Reply::FixChecks`].
        req: u64,
        /// Why the agent could not be started, or `None` once it has the
        /// prompt.
        error: Option<String>,
    },
    /// The result of sending a phone's review to an agent. The host waits
    /// for this before answering the phone.
    SendReviewResult {
        /// The request named in [`Reply::SendReview`].
        req: u64,
        /// Why it was not sent, as the desktop's own Send to agent says it,
        /// or `None` once the agent has it.
        error: Option<String>,
    },
    /// The result of deleting a worktree a phone asked to delete. The host
    /// waits for this before answering the phone.
    DeleteWorkResult {
        /// The request named in [`Reply::DeleteWork`].
        req: u64,
        /// Why it was not deleted, as the desktop's delete dialog says it,
        /// or `None` once it is gone.
        error: Option<String>,
        /// The branch kept because it has unmerged commits.
        #[serde(default)]
        kept_branch: Option<String>,
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
    /// New work asked for from a terminal — `ket start`, `ket backlog
    /// start` — handed to one window as [`Reply::StartWork`], the way a
    /// phone's is. Answered with [`Reply::WorkTaken`].
    StartWork {
        /// The project's id.
        project: String,
        /// What the agent is to do.
        prompt: String,
        /// Which agent, by name; `None` for the project's own.
        agent: Option<String>,
        /// The backlog note this is, for the window to build the work from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Something for a worktree's browser tab to do, asked from a terminal
    /// — `ket browser` — and handed to one window as
    /// [`Reply::BrowserAction`]. Answered with [`Reply::BrowserAnswer`].
    Browser {
        /// Echoed in the answer.
        req: u64,
        /// The worktree's id.
        worktree: String,
        /// What to do.
        action: BrowserAction,
    },
    /// What a window made of a [`Reply::BrowserAction`]. The host waits for
    /// this before answering the terminal.
    BrowserResult {
        /// The request named in [`Reply::BrowserAction`].
        req: u64,
        /// What the page did, or why it could not be done, as the terminal
        /// is to print it.
        result: Result<BrowserAnswer, String>,
    },
}

/// What `ket browser` asks of a worktree's browser tab — see
/// [`Request::Browser`]. The tab is the worktree's active Chromium tab, else
/// its most recent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAction {
    /// Load `url` and wait for it, opening a tab when the worktree has none;
    /// with no URL, only say what the tab shows.
    Open {
        /// Where to go; `None` to stay.
        #[serde(default)]
        url: Option<String>,
    },
    /// List the worktree's browser tabs.
    Tabs,
    /// Go back a page, and wait for it.
    Back,
    /// Go forward a page, and wait for it.
    Forward,
    /// Load the page again, and wait for it.
    Reload,
    /// Save a PNG of the page, and say where.
    Screenshot {
        /// The whole page rather than what fits in the tab.
        #[serde(default)]
        full_page: bool,
    },
    /// What the page has logged to its console, oldest first.
    Console {
        /// Only errors.
        #[serde(default)]
        errors_only: bool,
        /// Only what was logged in the last this many milliseconds.
        #[serde(default)]
        since_ms: Option<u64>,
        /// At most this many, the latest.
        lines: u32,
    },
    /// Run JavaScript in the page, and say what it came to.
    Eval {
        /// The JavaScript. A promise it comes to is awaited.
        expression: String,
    },
    /// The page's accessibility tree, a node a line, each with a ref the
    /// actions below take — see [`BrowserTarget::Ref`].
    Snapshot {
        /// Only what can be acted on: buttons, links, fields and the like.
        #[serde(default)]
        interactive: bool,
        /// At most this many lines, the first.
        max: u32,
    },
    /// Click an element with the mouse, as a person would: trusted events
    /// at its centre, once it is scrolled into view and nothing covers it.
    Click {
        /// The element.
        target: BrowserTarget,
        /// Twice, as a double-click.
        #[serde(default)]
        double: bool,
        /// With the right button.
        #[serde(default)]
        right: bool,
    },
    /// Put text in a field, in place of what it holds, as typing would; or
    /// pick a `<select>`'s option by its text or value.
    Fill {
        /// The field.
        target: BrowserTarget,
        /// What it is to hold.
        text: String,
    },
    /// Type text into whatever has focus.
    Type {
        /// What to type.
        text: String,
        /// A key down and up for each character, for pages that listen to
        /// keys rather than to input.
        #[serde(default)]
        keys: bool,
    },
    /// Press a key, with any modifiers: `Enter`, `Shift+Tab`, `Meta+A`.
    Press {
        /// The key, as the person asked for it.
        key: String,
    },
    /// Scroll the page, or an element in it, with the mouse wheel.
    Scroll {
        /// The element whose centre the wheel turns at; `None` for the
        /// page's.
        #[serde(default)]
        target: Option<BrowserTarget>,
        /// Which way, and how far.
        by: BrowserScroll,
    },
    /// Wait until something is true of the page, looking every tenth of a
    /// second.
    Wait {
        /// What.
        until: BrowserWait,
        /// How long to wait before saying it never happened, in
        /// milliseconds.
        timeout_ms: u64,
    },
    /// Start recording the tab to a video, which [`BrowserAction::RecordStop`]
    /// finishes. A worktree records one tab at a time.
    RecordStart {
        /// The longest the video may run, in seconds: it stops itself
        /// then, and the next stop says what it made.
        max_seconds: u32,
    },
    /// Stop the worktree's recording, and say what it made — see
    /// [`BrowserAnswer::Recording`].
    RecordStop,
    /// Answer the JavaScript dialog the page has open — `alert`, `confirm`,
    /// `prompt`, or `beforeunload`'s question — and wait for whatever that
    /// starts.
    Dialog {
        /// OK, or Cancel.
        accept: bool,
        /// What to put in a prompt's field; `None` leaves it as the page
        /// filled it in.
        #[serde(default)]
        text: Option<String>,
    },
}

/// The element a [`BrowserAction`] is aimed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTarget {
    /// `@eN`: the node [`BrowserAnswer::Snapshot`] gave this number, in the
    /// document it was taken of.
    Ref(u32),
    /// The first element a CSS selector matches.
    Selector(String),
}

impl std::fmt::Display for BrowserTarget {
    /// As the terminal writes it in a message: `@e6`, or `"#save"`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ref(n) => write!(f, "@e{n}"),
            Self::Selector(selector) => write!(f, "\"{selector}\""),
        }
    }
}

/// Which way [`BrowserAction::Scroll`] goes, and how far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserScroll {
    /// Down this many CSS pixels; `None` for one screen.
    Down(Option<u32>),
    /// Up this many CSS pixels; `None` for one screen.
    Up(Option<u32>),
    /// All the way down.
    Bottom,
    /// All the way up.
    Top,
}

/// What [`BrowserAction::Wait`] waits for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserWait {
    /// The element to be on the page and visible.
    Shown(BrowserTarget),
    /// The element to be off the page, or hidden.
    Gone(BrowserTarget),
    /// The page to show this text.
    Text(String),
    /// The page's address to contain this.
    Url(String),
    /// The page to finish loading.
    Load,
}

/// What a worktree's browser tab did — see [`Request::BrowserResult`].
/// Not `Eq`: a recording's length is a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAnswer {
    /// The page the tab shows now, once it has loaded or the window has
    /// stopped waiting for it to.
    Done {
        /// Its address.
        url: String,
        /// Its title.
        title: String,
    },
    /// The worktree's browser tabs, in the window's order.
    Tabs {
        /// Each tab.
        tabs: Vec<BrowserTab>,
    },
    /// Where the window saved the page's PNG — see
    /// [`crate::capture::save_screenshot`]. The image itself never crosses
    /// the socket.
    Screenshot {
        /// The file, absolute.
        path: String,
    },
    /// What the page logged, oldest first.
    Console {
        /// Each message.
        lines: Vec<ConsoleLine>,
    },
    /// What the expression came to: a string as it is, anything else as
    /// JSON.
    Value {
        /// The text, cut short when it was long.
        value: String,
        /// How many bytes it was before it was cut; `None` when it was not.
        #[serde(default)]
        cut_from: Option<u64>,
    },
    /// The page's accessibility tree, in document order.
    Snapshot {
        /// The page's address.
        url: String,
        /// Its title.
        title: String,
        /// Each node shown, a line each.
        nodes: Vec<SnapshotNode>,
        /// How many more lines there were past the most asked for.
        #[serde(default)]
        cut: u32,
    },
    /// The tab is being recorded; [`BrowserAction::RecordStop`] finishes
    /// the video.
    RecordingStarted,
    /// What a recording made, in the files the window wrote — see
    /// [`crate::capture::new_recording`]. The video itself never crosses the
    /// socket.
    Recording {
        /// The MP4, absolute.
        video: String,
        /// A few frames of it as PNGs beside it, in order, the last its
        /// final frame: for an agent that reads images but not video.
        frames: Vec<String>,
        /// How long the video runs.
        seconds: f64,
    },
}

/// A line of [`BrowserAnswer::Snapshot`]: one node of the page's
/// accessibility tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotNode {
    /// How far in it is under the lines above it; `0` for the outermost.
    pub depth: u16,
    /// What it is: `button`, `link`, `textbox`, `heading`, or `text` for
    /// text on its own.
    pub role: String,
    /// Its accessible name, or the text; empty when it has none.
    #[serde(default)]
    pub name: String,
    /// The number of its ref, `@eN`, which the actions take; `None` for a
    /// node with no element of its own.
    #[serde(default)]
    pub r#ref: Option<u32>,
    /// What a field holds, when it holds something.
    #[serde(default)]
    pub value: Option<String>,
    /// Its states, each a word: `checked`, `disabled`, `expanded`,
    /// `level=2`.
    #[serde(default)]
    pub states: Vec<String>,
}

/// A message a page logged to its console, for `ket browser console`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleLine {
    /// When, in milliseconds since the epoch.
    pub at_ms: u64,
    /// When, as `HH:MM:SS` on the Mac's clock: the window knows the time
    /// zone, and the CLI reads no clock of its own.
    pub time: String,
    /// `debug`, `info`, `warning` or `error`.
    pub level: String,
    /// What it logged.
    pub text: String,
    /// The script it came from.
    pub source: String,
    /// The line in that script; `0` when there is none.
    pub line: u32,
}

/// One of a worktree's browser tabs, for `ket browser tabs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserTab {
    /// What it shows.
    pub url: String,
    /// The page's title.
    pub title: String,
    /// Whether it is the tab `ket browser` acts on.
    pub active: bool,
    /// The JavaScript dialog its page has open, as a terminal says it: `a
    /// confirm: "Delete 3 files?"`. Every command but `dialog` waits on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<String>,
}

/// Who asked for work a window is handed: who its messages name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkSource {
    /// A paired phone. What a host from before this field means.
    #[default]
    Phone,
    /// `ket start` or `ket backlog start`, run in a terminal.
    Cli,
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

/// One of a phone's review comments, on the local wire — see
/// [`Reply::SendReview`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneReviewComment {
    /// The file, slash-separated from the worktree's root.
    pub rel: String,
    /// Which lines: `lines 12–18`.
    pub lines: String,
    /// The lines themselves, each with its `+`, `-` or space.
    pub text: String,
    /// What should change.
    pub comment: String,
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
        /// The process that roots the terminal's tree, when known.
        #[serde(default)]
        pid: Option<u32>,
    },
    /// The host's decision on a stop request.
    Stopped {
        /// From the request.
        req: u64,
        /// Why the request succeeded or was refused.
        outcome: StopOutcome,
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
    /// The network inspector logged a request — see [`crate::network`].
    ///
    /// Sent to every window as each request's reply ends, so a Network
    /// panel showing the worktree adds the row without reading the log
    /// again. The summary only: a body is read from disk when a tab wants
    /// it.
    NetworkRequest {
        /// The worktree the request's pane belongs to.
        worktree: String,
        /// The request, as the log has it.
        summary: Box<crate::network::store::RequestSummary>,
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
        /// The relay elsewhere the host links to as well, and how that
        /// link is doing; absent when there is none, and from a host older
        /// than #142.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hosted: Option<HostedRelay>,
    },
    /// The answer to [`Request::Revoke`].
    Revoked {
        /// Whether a phone by that id was paired.
        found: bool,
        /// Why it could not be done.
        error: Option<String>,
    },
    /// A phone or a terminal asked for new work: a worktree in `project`
    /// whose agent starts on `prompt`. Sent to one window, which makes the
    /// worktree the way its own dialog does — see `ket_ui`'s
    /// `start_phone_work`.
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
        /// Who asked. A host from before it sends none, which is a phone.
        #[serde(default)]
        from: WorkSource,
        /// A phone's selected base; absent for the project default.
        #[serde(default)]
        base: Option<String>,
        /// Session-only launch choices.
        #[serde(default)]
        model: Option<String>,
        /// Session-only effort.
        #[serde(default)]
        effort: Option<String>,
        /// Existing worktree to open a fresh session in.
        #[serde(default)]
        worktree: Option<String>,
        /// Open a plain shell rather than an agent.
        #[serde(default)]
        shell: bool,
    },
    /// The answer to [`Request::StartWork`].
    WorkTaken {
        /// Whether a window was open to hand it to.
        taken: bool,
    },
    /// A phone asked for a fresh session beside a terminal whose session
    /// went cold, as the window's own Start fresh does — see
    /// `crate::cold`. Sent to the window showing that terminal, which
    /// answers with [`Request::FreshResult`].
    StartFresh {
        /// Names the request in the answer.
        req: u64,
        /// The terminal's pane key.
        key: String,
        /// The cold session the phone was shown, which the window checks is
        /// still the one in the tab.
        session: String,
        /// When that session's last request was, which the window checks it
        /// has made none since.
        since_ms: u64,
    },
    /// A phone asked for a worktree to be merged, as ket's Merge does. Sent to
    /// one window, which runs its own merge on it.
    MergeWork {
        /// The worktree's id.
        worktree: String,
    },
    /// A phone asked for a worktree's failed checks to go to its agent, as
    /// the worktree menu's Fix checks with agent does. The host has read
    /// them, so that it could tell the phone if that failed; sent to one
    /// window, which types the message into the agent.
    FixChecks {
        /// Used to return the result once the agent has the prompt.
        req: u64,
        /// The worktree's id.
        worktree: String,
        /// What the agent is told — see [`crate::checks::hand_over`].
        prompt: String,
    },
    /// A phone's comments on a worktree's diff, for its agent, as the Review
    /// section's Send to agent sends them. Sent to one window, which writes
    /// them into one message and types it into the agent.
    SendReview {
        /// Used to return the result.
        req: u64,
        /// The worktree's id.
        worktree: String,
        /// The branch the phone's diff was read against; `None` for a
        /// checkout's uncommitted changes.
        base: Option<String>,
        /// The comments, in the order they were written.
        comments: Vec<PhoneReviewComment>,
        /// Unix milliseconds after which the phone has been told the review
        /// did not go: a window that takes it later drops it, so asking again
        /// does not send it twice.
        until_ms: u64,
    },
    /// A phone asked for a worktree to be deleted, as the worktree menu's
    /// Delete does once its dialog is confirmed. Sent to one window, which
    /// deletes it and closes its terminals and tabs, and answers with
    /// [`Request::DeleteWorkResult`].
    DeleteWork {
        /// Used to return the result.
        req: u64,
        /// The worktree's id.
        worktree: String,
        /// Delete it even though work would be lost, as Delete anyway does.
        force: bool,
    },
    /// A phone asked for an asleep tab to be woken — see
    /// [`Request::Asleep`]. Sent to the window that reported the tab. The
    /// host answers the phone once a terminal is opened under the tab's key,
    /// or when the window sends [`Request::WakeResult`] with why it won't be.
    Wake {
        /// Used to return a refusal.
        req: u64,
        /// The tab's key.
        key: String,
    },
    /// Something other than this window changed the worktrees in ket's store
    /// — a phone pinned one — so the sidebar is to be read again. Sent to
    /// every window.
    WorktreesChanged,
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
    /// `ket browser` asked something of a worktree's browser tab. Sent to
    /// one window — the one showing the worktree's agent, else the first —
    /// which does it and answers with [`Request::BrowserResult`].
    BrowserAction {
        /// Used to return the result.
        req: u64,
        /// The worktree's id.
        worktree: String,
        /// What to do.
        action: BrowserAction,
    },
    /// The answer to [`Request::Browser`].
    BrowserAnswer {
        /// From the request.
        req: u64,
        /// What the page did, or why it could not be done.
        result: Result<BrowserAnswer, String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether each of `actions` reads back as itself.
    fn round_trip(actions: &[BrowserAction]) {
        for action in actions {
            let json = serde_json::to_string(action).unwrap();
            let back: BrowserAction = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, action, "{json}");
        }
    }

    #[test]
    fn a_device_list_from_a_host_before_relays_elsewhere_still_reads() {
        let old = r#"{"type":"devices","devices":[]}"#;
        let Reply::Devices { devices, hosted } = serde_json::from_str(old).unwrap() else {
            panic!("not a device list");
        };
        assert_eq!(devices.map(|devices| devices.len()), Some(0));
        assert!(hosted.is_none());

        let hosted = HostedRelay {
            url: "wss://relay.example.test".into(),
            linked: false,
            error: Some("no answer".into()),
        };
        let json = serde_json::to_string(&Reply::Devices {
            devices: None,
            hosted: Some(hosted.clone()),
        })
        .unwrap();
        let Reply::Devices { hosted: back, .. } = serde_json::from_str(&json).unwrap() else {
            panic!("not a device list");
        };
        assert_eq!(back, Some(hosted));
    }

    #[test]
    fn a_snapshot_crosses_the_wire_intact() {
        round_trip(&[BrowserAction::Snapshot {
            interactive: true,
            max: 400,
        }]);
        let answer = BrowserAnswer::Snapshot {
            url: "http://localhost:3000/".to_owned(),
            title: "Sign in".to_owned(),
            nodes: vec![SnapshotNode {
                depth: 1,
                role: "checkbox".to_owned(),
                name: "Remember me".to_owned(),
                r#ref: Some(3),
                value: None,
                states: vec!["checked".to_owned()],
            }],
            cut: 12,
        };
        let json = serde_json::to_string(&answer).unwrap();
        assert!(json.contains(r#""ref":3"#), "{json}");
        assert_eq!(
            serde_json::from_str::<BrowserAnswer>(&json).unwrap(),
            answer
        );
    }

    #[test]
    fn clicks_and_fills_cross_the_wire_intact() {
        round_trip(&[
            BrowserAction::Click {
                target: BrowserTarget::Ref(6),
                double: true,
                right: false,
            },
            BrowserAction::Fill {
                target: BrowserTarget::Selector("#email".to_owned()),
                text: "brad@example.com".to_owned(),
            },
        ]);
        assert_eq!(BrowserTarget::Ref(6).to_string(), "@e6");
        assert_eq!(
            BrowserTarget::Selector("#save".to_owned()).to_string(),
            "\"#save\""
        );
    }

    #[test]
    fn keys_cross_the_wire_intact() {
        round_trip(&[
            BrowserAction::Type {
                text: "hello".to_owned(),
                keys: true,
            },
            BrowserAction::Press {
                key: "Shift+Tab".to_owned(),
            },
        ]);
    }

    #[test]
    fn recordings_cross_the_wire_intact() {
        round_trip(&[
            BrowserAction::RecordStart { max_seconds: 120 },
            BrowserAction::RecordStop,
        ]);
        for answer in [
            BrowserAnswer::RecordingStarted,
            BrowserAnswer::Recording {
                video: "/cache/recordings/recording-1-0.mp4".to_owned(),
                frames: vec!["/cache/recordings/recording-1-0-1.png".to_owned()],
                seconds: 4.25,
            },
        ] {
            let json = serde_json::to_string(&answer).unwrap();
            assert_eq!(
                serde_json::from_str::<BrowserAnswer>(&json).unwrap(),
                answer,
                "{json}"
            );
        }
    }

    #[test]
    fn scrolls_and_waits_cross_the_wire_intact() {
        round_trip(&[
            BrowserAction::Scroll {
                target: None,
                by: BrowserScroll::Down(None),
            },
            BrowserAction::Scroll {
                target: Some(BrowserTarget::Ref(2)),
                by: BrowserScroll::Up(Some(300)),
            },
            BrowserAction::Scroll {
                target: None,
                by: BrowserScroll::Bottom,
            },
            BrowserAction::Wait {
                until: BrowserWait::Gone(BrowserTarget::Selector(".spinner".to_owned())),
                timeout_ms: 10_000,
            },
            BrowserAction::Wait {
                until: BrowserWait::Load,
                timeout_ms: 500,
            },
        ]);
    }

    #[test]
    fn dialogs_cross_the_wire_intact() {
        round_trip(&[
            BrowserAction::Dialog {
                accept: true,
                text: Some("Sam".to_owned()),
            },
            BrowserAction::Dialog {
                accept: false,
                text: None,
            },
        ]);
        // A tab from a window that names no dialog has none.
        let tab: BrowserTab =
            serde_json::from_str(r#"{"url":"about:blank","title":"","active":true}"#).unwrap();
        assert_eq!(tab.dialog, None);
        let open = BrowserTab {
            dialog: Some("an alert: \"Saved\"".to_owned()),
            ..tab
        };
        let json = serde_json::to_string(&open).unwrap();
        assert_eq!(serde_json::from_str::<BrowserTab>(&json).unwrap(), open);
    }
}
