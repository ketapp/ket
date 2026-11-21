//! A project's glyph: a small picture in its badge where the letter goes.
//!
//! Forty-seven marks from **Fluent UI System Icons** 1.1.346, filled (MIT;
//! the licence sits beside them), vendored under `assets/icons/glyphs` the way
//! the chrome's icons are: the path data only, with the fill written out as a
//! literal. Fluent because its filled marks are drawn for 16 px rather than
//! shrunk from a large grid, and a badge draws its glyph at 14. Each keeps its
//! own grid: 16 units, or 20 for the three Fluent draws no smaller
//! (`terminal`, `layers`, `stethoscope`). The table below names each one's
//! Fluent icon.
//!
//! A fixed set rather than any file or emoji: a badge is twenty pixels, and a
//! picture there is noise. Filled rather than outlined, as Lucide drew them at
//! first: a stroke of under a pixel on a coloured tile could not be read at a
//! glance, and a solid shape can. Phosphor's fill weight came between, drawn
//! once on a 256-unit grid for every size.
//!
//! Three are stand-ins for marks Fluent does not draw: `toothbrush` is a tooth
//! (`dentist`), `telescope` a magnifying glass (`search`) and `orbit` a ringed
//! planet (`planet`).
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

/// Every glyph a badge can wear: work, then things, then the rest. Beside
/// each, the Fluent icon it was drawn from, `_filled` left off.
pub(crate) const GLYPHS: [(&str, &str, &[u8]); 47] = glyphs![
    "terminal",       // window_console_20
    "code",           // code_block_16
    "braces",         // braces_16
    "database",       // database_16
    "server",         // server_16
    "cpu",            // developer_board_16
    "bug",            // bug_16
    "git-branch",     // branch_16
    "package",        // box_16
    "layers",         // layer_20
    "cloud",          // cloud_16
    "wrench",         // wrench_16
    "rocket",         // rocket_16
    "flask-conical",  // beaker_16
    "bot",            // bot_16
    "brain",          // brain_16
    "smartphone",     // phone_16
    "monitor",        // desktop_16
    "globe",          // globe_16
    "mail",           // mail_16
    "message-square", // comment_16
    "camera",         // camera_16
    "music",          // music_note_2_16
    "gamepad-2",      // games_16
    "book-open",      // book_open_16
    "graduation-cap", // hat_graduation_16
    "heart",          // heart_16
    "stethoscope",    // stethoscope_20
    "toothbrush",     // dentist_16, a stand-in
    "coins",          // coin_multiple_16
    "chart-line",     // data_trending_16
    "shopping-cart",  // cart_16
    "house",          // home_16
    "briefcase",      // briefcase_16
    "map-pin",        // location_16
    "plane",          // airplane_16
    "leaf",           // leaf_one_16
    "sun",            // weather_sunny_16
    "moon",           // weather_moon_16
    "star",           // star_16
    "zap",            // flash_16
    "flame",          // fire_16
    "telescope",      // search_16, a stand-in
    "orbit",          // planet_16, a stand-in
    "shield",         // shield_16
    "key",            // key_16
    "sparkles",       // sparkle_16
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
    fn every_glyph_is_a_drawable_svg() {
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
