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
//!
//! # A chosen face
//!
//! Settings can name a family for the code and one for the interface —
//! `theme.code_font` and `theme.ui_font`. A chosen family that is installed
//! takes the role outright; the interface one takes both chrome roles, since
//! a person who picks a font for the interface means all of it. One that is
//! not installed is passed over for the lists above, with a note in the log,
//! so an uninstalled font cannot put the window in Helvetica.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::RwLock;

use gpui::{Pixels, SharedString, Styled, TextSystem, font, px};

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
/// decided at load and changes only when Settings picks another — which is
/// how a component vocabulary starts growing arguments that have nothing to
/// do with the component. Set by [`resolve`]; read through [`prose`]. A lock
/// rather than a `OnceLock` because a face picked in Settings has to take
/// effect in the window that picked it.
static RESOLVED_PROSE: RwLock<Option<SharedString>> = RwLock::new(None);

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

/// The faces the shell holds itself, resolved against what is installed.
///
/// Two rather than three: the prose face is reached through [`prose`] and
/// nothing else, so handing it back as well would be a value with no reader.
pub(crate) struct Faces {
    /// The terminal grid's and the editor's.
    pub(crate) code: SharedString,
    /// The window root's: identifiers and figures.
    pub(crate) chrome: SharedString,
}

/// Picks every family the chrome needs, given the text system's list of what
/// is installed and the families Settings chose, and remembers the chrome
/// pair for [`prose`] and [`chrome`].
///
/// One call rather than three, so the prose face cannot be resolved by one
/// caller and left unset for the rest of the window — which would show up as
/// a chrome that is proportional in the places that asked early and Helvetica
/// in the places that asked late. Called again when a choice changes, which
/// is what the lock on the globals is for.
pub(crate) fn resolve(installed: &[String], code: Option<&str>, ui: Option<&str>) -> Faces {
    let (code, chrome, prose) = pick(installed, code, ui);
    *RESOLVED_PROSE.write().unwrap_or_else(|e| e.into_inner()) = Some(prose);
    *RESOLVED_CHROME.write().unwrap_or_else(|e| e.into_inner()) = Some(chrome.clone());
    Faces { code, chrome }
}

/// The three faces a choice comes to — code, chrome, prose — before any of
/// them is remembered. Kept apart from [`resolve`] so it can be asked
/// without touching the globals, which is what a test needs.
fn pick(
    installed: &[String],
    code: Option<&str>,
    ui: Option<&str>,
) -> (SharedString, SharedString, SharedString) {
    let code: SharedString = match chosen(code, installed) {
        Some(family) => family.to_owned().into(),
        None => monospace_family(installed).into(),
    };
    let (chrome, prose): (SharedString, SharedString) = match chosen(ui, installed) {
        Some(family) => (family.to_owned().into(), family.to_owned().into()),
        None => (
            chrome_family(installed).into(),
            prose_family(installed).into(),
        ),
    };
    (code, chrome, prose)
}

/// A chosen family, when the text system knows it.
///
/// One it does not — uninstalled since it was picked, or misspelt in the
/// file — is noted and passed over, so the window opens in ket's own face
/// rather than in whatever `gpui` substitutes for a family it cannot find.
fn chosen<'a>(family: Option<&'a str>, installed: &[String]) -> Option<&'a str> {
    let family = family?;
    if installed.iter().any(|name| name == family) {
        Some(family)
    } else {
        tracing::warn!(
            family,
            "a chosen font is not installed; drawing with ket's own"
        );
        None
    }
}

/// Every family the text system knows, for the pickers in Settings, and the
/// fixed-width ones among them for the code picker.
///
/// Read once per window and kept — see `Shell::font_catalogue` — because
/// telling a fixed-width face from a proportional one means loading each
/// family, and a Mac has hundreds.
pub(crate) struct Catalogue {
    /// Every family, sorted without regard to case, less the hidden system
    /// faces whose names start with a dot.
    pub(crate) families: Vec<String>,
    /// The families that set an `i` as wide as an `m`: what a terminal grid
    /// can be drawn in.
    pub(crate) fixed: Vec<String>,
}

impl Catalogue {
    /// Reads what is installed, the bundled faces among it.
    pub(crate) fn read(text_system: &TextSystem) -> Self {
        let mut families: Vec<String> = text_system
            .all_font_names()
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        families.sort_by_key(|name| name.to_lowercase());
        let fixed = families
            .iter()
            .filter(|family| is_fixed_width(text_system, family))
            .cloned()
            .collect();
        Self { families, fixed }
    }
}

/// Whether `family` sets every glyph at one width.
///
/// Measured rather than asked: `gpui` does not pass Core Text's fixed-pitch
/// trait through, and the trait is unset on some faces that are. An `i` and
/// an `m` are the narrowest and widest of the lowercase, so a face that
/// gives the two one advance gives everything one advance. A face missing
/// either glyph — a symbol set, an emoji font — is not one to draw code in.
fn is_fixed_width(text_system: &TextSystem, family: &str) -> bool {
    let id = text_system.resolve_font(&font(family.to_owned()));
    let size: Pixels = px(16.0);
    match (
        text_system.advance(id, size, 'i'),
        text_system.advance(id, size, 'm'),
    ) {
        (Ok(i), Ok(m)) => i.width > px(0.0) && i.width == m.width,
        _ => false,
    }
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
static RESOLVED_CHROME: RwLock<Option<SharedString>> = RwLock::new(None);

/// The chrome's mono face: what the window root wears. Only needed where a
/// prose parent has to be undone for a figure.
pub(crate) fn chrome() -> SharedString {
    RESOLVED_CHROME
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| CHROME[CHROME.len() - 1].into())
}

pub(crate) fn prose() -> SharedString {
    RESOLVED_PROSE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn nothing_chosen_is_the_shipped_pair() {
        let (code, chrome, prose) = pick(
            &installed(&["Monaspace Neon", "Geist Mono", "Geist"]),
            None,
            None,
        );
        assert_eq!(code.as_ref(), "Monaspace Neon");
        assert_eq!(chrome.as_ref(), "Geist Mono");
        assert_eq!(prose.as_ref(), "Geist");
    }

    #[test]
    fn a_chosen_family_that_is_installed_takes_its_role() {
        let (code, chrome, prose) = pick(
            &installed(&["Monaspace Neon", "Geist Mono", "Geist", "Menlo", "Avenir"]),
            Some("Menlo"),
            Some("Avenir"),
        );
        assert_eq!(code.as_ref(), "Menlo");
        // The interface choice is the whole chrome, words and identifiers.
        assert_eq!(chrome.as_ref(), "Avenir");
        assert_eq!(prose.as_ref(), "Avenir");
    }

    #[test]
    fn a_chosen_family_that_is_not_installed_is_passed_over() {
        // Uninstalled since it was picked. The shipped faces, not Helvetica.
        let (code, chrome, prose) = pick(
            &installed(&["Monaspace Neon", "Geist Mono", "Geist"]),
            Some("Gone Mono"),
            Some("Gone Sans"),
        );
        assert_eq!(code.as_ref(), "Monaspace Neon");
        assert_eq!(chrome.as_ref(), "Geist Mono");
        assert_eq!(prose.as_ref(), "Geist");
    }
}
