//! Chromium, embedded in ket: the Chromium Embedded Framework (CEF) loaded
//! into ket.app, each browser a native child view of a slot ket places, and
//! each browser's DevTools protocol reached in process.
//!
//! Everything is behind the `cef` feature, which is off by default and is
//! macOS only. Building CEF downloads its distribution and compiles its C++
//! wrapper with cmake and Ninja, which neither CI nor a plain `cargo build`
//! should do; without the feature this crate is empty.
//!
//! How it sits inside a gpui app:
//!
//! - gpui owns `NSApplication` and its run loop. Chromium wants the
//!   application object to tell it when it is inside `sendEvent:`; ket's
//!   gpui patch adds the methods and `app` adds the protocols that name them.
//! - CEF's message loop runs on the main thread, pumped by a run-loop timer
//!   (`pump`) rather than by CEF's own loop, which gpui's would fight.
//! - Chromium's subprocesses run sandboxed from helper bundles inside
//!   ket.app, whose binary calls [`helper_main`].
//! - One root cache for Chromium as a whole ([`Config::root_cache`]) and a
//!   profile beneath it per worktree, so each worktree's pages keep their
//!   own cookies and storage.
//! - No remote-debugging port is opened. Agents reach a page through ket,
//!   which speaks the DevTools protocol to it in process ([`Cdp`]).
//!
//! Everything here is called on the main thread, which with the external
//! pump is also CEF's UI thread, except [`Cdp`], which may be used from any.

#![cfg(all(feature = "cef", target_os = "macos"))]

/// Chromium's application protocols, added to gpui's `NSApplication` class.
/// `class_addProtocol` has no safe binding; this module holds that call.
#[allow(unsafe_code)]
mod app;
mod browser;
mod cdp;
mod engine;
mod handlers;
/// CEF's message pump on the main run loop. Creating a CoreFoundation timer,
/// naming the common modes and handing a block to the main run loop have no
/// safe binding; this module holds those calls.
#[allow(unsafe_code)]
mod pump;
mod record;

pub use browser::{Browser, Dialog, DialogKind, Edit, Event, PopupPage, profile_in_use};
pub use cdp::{Cdp, CdpEvent};
pub use engine::{Config, helper_main, shutdown, start, started};
pub use record::{RecordOptions, Recorder, Recording};

/// The DevTools protocol's commands, events and types, generated from
/// Chrome's protocol files: what [`Cdp::call`] takes, under
/// `browser_protocol` and `js_protocol`.
pub use chromiumoxide_cdp::cdp as protocol;
/// A DevTools protocol command, as [`Cdp::call`] takes it.
pub use chromiumoxide_types::Command;

/// The version of Chromium ket's browser tabs run, `156.0.8078.48`: the one
/// in the CEF this crate was built against, which ket.app bundles.
pub fn chromium_version() -> String {
    use cef::sys::{
        CHROME_VERSION_BUILD as BUILD, CHROME_VERSION_MAJOR as MAJOR,
        CHROME_VERSION_MINOR as MINOR, CHROME_VERSION_PATCH as PATCH,
    };
    format!("{MAJOR}.{MINOR}.{BUILD}.{PATCH}")
}

/// Why Chromium could not do what was asked, said plainly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// ket is not running from a bundle with Chromium in it — a `cargo run`
    /// binary, say, rather than ket.app. Says what was looked for.
    NotBundled(String),
    /// Anything else.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotBundled(why) | Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}

fn failed(why: impl std::fmt::Display) -> Error {
    Error::Failed(why.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_chromium_version_is_the_one_cef_names() {
        let version = super::chromium_version();
        assert_eq!(version.split('.').count(), 4, "{version}");
        let cef = String::from_utf8_lossy(cef::sys::CEF_VERSION);
        assert!(
            cef.trim_end_matches('\0')
                .ends_with(&format!("chromium-{version}")),
            "{cef} is not Chromium {version}"
        );
    }
}
