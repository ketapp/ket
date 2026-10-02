//! A textarea: the well a multi-line draft is written in.
//!
//! The text itself is an editor composer — `Shell::editor_pane` over a key
//! opened with `open_composer` — because the editor is the one place in the
//! shell that already wraps, scrolls and takes a caret across lines. What
//! this owns is everything around it that the quick prompt and the snippet
//! body each drew for themselves: the well, the ring while it is being
//! written in, the placeholder over an empty draft, and a footer row inside
//! the well for whatever shapes the send — the style guide's "count, send
//! chord and action", the microphone among them.

use gpui::{
    AnyElement, Div, ElementId, MouseButton, Pixels, SharedString, Stateful, div, prelude::*, px,
    relative,
};
use ket_core::theme::Theme;

use super::RADIUS_MD;
use super::field::placeholder;
use crate::fonts::Prose;
use crate::paint::paint;

/// How far the draft stands in from the well's top and left edges.
const PAD_TOP: Pixels = px(12.0);
const PAD_LEFT: Pixels = px(8.0);

/// The composer's own gutter before its first character, so a placeholder
/// sits exactly where typed words will start.
const COMPOSER_GUTTER: Pixels = px(12.0);

/// The footer row's padding inside the well.
const FOOTER_PAD: Pixels = px(10.0);

/// Between an aside and the draft beside it.
const ASIDE_GAP: Pixels = px(8.0);

/// A textarea around a composer.
pub(crate) struct TextArea {
    id: ElementId,
    body: AnyElement,
    height: Pixels,
    active: bool,
    placeholder: Option<(SharedString, Pixels)>,
    footer: Option<AnyElement>,
    aside: Option<(AnyElement, Pixels)>,
}

/// A well of `height` holding `body`, the composer's pane.
pub(crate) fn textarea(id: impl Into<ElementId>, body: AnyElement, height: Pixels) -> TextArea {
    TextArea {
        id: id.into(),
        body,
        height,
        active: false,
        placeholder: None,
        footer: None,
        aside: None,
    }
}

impl TextArea {
    /// Whether the draft has the keyboard: the edge takes the focus ring.
    pub(crate) fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Words drawn over an empty draft, at the composer's `size` so they
    /// sit where typed ones will. Pass it only while the draft is empty.
    pub(crate) fn placeholder(mut self, text: impl Into<SharedString>, size: Pixels) -> Self {
        self.placeholder = Some((text.into(), size));
        self
    }

    /// A row along the bottom of the well, under a hairline: the controls
    /// that shape what the draft does when sent, and the send itself. The
    /// caller lays the row out; the draft gives up the height it takes.
    /// Presses on it do not reach the draft, which would move the caret.
    pub(crate) fn footer(mut self, row: impl IntoElement) -> Self {
        self.footer = Some(row.into_any_element());
        self
    }

    /// A column of `width` before the draft, inside the well: what the draft
    /// is about, such as the pictures sent with it. The placeholder moves
    /// over with the draft.
    pub(crate) fn aside(mut self, column: impl IntoElement, width: Pixels) -> Self {
        self.aside = Some((column.into_any_element(), width));
        self
    }

    pub(crate) fn render(self, t: &Theme) -> Stateful<Div> {
        let aside_w = self
            .aside
            .as_ref()
            .map_or(px(0.0), |(_, width)| *width + ASIDE_GAP);
        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_col()
            .h(self.height)
            .overflow_hidden()
            .rounded(RADIUS_MD)
            .bg(paint(t.sunken))
            .border_1()
            .border_color(paint(if self.active { t.focus_ring } else { t.sunken }))
            .when(!self.active, |el| {
                el.hover(|style| style.border_color(crate::paint::alpha(paint(t.text.dim), 0.28)))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .pt(PAD_TOP)
                    .pl(PAD_LEFT)
                    .children(self.aside.map(|(column, width)| {
                        div()
                            .flex_none()
                            .w(width)
                            .ml(px(4.0))
                            .mr(ASIDE_GAP - px(4.0))
                            .child(column)
                    }))
                    .child(div().flex_1().min_w_0().h_full().child(self.body)),
            )
            .children(self.footer.map(|row| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .p(FOOTER_PAD)
                    .border_t_1()
                    .border_color(crate::paint::alpha(paint(t.border), 0.6))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(row)
            }))
            // Drawn over the empty draft rather than in it, so the caret
            // stays at the start of the line with the words after it.
            .children(self.placeholder.map(|(text, size)| {
                div()
                    .absolute()
                    .top(PAD_TOP)
                    .left(PAD_LEFT + aside_w + COMPOSER_GUTTER)
                    .prose()
                    .text_size(size)
                    .line_height(relative(1.55))
                    .text_color(placeholder(t))
                    .child(text)
            }))
    }
}
