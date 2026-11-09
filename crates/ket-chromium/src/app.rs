//! Chromium's protocols on gpui's `NSApplication`.
//!
//! Chromium asks the application object whether it is in the middle of
//! `sendEvent:` before it handles an event of its own, and it asks through
//! three protocols: `CrAppProtocol`, `CrAppControlProtocol` and CEF's
//! `CefAppProtocol`. An app built around CEF adopts them by subclassing
//! `NSApplication`. ket's application class is gpui's (`GPUIApplication`),
//! and ket's patch to it adds the methods — `isHandlingSendEvent`,
//! `setHandlingSendEvent:` and the `sendEvent:` override that keeps them
//! honest. What it cannot add is the protocols themselves: they are defined
//! by the Chromium framework, which only exists once it is loaded. So they
//! are added here, at run time, to whatever class `NSApp` is.
//!
//! `class_addProtocol` has no safe binding, which is the one reason this
//! module may use `unsafe`. It holds that call and nothing else.

use std::ffi::CStr;
use std::ptr;

use objc2::MainThreadMarker;
use objc2::runtime::AnyProtocol;
use objc2::sel;
use objc2_app_kit::NSApplication;

use crate::{Error, failed};

/// The protocols, and whether Chromium cannot do without each. CEF's own is
/// only looked up by CEF's headers, so a framework that does not define it
/// does not need it either.
const PROTOCOLS: [(&CStr, bool); 3] = [
    (c"CrAppProtocol", true),
    (c"CrAppControlProtocol", true),
    (c"CefAppProtocol", false),
];

/// Makes `NSApp` conform to Chromium's protocols. Call once the framework is
/// loaded and before CEF is initialized.
///
/// Fails, rather than letting Chromium crash later on a selector it cannot
/// send, if the application class lacks the methods the protocols promise:
/// that means ket's gpui patch is missing.
pub(crate) fn adopt(mtm: MainThreadMarker) -> Result<(), Error> {
    let app = NSApplication::sharedApplication(mtm);
    let class = app.class();
    let name = class.name().to_string_lossy();
    for method in [sel!(isHandlingSendEvent), sel!(setHandlingSendEvent:)] {
        if !class.responds_to(method) {
            return Err(failed(format!(
                "{name} has no {} method, which Chromium needs; ket's gpui patch is missing",
                method.name().to_string_lossy()
            )));
        }
    }
    for (protocol_name, required) in PROTOCOLS {
        let Some(protocol) = AnyProtocol::get(protocol_name) else {
            if required {
                return Err(failed(format!(
                    "the Chromium framework does not define {}",
                    protocol_name.to_string_lossy()
                )));
            }
            continue;
        };
        if class.conforms_to(protocol) {
            continue;
        }
        // SAFETY: `class` is the registered class of the live application
        // object and `protocol` a registered protocol; both last as long as
        // the process. Adopting a protocol only changes what
        // `conformsToProtocol:` answers, and the methods it promises were
        // checked for above. Called on the main thread, before Chromium has
        // started asking.
        unsafe {
            objc2::ffi::class_addProtocol(ptr::from_ref(class).cast_mut(), protocol);
        }
    }
    Ok(())
}
