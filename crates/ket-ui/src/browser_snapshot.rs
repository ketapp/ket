//! A still of a browser pane's page, for ket's own menus and popovers to draw
//! over.
//!
//! A browser pane's page is a `WKWebView`, a native child view laid over the
//! window above everything gpui draws. Nothing ket draws can cover it, so while
//! a menu, popup or dialog is up the live page is hidden — and it used to
//! leave the pane blank behind the menu. Now the pane shows this still of the
//! page instead, taken as the overlay opens, and the live page comes back when
//! it closes. See `Shell::sync_browser_visibility`.
//!
//! # Why this is `unsafe`
//!
//! The fifth of the workspace's documented carve-outs (see its lint block).
//! WebKit's `takeSnapshotWithConfiguration:completionHandler:` has no safe
//! binding — `objc2` generates it as an `unsafe fn` — and the image it hands
//! back arrives as a raw pointer. What is done with them here is the
//! documented use, a snapshot of the visible page, and nothing else in ket
//! calls them.
//!
//! # Pixels
//!
//! None are touched here. The snapshot leaves as an uncompressed TIFF, written
//! by AppKit, and gpui decodes it into its own image off the main thread — see
//! `Shell::take_browser_still`. Ket's own code is unoptimised in a debug build,
//! where converting a full-window still pixel by pixel took most of 140 ms and
//! held the menu under the live page while it ran; and doing the byte order by
//! hand, the first time, read AppKit's layout wrong and tinted the still.

use std::cell::RefCell;

use block2::RcBlock;
use objc2_app_kit::NSImage;
use objc2_foundation::NSError;
use wry::WebViewExtMacOS as _;

/// Asks WebKit for a picture of `webview`'s page as it stands now. `done` is
/// called once, on the main thread, with it as TIFF bytes — or `None` when
/// WebKit had none to give.
pub(crate) fn take(webview: &wry::WebView, done: impl FnOnce(Option<Vec<u8>>) + 'static) {
    let done = RefCell::new(Some(done));
    let handler = RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
        let Some(done) = done.borrow_mut().take() else {
            return;
        };
        // SAFETY: WebKit passes the snapshot, or null when it failed; either
        // way it is valid for the duration of this call, which is as long as
        // it is used.
        let image = unsafe { image.as_ref() };
        done(
            image
                .and_then(|image| image.TIFFRepresentation())
                .map(|tiff| tiff.to_vec()),
        );
    });
    // SAFETY: a nil configuration is WebKit's documented default — the
    // visible part of the page at the screen's density — and WebKit copies
    // the block it is given, so it outlives this call.
    unsafe {
        webview
            .webview()
            .takeSnapshotWithConfiguration_completionHandler(None, &handler);
    }
}
