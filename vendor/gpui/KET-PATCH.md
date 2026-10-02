# gpui 0.2.2, patched for ket

This is gpui 0.2.2 exactly as published on crates.io, with three changes
applied: frame pacing, movable traffic lights, and a text truncation fix.
`Cargo.toml` at the workspace root points `gpui` here through
`[patch.crates-io]`, and excludes this directory from the workspace so ket's
own lints do not apply to Zed's code. `ket.patch` is the whole change as a
unified diff against the published crate's `src/`, for re-applying on an
upgrade (`patch -p1 < ket.patch` from a fresh copy of the crate).

Every changed line is marked `ket patch` in a comment.

## Frame pacing

### Why

gpui drives a macOS window from a `CVDisplayLink` that fires on every display
refresh — 120 times a second on a ProMotion panel — for as long as the window
is visible, whether or not anything changed. Each tick wakes the display-link
thread and the main thread, which then finds nothing to do. That was most of
ket's idle CPU and a steady few percent under load.

gpui also re-presented the unchanged frame on every refresh for a full second
after *any* input, so every keystroke bought a second of full-rate GPU submits.

As of September 2026 Zed's own gpui has fixed the second (it sustains only
under display-rate input) but not the first, and has not published a release
since 0.2.2.

### What changed

- **Frames pause when the window is idle.** After `IDLE_FRAMES_BEFORE_PAUSE`
  consecutive frames with nothing to draw, present or run, the window calls
  `PlatformWindow::pause_frames`, which on macOS stops the display link
  (`src/window.rs`, `src/platform/mac/window.rs`).
- **Anything that needs a frame resumes them.** The window invalidator carries a
  waker from `PlatformWindow::frame_waker` and calls it whenever the window is
  marked dirty — a view notified, a refresh — and `on_next_frame` and input
  dispatch call it too. The macOS waker is one atomic read when frames are
  already running; if the window's lock is held on this thread it defers the
  restart to the main queue instead of deadlocking (`resume_frames`).
- **`DisplayLink::start`/`stop` are idempotent**, keeping the dispatch source's
  suspend count balanced now that they are called repeatedly
  (`src/platform/mac/display_link.rs`).
- **Re-presenting after input follows the input rate** (`InputRate` in
  `src/window.rs`): six or more scroll or drag events inside 100ms sustain it
  for a second. Only those count — keys, clicks and plain mouse movement do
  not. Counting every event let a burst of typing trip it, and each of the
  resulting presents blocked the main thread in `nextDrawable`, which was the
  lag typing into a busy terminal.

Other platforms get the trait's default methods: they never pause, and behave
exactly as upstream.

## Movable traffic lights

`Window::set_traffic_light_position` moves the macOS close, minimise and zoom
buttons after the window has opened, for a title bar whose height changes
(`src/window.rs`, `src/platform.rs`, `src/platform/mac/window.rs`). AppKit's
title-bar container is resized to hold the buttons where they are placed:
otherwise buttons drawn below a standard title bar sit outside their
superview's bounds and stop receiving clicks. Other platforms ignore the call.

## Text truncation

A truncated text element's layout cache now keys on the width it was cut to,
not just its wrap width (`src/elements/text.rs`). A flex item is measured more
than once per frame, and a layout cut for the first width was reused for the
second. Each measurement also cuts from a fresh copy of the element's runs:
`truncate_line` shortens the runs it is given, and reusing the shortened runs
could end a run inside a character, which panics.
