//! What Chromium is told not to do: call Google, fetch add-ons and models,
//! or offer its AI features. A tab is for the page in it; Chromium has no
//! business of its own on the network. [`quiet`] adds it all to the browser
//! process's command line.

use cef::{CefString, CommandLine, ImplCommandLine};

/// Chromium's own services that talk to Google in the background: the
/// component updater (on-device AI models, Safe Browsing lists, Widevine and
/// the rest), sync, and Domain Reliability's reports.
const QUIET: [&str; 4] = [
    "disable-background-networking",
    "disable-component-update",
    "disable-domain-reliability",
    "disable-sync",
];

/// Turned off on top of those CEF turns off itself: Chrome's AI features and
/// the AI APIs it gives pages, and the features that send what a page holds
/// to Google — optimization hints, Translate, Safe Browsing's real-time
/// lookups — or look for Cast devices on the network.
const FEATURES_OFF: &[&str] = &[
    "Glic",
    "Compose",
    "HistoryEmbeddings",
    "AutofillAi",
    "AIPromptAPI",
    "AISummarizationAPI",
    "AIWriterAPI",
    "AIRewriterAPI",
    "AIProofreaderAPI",
    "AIProofreadingAPI",
    "AIClassifierAPI",
    "AIEmbeddingsAPI",
    "LanguageDetectionAPI",
    "TranslationAPI",
    "SegmentationPlatform",
    "OptimizationHints",
    "Translate",
    "SafeBrowsingHashPrefixRealTimeLookups",
    "MediaRouter",
];

/// Keeps the browser process to the pages ket opens: [`QUIET`], every one
/// of [`FEATURES_OFF`], and no RLZ ping.
pub(crate) fn quiet(command_line: &mut CommandLine) {
    for switch in QUIET {
        command_line.append_switch(Some(&CefString::from(switch)));
    }
    // RLZ, Chrome's install-tracking ping, has no switch. It reads the brand
    // Google Chrome's installer leaves in `~/Library/Google/Google Chrome
    // Brand.plist` and pings clients1.google.com unless that brand is one
    // of Google's organic ones, so the host goes unresolved instead. Pages
    // can't reach it either.
    command_line.append_switch_with_value(
        Some(&CefString::from("host-resolver-rules")),
        Some(&CefString::from("MAP clients1.google.com ~NOTFOUND")),
    );
    // Added to the list rather than in place of it, whichever of ket and
    // CEF writes it first.
    let name = CefString::from("disable-features");
    let mut off = CefString::from(&command_line.switch_value(Some(&name))).to_string();
    for feature in FEATURES_OFF {
        if !off.is_empty() {
            off.push(',');
        }
        off.push_str(feature);
    }
    command_line.append_switch_with_value(Some(&name), Some(&CefString::from(off.as_str())));
}
