//! The overlay picker: the command palette and the file finder.
//!
//! These were two copies of one widget. Both sat 80px from the top, both were
//! 620 wide and capped at 460 tall, both drew the same query line above the
//! same rows, and both carried their own copy of the match highlighter. They
//! differed only in what a row contains, which is the one thing a caller
//! should be supplying.

use gpui::{
    AnyElement, Div, ElementId, FontWeight, Pixels, Rgba, ScrollHandle, SharedString, Stateful,
    div, prelude::*, px,
};
use ket_core::theme::Theme;

use super::chip::{caption, kbd};
use super::icon::{Icon, icon};
use super::{LABEL, PAD_X, RADIUS_LG, RADIUS_SM};
use crate::paint::{paint, scrim};

/// How far below the title bar a picker hangs.
const TOP: Pixels = px(80.0);

/// Wide enough for a command title, its category and its binding.
const WIDTH: Pixels = px(620.0);

/// One result's height. Public because a virtualised list has to be told how
/// tall it will be before it has drawn a row.
pub(crate) const ROW_HEIGHT: Pixels = super::ROW_H;

/// How many results are on screen before the rest are scrolled to.
///
/// This, not a height on the panel, is what bounds a picker. The panel used to
/// carry a `max_h` and let its children overflow into it, which meant the sum
/// of a query line, twelve rows and a hint strip could exceed it — and what
/// `overflow_hidden` then cut off was the strip at the bottom, the one part
/// that says which keys work. Every part states its own height now and the
/// panel is the sum of them.
const VISIBLE_ROWS: f32 = 12.0;

/// Positions a picker over the window, on the same wash a modal gets.
///
/// A picker takes the keyboard the way a dialog does, so it dims what it took
/// it from — the shell behind it was still at full strength, which read as two
/// live surfaces at once. The wash is `Shell::render`'s own, not a second
/// recipe: one dimming over every theme and every surface that steals focus.
///
/// It covers the whole window rather than a strip across the top, so the
/// pointer cannot reach the shell through it. Attach `on_mouse_down` to what
/// comes back to say what a click on the wash does.
pub(crate) fn overlay(child: impl IntoElement) -> Div {
    div()
        .absolute()
        .inset_0()
        .bg(scrim())
        .flex()
        // The panel keeps hanging `TOP` below the title bar; the wash is what
        // grew to fill the window.
        .items_start()
        .justify_center()
        .pt(TOP)
        .child(child)
}

/// The picker's slab. Add the query line and the rows to it.
pub(crate) fn panel(t: &Theme) -> Div {
    panel_w(t, WIDTH)
}

/// [`panel`] at a width of the caller's choosing.
///
/// The overlay pickers are all one width because they are all the same
/// surface in the middle of the window. The sidebar's search is not: it hangs
/// off a field at the window's leading edge and has to stay narrow enough that
/// what it covers is still mostly readable. Same slab, one number apart.
pub(crate) fn panel_w(t: &Theme, width: Pixels) -> Div {
    div()
        // The wash behind it answers clicks; the panel must not let one
        // through, or picking a row would dismiss the picker under the
        // pointer before the row's own handler ran.
        .occlude()
        .flex()
        .flex_col()
        .w(width)
        // No height of its own: it is as tall as what is put in it, and the
        // one part that could grow without limit — the results — stops itself
        // at [`VISIBLE_ROWS`]. See that constant for what this cost before.
        .overflow_hidden()
        .rounded(RADIUS_LG)
        .border_1()
        .border_color(paint(t.border))
        .bg(paint(t.elevated))
        .shadow_lg()
}

/// The line the query is typed on, above the results.
///
/// It holds a field rather than a string now: what is typed here is a real
/// [`crate::input::TextInput`], so the ink is the field's own — dim while it
/// shows its placeholder, and full strength once there is something in it.
///
/// The magnifier is the one thing drawn for the reader rather than typed by
/// them: the line has no box around it, so without a mark there is nothing but
/// a caret to say the panel is asking a question.
pub(crate) fn query_line(field: impl IntoElement, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.0))
        .h(px(48.0))
        .px(PAD_X)
        .border_b_1()
        .border_color(paint(t.border))
        .text_size(px(13.0))
        .child(icon(Icon::Search, paint(t.text.dim)))
        .child(div().flex_1().min_w_0().child(field))
}

/// One result. Attach `on_click` to what comes back.
///
/// The selected fill is a rounded pill inset from the panel's edge rather than
/// a band running into it, which is what every row in the shell wears — see
/// `ui::row`.
pub(crate) fn row(id: impl Into<ElementId>, selected: bool, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(9.0))
        .w_full()
        .h(ROW_HEIGHT)
        .px(px(10.0))
        .rounded(RADIUS_SM)
        .text_size(LABEL)
        .cursor_pointer()
        .when(selected, |el| el.bg(paint(t.selection)))
        .hover(|style| style.bg(paint(t.hover)))
}

/// How tall a virtualised list of `count` results should be drawn.
///
/// Stated rather than inferred: a `uniform_list` fills the space it is given,
/// and the space here comes from a panel that is only *capped* at
/// [`MAX_HEIGHT`] — it has no height of its own for the list to take a share
/// of. So the list says how tall it is, from the one row height everything
/// here agrees on, and stops at [`VISIBLE_ROWS`].
pub(crate) fn list_height(count: usize) -> Pixels {
    let wanted = ROW_HEIGHT * (count as f32);
    let most = ROW_HEIGHT * VISIBLE_ROWS;
    if wanted > most { most } else { wanted }
}

/// The strip along the bottom saying what the keys do.
///
/// `items` is `(keys, what it does)`, drawn in the order given. A picker is
/// driven from the keyboard, and the two keys that matter are not discoverable
/// from a list of file names.
pub(crate) fn hints(items: &[(&'static str, &'static str)], mono: SharedString, t: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(px(12.0))
        // Stated, so the panel above it can be the sum of its parts.
        .h(px(34.0))
        .px(PAD_X)
        .border_t_1()
        .border_color(paint(t.border))
        .children(items.iter().map(|(keys, label)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(kbd(*keys, mono.clone(), t))
                .child(caption(*label, t))
        }))
}

/// The results, which scroll.
///
/// Two things are load-bearing and neither is obvious. `min_h_0`, because a
/// flex child will not shrink below its content: without it the list makes the
/// panel taller than its own `max_h` instead of scrolling inside it, which is
/// exactly how a ranked list of fifty ran off the bottom of the window with no
/// way to reach the rest. And `track_scroll`, so the caller can pull the
/// selected row back into view — a list that scrolls with the wheel but not
/// with the arrow keys loses the selection the moment you hold one down.
pub(crate) fn results(id: impl Into<ElementId>, scroll: &ScrollHandle) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .min_h_0()
        .max_h(ROW_HEIGHT * VISIBLE_ROWS)
        .p(px(6.0))
        .overflow_y_scroll()
        .track_scroll(scroll)
}

/// The heading above one group of results.
///
/// Quieter than the sidebar's own "Projects" — it is a label on a list you are
/// already reading, not the name of a region you are navigating to — and in
/// title case rather than small caps, because `gpui` has no letter-spacing and
/// unspaced capitals at 11px read as a word with the air squeezed out of it.
pub(crate) fn group_header(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex_none()
        .px(px(10.0))
        .pt(px(10.0))
        .pb(px(4.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(paint(t.text.dim))
        .child(text.into())
}

/// A footnote under the results — what the list could not show.
pub(crate) fn note(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex_none()
        .px(PAD_X)
        .py(px(7.0))
        .border_t_1()
        .border_color(paint(t.border))
        .text_size(super::CAPTION)
        .text_color(paint(t.text.dim))
        .child(text.into())
}

/// Renders `text` with the characters at `hits` lit.
///
/// A fuzzy matcher that will not show you *why* something matched is guessing
/// at you; the fix is to show the evidence rather than to explain the
/// algorithm.
///
/// One span per *run* rather than per character. This was a span per
/// character, which is a separate text layout for every letter on screen: a
/// scrolling list of paths cost well over a thousand of them a frame, and it
/// was visible as stutter. A fuzzy match is a handful of runs, and an empty
/// query — what a picker shows before anything is typed — is exactly one.
pub(crate) fn lit(text: &str, hits: &[usize], t: &Theme) -> AnyElement {
    lit_in(text, hits, paint(t.text.primary), t)
}

/// [`lit`] over ink of the caller's choosing.
///
/// A row's dim second line lights its matches too — a worktree found by its
/// *project's* name has nothing lit in its own, and the evidence has to be
/// somewhere — but it must stay the dim line while it does, or every context
/// that happened to match would read as loudly as the names above it.
pub(crate) fn lit_in(text: &str, hits: &[usize], plain: Rgba, t: &Theme) -> AnyElement {
    let matched = paint(t.accent);

    let mut runs: Vec<(String, bool)> = Vec::new();
    for (index, character) in text.chars().enumerate() {
        let on = hits.contains(&index);
        match runs.last_mut() {
            Some((run, lit)) if *lit == on => run.push(character),
            _ => runs.push((character.to_string(), on)),
        }
    }

    div()
        .flex()
        .children(runs.into_iter().map(|(run, on)| {
            div()
                .text_color(if on { matched } else { plain })
                .child(run)
        }))
        .into_any_element()
}
