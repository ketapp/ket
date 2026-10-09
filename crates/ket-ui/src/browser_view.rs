//! A browser pane's page as a native child view, and the slot it sits in.
//!
//! Wry builds the `WKWebView` as a child of gpui's own view — the whole
//! window — and the obvious thing, done by `gpui-wry` before this module, is
//! to set the page's frame to the pane's every frame. That falls apart the
//! moment Web Inspector docks. WebKit docks the inspector *beside the page in
//! the page's superview*: it adds its own view there, watches the page's frame,
//! and whenever that frame changes it re-cuts both from the superview's
//! bounds — the page across the superview's full width, the inspector along
//! its bottom. With the window as the superview, the page jumped over the
//! sidebar and the inspector lay across ket's status bar; and with the frame
//! being set each frame, the two took turns, every frame, and the page
//! flickered between the pane and the window.
//!
//! So the page sits in a slot of its own — a plain `NSView` that is the pane's
//! size — and ket only ever moves the slot. The page fills the slot by
//! autoresizing, so nothing writes its frame; when the inspector docks, it
//! docks inside the slot and shares it with the page, the way it would share a
//! Safari tab. Hiding or parking the slot takes the inspector with it.
//!
//! While a ket menu is over the pane, the still it shows is of the page alone
//! (see [`crate::browser_snapshot`]), drawn across the whole pane — so with
//! the inspector docked it is stretched over the inspector's band until the
//! menu closes.

use std::cell::Cell;

use gpui::{Bounds, Pixels};
use objc2::MainThreadOnly as _;
use objc2::rc::Retained;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use wry::WebViewExtMacOS as _;

/// Where a hidden page waits, in points from the window's corner: far enough
/// down and left that no window is big enough to reach it — see
/// [`NativeBrowser::hide`].
const PARKED: f64 = -100_000.0;

/// A page and the slot it lives in, on the window's main thread.
pub(crate) struct NativeBrowser {
    raw: wry::WebView,
    /// The pane-sized view the page and, when docked, its inspector share.
    slot: Retained<NSView>,
    /// gpui's view: the slot's parent, and where the keyboard goes back to.
    root: Retained<NSView>,
    shown: Cell<bool>,
}

impl NativeBrowser {
    /// Moves a page Wry built as a child of `root` into a slot of its own
    /// there. The slot starts hidden; [`NativeBrowser::show`] brings it up.
    pub(crate) fn new(raw: wry::WebView, root: Retained<NSView>, main: MainThreadMarker) -> Self {
        let page = raw.webview();
        // Wry sized the page to the whole of `root`, so the slot starts at a
        // real size and the page's autoresizing never has to grow from zero.
        let slot = NSView::initWithFrame(NSView::alloc(main), page.frame());
        slot.setHidden(true);
        // Anchored at the top left like the page was, so a window resize
        // between frames moves it no more than it did; `place` sets its frame
        // with the pane's each frame.
        slot.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        root.addSubview(&slot);
        page.removeFromSuperview();
        slot.addSubview(&page);
        page.setFrame(slot.bounds());
        page.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // Wry built the page hidden. From here on the slot is what hides —
        // WebKit will not dock the inspector to a hidden page.
        let _ = raw.set_visible(true);
        let this = Self {
            raw,
            slot,
            root,
            shown: Cell::new(false),
        };
        // Wry may make a newly-created child first responder. Put the
        // keyboard back before anything of ket's asks for it.
        this.release_keyboard();
        this
    }

    /// Wry's handle on the page: loads, scripts, its address, a snapshot.
    pub(crate) fn raw(&self) -> &wry::WebView {
        &self.raw
    }

    /// Puts the page up. Its frame comes back with the next layout — see
    /// [`NativeBrowser::place`].
    pub(crate) fn show(&self) {
        self.slot.setHidden(false);
        self.shown.set(true);
    }

    /// Takes the page down, and the keyboard away from it.
    ///
    /// A hidden view still takes drops — AppKit finds drag destinations by
    /// frame, hidden or not — so a browser tab behind a terminal swallowed the
    /// files dropped on it. Moved off the window it has nothing to land on; at
    /// its own size, so coming back is a move rather than a resize — shrunk to
    /// nothing, WebKit redrew the page from white as a menu over it closed.
    pub(crate) fn hide(&self) {
        self.release_keyboard();
        self.slot.setHidden(true);
        self.slot.setFrameOrigin(NSPoint::new(PARKED, PARKED));
        self.shown.set(false);
    }

    /// Puts the slot where the pane laid it out, if that moved. Nothing while
    /// hidden: a parked slot stays parked until it is shown again.
    pub(crate) fn place(&self, bounds: Bounds<Pixels>) {
        if !self.shown.get() {
            return;
        }
        // gpui's coordinates start at the window's top left; a child's frame
        // in AppKit starts at its parent's bottom left. Whole points: WebKit
        // gets no fractional frames, and the comparison below is exact.
        let height = self.root.bounds().size.height;
        let x = f64::from(f32::from(bounds.origin.x)).round();
        let top = f64::from(f32::from(bounds.origin.y)).round();
        let width = f64::from(f32::from(bounds.size.width)).round().max(1.0);
        let h = f64::from(f32::from(bounds.size.height)).round().max(1.0);
        let frame = NSRect::new(NSPoint::new(x, height - top - h), NSSize::new(width, h));
        let current = self.slot.frame();
        if current.origin.x == frame.origin.x
            && current.origin.y == frame.origin.y
            && current.size.width == frame.size.width
            && current.size.height == frame.size.height
        {
            return;
        }
        self.slot.setFrame(frame);
    }

    /// Whether the page, or an inspector docked beside it, has the window's
    /// keyboard.
    pub(crate) fn has_keyboard(&self) -> bool {
        self.slot
            .window()
            .and_then(|window| window.firstResponder())
            .and_then(|responder| responder.downcast::<NSView>().ok())
            .is_some_and(|responder| responder.isDescendantOf(&self.slot))
    }

    /// Gives the keyboard back to gpui if the page or its inspector has it.
    pub(crate) fn release_keyboard(&self) {
        if !self.has_keyboard() {
            return;
        }
        if let Some(window) = self.slot.window() {
            window.makeFirstResponder(Some(&self.root));
        }
    }
}

impl Drop for NativeBrowser {
    fn drop(&mut self) {
        // The slot goes, and a docked inspector with it; Wry then takes the
        // page out of the slot on its own.
        self.release_keyboard();
        self.slot.removeFromSuperview();
    }
}
