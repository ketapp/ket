//! Moving the window from the chrome ket chooses, rather than from wherever
//! the platform decides.
//!
//! macOS drags a window built with a full-size content view — which is what a
//! transparent title bar means — from the top of that view, whatever is drawn
//! there. ket draws its tab strip there, and a tab inside that band could not
//! be picked up: AppKit began moving the window before gpui's own drag
//! threshold was reached, so the window came along instead of the tab.
//!
//! gpui has the concept that would have exempted the strip.
//! [`gpui::WindowControlArea`] marks the regions that should move a window, and
//! its Windows and Linux backends honour it — but `MacWindow`'s
//! `on_hit_test_window_control` is an empty function body, so on macOS the
//! annotation reaches nothing and the band cannot be given up.
//!
//! So the window is opened with `is_movable: false`, refusing AppKit the whole
//! of it, and the chrome that *should* move it asks here on the way down: the
//! sidebar header, the panel header, and the empty run of a tab strip past the
//! last tab. Those are the same regions an Electron application marks with
//! `-webkit-app-region: drag`, and this is the call that framework makes for
//! them.
//!
//! `performWindowDragWithEvent:` rather than a `setFrameOrigin:` loop of our
//! own: it is a *real* window drag, so the window still snaps to screen edges,
//! still moves between Spaces at the screen edge, and still shows the drop
//! feedback macOS gives a window being placed. Following the pointer by hand
//! loses all three and gains nothing.

/// Starts a window drag from the element under the pointer.
///
/// Safe to call from any mouse-down handler: without a current event, or off
/// macOS, it does nothing rather than failing.
#[cfg(target_os = "macos")]
pub(crate) fn drag_window(window: &gpui::Window) {
    use objc2_app_kit::{NSApplication, NSView};
    use objc2_foundation::MainThreadMarker;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Named rather than called as a method: gpui has an inherent
    // `window_handle` of its own, returning its own handle type, and an
    // inherent method wins over a trait's.
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // gpui runs its window on the main thread, and a mouse-down handler is
    // called from that window's event dispatch — but this asks rather than
    // assumes, because every AppKit call below needs it to be true.
    let Some(main) = MainThreadMarker::new() else {
        return;
    };

    // SAFETY: `ns_view` comes from gpui's own window handle, which
    // `raw-window-handle` documents as a valid, non-null pointer to the
    // `NSView` backing this window, and which is already relied on this way to
    // parent browser tabs — see `crate::browser`. The borrow does not outlive
    // this function, and nothing here retains it. Every AppKit call made
    // through it is a safe binding from `objc2-app-kit`; this cast is the only
    // thing that cannot be expressed safely, because turning a pointer into a
    // reference never can be.
    #[allow(unsafe_code)]
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };

    let Some(ns_window) = view.window() else {
        return;
    };
    // The event still being dispatched — the mouse-down that got us here. A
    // window drag has to be started from one, since AppKit tracks the pointer
    // from where that event landed.
    let Some(event) = NSApplication::sharedApplication(main).currentEvent() else {
        return;
    };

    // Movable only for as long as the drag lasts. The window is deliberately
    // immovable the rest of the time — that is what stops AppKit claiming the
    // tab strip — and `performWindowDragWithEvent:` runs its own event loop,
    // returning when the drag ends, so this closes again on the far side of it.
    ns_window.setMovable(true);
    ns_window.performWindowDragWithEvent(&event);
    ns_window.setMovable(false);
}

/// Starts a window drag from the element under the pointer.
///
/// Nothing to do away from macOS: `is_movable` is left alone there, and
/// [`gpui::WindowControlArea`] is honoured by those backends, so the regions
/// that should move the window already do.
#[cfg(not(target_os = "macos"))]
pub(crate) fn drag_window(_: &gpui::Window) {}
