//! A terminal the host runs, as a window sees it.
//!
//! The window keeps its own grid — an `alacritty_terminal` `Term`, the same
//! type a [`LocalTerminal`](super::LocalTerminal) has — and rebuilds it from
//! the host's checkpoints and output, so the renderer reads it exactly the way
//! it reads a local one. Commands go to the host: keystrokes, resizes, kills.
//!
//! The grid here never answers the program. Replies to the program's queries —
//! cursor position, device status, size — are the host's grid's job, and this
//! one's would arrive as a second, stale answer. Its notifier's reply channel
//! is closed before it is ever used.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::Processor;
use portable_pty::ExitStatus;
use tokio::sync::watch;

use super::{Notifier, TerminalSize, TerminalSpec, lock, term_config};
use crate::activity::Foreground;
use crate::host::client::{Connection, connection};
use crate::host::wire::Request;
use crate::{KetError, Result};

/// A host terminal's state in this process, shared with the thread that
/// applies what the host sends.
pub(crate) struct Shared {
    id: u64,
    term: Arc<FairMutex<Term<Notifier>>>,
    parser: Mutex<Processor>,
    frames: watch::Sender<u64>,
    /// The size last asked for, which is what the window believes.
    size: Arc<AtomicU32>,
    scrollback: std::sync::atomic::AtomicUsize,
    title: Arc<Mutex<Option<String>>>,
    closed: AtomicBool,
    status: Mutex<Option<ExitStatus>>,
    foreground: Mutex<Option<Foreground>>,
    /// Whether this is a terminal that was already running when opened.
    adopted: bool,
}

impl Shared {
    pub(crate) fn new(id: u64, spec: &TerminalSpec, adopted: bool) -> Arc<Self> {
        let size = TerminalSize::new(spec.size.cols, spec.size.rows);
        let shared_size = Arc::new(AtomicU32::new(size.pack()));
        let title = Arc::new(Mutex::new(None));
        // Closed at once: see the module docs.
        let (input, _) = sync_channel(1);
        let notifier = Notifier {
            input,
            size: shared_size.clone(),
            title: title.clone(),
        };
        let limit = super::scrollback_limit(size);
        Arc::new(Self {
            id,
            term: Arc::new(FairMutex::new(Term::new(
                term_config(limit),
                &size,
                notifier,
            ))),
            parser: Mutex::new(Processor::new()),
            frames: watch::channel(0u64).0,
            size: shared_size,
            scrollback: std::sync::atomic::AtomicUsize::new(limit),
            title,
            closed: AtomicBool::new(false),
            status: Mutex::new(None),
            foreground: Mutex::new(None),
            adopted,
        })
    }

    fn repaint(&self) {
        self.frames.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Replaces the screen with a checkpoint.
    pub(crate) fn checkpoint(&self, size: TerminalSize, scrollback: usize, ansi: &[u8]) {
        {
            let mut term = self.term.lock();
            if term.columns() != usize::from(size.cols)
                || term.screen_lines() != usize::from(size.rows)
            {
                term.resize(size);
            }
            term.set_options(term_config(scrollback));
            // A fresh parser too: whatever half-sequence the old one held
            // belongs to output the checkpoint already accounts for.
            let mut parser = lock(&self.parser);
            *parser = Processor::new();
            parser.advance(&mut *term, ansi);
        }
        self.scrollback
            .store(scrollback, std::sync::atomic::Ordering::Relaxed);
        self.repaint();
    }

    /// Applies output that followed the last checkpoint.
    pub(crate) fn output(&self, bytes: &[u8]) {
        {
            let mut term = self.term.lock();
            lock(&self.parser).advance(&mut *term, bytes);
        }
        self.repaint();
    }

    pub(crate) fn closed(&self, status: Option<ExitStatus>) {
        *lock(&self.status) = status;
        self.closed.store(true, Ordering::Release);
        self.repaint();
    }

    /// The host went away with this terminal in it.
    pub(crate) fn lost(&self) {
        self.closed.store(true, Ordering::Release);
        self.repaint();
    }

    pub(crate) fn foreground(&self, foreground: Option<Foreground>) {
        *lock(&self.foreground) = foreground;
    }
}

/// A terminal the host runs. See the module docs.
pub(crate) struct RemoteTerminal {
    shared: Arc<Shared>,
    conn: Arc<Connection>,
}

impl RemoteTerminal {
    /// Asks the host to start `spec`, or to hand over the running terminal it
    /// names — see [`TerminalSpec::adopt`].
    pub(crate) fn open(spec: &TerminalSpec) -> Result<Self> {
        let conn = connection()?;
        let shared = conn.open(spec)?;
        Ok(Self { shared, conn })
    }

    pub(crate) fn adopted(&self) -> bool {
        self.shared.adopted
    }

    pub(crate) fn with_term<R>(&self, f: impl FnOnce(&mut Term<Notifier>) -> R) -> R {
        let mut term = self.shared.term.lock();
        f(&mut term)
    }

    pub(crate) fn clear(&self) {
        let _ = self.conn.request(&Request::Clear { id: self.shared.id });
    }

    pub(crate) fn frames(&self) -> watch::Receiver<u64> {
        self.shared.frames.subscribe()
    }

    pub(crate) fn frame(&self) -> u64 {
        *self.shared.frames.borrow()
    }

    pub(crate) fn size(&self) -> TerminalSize {
        TerminalSize::unpack(self.shared.size.load(Ordering::Relaxed))
    }

    pub(crate) fn pty_size(&self) -> Result<TerminalSize> {
        Ok(self.size())
    }

    pub(crate) fn scrollback_limit(&self) -> usize {
        self.shared.scrollback.load(Ordering::Relaxed)
    }

    pub(crate) fn title(&self) -> Option<String> {
        lock(&self.shared.title).clone()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    pub(crate) fn exit_status(&self) -> Option<ExitStatus> {
        lock(&self.shared.status).clone()
    }

    /// Asks the host to resize. The screen here changes when the checkpoint
    /// the resize causes arrives: a reflow is the host's to do, because a
    /// client reflowing on its own could reach a different answer.
    pub(crate) fn resize(&self, size: TerminalSize) -> Result<()> {
        let size = TerminalSize::new(size.cols, size.rows);
        if self.shared.size.swap(size.pack(), Ordering::Relaxed) == size.pack() {
            return Ok(());
        }
        self.conn.request(&Request::Resize {
            id: self.shared.id,
            size,
        })
    }

    pub(crate) fn send_input(&self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        if self.is_closed() {
            return Err(KetError::Conflict("terminal has closed".to_owned()));
        }
        self.conn.input(self.shared.id, &bytes.into())
    }

    pub(crate) fn foreground(&self) -> Option<Foreground> {
        lock(&self.shared.foreground).clone()
    }

    pub(crate) fn kill(&self) {
        let _ = self.conn.request(&Request::Kill { id: self.shared.id });
    }
}

impl Drop for RemoteTerminal {
    /// Lets go of the terminal without ending it. That is the point: the
    /// window is going, the program is not. Closing a pane kills it first.
    fn drop(&mut self) {
        let _ = self.conn.request(&Request::Detach { id: self.shared.id });
        self.conn.forget(self.shared.id);
    }
}
