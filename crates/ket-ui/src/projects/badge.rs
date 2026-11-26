//! A project form's badge: the icon and the colour it wears.
//!
//! Both dialogs' appearance column is made of these: the glyph grid, the
//! swatches and the hex field. The drafts they edit are [`ProjectForm`]'s.

use gpui::{App, Context, Div, Pixels, SharedString, Stateful, Window, div, prelude::*, px};
use ket_core::theme::Color;

use super::form::{Field, ProjectForm};
use super::{PROJECT_COLORS, default_color};
use crate::Shell;
use crate::ui::button::icon_button;
use crate::ui::chip::{caption, swatch};
use crate::ui::glyph::{self, Glyph};
use crate::ui::icon::Icon;

/// The gap between swatches: nine 24pt swatches and eight of these fill the
/// 300pt column, so the last in a row ends at the column's edge.
pub(super) const SWATCH_GAP: Pixels = px(10.5);

/// The hex field's width: `#rrggbb` in the mono, and the field's padding.
pub(super) const HEX_W: Pixels = px(96.0);

/// Glyphs to a row: eight cells and their gaps fill the width two rows of
/// nine swatches set.
const GLYPHS_PER_ROW: usize = 8;

/// A glyph cell's side.
const GLYPH_CELL: Pixels = px(30.0);

/// The glyph in a cell: the chrome's icon size, so the grid reads as icons.
const GLYPH_ICON: Pixels = px(16.0);

/// The gap between glyph cells.
const GLYPH_GAP: Pixels = px(6.0);

impl Shell {
    /// The colour as `#rrggbb`, refused in place while it will not parse.
    pub(super) fn hex_field(
        &self,
        form: &ProjectForm,
        colour: Option<Color>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.form_field(form, Field::Hex, colour.is_none(), None, window, cx)
            .w(HEX_W)
            .flex_none()
            .font_family(self.font_family.clone())
    }

    /// The swatches, two rows of nine, the one `colour` matches held.
    pub(super) fn swatch_rows(&self, colour: Option<Color>, cx: &mut Context<Self>) -> Vec<Div> {
        let t = &self.theme;
        PROJECT_COLORS
            .chunks(9)
            .enumerate()
            .map(|(line, chunk)| {
                div()
                    .flex()
                    .gap(SWATCH_GAP)
                    .children(chunk.iter().enumerate().map(|(at, (_, hex))| {
                        let index = line * 9 + at;
                        let each = Color::from_hex(hex).unwrap_or_else(|_| default_color());
                        swatch(("swatch", index), each, colour == Some(each), t).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(form) = this.project_form_mut() {
                                    // The default is the field's placeholder,
                                    // so choosing it empties the field.
                                    let text = if index == 0 { "" } else { *hex };
                                    form.hex.update(cx, |input, _| input.set_text(text));
                                    form.error = None;
                                }
                                cx.notify();
                            }),
                        )
                    }))
            })
            .collect()
    }

    /// The glyphs: letters first — the name's initial, or whatever is typed
    /// in the icon field — then every glyph, the chosen one held down. Text
    /// typed in the icon field outranks the glyphs, so while there is some,
    /// the letters cell is the one held.
    ///
    /// In the icon panel the grid keeps to the glyphs its filter names, and
    /// the letters cell, which has no name to match, steps out while
    /// anything is typed there.
    pub(super) fn glyph_grid(&self, form: &ProjectForm, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let typed = !form.icon.read(cx).text().trim().is_empty();
        let query = form.icon_filter.read(cx).text();
        let query = query.trim();
        let letters = query.is_empty().then_some((Icon::TextCursor, None));
        let choices: Vec<(Icon, Option<Glyph>)> = letters
            .into_iter()
            .chain(matching_glyphs(query).map(|glyph| (Icon::Glyph(glyph), Some(glyph))))
            .collect();
        if choices.is_empty() {
            return div()
                .py(px(6.0))
                .child(caption("No icon goes by that name", t));
        }
        let cells = choices.into_iter().enumerate().map(|(at, (which, glyph))| {
            let chosen = if typed {
                glyph.is_none()
            } else {
                form.glyph == glyph
            };
            icon_button(("glyph-cell", at), which)
                .bare()
                .sized(GLYPH_CELL, GLYPH_ICON)
                .pressed(chosen)
                .render(t)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(form) = this.project_form_mut() {
                        form.pick_glyph(glyph, window, cx);
                    }
                    cx.notify();
                }))
                .into_any_element()
        });
        let mut rows = Vec::new();
        let mut row = Vec::new();
        for cell in cells {
            row.push(cell);
            if row.len() == GLYPHS_PER_ROW {
                rows.push(
                    div()
                        .flex()
                        .gap(GLYPH_GAP)
                        .children(std::mem::take(&mut row)),
                );
            }
        }
        if !row.is_empty() {
            rows.push(div().flex().gap(GLYPH_GAP).children(row));
        }
        div().flex().flex_col().gap(GLYPH_GAP).children(rows)
    }
}

/// A glyph's name as words — `git-branch` is "Git branch" — for the icon
/// trigger to show and the filter to match. A trailing number, which only
/// tells two versions of one drawing apart in the set it came from, is left
/// off: `gamepad-2` is "Gamepad".
pub(super) fn glyph_words(glyph: Glyph) -> String {
    let mut parts: Vec<&str> = glyph.name().split('-').collect();
    if parts.len() > 1
        && parts
            .last()
            .is_some_and(|part| part.chars().all(|c| c.is_ascii_digit()))
    {
        parts.pop();
    }
    let words = parts.join(" ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => words,
    }
}

/// The glyphs whose name matches `query`, in the grid's order, by the
/// palette's matcher — the one the menus filter with, so the icon filter
/// finds what a menu's filter would. Every glyph for an empty query.
pub(super) fn matching_glyphs(query: &str) -> impl Iterator<Item = Glyph> + '_ {
    glyph::every().filter(move |glyph| crate::palette::score(query, &glyph_words(*glyph)).is_some())
}

/// What the colour is called: a swatch's name, "Custom" for any other hex,
/// and "Not a color" for what will not parse.
pub(super) fn colour_name(colour: Option<Color>) -> &'static str {
    match colour {
        Some(colour) => PROJECT_COLORS
            .iter()
            .find(|(_, hex)| Color::from_hex(hex).ok() == Some(colour))
            .map_or("Custom", |(name, _)| *name),
        None => "Not a color",
    }
}

/// The badge's mark: the typed icon, else the glyph, else the first letter of
/// the name, else of the folder's name.
pub(super) fn badge_initial(form: &ProjectForm, cx: &App) -> SharedString {
    let icon = form.icon.read(cx).text();
    if !icon.trim().is_empty() {
        return icon.trim().to_owned().into();
    }
    if let Some(glyph) = form.glyph {
        return glyph.stored().into();
    }
    let name = form.name.read(cx).text();
    let name = match name.trim() {
        "" => form.dir_name.as_ref(),
        typed => typed,
    };
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Glyph {
        glyph::named(&format!("{}{name}", glyph::PREFIX)).expect("the glyph exists")
    }

    #[test]
    fn a_glyph_reads_as_words() {
        assert_eq!(glyph_words(named("rocket")), "Rocket");
        assert_eq!(glyph_words(named("git-branch")), "Git branch");
        // The version number is the icon set's, not a word.
        assert_eq!(glyph_words(named("gamepad-2")), "Gamepad");
    }

    #[test]
    fn the_filter_finds_each_glyph_by_its_words() {
        for glyph in glyph::every() {
            let words = glyph_words(glyph);
            assert!(!words.is_empty(), "{} has no words", glyph.name());
            assert!(
                matching_glyphs(&words.to_lowercase()).any(|found| found == glyph),
                "{words} does not find {}",
                glyph.name()
            );
        }
    }

    #[test]
    fn the_filter_narrows_and_an_empty_one_keeps_everything() {
        assert_eq!(matching_glyphs("").count(), glyph::every().count());
        let rockets: Vec<Glyph> = matching_glyphs("rocket").collect();
        assert_eq!(rockets.first(), Some(&named("rocket")));
        assert!(rockets.len() < glyph::every().count());
        assert_eq!(matching_glyphs("zzzz").count(), 0);
    }

    #[test]
    fn a_colour_is_named_by_its_swatch() {
        let violet = Color::from_hex("#8b5cf6").expect("parses");
        assert_eq!(colour_name(Some(violet)), "Violet");
        let other = Color::from_hex("#123456").expect("parses");
        assert_eq!(colour_name(Some(other)), "Custom");
        assert_eq!(colour_name(None), "Not a color");
    }
}
