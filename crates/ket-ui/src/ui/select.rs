//! A select: a field-shaped trigger for a value picked from a menu.
//!
//! `ui::menu::dropdown` places the panel and `ui::menu` draws it; this is the
//! part a person presses. Every picker in the shell — an agent, a theme, a
//! level, a project — used to draw its own box with a chevron beside the
//! value, at three different heights. This is that box once: the same well a
//! text field is, so a form of fields and selects lines up, with the value on
//! the left and the chevron on the right that flips while the menu is open.
//!
//! Like every recipe here it holds no state: the caller says whether its menu
//! is open and wraps the result in `dropdown` with the menu's view.

use gpui::{AnyElement, ElementId, Pixels, SharedString, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::RADIUS_SM;
use super::field::{field, placeholder};
use super::icon::{Icon, icon, sized_icon};
use crate::paint::paint;

/// An inline trigger's height: the style guide's small button.
const INLINE_H: Pixels = px(26.0);

/// An inline trigger's chevron, a step under its value.
const INLINE_CHEVRON: Pixels = px(12.0);

/// A select's trigger.
pub(crate) struct Select {
    id: ElementId,
    value: SharedString,
    placeholder: SharedString,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
    open: bool,
    focused: bool,
    mono: Option<SharedString>,
    inline: bool,
}

/// A trigger showing `value`.
pub(crate) fn select(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Select {
    Select {
        id: id.into(),
        value: value.into(),
        placeholder: SharedString::default(),
        leading: None,
        trailing: None,
        open: false,
        focused: false,
        mono: None,
        inline: false,
    }
}

impl Select {
    /// What it says when nothing is chosen.
    pub(crate) fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// A mark before the value — an agent's logo, a level's glyph.
    pub(crate) fn leading(mut self, mark: impl IntoElement) -> Self {
        self.leading = Some(mark.into_any_element());
        self
    }

    /// A note after the value, before the chevron — what the choice will
    /// actually run, say, when that is not what its name suggests.
    pub(crate) fn trailing(mut self, note: impl IntoElement) -> Self {
        self.trailing = Some(note.into_any_element());
        self
    }

    /// Whether its menu is showing: the ring is on and the chevron points up.
    pub(crate) fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Whether it holds the keyboard, for a form walked with Tab.
    pub(crate) fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Sets the value in `family` — a branch, a ref, anything quoted — rather
    /// than the prose a field takes.
    pub(crate) fn mono(mut self, family: SharedString) -> Self {
        self.mono = Some(family);
        self
    }

    /// The small trigger that sits in a sentence — "Send to [ket ▾] in
    /// [main ▾]" — rather than in a form: 26 tall, a chip's corner, an edge
    /// instead of a well, and as wide as its value. Give it a 14px mark.
    pub(crate) fn inline(mut self) -> Self {
        self.inline = true;
        self
    }

    /// The trigger. Size it with `.w(..)` at the call site; it fills its
    /// parent otherwise, the way a field does. An [`inline`](Self::inline)
    /// one hugs its value instead.
    pub(crate) fn render(self, t: &Theme) -> Stateful<gpui::Div> {
        if self.inline {
            return self.render_inline(t);
        }
        let empty = self.value.is_empty();
        let shown = if empty {
            self.placeholder.clone()
        } else {
            self.value.clone()
        };
        let body = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(super::ICON_GAP)
            .children(self.leading)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when_some(self.mono, |el, family| el.font_family(family))
                    .text_color(if empty {
                        placeholder(t)
                    } else {
                        paint(t.text.primary)
                    })
                    .child(shown),
            )
            .children(self.trailing)
            .child(icon(
                if self.open {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                },
                paint(t.text.dim),
            ))
            .into_any_element();
        field(self.id, "", "")
            .focused(self.open || self.focused)
            .body(body)
            .render(t)
            .cursor_pointer()
    }

    fn render_inline(self, t: &Theme) -> Stateful<gpui::Div> {
        let empty = self.value.is_empty();
        let lit = self.open || self.focused;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .h(INLINE_H)
            .pl(px(8.0))
            .pr(px(7.0))
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(paint(if lit { t.focus_ring } else { t.border }))
            .cursor_pointer()
            .hover(|style| style.bg(paint(t.hover)))
            .text_size(px(12.5))
            .children(self.leading)
            .child(
                div()
                    .whitespace_nowrap()
                    .when_some(self.mono, |el, family| {
                        el.font_family(family).text_size(px(12.0))
                    })
                    .text_color(if empty {
                        placeholder(t)
                    } else {
                        paint(t.text.primary)
                    })
                    .child(if empty { self.placeholder } else { self.value }),
            )
            .children(self.trailing)
            .child(sized_icon(
                if self.open {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                },
                INLINE_CHEVRON,
                paint(t.text.dim),
            ))
    }
}
