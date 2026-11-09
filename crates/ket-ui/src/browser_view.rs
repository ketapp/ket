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
//! (see [`NativeBrowser::still`]), drawn across the whole pane — so with the
//! inspector docked it is stretched over the inspector's band until the menu
//! closes.
//!
//! Built with the `chromium` feature, the page is Chromium's instead (see
//! [`crate::chromium`]) and the slot is the same: ket still only ever moves
//! the slot. Chromium's DevTools open in a window of their own; on macOS CEF
//! cannot dock them in a view of ket's.

use std::cell::Cell;

use gpui::{Bounds, Pixels};
use objc2::MainThreadOnly as _;
use objc2::rc::Retained;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
#[cfg(not(feature = "chromium"))]
use wry::WebViewExtMacOS as _;

/// Where a hidden page waits, in points from the window's corner: far enough
/// down and left that no window is big enough to reach it — see
/// [`NativeBrowser::hide`].
const PARKED: f64 = -100_000.0;

/// A page and the slot it lives in, on the window's main thread.
pub(crate) struct NativeBrowser {
    #[cfg(not(feature = "chromium"))]
    raw: wry::WebView,
    /// Chromium's page. Dropping it closes the page.
    #[cfg(feature = "chromium")]
    page: ket_chromium::Browser,
    /// The view the page fills, the whole of the slot.
    #[cfg(feature = "chromium")]
    page_area: Retained<NSView>,
    /// The pane-sized view the page and, when docked, its inspector share.
    slot: Retained<NSView>,
    /// gpui's view: the slot's parent, and where the keyboard goes back to.
    root: Retained<NSView>,
    shown: Cell<bool>,
}

/// A still of a page as its engine handed it over, not yet decoded — see
/// [`NativeBrowser::still`].
pub(crate) struct Snapshot {
    format: gpui::ImageFormat,
    /// The image, or with Chromium its base64 text, as the DevTools protocol
    /// sends it.
    bytes: Vec<u8>,
}

impl Snapshot {
    /// The still as gpui's image, still encoded in the format it came in.
    /// Meant for gpui's background executor, which then decodes it, off the
    /// main thread, in gpui's own optimised code — see `browser_snapshot` for
    /// why not by hand.
    pub(crate) fn image(self) -> Option<gpui::Image> {
        #[cfg(feature = "chromium")]
        let bytes = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&self.bytes)
                .ok()?
        };
        #[cfg(not(feature = "chromium"))]
        let bytes = self.bytes;
        Some(gpui::Image::from_bytes(self.format, bytes))
    }
}

#[cfg(not(feature = "chromium"))]
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

    /// Asks WebKit for a still of the page as it stands, which comes back on
    /// the main thread — or `None` when WebKit had none to give.
    pub(crate) fn still(&self) -> impl Future<Output = Option<Snapshot>> + 'static + use<> {
        let (tell, still) = tokio::sync::oneshot::channel();
        crate::browser_snapshot::take(&self.raw, move |tiff| {
            let _ = tell.send(tiff);
        });
        async move {
            let bytes = still.await.ok().flatten()?;
            Some(Snapshot {
                format: gpui::ImageFormat::Tiff,
                bytes,
            })
        }
    }
}

#[cfg(feature = "chromium")]
impl NativeBrowser {
    /// Opens `url` in a new Chromium page, keeping its cookies and storage
    /// in `profile`, in a slot of its own in `root`. The slot starts hidden
    /// and parked, the window's size, so the page can be created — and
    /// driven — before its pane is ever on screen, and its first layout is
    /// not from nothing; [`NativeBrowser::show`] brings it up.
    pub(crate) fn open(
        root: Retained<NSView>,
        profile: &std::path::Path,
        url: &str,
        main: MainThreadMarker,
    ) -> Result<
        (
            Self,
            tokio::sync::mpsc::UnboundedReceiver<ket_chromium::Event>,
        ),
        ket_chromium::Error,
    > {
        let (slot, page_area) = Self::slot_in(&root, main);
        let (page, events) = match ket_chromium::Browser::open(&page_area, profile, url) {
            Ok(opened) => opened,
            Err(error) => {
                slot.removeFromSuperview();
                return Err(error);
            }
        };
        let this = Self {
            page,
            page_area,
            slot,
            root,
            shown: Cell::new(false),
        };
        this.hide();
        Ok((this, events))
    }

    /// Takes a page another page opened, in a slot of its own in `root`,
    /// hidden and parked as [`NativeBrowser::open`]'s starts.
    pub(crate) fn adopt(
        root: Retained<NSView>,
        popup: ket_chromium::PopupPage,
        main: MainThreadMarker,
    ) -> (
        Self,
        tokio::sync::mpsc::UnboundedReceiver<ket_chromium::Event>,
    ) {
        let (slot, page_area) = Self::slot_in(&root, main);
        let (page, events) = popup.adopt(&page_area);
        let this = Self {
            page,
            page_area,
            slot,
            root,
            shown: Cell::new(false),
        };
        this.hide();
        (this, events)
    }

    /// A hidden slot the window's size in `root`, and the area inside it
    /// that the page fills.
    fn slot_in(root: &NSView, main: MainThreadMarker) -> (Retained<NSView>, Retained<NSView>) {
        let slot = NSView::initWithFrame(NSView::alloc(main), root.bounds());
        slot.setHidden(true);
        // As the WebKit page's slot is: anchored at the top left.
        slot.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        root.addSubview(&slot);
        let page_area = NSView::initWithFrame(NSView::alloc(main), slot.bounds());
        page_area.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        slot.addSubview(&page_area);
        (slot, page_area)
    }

    /// Chromium's handle on the page: loads, history, editing, its DevTools
    /// protocol.
    pub(crate) fn page(&self) -> &ket_chromium::Browser {
        &self.page
    }

    /// Asks the page, through its DevTools protocol, for a still of what it
    /// shows — `None` when it had none to give.
    pub(crate) fn still(&self) -> impl Future<Output = Option<Snapshot>> + 'static + use<> {
        use ket_chromium::protocol::browser_protocol::page::{
            CaptureScreenshotFormat, CaptureScreenshotParams,
        };
        let cdp = self.page.cdp();
        async move {
            let shot = cdp
                .call(CaptureScreenshotParams {
                    format: Some(CaptureScreenshotFormat::Png),
                    // A still to look at for as long as a menu is up, not to
                    // keep: a faster encode is worth a bigger image.
                    optimize_for_speed: Some(true),
                    ..Default::default()
                })
                .await
                .ok()?;
            Some(Snapshot {
                format: gpui::ImageFormat::Png,
                bytes: String::from(shot.data).into_bytes(),
            })
        }
    }

    /// Whether the page itself has the window's keyboard.
    pub(crate) fn page_has_keyboard(&self) -> bool {
        self.keyboard_in(&self.page_area)
    }

    /// Whether the page's DevTools window is open.
    pub(crate) fn devtools_open(&self) -> bool {
        self.page.devtools_open()
    }

    /// Opens the page's DevTools in a window of their own, or closes it.
    /// Nothing until the page exists.
    pub(crate) fn toggle_devtools(&self) {
        if self.page.devtools_open() {
            self.page.close_devtools();
        } else if self.page.view().is_some() {
            self.page.show_devtools();
        }
    }
}

impl NativeBrowser {
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
        self.keyboard_in(&self.slot)
    }

    /// Whether the window's keyboard is in `view` or beneath it.
    fn keyboard_in(&self, view: &NSView) -> bool {
        self.slot
            .window()
            .and_then(|window| window.firstResponder())
            .and_then(|responder| responder.downcast::<NSView>().ok())
            .is_some_and(|responder| responder.isDescendantOf(view))
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
        // page out of the slot on its own. Chromium's page closes as it is
        // dropped, after this, and frees its view itself.
        self.release_keyboard();
        #[cfg(feature = "chromium")]
        if self.page.devtools_open() {
            self.page.close_devtools();
        }
        self.slot.removeFromSuperview();
    }
}
