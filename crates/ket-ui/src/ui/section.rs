//! The parts a page of settings is made of: a section with a heading and
//! one line on what it is for, a setting's row with its label at the start
//! and its control at the far end, and a label carrying an ⓘ whose tooltip
//! says what the item is for.
//!
//! Project settings is built from these. The Settings window still draws
//! its own, as `preferences::subsection` and `preferences::row`, until it
//! moves onto these.

use gpui::{Div, ElementId, FontWeight, Pixels, SharedString, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::button::icon_button;
use super::chip::caption;
use super::icon::Icon;
use super::tooltip::{Side, tooltip};
use super::{FIELD_H, LABEL};
use crate::paint::paint;

/// Between a heading's title and its line.
const HEADING_GAP: Pixels = px(2.0);

/// Between a section's rows, and between its heading and the first.
const SECTION_GAP: Pixels = px(6.0);

/// Added under a section's heading, so it reads as the rows' heading rather
/// than as another row.
const HEADING_AFTER: Pixels = px(2.0);

/// Between a row's label and the control at its end.
const ROW_GAP: Pixels = px(16.0);

/// Between a label's words and its ⓘ.
const INFO_GAP: Pixels = px(4.0);

/// The ⓘ button: a step under a dense icon button, so it sits in a label's
/// line without making it taller than a line of the label's type.
const INFO: Pixels = px(20.0);

/// The ⓘ's glyph, a step under the chrome's icons.
const INFO_GLYPH: Pixels = px(14.0);

/// A heading: its title, and one sentence on what it is for.
pub(crate) fn heading(
    title: impl Into<SharedString>,
    line: impl Into<SharedString>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(HEADING_GAP)
        .child(
            div()
                .text_size(LABEL)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(paint(t.text.primary))
                .child(title.into()),
        )
        .child(caption(line, t))
}

/// A section: its [`heading`], then the rows the caller adds to it.
pub(crate) fn section(
    title: impl Into<SharedString>,
    line: impl Into<SharedString>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(SECTION_GAP)
        .child(heading(title, line, t).mb(HEADING_AFTER))
}

/// A setting's row: `label` taking the room there is, then what the caller
/// adds after it, its control, at the row's far end. A field's height at the
/// least, so a row holding a switch is as tall as one holding a field.
pub(crate) fn setting_row(label: Div) -> Div {
    div()
        .flex()
        .items_center()
        .gap(ROW_GAP)
        .min_h(FIELD_H)
        .child(label.flex_1().min_w_0())
}

/// An item's label: its words, then `info`, the ⓘ that says what the item
/// is for.
pub(crate) fn info_label(words: impl IntoElement, info: impl IntoElement, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(INFO_GAP)
        .min_h(INFO)
        .child(
            div()
                .text_size(LABEL)
                .text_color(paint(t.text.primary))
                .child(words),
        )
        .child(info)
}

/// The ⓘ, with `words` in a tooltip under it while `shown`.
///
/// Like [`tooltip`], nothing here knows where the pointer is: the caller
/// decides `shown`, and chains `.on_hover(...)` onto what this returns to
/// learn when to change it.
pub(crate) fn info(
    id: impl Into<ElementId>,
    words: impl Into<SharedString>,
    shown: bool,
    t: &Theme,
) -> Stateful<Div> {
    let button = icon_button("info", Icon::Info)
        .bare()
        .dense()
        .sized(INFO, INFO_GLYPH)
        .render(t)
        .into_any_element();
    tooltip(id, button, words, Side::Bottom, shown, t)
}
