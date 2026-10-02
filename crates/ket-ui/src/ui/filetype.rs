//! The file-type marks the tab strip and the file tree share.
//!
//! A lettermark in a tinted well rather than a logo: two characters taken from
//! the extension, drawn in the code face. It covers every extension ket will
//! ever open instead of the dozen somebody remembered to draw, needs no asset
//! per language, recolours with the theme, and keeps other people's trademarks
//! out of the binary.
//!
//! The colours are a fixed palette like `projects::PROJECT_COLORS` rather than
//! theme tokens: they identify a *file type*, which does not change meaning
//! when the theme does, and eight more tokens in `Theme` for something no
//! theme would sensibly override is a worse trade than a table here.

use super::icon::{Icon, sized_icon};
use crate::paint::on_ground;
use gpui::{Div, Pixels, Rgba, SharedString, div, prelude::*, px, rgb};
use ket_core::theme::Theme;

/// The well a mark sits in.
const SIZE: Pixels = px(18.0);

/// Its corner.
const RADIUS: Pixels = super::RADIUS_SM;

/// The well a tree row's mark sits in, which is smaller because a row is
/// shorter than a tab. The letters keep the tab's size: shrinking them with
/// the well is what makes a two-character mark stop being readable.
const TREE_SIZE: Pixels = px(16.0);

/// Its corner, scaled from [`RADIUS`] so both wells read as the same shape.
const TREE_RADIUS: Pixels = super::RADIUS_SM;

/// How opaque a mark's ground is against its own colour.
///
/// Faint, because the mark is outlined now: a fifth-opacity slab behind a
/// hairline of the same hue reads as two marks stacked.
const GROUND: f32 = 0.08;

/// How opaque its hairline is.
const EDGE: f32 = 0.45;

/// An inactive tab's mark, so the active tab's is the one that carries colour.
const DIMMED: f32 = 0.55;

/// The extensions worth colouring, and what they are drawn as.
///
/// Two characters, because three stops fitting an 18px well at a size anyone
/// can read. Anything not listed falls through to [`neutral`].
const KNOWN: [(&str, &str, u32); 14] = [
    ("rs", "rs", 0xe09a56),
    ("toml", "tm", 0xa99cf7),
    ("md", "md", 0x7fb0e8),
    ("json", "js", 0xe8c15f),
    ("ts", "ts", 0x6f9dff),
    ("tsx", "ts", 0x6f9dff),
    ("js", "js", 0xe8c15f),
    ("jsx", "js", 0xe8c15f),
    ("yml", "yml", 0x7fc98a),
    ("yaml", "yml", 0x7fc98a),
    ("sh", "sh", 0x9aa4ad),
    ("zsh", "sh", 0x9aa4ad),
    ("html", "ht", 0xe09a56),
    ("css", "cs", 0x7fb0e8),
];

/// What an unrecognised file is drawn in.
const NEUTRAL: u32 = 0x9aa4ad;

/// The letters and colour for a file called `name`.
///
/// An unknown extension still gets a mark — its own first two characters, in
/// the neutral colour — because a tab with a blank square beside it looks
/// broken, and "I do not know this type" is a thing the strip can say plainly.
fn for_name(name: &str) -> (SharedString, Rgba) {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default();
    let lower = extension.to_ascii_lowercase();

    for (ext, letters, colour) in KNOWN {
        if lower == ext {
            return (SharedString::from(letters), rgb(colour));
        }
    }

    let letters: String = lower.chars().take(2).collect();
    let shown = if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    };
    (SharedString::from(shown), rgb(NEUTRAL))
}

/// The well every mark shares.
fn well(colour: Rgba, dim: bool) -> Div {
    sized_well(colour, dim, SIZE, RADIUS)
}

/// The same well at a size of the caller's choosing.
fn sized_well(colour: Rgba, dim: bool, size: Pixels, radius: Pixels) -> Div {
    let mut ground = colour;
    ground.a = if dim { GROUND * DIMMED } else { GROUND };
    let mut edge = colour;
    edge.a = if dim { EDGE * DIMMED } else { EDGE };

    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(size)
        .rounded(radius)
        .bg(ground)
        .border_1()
        .border_color(edge)
}

/// The letters a mark draws, in the code face at the one size they fit.
fn letters(mark: Div, text: SharedString, colour: Rgba, mono: SharedString) -> Div {
    mark.font_family(mono)
        .text_size(px(8.5))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(colour)
        .child(text)
}

/// The mark for a file tab, from its label.
pub(crate) fn file_mark(name: &str, dim: bool, mono: SharedString, t: &Theme) -> Div {
    let (text, colour) = for_name(name);
    let mut colour = on_ground(colour, t);
    if dim {
        colour.a = DIMMED;
    }

    letters(well(colour, dim), text, colour, mono)
}

/// The mark for a file row in the tree.
///
/// The same lettermark the tab strip wears, so one file is the same two
/// characters in the same colour wherever it appears — which is the whole
/// reason the tree does not get a mark of its own. `dim` is a file git ignores,
/// and it is thinned the same way an inactive tab's mark is: the type is still
/// legible, it just stops competing with the files that are in the project.
pub(crate) fn tree_mark(name: &str, dim: bool, mono: SharedString, t: &Theme) -> Div {
    let (text, colour) = for_name(name);
    let mut colour = on_ground(colour, t);
    if dim {
        colour.a = DIMMED;
    }
    letters(
        sized_well(colour, dim, TREE_SIZE, TREE_RADIUS),
        text,
        colour,
        mono,
    )
}

/// The mark a directory row wears: a folder outline in the tree's own ink,
/// sized to the well the files beside it use so both columns line up.
pub(crate) fn folder_mark(open: bool, colour: Rgba) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(TREE_SIZE)
        .child(sized_icon(
            if open { Icon::FolderOpen } else { Icon::Folder },
            px(13.0),
            colour,
        ))
}

/// The mark for a tab that is not a file — a terminal or browser.
///
/// The bare glyph, with no well: a glyph is already a shape, and boxing it
/// cost the few pixels that decide whether it reads at a glance. It keeps the
/// well's footprint so a tab's label starts at the same x whichever mark it
/// wears.
pub(crate) fn icon_mark(which: Icon, colour: Rgba, dim: bool) -> Div {
    let mut tint = colour;
    if dim {
        tint.a = DIMMED;
    }
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(SIZE)
        .child(sized_icon(which, px(14.0), tint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_extension_gets_its_own_letters_and_colour() {
        let (letters, colour) = for_name("theme.rs");
        assert_eq!(letters.as_ref(), "rs");
        assert_eq!(colour, rgb(0xe09a56));
    }

    #[test]
    fn the_extension_is_matched_regardless_of_case() {
        assert_eq!(for_name("README.MD").0.as_ref(), "md");
    }

    #[test]
    fn two_extensions_can_share_one_mark() {
        // `.ts` and `.tsx` are the same language wearing two suffixes.
        assert_eq!(for_name("a.ts").0, for_name("a.tsx").0);
        assert_eq!(for_name("a.yml").0, for_name("a.yaml").0);
    }

    #[test]
    fn an_unknown_extension_still_gets_a_mark() {
        // A blank square beside a tab reads as a bug, not as "unrecognised".
        let (letters, colour) = for_name("notes.wxyz");
        assert_eq!(letters.as_ref(), "wx");
        assert_eq!(colour, rgb(NEUTRAL));
    }

    #[test]
    fn a_file_with_no_extension_is_still_marked() {
        assert_eq!(for_name("Makefile").0.as_ref(), "?");
    }

    #[test]
    fn every_known_mark_fits_the_well() {
        for (ext, letters, _) in KNOWN {
            assert!(
                letters.len() <= 3,
                "{ext} draws {letters:?}, which will not fit 18px"
            );
        }
    }
}
