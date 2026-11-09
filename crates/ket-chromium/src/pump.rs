//! CEF's message pump, driven from gpui's run loop.
//!
//! CEF has no multi-threaded message loop on macOS, and gpui owns
//! `[NSApp run]`, so CEF runs with `external_message_pump`: it says, from
//! whichever thread it is on, how soon it next wants
//! `cef::do_message_loop_work`, and this module makes that happen on the
//! main thread. A CoreFoundation timer does it, in the run loop's common
//! modes so pages keep working while a menu is open or a window is being
//! resized. It repeats every 1/30 s, so Chromium is never left idle longer
//! than that even when it asks for nothing (CEF's own sample pump keeps the
//! same ceiling), and is brought forward whenever Chromium asks for sooner.
//!
//! gpui's own tasks are not used for this: an earlier embedding that pumped
//! from them measured stalls of one to five seconds.
//!
//! `unsafe` is allowed here for three CoreFoundation calls with no safe
//! binding — creating the timer, reading the common-modes constant and
//! handing a block to the main run loop — and nothing else.

use std::cell::{Cell, RefCell};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopMode, CFRunLoopTimer,
    kCFRunLoopCommonModes,
};

use crate::{Error, failed};

/// The longest CEF waits for work, in seconds.
const MAX_IDLE: f64 = 1.0 / 30.0;

thread_local! {
    /// The timer, while CEF is running. Main thread only, like everything
    /// in this module but [`schedule`].
    static TIMER: RefCell<Option<CFRetained<CFRunLoopTimer>>> = const { RefCell::new(None) };
    /// Whether `do_message_loop_work` is on the stack: a nested run loop
    /// inside it (a menu, a modal) would otherwise fire the timer into it.
    static PUMPING: Cell<bool> = const { Cell::new(false) };
    /// Whether the timer fired while [`PUMPING`], so the skipped work is
    /// done as soon as the outer call returns.
    static SKIPPED: Cell<bool> = const { Cell::new(false) };
}

/// The run loop's common modes: the default mode and the event-tracking
/// one that menus and live resizing run in.
fn common_modes() -> Option<&'static CFRunLoopMode> {
    // SAFETY: a constant CoreFoundation defines once and never changes;
    // reading it is how it is meant to be used.
    unsafe { kCFRunLoopCommonModes }
}

/// Puts the pump on the main run loop. Call before `cef::initialize`, which
/// may already ask for work.
pub(crate) fn install(_mtm: MainThreadMarker) -> Result<(), Error> {
    let fired = RcBlock::new(|_timer: *mut CFRunLoopTimer| work());
    let first = CFAbsoluteTimeGetCurrent() + MAX_IDLE;
    // SAFETY: `None` is the default allocator, which CF accepts; the block
    // is a valid handler for this signature, and CF copies it, so it lives
    // as long as the timer does.
    let timer = unsafe { CFRunLoopTimer::with_handler(None, first, MAX_IDLE, 0, 0, Some(&fired)) }
        .ok_or_else(|| failed("could not make Chromium's message-pump timer"))?;
    let main = CFRunLoop::main().ok_or_else(|| failed("there is no main run loop"))?;
    main.add_timer(Some(&timer), common_modes());
    TIMER.set(Some(timer));
    Ok(())
}

/// Takes the pump off the run loop; [`work`] is then only done by hand.
pub(crate) fn uninstall() {
    if let Some(timer) = TIMER.take() {
        timer.invalidate();
    }
}

/// What CEF's `on_schedule_message_pump_work` hands on: do the work in
/// `delay_ms`, or at once when that is zero or less. Any thread.
///
/// Work wanted at once is done on the main run loop's next pass rather than
/// by the timer, and a later ask only ever brings the timer forward. CEF asks
/// for a later round all the time, so one timer reset by every ask let a
/// later one push back an urgent one: a keystroke or a scroll left waiting up
/// to 1/30 s.
pub(crate) fn schedule(delay_ms: i64) {
    if delay_ms <= 0 {
        on_main(work);
        return;
    }
    let delay = (delay_ms as f64 / 1000.0).min(MAX_IDLE);
    if MainThreadMarker::new().is_some() {
        bring_forward(delay);
    } else {
        on_main(move || bring_forward(delay));
    }
}

/// Runs `job` on the main run loop's next pass, in the common modes, from
/// any thread.
fn on_main(job: impl Fn() + 'static) {
    let Some(main) = CFRunLoop::main() else {
        return;
    };
    let job = RcBlock::new(job);
    // SAFETY: CFRunLoopPerformBlock exists to be called from another thread
    // for a run loop's own thread to run, and copies the block. The mode is
    // CF's own constant. The block touches only main-thread state, and runs
    // on the main thread.
    unsafe { main.perform_block(common_modes().map(|mode| mode.as_ref()), Some(&job)) };
    main.wake_up();
}

/// Brings the timer's next firing to `delay` seconds from now when that is
/// sooner than it would fire anyway, and never later. A next firing already
/// past is the one under way: the timer's own callout, where CF keeps a date
/// set during it.
fn bring_forward(delay: f64) {
    TIMER.with_borrow(|timer| {
        if let Some(timer) = timer {
            let now = CFAbsoluteTimeGetCurrent();
            let next = timer.next_fire_date();
            if next <= now || now + delay < next {
                timer.set_next_fire_date(now + delay);
            }
        }
    });
}

/// One round of CEF's work, unless one is already under way further up the
/// stack. Main thread.
pub(crate) fn work() {
    if PUMPING.get() {
        SKIPPED.set(true);
        return;
    }
    PUMPING.set(true);
    SKIPPED.set(false);
    autoreleasepool(|_| cef::do_message_loop_work());
    PUMPING.set(false);
    if SKIPPED.get() {
        on_main(work);
    }
}
