//! The clock behind the looping animations, and how often they redraw.
//!
//! `gpui` animates by asking for another frame the moment it draws one, and
//! a repeating animation never says it is done. So one travelling rail in the
//! sidebar redrew the whole window at the display's rate — 120 times a second
//! on a ProMotion panel — for as long as an agent was working, and an agent is
//! nearly always working. There is no partial redraw: every one of those frames
//! re-laid-out the tree and re-painted every terminal. A full core in a debug
//! build, spent on a 4px bar.
//!
//! So the looping animations do not use `with_animation`. They read where they
//! are in their cycle from the wall clock here, and this module asks for one
//! redraw a tick while anything read it. The tick is chosen for a crest that
//! still reads as travelling, not for the fastest the display can manage, and
//! slows further when the window is not the key one — the case, most of the
//! day, of agents running in the background. When nothing on screen is moving,
//! nothing is scheduled and the window goes quiet.
//!
//! The phase is a function of time rather than of the element, so every rail
//! agrees without an id, and a row that re-renders mid-cycle does not restart.
//!
//! A tick **notifies the shell**; it does not refresh the window. The two look
//! alike and are not: `refresh` sets gpui's `refreshing` flag, which switches
//! off view caching for that frame, so every terminal pane re-walked and
//! re-shaped its whole grid twenty times a second to move a 4px bar beside
//! it. A notify leaves the panes' cached frames standing, and only the shell's
//! own tree — the sidebar among it — is built again.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Context, WeakEntity, Window};

use crate::Shell;

/// How often a moving element is redrawn while the window is key.
///
/// Twenty a second. Each tick is a full-window redraw, so this is a direct
/// multiplier on what an animation costs; the rail's crest moves a thirtieth
/// of its trip per tick here, which still reads as travel rather than steps.
const TICK: Duration = Duration::from_millis(50);

/// How often when it is not.
///
/// Eight a second is visibly stepped up close, and a window that is not key
/// is not being looked at up close: it is beside a browser, or behind one.
/// What matters there is that the bar is seen to move at all.
const IDLE_TICK: Duration = Duration::from_millis(125);

/// The clock and what it has been asked for, on the window's thread.
#[derive(Default)]
struct Clock {
    /// A handle for spawning the tick, taken from the first frame.
    app: Option<AsyncApp>,
    /// The view the moving elements are drawn in, which is the one a tick
    /// notifies. Weak, so the clock never keeps a closed window alive.
    shell: Option<WeakEntity<Shell>>,
    /// When the first phase was read: the origin every cycle is measured from.
    started: Option<Instant>,
    /// When the frame being drawn began. Every phase read in one frame sees
    /// the same instant, so two rails in one row cannot land a tick apart.
    now: Option<Instant>,
    /// Whether the window was key when the frame began.
    active: bool,
    /// Whether a redraw is already on its way, so a frame with forty moving
    /// rows asks for one tick, not forty.
    scheduled: bool,
}

thread_local! {
    static CLOCK: RefCell<Clock> = RefCell::new(Clock::default());
}

/// Opens a frame: called at the top of [`crate::Shell::render`], before any
/// element reads its phase.
pub(crate) fn frame(now: Instant, window: &Window, cx: &Context<Shell>) {
    CLOCK.with_borrow_mut(|clock| {
        clock.now = Some(now);
        clock.active = window.is_window_active();
        if clock.app.is_none() {
            clock.app = Some(cx.to_async());
        }
        if clock.shell.is_none() {
            clock.shell = Some(cx.entity().downgrade());
        }
    });
}

/// Where a looping animation of `cycle` is right now, from 0 at the start of
/// a cycle to just under 1 at its end — and a request for another redraw a
/// tick from now, since something on screen has just said it is moving.
///
/// Under `KET_STILL` the answer is always the start of the cycle and nothing
/// is scheduled, which is how the cost of the animations is told from the
/// cost of everything else. See [`crate::frametrace::still`].
pub(crate) fn phase(cycle: Duration) -> f32 {
    if crate::frametrace::still() {
        return 0.0;
    }
    CLOCK.with_borrow_mut(|clock| {
        let now = clock.now.unwrap_or_else(Instant::now);
        let started = *clock.started.get_or_insert(now);
        schedule(clock);
        (now.duration_since(started).as_secs_f32() / cycle.as_secs_f32()).fract()
    })
}

/// Asks for one redraw a tick from now, unless one is already coming.
fn schedule(clock: &mut Clock) {
    if clock.scheduled {
        return;
    }
    // Before the first frame there is nothing to wake; the frame that installs
    // the handle is the one being drawn, and it will read the phase again.
    let (Some(app), Some(shell)) = (clock.app.clone(), clock.shell.clone()) else {
        return;
    };
    clock.scheduled = true;
    let tick = if clock.active { TICK } else { IDLE_TICK };
    app.spawn(async move |cx: &mut AsyncApp| {
        cx.background_executor().timer(tick).await;
        // Cleared before the redraw rather than after: the redraw is what
        // reads the phases, and each read must be free to book the next tick.
        CLOCK.with_borrow_mut(|clock| clock.scheduled = false);
        // Not `cx.refresh()` — see the module docs. The only failure is the
        // shell being gone, and then there is nothing left to draw.
        shell.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}
