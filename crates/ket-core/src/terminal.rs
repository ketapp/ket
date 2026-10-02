//! One terminal per worktree: a pty, a parser, and the grid between them.
//!
//! ket does not emulate a terminal. `alacritty_terminal` does, and it owns the
//! grid, the scrollback and every escape sequence in between — that is a decade
//! of other people's bug reports, and reimplementing it is how a tool like this
//! spends a year drawing boxes slightly wrong. What ket owns is the three things
//! a library cannot decide for it: when the pty is read, when the screen is
//! declared changed, and how much of it is kept.
//!
//! There is no wire format for the renderer. The shell is `gpui` in the same
//! process (Epic 5), so it reads [`Term`] under a lock rather than reading a
//! stream of diffs off a socket. A `Terminal` is *state*, not a transport. The
//! one exception is a client in another process, which cannot share the lock:
//! [`LocalTerminal::checkpoint`] writes the screen out as escape sequences for
//! it, and [`LocalTerminal::output_since`] hands it the bytes after — see
//! [`checkpoint`]. The ket host (`crate::host`) is that other process's
//! counterpart: with it running, a window's [`Terminal`] is a copy of one the
//! host owns, kept up to date that way, and outlives the window.
//!
//! Three bounds, and each one is a specific failure:
//!
//! - **Frame pacing.** A build emitting ten thousand lines a second must not ask
//!   for ten thousand repaints. Damage accumulates and becomes at most one frame
//!   per [`FRAME_INTERVAL`], so the firehose costs ~60 repaints a second instead
//!   of ~10,000 — see `FrameClock`.
//! - **A bounded queue.** Exactly [`OUTPUT_QUEUE`] reads may be in flight between
//!   the reader thread and the parser. When the parser falls behind, the reader
//!   blocks, then the pty buffer fills, then the *program* blocks. Backpressure
//!   reaches the writer instead of a queue growing until something dies.
//! - **Bounded scrollback.** [`MAX_SCROLLBACK_LINES`] or [`MAX_SCROLLBACK_BYTES`],
//!   whichever binds first. Unbounded scrollback across many worktrees in many
//!   projects is the most likely way this tool quietly becomes the memory hog it
//!   was built to avoid.
//!
//! Invariant 4 runs through all of it: no operation on a `Terminal` waits on the
//! program inside it. [`Terminal::kill`] does not wait for the child to die,
//! [`Terminal::send_input`] refuses rather than blocks when the program has
//! stopped reading, and the one thread that can block forever on a `read(2)` is
//! never joined.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{Processor, Timeout};
use portable_pty::{
    ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system,
};
use tokio::sync::watch;

use crate::activity::Foreground;
use crate::{KetError, Result};

pub mod checkpoint;
pub(crate) mod remote;

use checkpoint::Stream;
pub use checkpoint::{Checkpoint, OutputCursor, Since};

/// Re-exported so a renderer can read the grid without pinning the version twice.
///
/// [`Terminal::with_term`] hands out an `alacritty_terminal` type, which makes
/// that crate part of this module's public surface whether it is named in the
/// consumer's manifest or not. Two manifests naming two versions would compile
/// and then fail to typecheck at the seam, which is a confusing way to learn
/// about a duplicate dependency.
pub use alacritty_terminal;

/// Most scrollback lines kept for one terminal.
///
/// The line cap and [`MAX_SCROLLBACK_BYTES`] both apply; see
/// [`scrollback_limit`] for which one actually binds, because at any realistic
/// width it is not this one.
pub const MAX_SCROLLBACK_LINES: usize = 10_000;

/// Most memory one terminal's grid may hold, in bytes.
///
/// Counts the scrollback *and* both live screens, because a terminal costs what
/// it costs and a budget that excludes half of the allocation is not a budget.
pub const MAX_SCROLLBACK_BYTES: usize = 4 * 1024 * 1024;

/// How long damage accumulates before it becomes a repaint.
///
/// One frame at 60Hz. Long enough that a firehose coalesces, short enough that a
/// person typing does not see the gap.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Reads in flight between the reader thread and the parser.
///
/// The knob that turns "the consumer fell behind" into backpressure rather than
/// into a growing queue. Four reads of 64 KiB is a quarter of a
/// megabyte per terminal at the very worst, and the reader blocks past that.
pub const OUTPUT_QUEUE: usize = 4;

/// Bytes read from the pty in one call.
const READ_BUFFER: usize = 64 * 1024;

/// Writes queued for the pty before input is refused.
///
/// Generous for keystrokes, which is all this is ever meant to carry. A queue
/// this deep only fills when the program has stopped reading, and then blocking
/// would be the wrong answer anyway.
const INPUT_QUEUE: usize = 64;

/// Smallest screen alacritty's grid arithmetic is defined for.
///
/// A pane collapsed to nothing is an ordinary UI event, and `columns() - 1` on
/// an empty grid is a panic in a library that has no reason to expect one.
const MIN_COLS: u16 = 2;
/// Smallest screen height, for the same reason as [`MIN_COLS`].
const MIN_ROWS: u16 = 1;

/// Columns and rows of a terminal's visible screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalSize {
    /// Width in character cells.
    pub cols: u16,
    /// Height in character cells.
    pub rows: u16,
}

impl TerminalSize {
    /// Builds a size, clamped to something the grid can represent.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(MIN_COLS),
            rows: rows.max(MIN_ROWS),
        }
    }

    /// The kernel's view of this size.
    fn pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// The answer to a program that asks the terminal how big it is.
    ///
    /// Pixel dimensions are zero because core has no idea what a cell looks
    /// like; the renderer owns that, and a made-up number is worse than none.
    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: 0,
            cell_height: 0,
        }
    }

    /// Packs into one word, so the parser thread can read it without a lock.
    fn pack(self) -> u32 {
        (u32::from(self.cols) << 16) | u32::from(self.rows)
    }

    /// Inverse of [`TerminalSize::pack`].
    fn unpack(bits: u32) -> Self {
        Self {
            cols: (bits >> 16) as u16,
            rows: (bits & 0xffff) as u16,
        }
    }
}

impl Default for TerminalSize {
    /// What every tool assumes when asked nothing, widened for modern panes.
    fn default() -> Self {
        Self::new(120, 40)
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// What to run inside a terminal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalSpec {
    /// Program to run.
    pub command: String,
    /// Arguments to it.
    pub args: Vec<String>,
    /// Directory to run in — for ket, the worktree this pane belongs to.
    pub cwd: PathBuf,
    /// Environment entries set on top of the inherited environment.
    pub env: BTreeMap<String, String>,
    /// Variables stripped from the inherited environment before spawning.
    ///
    /// The reason [`crate::config::AgentSpec::env_remove`] exists: ket is
    /// regularly launched from inside an agent session, and an agent that
    /// finds its own marker variable refuses to start. A pane running an
    /// agent needs the same stripping the ACP transport already does, or the
    /// session dies on the first keystroke with a message about nesting.
    pub env_remove: Vec<String>,
    /// Screen size the pty is opened at.
    pub size: TerminalSize,
    /// What the ket host files this terminal under, so a window that comes
    /// back can find it again — the worktree and the agent, in ket's own use.
    /// Ignored when the terminal runs in-process.
    pub key: Option<String>,
    /// Take over a live terminal the host has under `key` that no window is
    /// showing, instead of starting a new one. What a window restoring its
    /// tabs asks for; a new tab never does.
    pub adopt: bool,
}

impl TerminalSpec {
    /// An interactive shell in `cwd`.
    ///
    /// `$SHELL` rather than a hard-coded path: a pane that ignores the shell
    /// someone configured is a pane with the wrong aliases, prompt and history.
    pub fn shell(cwd: impl Into<PathBuf>) -> Self {
        let launch = crate::shell::launch(&crate::shell::login_shell(), None);
        Self {
            command: launch.command,
            args: launch.args,
            cwd: cwd.into(),
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            size: TerminalSize::default(),
            key: None,
            adopt: false,
        }
    }

    /// An agent's own command, run interactively in `cwd`.
    ///
    /// The agent's terminal UI, not ACP: this is the pane a person types into,
    /// and what they expect from clicking a worktree is the session they would
    /// have started by hand.
    pub fn agent(cwd: impl Into<PathBuf>, spec: &crate::config::AgentSpec) -> Self {
        Self::agent_with(cwd, spec, &[])
    }

    /// The same, with `extra` appended to the agent's own arguments.
    ///
    /// What resuming a session uses. The extra words join the command *line*
    /// rather than an argument vector, because the line is what gets typed:
    /// see [`crate::shell`] for why an agent is not spawned directly.
    pub fn agent_with(
        cwd: impl Into<PathBuf>,
        spec: &crate::config::AgentSpec,
        extra: &[String],
    ) -> Self {
        let launch =
            crate::shell::launch(&crate::shell::login_shell(), Some(&spec.launch_line(extra)));
        // The shell's own variables go on last: they are how the wrapper is
        // told which command to type, and a spec that happened to set the same
        // name would otherwise launch a pane that runs the wrong thing.
        let mut env = spec.env.clone();
        env.extend(launch.env);
        Self {
            command: launch.command,
            args: launch.args,
            cwd: cwd.into(),
            env,
            env_remove: spec.env_remove.clone(),
            size: TerminalSize::default(),
            key: None,
            adopt: false,
        }
    }
}

/// Bytes one allocated grid row costs at this width.
///
/// Measured from the types rather than guessed, because `Cell` is the widest
/// lever on this budget and it is not ket's to hold still.
fn row_bytes(cols: usize) -> usize {
    size_of::<Row<Cell>>() + cols * size_of::<Cell>()
}

/// Scrollback lines that satisfy both caps at this size.
///
/// The two caps are not close. A row is `cols` cells wide however few of them
/// are written, so at 120 columns [`MAX_SCROLLBACK_BYTES`] is spent at roughly
/// 1,300 lines and [`MAX_SCROLLBACK_LINES`] never comes near binding; the line
/// cap only matters for a very narrow pane. That is the honest shape of the
/// budget, and it is why the byte cap is the one written down first.
///
/// Both live screens are charged against the budget: alacritty keeps the
/// alternate screen allocated alongside the primary one, so a terminal running
/// `vim` holds two full grids whether or not anyone is looking at both.
pub fn scrollback_limit(size: TerminalSize) -> usize {
    let per_row = row_bytes(size.columns()).max(1);
    let rows_in_budget = MAX_SCROLLBACK_BYTES / per_row;
    rows_in_budget
        .saturating_sub(2 * size.screen_lines())
        .min(MAX_SCROLLBACK_LINES)
}

/// The terminal configuration ket runs every pane with.
fn term_config(scrolling_history: usize) -> TermConfig {
    TermConfig {
        scrolling_history,
        ..TermConfig::default()
    }
}

/// Takes a lock, ignoring poisoning.
///
/// Every mutex here guards a small independent fact — the size, the title, the
/// exit status. A panic while one of them was held says nothing about the
/// others, and propagating it would turn one bad pane into a dead window.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The full command line for a pid, or `None` if it cannot be read.
///
/// `args=` rather than `comm=`, and that matters: an agent installed through
/// npm runs as `node /…/@anthropic-ai/claude-code/cli.js`, so the executable
/// name is `node` and says nothing. The arguments are where the agent's own
/// name survives, whichever way it was installed.
///
/// `ps` rather than a process-inspection crate: this runs once per terminal
/// per *change* of foreground, not per poll, so the spawn is not the cost it
/// would be on a hot path — and it keeps `ket-core` from growing a dependency
/// on a platform API for one string.
fn command_name(pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// What the reader thread hands the parser.
enum Pulse {
    /// Bytes read from the pty.
    Bytes(Vec<u8>),
    /// The pty reached end of file. No more bytes will ever arrive.
    Eof,
    /// Nothing to apply; look at the shutdown flag.
    ///
    /// The parser blocks indefinitely when there is no damage pending, which is
    /// the only way an idle terminal costs nothing. This is how it is woken.
    Wake,
}

/// Answers the terminal's own questions and records what it announces.
///
/// `Term` reports back through this. Most of what it reports is for a UI, but
/// two kinds are not optional: a program that asks the terminal for its size, or
/// that issues a device-status query, is *waiting for a reply*. A terminal that
/// never answers is a program that never continues — invariant 4 as it shows up
/// inside the emulator rather than around it.
///
/// Colour queries are deliberately left unanswered: core has no palette, the
/// renderer owns colour, and inventing an answer makes a program pick a theme
/// for a screen nobody has drawn yet.
#[derive(Debug)]
pub struct Notifier {
    /// Replies go back to the program the same way keystrokes do.
    input: SyncSender<Vec<u8>>,
    /// Current screen size, packed so this never takes a lock the resize path
    /// also wants — see [`Terminal::resize`] for the ordering that avoids.
    size: Arc<AtomicU32>,
    /// Last title the program set, for whoever labels the pane.
    title: Arc<Mutex<Option<String>>>,
}

impl Notifier {
    /// Queues a reply to the program running in the terminal.
    ///
    /// Dropped rather than blocked on when the queue is full. This runs on the
    /// thread that owns the grid, so blocking here stops every repaint; a lost
    /// reply to a program that has stopped reading its own answers is the
    /// cheaper failure by a wide margin.
    fn reply(&self, bytes: Vec<u8>) {
        if self.input.try_send(bytes).is_err() {
            tracing::debug!("terminal reply dropped: the program is not reading");
        }
    }
}

impl EventListener for Notifier {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => self.reply(text.into_bytes()),
            TermEvent::TextAreaSizeRequest(format) => {
                let size = TerminalSize::unpack(self.size.load(Ordering::Relaxed));
                self.reply(format(size.window_size()).into_bytes());
            }
            TermEvent::Title(title) => *lock(&self.title) = Some(title),
            TermEvent::ResetTitle => *lock(&self.title) = None,
            _ => {}
        }
    }
}

/// Decides when accumulated damage becomes a repaint.
///
/// The whole of the coalescing rule, extracted so it can be tested against
/// stated instants instead of against a stopwatch. Two properties matter: a
/// burst inside one interval produces exactly one frame, and the first byte
/// after a quiet period produces one immediately — a terminal that waits 16ms
/// to show the first character of a prompt feels broken even though it is only
/// ever one frame behind.
#[derive(Debug)]
struct FrameClock {
    /// Gap enforced between frames.
    interval: Duration,
    /// When the last frame was published.
    last: Instant,
    /// Whether anything has changed since then.
    dirty: bool,
}

impl FrameClock {
    /// Starts a clock that is immediately due once anything damages it.
    fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            last: now.checked_sub(interval).unwrap_or(now),
            dirty: false,
        }
    }

    /// Records that the grid changed.
    fn damaged(&mut self) {
        self.dirty = true;
    }

    /// How long the parser may block, or `None` to block until woken.
    ///
    /// `None` when nothing is pending is what keeps an idle terminal off the
    /// scheduler entirely: with many worktrees open, a 60Hz poll per pane is a
    /// measurable amount of nothing.
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.dirty
            .then(|| self.interval.saturating_sub(now.duration_since(self.last)))
    }

    /// Consumes pending damage if a frame is due.
    fn due(&mut self, now: Instant) -> bool {
        if self.dirty && now.duration_since(self.last) >= self.interval {
            self.last = now;
            self.dirty = false;
            true
        } else {
            false
        }
    }
}

/// One terminal run in this process: a pty, the program inside it, and the
/// grid its output paints.
///
/// Shareable across threads. The renderer reads the grid through
/// [`LocalTerminal::with_term`] and waits for repaints on
/// [`LocalTerminal::frames`]; everything else on this type is a command, and
/// none of them block on the program. The ket host owns these; a window holds
/// a [`Terminal`], which is one of these or a copy of one the host owns.
pub struct LocalTerminal {
    /// The authoritative grid. Held by the parser thread and by the renderer.
    term: Arc<FairMutex<Term<Notifier>>>,
    /// Kept for resize; the reader and writer hold clones of its ends.
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// Signals the direct child. Only reaches it, and only with `SIGHUP`.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The child's pid, which `setsid` also makes its process group id.
    pid: Option<u32>,
    /// The last foreground process group seen, and the command name resolved
    /// for it.
    ///
    /// Resolving a pid to a name costs a process spawn, and the shell polls
    /// this every few seconds per terminal. The foreground changes when a
    /// person runs something, which is rare next to how often it is asked —
    /// so the answer is kept until the pid underneath it does change.
    foreground: Mutex<Option<(i32, String)>>,
    /// Identifies this terminal in errors. The worktree, in ket's own use.
    cwd: PathBuf,
    /// Keystrokes and replies, on their way to the pty.
    input: SyncSender<Vec<u8>>,
    /// Kept only to wake the parser; the reader thread holds the real producer.
    output: SyncSender<Pulse>,
    /// Current screen size.
    size: Arc<AtomicU32>,
    /// Scrollback lines currently allowed, which changes with the width.
    limit: Arc<AtomicUsize>,
    /// Last title the program set.
    title: Arc<Mutex<Option<String>>>,
    /// How the program exited, once it has and once it has been reaped.
    status: Arc<Mutex<Option<ExitStatus>>>,
    /// Set when no further output will ever arrive.
    closed: Arc<AtomicBool>,
    /// Asks the parser to stop, whatever the program is doing.
    shutdown: Arc<AtomicBool>,
    /// Bumped once per coalesced repaint.
    frames: Arc<watch::Sender<u64>>,
    /// The output, numbered, for clients in another process.
    ///
    /// Only ever locked while `term` is held, and after it, by everything
    /// that changes the grid — which is what keeps a checkpoint and its
    /// cursor describing the same screen.
    stream: Arc<Mutex<Stream>>,
    /// The parser. The only thread here that is ever joined.
    parser: Mutex<Option<JoinHandle<()>>>,
}

impl LocalTerminal {
    /// Opens a pty, starts `spec`'s program in it, and begins parsing.
    ///
    /// Returns as soon as the program is spawned. There is no handshake to wait
    /// for and nothing to be ready: a terminal is usable the moment it exists,
    /// and a program that dies immediately shows up as output and an exit
    /// status rather than as a failure to open.
    pub fn open(spec: &TerminalSpec) -> Result<Self> {
        let size = TerminalSize::new(spec.size.cols, spec.size.rows);
        let cwd = spec.cwd.clone();

        let pair = {
            // See `crate::pty_lock` for why this has to be a lock shared with
            // every other place in this crate that calls `openpty`, and not one
            // private to `Terminal::open`.
            let _serialised = crate::pty_lock::lock();
            native_pty_system().openpty(size.pty_size())
        }
        .map_err(|e| pty_error(&cwd, format!("could not open a pty: {e}")))?;

        let mut command = CommandBuilder::new(&spec.command);
        command.args(&spec.args);
        command.cwd(&cwd);
        // A program that finds no TERM assumes a teletype and stops drawing.
        // These two describe what the grid can actually store, which is a full
        // 256-colour palette plus direct RGB.
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // ket's own switches are for the ket that was started with them, not
        // for whatever runs in its terminals: an agent that builds and runs
        // ket-ui from a pane would otherwise inherit host mode — and a relay —
        // against the owner's real data without anyone having asked for it.
        for key in [crate::host::ENABLE_ENV, crate::host::RELAY_ENV] {
            command.env_remove(key);
        }
        // Stripped before the spec's own entries, so a spec that both removes
        // and sets a variable ends up with it set.
        for key in &spec.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| pty_error(&cwd, format!("could not spawn {}: {e}", spec.command)))?;

        // The slave handle must go before the child's output can ever reach EOF:
        // while ket holds it open the master reads an empty stream forever, and
        // a pane whose program exited never notices. Same trap as `agent::pty`.
        drop(pair.slave);

        let pid = child.process_id();
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| pty_error(&cwd, format!("could not read the pty: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| pty_error(&cwd, format!("could not write to the pty: {e}")))?;

        let (input_tx, input_rx) = sync_channel::<Vec<u8>>(INPUT_QUEUE);
        let (output_tx, output_rx) = sync_channel::<Pulse>(OUTPUT_QUEUE);

        let shared_size = Arc::new(AtomicU32::new(size.pack()));
        let title = Arc::new(Mutex::new(None));
        let status = Arc::new(Mutex::new(None));
        let closed = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let limit = Arc::new(AtomicUsize::new(scrollback_limit(size)));
        let frames = Arc::new(watch::channel(0u64).0);
        let stream = Arc::new(Mutex::new(Stream::new()));

        let notifier = Notifier {
            input: input_tx.clone(),
            size: shared_size.clone(),
            title: title.clone(),
        };
        let term = Arc::new(FairMutex::new(Term::new(
            term_config(limit.load(Ordering::Relaxed)),
            &size,
            notifier,
        )));

        spawn_thread("ket-pty-writer", &cwd, move || write_pty(writer, &input_rx))?;

        let reader_out = output_tx.clone();
        let reader_status = status.clone();
        spawn_thread("ket-pty-reader", &cwd, move || {
            read_pty(reader, child, &reader_out, &reader_status);
        })?;

        let parser = {
            let term = term.clone();
            let frames = frames.clone();
            let closed = closed.clone();
            let shutdown = shutdown.clone();
            let limit = limit.clone();
            let stream = stream.clone();
            spawn_thread("ket-pty-parser", &cwd, move || {
                let shared = Shared {
                    term: &term,
                    stream: &stream,
                    frames: &frames,
                    limit: &limit,
                    closed: &closed,
                    shutdown: &shutdown,
                };
                parse_pty(&output_rx, &shared);
            })?
        };

        Ok(Self {
            term,
            master: Mutex::new(pair.master),
            foreground: Mutex::new(None),
            killer: Mutex::new(killer),
            pid,
            cwd,
            input: input_tx,
            output: output_tx,
            size: shared_size,
            limit,
            title,
            status,
            closed,
            shutdown,
            frames,
            stream,
            parser: Mutex::new(Some(parser)),
        })
    }

    /// The screen as escape sequences a fresh terminal can rebuild it from,
    /// and the point in the output they are accurate through.
    ///
    /// The grid lock is held only while the screen is copied; the escape
    /// sequences are written after it is released. The first call also starts
    /// keeping recent output for [`LocalTerminal::output_since`]; a terminal nobody
    /// checkpoints keeps none.
    pub fn checkpoint(&self) -> Checkpoint {
        let (snapshot, tail, cursor) = {
            let term = self.term.lock();
            let mut stream = lock(&self.stream);
            stream.arm();
            (
                checkpoint::Snapshot::of(&term),
                stream.tail(),
                stream.cursor(),
            )
        };
        let ansi = checkpoint::serialise(&snapshot, self.title().as_deref(), &tail);
        Checkpoint {
            cursor,
            size: snapshot.size(),
            // At least what is already there: the cap changes with the width
            // before the grid does, and a client that keeps less than the
            // checkpoint writes would lose the top of it.
            scrollback: self.scrollback_limit().max(snapshot.history()),
            ansi,
        }
    }

    /// The output after `from`, for a client that has applied a checkpoint.
    pub fn output_since(&self, from: OutputCursor) -> Since {
        lock(&self.stream).since(from)
    }

    /// Reads or drives the grid under the lock the parser also takes.
    ///
    /// A closure rather than a guard, so the lock cannot be held across an await
    /// point or parked in a struct: everything that stops the parser stops the
    /// screen. `&mut` because the useful reads are not read-only —
    /// `Term::damage` and `Term::scroll_display` both mutate.
    pub fn with_term<R>(&self, f: impl FnOnce(&mut Term<Notifier>) -> R) -> R {
        let mut term = self.term.lock();
        f(&mut term)
    }

    /// Empties the pane: the screen, and the history above it.
    ///
    /// Both halves, because a "clear" that leaves ten thousand lines one
    /// scroll away has not cleared anything — it has moved it. This is what
    /// every terminal on the platform does with its own clear command, and it
    /// is why the shell's `clear` (which only pushes the screen up into the
    /// scrollback) is not what a person pressing the chord means.
    ///
    /// The cursor is deliberately left where it was. Where it belongs after a
    /// clear is the shell's opinion — see the form feed the caller sends it —
    /// and moving it out from under a full-screen program that is mid-draw
    /// leaves the pane a mess until that program next repaints.
    pub fn clear(&self) {
        use alacritty_terminal::vte::ansi::{ClearMode, Handler};

        self.with_term(|term| {
            // In this order: clearing the screen pushes what was on it into
            // the scrollback, so the history has to go second or it goes
            // straight back in.
            term.clear_screen(ClearMode::All);
            term.clear_screen(ClearMode::Saved);
            // Not output, so no client can replay it.
            lock(&self.stream).bump();
        });
        // Nothing came from the pty to say the screen changed, and a window
        // watching through the host only hears about changes this way.
        self.frames.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A stream of repaint notifications, one per coalesced frame.
    ///
    /// The counter, not the content: what changed is in the grid, and the
    /// renderer reads it through [`Terminal::with_term`] when it is ready to.
    pub fn frames(&self) -> watch::Receiver<u64> {
        self.frames.subscribe()
    }

    /// Frames published so far.
    pub fn frame(&self) -> u64 {
        *self.frames.borrow()
    }

    /// The screen size the grid and the pty currently agree on.
    pub fn size(&self) -> TerminalSize {
        TerminalSize::unpack(self.size.load(Ordering::Relaxed))
    }

    /// The size the kernel has, read back from the pty.
    ///
    /// The one place resize can be checked rather than assumed: a program reads
    /// its width from the kernel's winsize, not from anything ket holds.
    pub fn pty_size(&self) -> Result<TerminalSize> {
        let size = lock(&self.master)
            .get_size()
            .map_err(|e| pty_error(&self.cwd, format!("could not read the pty size: {e}")))?;
        Ok(TerminalSize::new(size.cols, size.rows))
    }

    /// Scrollback lines currently retained at most.
    pub fn scrollback_limit(&self) -> usize {
        self.limit.load(Ordering::Relaxed)
    }

    /// Title the program last set, if it set one.
    pub fn title(&self) -> Option<String> {
        lock(&self.title).clone()
    }

    /// Whether the pty has closed and no further output can arrive.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// How the program exited, once it has been reaped.
    ///
    /// `None` covers three different things — still running, exited but not yet
    /// reaped, and closed its output without exiting — because none of them is
    /// an exit status and inventing one would be worse than saying nothing.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        lock(&self.status).clone()
    }

    /// Resizes the screen and tells the kernel.
    ///
    /// Grid first, then the pty. The other order leaves a window in which the
    /// program has already emitted output for a width the grid does not have
    /// yet, which is how a resize leaves a line of garbage behind.
    ///
    /// The scrollback cap is recomputed here, because it is a byte budget and a
    /// wider screen buys fewer lines with it.
    pub fn resize(&self, size: TerminalSize) -> Result<()> {
        let size = TerminalSize::new(size.cols, size.rows);
        if self.size.swap(size.pack(), Ordering::Relaxed) == size.pack() {
            // Rapid resizes repeat themselves constantly — a drag reports the
            // same size several times per frame — and a reflow is not free.
            return Ok(());
        }

        let limit = scrollback_limit(size);
        self.limit.store(limit, Ordering::Relaxed);

        {
            // Never taken while `self.size` is held: the parser thread takes
            // this lock and *then* reads the size, so the reverse order here
            // would be a deadlock between a resize and a program asking how
            // wide it is. `self.size` is an atomic precisely so it cannot be
            // half of that cycle.
            let mut term = self.term.lock();
            term.resize(size);
            term.set_options(term_config(limit));
            // A reflow is the emulator's own opinion, and a client running a
            // different emulator would reach a different one.
            lock(&self.stream).bump();
        }

        lock(&self.master)
            .resize(size.pty_size())
            .map_err(|e| pty_error(&self.cwd, format!("could not resize the pty: {e}")))?;

        // A reflow changes every line. Nothing arrived from the pty to mark the
        // grid dirty, so the repaint has to be asked for here.
        self.frames.send_modify(|n| *n = n.wrapping_add(1));
        Ok(())
    }

    /// Sends bytes to the program, as though they had been typed.
    ///
    /// Refuses rather than blocks when the queue is full. The queue only fills
    /// when the program has stopped reading its input, and blocking a UI thread
    /// on a program that is not listening is exactly the wedge invariant 4
    /// exists to forbid.
    pub fn send_input(&self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        match self.input.try_send(bytes.into()) {
            Ok(()) => Ok(()),
            // `Conflict` in its stated sense: nothing failed, ket is declining.
            Err(TrySendError::Full(_)) => Err(KetError::Conflict(
                "terminal input is backed up: the program is not reading it".to_owned(),
            )),
            Err(TrySendError::Disconnected(_)) => {
                Err(KetError::Conflict("terminal has closed".to_owned()))
            }
        }
    }

    /// Ends the terminal, whatever the program inside it thinks about that.
    ///
    /// Returns immediately and never fails. Two halves, and both are needed:
    ///
    /// - ket's side stops regardless. The parser is told to exit, so the pane
    ///   stops consuming output even if the program survives everything below.
    /// - the program is killed as a *group*. `ChildKiller` sends `SIGHUP`,
    ///   which any program may trap and which reaches only the direct child —
    ///   so a pane that started a background job keeps the pty open through it
    ///   and never reaches EOF. `SIGKILL` to the session leader's process group
    ///   is the only form that a pane someone is closing can rely on, and with
    ///   `unsafe` denied in this workspace the way to send it is the `kill`
    ///   binary. The same escape hatch `ket-core::git` already uses.
    ///
    /// There is no grace period. A terminal pane is not where a program's
    /// What has the terminal's foreground right now.
    ///
    /// `None` when the pty cannot answer — it has gone away, or the platform
    /// does not implement the query. `Foreground::Shell` means the program ket
    /// spawned is itself in the foreground, which is a shell sitting at a
    /// prompt: nothing is running. Anything else is whatever a person started.
    ///
    /// This is how ket sees an agent it did not launch. Someone typing
    /// `claude` into a pane produces no session record and no event; the only
    /// evidence is that the terminal's foreground process group is no longer
    /// the shell. See [`crate::activity`].
    pub fn foreground(&self) -> Option<Foreground> {
        let leader = lock(&self.master).process_group_leader()?;

        // The shell ket spawned is its own group leader, so this is the pty
        // saying "nothing else is running in here".
        if self.pid.is_some_and(|pid| pid as i32 == leader) {
            return Some(Foreground::Shell);
        }

        let mut cached = lock(&self.foreground);
        if let Some((pid, name)) = cached.as_ref()
            && *pid == leader
        {
            return Some(Foreground::Running(name.clone()));
        }

        let name = command_name(leader)?;
        *cached = Some((leader, name.clone()));
        Some(Foreground::Running(name))
    }

    /// cleanup belongs, and a pane that will not close is the failure this
    /// guards against.
    pub fn kill(&self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.output.try_send(Pulse::Wake);
        let _ = lock(&self.killer).kill();
        if let Some(pid) = self.pid {
            kill_group(pid);
        }
    }
}

impl Drop for LocalTerminal {
    /// Kills the program and waits for the parser, and for nothing else.
    ///
    /// The parser is joinable because every path through it terminates on the
    /// shutdown flag. The reader is not: it can be parked in `read(2)` on a pty
    /// some grandchild is still holding open, and a join there would hang the
    /// window on a process ket does not know about. It is left to end on its
    /// own, which it does as soon as that read returns.
    fn drop(&mut self) {
        self.kill();
        if let Some(parser) = lock(&self.parser).take()
            && parser.join().is_err()
        {
            tracing::warn!("terminal parser thread panicked");
        }
    }
}

impl std::fmt::Debug for LocalTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTerminal")
            .field("cwd", &self.cwd)
            .field("pid", &self.pid)
            .field("size", &self.size())
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

/// A terminal a window draws: run in this process, or by the ket host.
///
/// The same API either way, so the shell never has to know which. Which one
/// [`Terminal::open`] returns is decided by `crate::host::enabled`. Hosted mode
/// does not silently fall back: doing so would make a terminal look durable
/// while tying its lifetime to the window again.
pub struct Terminal {
    inner: Inner,
}

enum Inner {
    // Boxed: a local terminal is most of three hundred bytes, a hosted one
    // is two pointers.
    Local(Box<LocalTerminal>),
    Remote(remote::RemoteTerminal),
}

/// Calls the same method on whichever kind of terminal this is.
macro_rules! either {
    ($self:ident, $terminal:ident => $call:expr) => {
        match &$self.inner {
            Inner::Local($terminal) => $call,
            Inner::Remote($terminal) => $call,
        }
    };
}

impl Terminal {
    /// Starts `spec`'s program — in the host by default, or in this process
    /// only when hosting was explicitly disabled.
    pub fn open(spec: &TerminalSpec) -> Result<Self> {
        if crate::host::enabled() {
            return remote::RemoteTerminal::open(spec).map(|remote| Self {
                inner: Inner::Remote(remote),
            });
        }
        LocalTerminal::open(spec).map(|local| Self {
            inner: Inner::Local(Box::new(local)),
        })
    }

    /// Whether the ket host runs this terminal, so that it outlives the window.
    pub fn is_hosted(&self) -> bool {
        matches!(self.inner, Inner::Remote(_))
    }

    /// Whether this is a terminal that was already running when it was
    /// opened — see [`TerminalSpec::adopt`]. Always `false` in-process.
    pub fn adopted(&self) -> bool {
        match &self.inner {
            Inner::Local(_) => false,
            Inner::Remote(remote) => remote.adopted(),
        }
    }

    /// See [`LocalTerminal::with_term`].
    pub fn with_term<R>(&self, f: impl FnOnce(&mut Term<Notifier>) -> R) -> R {
        either!(self, t => t.with_term(f))
    }

    /// See [`LocalTerminal::clear`].
    pub fn clear(&self) {
        either!(self, t => t.clear());
    }

    /// See [`LocalTerminal::frames`].
    pub fn frames(&self) -> watch::Receiver<u64> {
        either!(self, t => t.frames())
    }

    /// See [`LocalTerminal::frame`].
    pub fn frame(&self) -> u64 {
        either!(self, t => t.frame())
    }

    /// See [`LocalTerminal::size`].
    pub fn size(&self) -> TerminalSize {
        either!(self, t => t.size())
    }

    /// See [`LocalTerminal::pty_size`].
    pub fn pty_size(&self) -> Result<TerminalSize> {
        either!(self, t => t.pty_size())
    }

    /// See [`LocalTerminal::scrollback_limit`].
    pub fn scrollback_limit(&self) -> usize {
        either!(self, t => t.scrollback_limit())
    }

    /// See [`LocalTerminal::title`].
    pub fn title(&self) -> Option<String> {
        either!(self, t => t.title())
    }

    /// See [`LocalTerminal::is_closed`].
    pub fn is_closed(&self) -> bool {
        either!(self, t => t.is_closed())
    }

    /// See [`LocalTerminal::exit_status`].
    pub fn exit_status(&self) -> Option<ExitStatus> {
        either!(self, t => t.exit_status())
    }

    /// See [`LocalTerminal::resize`].
    pub fn resize(&self, size: TerminalSize) -> Result<()> {
        either!(self, t => t.resize(size))
    }

    /// See [`LocalTerminal::send_input`].
    pub fn send_input(&self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        either!(self, t => t.send_input(bytes))
    }

    /// See [`LocalTerminal::foreground`].
    pub fn foreground(&self) -> Option<Foreground> {
        either!(self, t => t.foreground())
    }

    /// Ends the terminal. In the host too: this is a pane being closed, not a
    /// window going away — dropping a hosted terminal only lets go of it.
    pub fn kill(&self) {
        either!(self, t => t.kill());
    }
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Inner::Local(local) => local.fmt(f),
            Inner::Remote(_) => f
                .debug_struct("Terminal")
                .field("hosted", &true)
                .field("size", &self.size())
                .field("closed", &self.is_closed())
                .finish_non_exhaustive(),
        }
    }
}

/// Reads the pty until EOF, then reaps the program.
///
/// Runs on its own thread because `portable-pty` is a blocking API. Sends on a
/// bounded channel, so a parser that falls behind stops this thread rather than
/// letting it buffer the world.
fn read_pty(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    out: &SyncSender<Pulse>,
    status: &Mutex<Option<ExitStatus>>,
) {
    let mut buffer = vec![0u8; READ_BUFFER];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                // Blocking on purpose. This is the backpressure.
                if out.send(Pulse::Bytes(buffer[..n].to_vec())).is_err() {
                    // Nobody is parsing any more, but the child is still this
                    // thread's to reap: falling through rather than returning is
                    // the difference between a closed pane and a zombie.
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!(%e, "pty read ended");
                break;
            }
        }
    }

    // Announce the close before reaping. A program can close its output and keep
    // running, and a pane that waits for an exit status before admitting the
    // stream is over would sit there looking alive.
    let _ = out.send(Pulse::Eof);

    if let Ok(exited) = child.wait() {
        *lock(status) = Some(exited);
    }
}

/// Writes queued bytes to the pty.
///
/// Its own thread for one reason: a pty's input buffer is small and a program
/// that has stopped reading fills it, at which point this write blocks. On the
/// caller's thread that is a frozen window; here it is a stalled queue that
/// [`Terminal::send_input`] reports instead of joining.
fn write_pty(mut writer: Box<dyn Write + Send>, input: &Receiver<Vec<u8>>) {
    while let Ok(bytes) = input.recv() {
        if writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .is_err()
        {
            // The pty is gone. So is any point in queueing more.
            break;
        }
    }
}

/// Ends a synchronized update.
const END_SYNC: &[u8] = b"\x1b[?2026l";

/// What the parser thread shares with the [`LocalTerminal`] that started it.
struct Shared<'a> {
    term: &'a FairMutex<Term<Notifier>>,
    stream: &'a Mutex<Stream>,
    frames: &'a watch::Sender<u64>,
    limit: &'a AtomicUsize,
    closed: &'a AtomicBool,
    shutdown: &'a AtomicBool,
}

/// Feeds pty output into the grid and paces the repaints it causes.
///
/// The whole of the coalescing: bytes are applied as fast as they arrive, and
/// the frame counter moves at most once per [`FRAME_INTERVAL`]. Ten thousand
/// lines a second becomes sixty repaints a second, not ten thousand.
fn parse_pty(output: &Receiver<Pulse>, shared: &Shared<'_>) {
    let Shared {
        term,
        stream,
        frames,
        limit,
        closed,
        shutdown,
    } = *shared;
    let mut parser: Processor = Processor::new();
    let mut clock = FrameClock::new(FRAME_INTERVAL, Instant::now());
    let mut trim = ScrollbackTrim::new(limit.load(Ordering::Relaxed));

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        // A synchronized update is the program promising to finish a frame
        // within 150ms. One that never does — it crashed, or was killed
        // mid-frame — would otherwise hold every byte after it back until two
        // megabytes had piled up, and the pane would look frozen. So the wait
        // is bounded by the update's deadline too, and the update is ended
        // when it passes, which is what alacritty itself does.
        let now = Instant::now();
        let sync_left = parser
            .sync_timeout()
            .sync_timeout()
            .map(|deadline| deadline.saturating_duration_since(now));
        let wait = match (clock.wait(now), sync_left) {
            (Some(frame), Some(sync)) => Some(frame.min(sync)),
            (frame, sync) => frame.or(sync),
        };
        let pulse = match wait {
            Some(budget) => match output.recv_timeout(budget) {
                Ok(pulse) => Some(pulse),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match output.recv() {
                Ok(pulse) => Some(pulse),
                Err(_) => break,
            },
        };

        match pulse {
            Some(Pulse::Bytes(bytes)) => {
                let mut term = term.lock();
                parser.advance(&mut *term, &bytes);
                // Still under the grid lock: a checkpoint must never see the
                // grid after these bytes and the stream before them.
                let held = parser
                    .sync_timeout()
                    .pending_timeout()
                    .then(|| parser.sync_bytes_count());
                lock(stream).record(&bytes, held);
                drop(term);
                clock.damaged();
            }
            Some(Pulse::Eof) => break,
            Some(Pulse::Wake) | None => {}
        }

        if parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            // Ended by writing the end of the update into the stream, rather
            // than by `stop_sync`: a client replaying this output holds the
            // same bytes back, and has to be told to let go of them too.
            let mut term = term.lock();
            parser.advance(&mut *term, END_SYNC);
            let held = parser
                .sync_timeout()
                .pending_timeout()
                .then(|| parser.sync_bytes_count());
            lock(stream).record(END_SYNC, held);
            drop(term);
            clock.damaged();
        }

        if clock.due(Instant::now()) {
            trim.settle(term, limit.load(Ordering::Relaxed));
            frames.send_modify(|n| *n = n.wrapping_add(1));
        }
    }

    // One last frame, unconditionally, and `closed` set before it. Whatever
    // arrived inside the final incomplete interval is still worth drawing, and
    // anything waiting on [`Terminal::frames`] has to be woken to notice the
    // terminal closed at all — pacing has nothing left to skip past.
    closed.store(true, Ordering::Release);
    trim.settle(term, limit.load(Ordering::Relaxed));
    frames.send_modify(|n| *n = n.wrapping_add(1));
}

/// Gives back the slack alacritty allocates while the scrollback is filling.
///
/// The grid grows its backing buffer a thousand rows at a time, which is the
/// right trade for a terminal with an unbounded history and the wrong one for a
/// terminal with a 4 MB budget: at 120 columns that slack is most of the budget
/// again. Once the history has reached its cap the buffer will never grow again
/// — alacritty stops asking, and the ring simply rotates — so the slack can be
/// handed back exactly once and never re-earned.
///
/// Once, and not per frame: reclaiming it walks the ring, and doing that at
/// 60Hz would cost more than it saves.
#[derive(Debug)]
struct ScrollbackTrim {
    /// The cap this state was decided against.
    limit: usize,
    /// Whether the slack has already been reclaimed at that cap.
    done: bool,
}

impl ScrollbackTrim {
    /// Starts untrimmed at `limit`.
    fn new(limit: usize) -> Self {
        Self { limit, done: false }
    }

    /// Reclaims the slack the first time the history reaches `limit`.
    fn settle(&mut self, term: &FairMutex<Term<Notifier>>, limit: usize) {
        if limit != self.limit {
            // A resize re-sized every row and re-earned the slack.
            self.limit = limit;
            self.done = false;
        }
        if self.done {
            return;
        }

        let mut term = term.lock();
        if term.grid().history_size() >= limit {
            term.grid_mut().truncate();
            self.done = true;
        }
    }
}

/// Kills a process group with `SIGKILL`.
///
/// `pid` is the child's, which `portable-pty` also made a session leader, so its
/// process group id is the same number and `-pid` reaches everything it started.
/// Both forms are passed: if `setsid` ever failed, the group does not exist and
/// only the direct target is left, which is still better than nothing.
///
/// Failures are ignored on purpose. Every one of them means the same thing —
/// the process is already gone — and there is no second thing to try.
#[cfg(unix)]
fn kill_group(pid: u32) {
    use std::process::{Command, Stdio};

    for program in ["/bin/kill", "kill"] {
        let sent = Command::new(program)
            .arg("-KILL")
            .arg(format!("-{pid}"))
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if sent.is_ok() {
            return;
        }
    }
    tracing::debug!(pid, "no kill binary; the pty may outlive its pane");
}

/// Nothing to escalate to off unix; `ChildKiller` is the whole story there.
#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// Spawns a named thread, naming the terminal if it cannot.
///
/// Named because three of these exist per pane, and "which of the sixty threads
/// is spinning" is a question a profiler should be able to answer.
fn spawn_thread(
    name: &str,
    cwd: &Path,
    body: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map_err(|e| pty_error(cwd, format!("could not start the {name} thread: {e}")))
}

/// Builds a [`KetError::Io`] naming the directory the terminal belongs to.
///
/// `portable-pty` reports through `anyhow`, which the library layer does not
/// carry, and the path is the part that makes the message useful: with several
/// worktrees open, "could not open a pty" on its own names nothing.
fn pty_error(cwd: &Path, why: String) -> KetError {
    KetError::io(cwd, std::io::Error::other(why))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_terminal_can_be_shared_across_threads() {
        // The renderer holds one and the rest of the app pokes it; if this ever
        // stops holding, every caller learns about it at once.
        fn shareable<T: Send + Sync>() {}
        shareable::<Terminal>();
    }

    #[test]
    fn the_first_change_after_a_quiet_period_paints_immediately() {
        // Otherwise the first character of a prompt waits a frame for no reason,
        // which reads as lag on exactly the keystroke people notice.
        let t0 = Instant::now();
        let mut clock = FrameClock::new(FRAME_INTERVAL, t0);

        clock.damaged();
        assert!(clock.due(t0));
    }

    #[test]
    fn an_unchanged_grid_never_asks_for_a_frame() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(FRAME_INTERVAL, t0);

        assert!(!clock.due(t0));
        assert!(!clock.due(t0 + ms(1000)));
        assert_eq!(clock.wait(t0), None);
    }

    #[test]
    fn a_burst_inside_one_interval_becomes_one_frame() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(FRAME_INTERVAL, t0);

        clock.damaged();
        assert!(clock.due(t0));

        for tick in 1..16 {
            clock.damaged();
            assert!(!clock.due(t0 + ms(tick)), "second frame at {tick}ms");
        }

        assert!(clock.due(t0 + ms(16)));
    }

    #[test]
    fn ten_thousand_changes_a_second_becomes_sixty_frames_a_second() {
        // The claim the whole design rests on, stated as arithmetic: one second
        // of a build's output at one change per 100µs.
        let t0 = Instant::now();
        let mut clock = FrameClock::new(FRAME_INTERVAL, t0);
        let mut frames = 0;

        for step in 0..10_000u64 {
            let now = t0 + Duration::from_micros(step * 100);
            clock.damaged();
            if clock.due(now) {
                frames += 1;
            }
        }

        assert!((60..=64).contains(&frames), "{frames} frames in a second");
    }

    #[test]
    fn a_pending_frame_bounds_how_long_the_parser_may_block() {
        let t0 = Instant::now();
        let mut clock = FrameClock::new(FRAME_INTERVAL, t0);
        clock.damaged();
        assert!(clock.due(t0));

        clock.damaged();
        assert_eq!(clock.wait(t0 + ms(6)), Some(ms(10)));
        // Past due, so the parser must not block at all.
        assert_eq!(clock.wait(t0 + ms(20)), Some(Duration::ZERO));
    }

    #[test]
    fn a_frame_is_not_published_twice_for_the_same_damage() {
        // The counter is what a renderer wakes on. Publishing twice for one
        // change is a repaint of a screen that did not move.
        let t0 = Instant::now();
        let mut clock = FrameClock::new(FRAME_INTERVAL, t0);

        clock.damaged();
        assert!(clock.due(t0));
        assert!(!clock.due(t0 + ms(500)), "the same damage paid twice");
    }

    #[test]
    fn scrollback_obeys_whichever_cap_binds_first() {
        // At any width worth using, the byte cap is the one that binds; the line
        // cap only takes over when a row is cheap enough that 10k of them fit.
        let wide = TerminalSize::new(200, 50);
        let narrow = TerminalSize::new(MIN_COLS, 1);

        assert!(scrollback_limit(wide) < MAX_SCROLLBACK_LINES);
        assert!(
            scrollback_limit(wide) * row_bytes(wide.columns()) <= MAX_SCROLLBACK_BYTES,
            "the byte cap was exceeded"
        );
        assert_eq!(scrollback_limit(narrow), MAX_SCROLLBACK_LINES);
    }

    #[test]
    fn a_screen_too_large_for_the_budget_keeps_no_scrollback_rather_than_lying() {
        // A pane that cannot afford one line of history must report none, not a
        // negative number wrapped into a very large one.
        assert_eq!(scrollback_limit(TerminalSize::new(u16::MAX, u16::MAX)), 0);
    }

    #[test]
    fn a_wider_screen_buys_fewer_lines_of_history() {
        let narrow = scrollback_limit(TerminalSize::new(80, 24));
        let wide = scrollback_limit(TerminalSize::new(400, 24));
        assert!(wide < narrow, "{wide} >= {narrow}");
    }

    #[test]
    fn a_collapsed_pane_is_clamped_to_a_grid_that_exists() {
        // `columns() - 1` on a zero-width grid panics inside alacritty, and a
        // pane dragged shut is an ordinary thing for a person to do.
        let size = TerminalSize::new(0, 0);
        assert_eq!(size.columns(), usize::from(MIN_COLS));
        assert_eq!(size.screen_lines(), usize::from(MIN_ROWS));
    }

    #[test]
    fn a_size_survives_the_round_trip_through_one_word() {
        for size in [
            TerminalSize::new(2, 1),
            TerminalSize::new(120, 40),
            TerminalSize::new(u16::MAX, u16::MAX),
        ] {
            assert_eq!(TerminalSize::unpack(size.pack()), size);
        }
    }
}
