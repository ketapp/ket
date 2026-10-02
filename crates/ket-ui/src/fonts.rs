//! Which typefaces the shell draws with, and the ones it brings along.
//!
//! Asking `gpui` for a family it cannot find does not fail: it quietly
//! substitutes its own fallback, Helvetica on macOS. A proportional face
//! spread across the terminal grid, and a window root that had asked for
//! `SF Mono` and been rendering in Helvetica all along, were both this. So
//! every family the shell uses is chosen here from a preference list
//! checked against what the text system says is installed — once, at
//! load, because enumerating the system's fonts is not free.
//!
//! # Three roles, not two
//!
//! The chrome used to be one face, and four rounds of picking that one face
//! ended in the same place each time, because a sidebar row is not one kind
//! of text. `test-token-reduction` is a **git identifier** — read character
//! by character, compared against another one, and wrong if a `rn` can be
//! taken for an `m`. "Claude Code · needs you" is **prose**, read as words at
//! 9.5px, where a monospace is at its worst: it is the widest way to set a
//! line that already has to fit a 300px sidebar, and the even rhythm that
//! makes it good for the branch name above is what makes it grey here.
//!
//! So the chrome has two faces and the editor keeps its own:
//!
//! - [`chrome_family`] — Geist Mono, on trial with the Desk redesign;
//!   Monaspace Argon before it. The window root, and anything that is an
//!   identifier: branch names, commit shas, paths, figures.
//! - [`prose_family`] — Geist, on trial alongside it; SF Pro and Mona Sans
//!   before it. Anything that is words. Reached through [`prose`] or the [`Prose`] extension trait
//!   rather than threaded through every component — see below.
//! - [`monospace_family`] — Monaspace Neon. The terminal grid and the editor.
//!
//! All three from GitHub's one foundry, which is the reason for the choice:
//! Monaspace was drawn as Mona Sans's monospaced companion, so the branch line
//! and the prose line under it share a skeleton rather than two unrelated
//! faces sharing a row. Argon and Neon are two voices of one metric-compatible
//! superfamily — Argon softer and humanist for the chrome, Neon neutral for
//! code — so the labels and the code they name stay related without being
//! indistinguishable. Geist Mono and IBM Plex Mono held these two slots
//! before; Intel One Mono held both for a day.
//!
//! # What is bundled
//!
//! Five families are compiled into the binary so the preference lists always
//! bottom out somewhere deliberate rather than in whatever the host has:
//! Geist Mono and Geist for the chrome on trial, Monaspace Argon and Mona Sans
//! kept behind them, Monaspace Neon for code, all under the SIL Open Font License (the licences
//! sit beside the files in `assets/fonts`). They are registered with the text
//! system by [`install_bundled`] before the window opens, which is what makes
//! them show up as installed to the pickers below.

use std::borrow::Cow;
use std::sync::OnceLock;

use gpui::{SharedString, Styled, TextSystem};

/// The faces compiled into the binary.
///
/// Static instances rather than the variable files the foundries also ship,
/// because a variable font registers as a single face at its default weight
/// and the chrome's semibolds would come back regular.
///
/// Four weights each of Geist Mono, Geist, Monaspace Argon and Mona Sans, no
/// italics: the
/// chrome never slants. Five faces of Monaspace Neon: four weights, because
/// the editor leans on weight where it used to lean on colour — a semibold
/// keyword — and one italic, which is what a comment is set in.
const BUNDLED: [&[u8]; 21] = [
    include_bytes!("../assets/fonts/geist-mono/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/geist-mono/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/geist-mono/GeistMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/geist-mono/GeistMono-Bold.ttf"),
    include_bytes!("../assets/fonts/geist/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/geist/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/geist/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/geist/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceArgon-Regular.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceArgon-Medium.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceArgon-SemiBold.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceArgon-Bold.otf"),
    include_bytes!("../assets/fonts/mona-sans/MonaSans-Regular.ttf"),
    include_bytes!("../assets/fonts/mona-sans/MonaSans-Medium.ttf"),
    include_bytes!("../assets/fonts/mona-sans/MonaSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/mona-sans/MonaSans-Bold.ttf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceNeon-Regular.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceNeon-Medium.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceNeon-SemiBold.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceNeon-Bold.otf"),
    include_bytes!("../assets/fonts/monaspace/MonaspaceNeon-Italic.otf"),
];

/// Monospace, for the terminal grid and the editor's text, in order of
/// preference.
///
/// Monaspace Neon is bundled and so always wins. The rest of the list only
/// decides what happens if registering the bundled faces ever fails, which
/// is why it ends where it does: `SF Mono` is what Terminal.app draws with,
/// but Apple ships it only inside Terminal's and Xcode's bundles, so it is
/// available to other apps only where someone has installed it system-wide.
/// `Menlo` is on every Mac and was Terminal's default for a decade before
/// it — and unlike a second bundled family, it is still there in exactly the
/// case this fallback exists for.
const MONOSPACE: [&str; 3] = ["Monaspace Neon", "SF Mono", "Menlo"];

/// The chrome's default face, in order of preference.
///
/// Monaspace Argon is bundled and so always wins. Still a mono, and still a
/// different voice from the code's Neon, so the labels and the text they
/// describe stay the same kind of text without being indistinguishable — but
/// it is the face for the chrome's *identifiers* rather than for all of it.
///
/// `.SystemUIFont` is `gpui`'s name for whatever the platform draws its own
/// UI in — SF Pro on a Mac — and the one family guaranteed to resolve, so it
/// is the floor should registering the bundled faces ever fail. Inter was
/// tried here first and rejected on sight; Instrument Sans held the slot for
/// a week; Spline Sans Mono for rather longer; Red Hat Text, the code's own
/// mono, Recursive and Intel One Mono each had a morning; Geist Mono held it
/// before Argon.
///
/// On trial: Geist Mono again, with the Desk redesign whose mockup was drawn
/// in it. Argon stays bundled and second, so the trial is undone by dropping
/// the first entry.
const CHROME: [&str; 3] = ["Geist Mono", "Monaspace Argon", ".SystemUIFont"];

/// The chrome's prose face, in order of preference.
///
/// On trial: `.SystemUIFont`, which is SF Pro on a Mac. It always resolves,
/// so Mona Sans below it is unreachable while it leads — kept bundled and
/// listed so the trial is undone by swapping the first two entries. SF Pro is
/// the one sans drawn and hinted for the platform's own text at these sizes,
/// and Core Text switches it to its Text optical size below 20pt, which a
/// bundled static face cannot do. Proportional, which is the point: this is
/// the face for the lines that are words, and it buys back the width a mono
/// spends on them. Mona Sans held this slot before it, and Geist before that.
///
/// Geist leads for now, beside Geist Mono in the chrome: the pair the Desk
/// mockup was drawn in. SF Pro behind it is the trial before this one.
const PROSE: [&str; 3] = ["Geist", ".SystemUIFont", "Mona Sans"];

/// The prose face, once the shell has resolved it.
///
/// A global rather than a field threaded through the component tree. Every
/// constructor in `ui::` takes a `&Theme` and nothing else, and the
/// alternative is a second parameter on all of them carrying a value that is
/// decided once at load and never changes again — which is how a component
/// vocabulary starts growing arguments that have nothing to do with the
/// component. Set once by [`resolve`]; read through [`prose`].
static RESOLVED_PROSE: OnceLock<SharedString> = OnceLock::new();

/// Registers the bundled faces with the text system.
///
/// Call before enumerating fonts or opening a window. Failure is not fatal
/// — the preference lists fall through to what the host has — so it is
/// reported rather than propagated.
pub(crate) fn install_bundled(text_system: &TextSystem) {
    let fonts = BUNDLED.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(e) = text_system.add_fonts(fonts) {
        eprintln!("could not register the bundled fonts: {e}");
    }
}

/// Picks every family the chrome needs, given the text system's list of what
/// is installed, and remembers the prose one for [`prose`].
///
/// One call rather than three, so the prose face cannot be resolved by one
/// caller and left unset for the rest of the window — which would show up as
/// a chrome that is proportional in the places that asked early and Helvetica
/// in the places that asked late.
pub(crate) fn resolve(installed: &[String]) -> (SharedString, SharedString, SharedString) {
    let code: SharedString = monospace_family(installed).into();
    let chrome: SharedString = chrome_family(installed).into();
    let prose: SharedString = prose_family(installed).into();
    // `set` rather than `get_or_init`: a second window would resolve to the
    // same names anyway, and swallowing the second call is the behaviour that
    // keeps this from caring how many there are.
    let _ = RESOLVED_PROSE.set(prose.clone());
    let _ = RESOLVED_CHROME.set(chrome.clone());
    (code, chrome, prose)
}

/// The chrome's prose face, wherever a component needs it.
///
/// Before [`resolve`] has run this is the last entry of [`PROSE`] — the same
/// answer the picker gives for a host with nothing installed. That only
/// happens in a test or a component built outside a window; a real frame is
/// always drawn after `load`.
/// The chrome's mono, once the shell has resolved it — for a figure inside
/// a component that has been set in prose, such as a count in a segment.
/// See [`RESOLVED_PROSE`] for why this is a global.
static RESOLVED_CHROME: OnceLock<SharedString> = OnceLock::new();

/// The chrome's mono face: what the window root wears. Only needed where a
/// prose parent has to be undone for a figure.
pub(crate) fn chrome() -> SharedString {
    RESOLVED_CHROME
        .get()
        .cloned()
        .unwrap_or_else(|| CHROME[CHROME.len() - 1].into())
}

pub(crate) fn prose() -> SharedString {
    RESOLVED_PROSE
        .get()
        .cloned()
        .unwrap_or_else(|| PROSE[PROSE.len() - 1].into())
}

/// `.prose()` on any element, for the parts of the chrome that are words
/// rather than identifiers.
///
/// The one way to reach the proportional face. A call site says what its text
/// *is* and the family follows, which is what keeps the pairing from decaying
/// into two fonts sprinkled by eye: reviewing it is reading for `.prose()` on
/// things that are not prose, rather than diffing font names.
pub(crate) trait Prose: Styled + Sized {
    /// Sets this element's text in the prose face. Inherited by its children,
    /// so a container carries the whole block.
    fn prose(self) -> Self {
        self.font_family(prose())
    }
}

impl<E: Styled> Prose for E {}

/// The first monospace family in [`MONOSPACE`] that is installed, given
/// the text system's list of every family it knows.
pub(crate) fn monospace_family(installed: &[String]) -> &'static str {
    first_installed(&MONOSPACE, installed)
}

/// The first chrome family in [`CHROME`] that is installed.
pub(crate) fn chrome_family(installed: &[String]) -> &'static str {
    first_installed(&CHROME, installed)
}

/// The first prose family in [`PROSE`] that is installed.
pub(crate) fn prose_family(installed: &[String]) -> &'static str {
    first_installed(&PROSE, installed)
}

/// The first of `preferred` that `installed` names, or the last of
/// `preferred` when none is — a guess is better than a panic, and the
/// last entry of each list is the one most likely to exist.
fn first_installed(preferred: &[&'static str], installed: &[String]) -> &'static str {
    preferred
        .iter()
        .copied()
        .find(|family| installed.iter().any(|name| name == family))
        .unwrap_or(preferred[preferred.len() - 1])
}
