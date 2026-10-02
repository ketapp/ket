//! Opt-in frame timing, for telling scroll jank from a slow build.
//!
//! Set `KET_FRAME_TRACE=1` and the shell prints one line per redraw while the
//! window is redrawing continuously — a scroll, a caret blink, a terminal
//! writing — with the interval since the last redraw and where the time in the
//! *previous* frame went.
//!
//! Previous rather than current because the expensive half of a frame happens
//! after the view builds: `Render` only assembles the element tree, and the
//! terminal grid is shaped in prepaint and paint, long after `Shell::render`
//! has returned. Timing the build alone would clear the terminal of a cost it
//! is actually paying. So a frame's spans are flushed at the start of the next
//! one, when all of them are in.
//!
//! Idle frames are skipped: a redraw 4 seconds after the last one says nothing
//! about jank, and printing it buries the burst that does.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Longest gap between two redraws that still counts as continuous.
///
/// A scroll at 60Hz arrives every 16ms and at 120Hz every 8; a gap longer than
/// this is the window waking up, not a frame that ran late.
const CONTINUOUS: Duration = Duration::from_millis(250);

/// Whether the trace was asked for, read once.
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KET_FRAME_TRACE").is_some_and(|v| v != "0"))
}

/// Whether `KET_STILL` asks the looping animations to hold their first frame.
///
/// A moving rail books a redraw every tick for as long as it is on screen, so
/// a single running row keeps the whole window redrawing. This is the switch
/// for telling that cost from everything else: with it set, an idle window
/// should stop redrawing. Read by [`crate::motion::phase`].
pub(crate) fn still() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KET_STILL").is_some_and(|v| v != "0"))
}

/// What the current frame has spent, and when the last one began.
#[derive(Default)]
struct State {
    /// Section name to time spent in it this frame, summed over every span
    /// that named it — a section entered once per pane is one line, not four.
    spans: Vec<(&'static str, Duration)>,
    /// When the last frame started, for the interval.
    last: Option<Instant>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Closes the previous frame and opens a new one.
///
/// Called at the top of [`crate::Shell::render`], which is the first thing a
/// redraw does.
pub(crate) fn frame(now: Instant) {
    if !enabled() {
        return;
    }
    STATE.with_borrow_mut(|state| {
        if let Some(last) = state.last.replace(now)
            && now - last <= CONTINUOUS
            && !state.spans.is_empty()
        {
            let mut line = format!("frame {:>6.2}ms", (now - last).as_secs_f64() * 1000.0);
            // Costliest first: the point of the line is which section to look
            // at, and that is rarely the one that happens to be listed first.
            state
                .spans
                .sort_by_key(|(_, spent)| std::cmp::Reverse(*spent));
            for (name, spent) in &state.spans {
                line.push_str(&format!("  {name} {:.2}", spent.as_secs_f64() * 1000.0));
            }
            eprintln!("{line}");
        }
        state.spans.clear();
    });
}

/// Times a section of the frame until the returned guard drops.
///
/// Cheap enough to leave in place when the trace is off: an untraced span is
/// an `Instant::now` that nothing reads.
pub(crate) fn span(name: &'static str) -> Span {
    Span {
        name,
        started: Instant::now(),
    }
}

/// The guard [`span`] returns. See it.
pub(crate) struct Span {
    name: &'static str,
    started: Instant,
}

impl Drop for Span {
    fn drop(&mut self) {
        if !enabled() {
            return;
        }
        let spent = self.started.elapsed();
        STATE.with_borrow_mut(|state| {
            if let Some(entry) = state.spans.iter_mut().find(|(name, _)| *name == self.name) {
                entry.1 += spent;
            } else {
                state.spans.push((self.name, spent));
            }
        });
    }
}

/// Longest a periodic-tick section may run on the UI thread before it is named.
const SLOW_TICK: Duration = Duration::from_millis(4);

/// Times a section of the 5s tick and prints it if it ran long.
///
/// The frame trace above skips idle frames, and a tick lands between them, so
/// its cost would never show. Prints only under `KET_FRAME_TRACE`, and only
/// past [`SLOW_TICK`]: the tick runs on the thread that echoes keystrokes.
pub(crate) fn tick(name: &'static str) -> TickSpan {
    TickSpan {
        name,
        started: Instant::now(),
    }
}

/// The guard [`tick`] returns. See it.
pub(crate) struct TickSpan {
    name: &'static str,
    started: Instant,
}

impl Drop for TickSpan {
    fn drop(&mut self) {
        if !enabled() {
            return;
        }
        let spent = self.started.elapsed();
        if spent >= SLOW_TICK {
            eprintln!(
                "tick  {:>6.2}ms  {}",
                spent.as_secs_f64() * 1000.0,
                self.name
            );
        }
    }
}
