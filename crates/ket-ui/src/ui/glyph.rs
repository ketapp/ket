//! A project's glyph: a small picture in its badge where the letter goes.
//!
//! Forty-seven marks from **Lucide** 1.64 (ISC; the licence sits in
//! `assets/icons`), vendored under `assets/icons/glyphs` the way the chrome's
//! icons are, with `currentColor` written out as a literal. A fixed set rather
//! than any file or emoji: a badge is sixteen pixels, a picture there is noise,
//! and a stroke drawn on the same grid as every other icon reads as part of the
//! sidebar rather than pasted onto it. Each was picked for holding its shape at
//! the badge's size.
//!
//! Stored in `ProjectSettings::icon` as `glyph:<name>`. Core keeps that field
//! verbatim and leaves its meaning to the shell, so the prefix is what tells a
//! glyph from a letter someone typed — `ket` typed as an icon stays the word.
//! The phone reads the same names (`mobile/app/src/lib/glyphs.ts`, generated
//! from these files), so a name here is a promise to it: rename one and a
//! project that picked it draws its letter instead.

/// What marks an icon as a glyph rather than text.
pub(crate) const PREFIX: &str = "glyph:";

/// Each glyph's name, its asset path, and its bytes, in the order the picker
/// lays them out.
macro_rules! glyphs {
    ($($name:literal),* $(,)?) => {
        [$((
            $name,
            concat!("glyphs/", $name, ".svg"),
            include_bytes!(concat!("../../assets/icons/glyphs/", $name, ".svg")) as &[u8],
        )),*]
    };
}

/// Every glyph a badge can wear: work, then things, then the rest.
pub(crate) const GLYPHS: [(&str, &str, &[u8]); 47] = glyphs![
    "terminal",
    "code",
    "braces",
    "database",
    "server",
    "cpu",
    "bug",
    "git-branch",
    "package",
    "layers",
    "cloud",
    "wrench",
    "rocket",
    "flask-conical",
    "bot",
    "brain",
    "smartphone",
    "monitor",
    "globe",
    "mail",
    "message-square",
    "camera",
    "music",
    "gamepad-2",
    "book-open",
    "graduation-cap",
    "heart",
    "stethoscope",
    "toothbrush",
    "coins",
    "chart-line",
    "shopping-cart",
    "house",
    "briefcase",
    "map-pin",
    "plane",
    "leaf",
    "sun",
    "moon",
    "star",
    "zap",
    "flame",
    "telescope",
    "orbit",
    "shield",
    "key",
    "sparkles",
];

/// One of [`GLYPHS`], by its place in the table.
///
/// Only this module makes one, so [`Icon::Glyph`](super::icon::Icon::Glyph)
/// keeps the promise the rest of that enum makes: an icon that is not there
/// cannot be named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Glyph(usize);

impl Glyph {
    /// Its name, as stored after [`PREFIX`].
    pub(crate) fn name(self) -> &'static str {
        GLYPHS[self.0].0
    }

    /// The asset path it is registered under.
    pub(crate) fn asset(self) -> &'static str {
        GLYPHS[self.0].1
    }

    /// How it is stored in a project's icon.
    pub(crate) fn stored(self) -> String {
        format!("{PREFIX}{}", self.name())
    }
}

/// Every glyph, in the picker's order.
pub(crate) fn every() -> impl Iterator<Item = Glyph> {
    (0..GLYPHS.len()).map(Glyph)
}

/// The glyph `icon` names, if it names one ket has.
///
/// An unknown name is `None`, so a project that picked a glyph a later ket
/// dropped falls back to its letter rather than to an empty badge.
pub(crate) fn named(icon: &str) -> Option<Glyph> {
    let name = icon.trim().strip_prefix(PREFIX)?;
    GLYPHS
        .iter()
        .position(|(each, _, _)| *each == name)
        .map(Glyph)
}

/// `icon` as a badge should take it: the icon itself, or `None` for a glyph
/// this ket does not have, so the badge falls back to the letter rather than
/// spelling out `glyph:unicorn`.
pub(crate) fn drawable(icon: &str) -> Option<&str> {
    if icon.trim().starts_with(PREFIX) && named(icon).is_none() {
        None
    } else {
        Some(icon)
    }
}

/// The bytes registered under `path`, for [`super::icon::Assets`].
pub(crate) fn load(path: &str) -> Option<&'static [u8]> {
    GLYPHS
        .iter()
        .find(|(_, each, _)| *each == path)
        .map(|(_, _, bytes)| *bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_glyph_reads_back_as_itself() {
        for glyph in every() {
            assert_eq!(named(&glyph.stored()), Some(glyph));
        }
    }

    #[test]
    fn text_is_not_a_glyph() {
        // A word that happens to be a glyph's name is still a word: only the
        // prefix makes it a picture.
        assert_eq!(named("terminal"), None);
        assert_eq!(named("K"), None);
        assert_eq!(named(""), None);
    }

    #[test]
    fn a_glyph_this_ket_does_not_have_falls_back() {
        assert_eq!(named("glyph:unicorn"), None);
        // No icon, so the badge draws the letter rather than the word.
        assert_eq!(drawable("glyph:unicorn"), None);
        assert_eq!(drawable("glyph:leaf"), Some("glyph:leaf"));
        assert_eq!(drawable("K"), Some("K"));
    }

    #[test]
    fn every_glyph_is_a_drawable_stroke_svg() {
        // `resvg` draws `currentColor` as nothing; see `super::super::icon`.
        for (name, path, bytes) in GLYPHS {
            let text = std::str::from_utf8(bytes).expect("svg is text");
            assert!(text.contains("<svg"), "{name} is not an svg");
            assert!(
                !text.contains("currentColor"),
                "{name} still says currentColor"
            );
            assert_eq!(load(path), Some(bytes));
        }
    }
}
