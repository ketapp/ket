//! Text fields and the labels above them.
//!
//! There were three field recipes, at three heights and two radii, and they
//! disagreed about the accent border: the create-worktree field wore it
//! permanently, so a dialog opened looking as though its name box already had
//! the keyboard. Focus is a state, not a decoration, and now only `focused`
//! draws the ring.
//!
//! This is the *box*: the well, the border, the ring and whatever sits in the
//! row. What goes inside it — the text, the caret, the selection — belongs to
//! [`crate::input`], which is where every editable field in the shell now gets
//! it from. This module drew a caret of its own once, pinned after the last
//! character because nothing here knew where any character was; it went when
//! the last field still relying on it grew a real editing model.

use gpui::{AnyElement, Div, ElementId, SharedString, Stateful, div, prelude::*, px};
use ket_core::theme::Theme;

use super::icon::{Icon, icon};
use super::{CAPTION, FIELD_H, FIELD_TEXT, ICON_GAP, PAD_X, RADIUS_MD};
use crate::fonts::Prose;
use crate::paint::paint;

/// The placeholder's colour: `text.dim`, the same ink the sidebar's query
/// line has always used for its hint.
///
/// It was thinned to under half strength here for a while, on the theory
/// that a hint should sit under every other de-emphasised ink in the shell.
/// What that produced was two placeholders a few pixels apart in two greys —
/// the explorer's filter faint, the search field above it not — and no
/// reading of either as anything but a mistake. One ink, named once.
pub(crate) fn placeholder(t: &Theme) -> gpui::Rgba {
    paint(t.text.dim)
}

/// The border a pointer draws on an idle field.
///
/// A step up from `border`, not all the way to `text.dim`: hover is the
/// quietest state a control has, and a hairline as bright as the label above
/// it reads as focus.
fn hover_border(t: &Theme) -> gpui::Rgba {
    let mut colour = paint(t.text.dim);
    colour.a = 0.28;
    colour
}

/// The caption above a field.
pub(crate) fn label(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .prose()
        .mb(px(7.0))
        .text_size(CAPTION)
        .text_color(paint(t.text.dim))
        .child(text.into())
}

/// A single-line text field.
pub(crate) struct Field {
    id: ElementId,
    value: SharedString,
    placeholder: SharedString,
    focused: bool,
    invalid: bool,
    editable: bool,
    leading: Option<Icon>,
    body: Option<AnyElement>,
    compact: bool,
}

/// A compact field's height: an inline edit inside a strip or a row, where a
/// full field would be taller than the thing being renamed.
const COMPACT_H: gpui::Pixels = px(22.0);

/// A field showing `value`, or `placeholder` when it is empty.
pub(crate) fn field(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    placeholder: impl Into<SharedString>,
) -> Field {
    Field {
        id: id.into(),
        value: value.into(),
        placeholder: placeholder.into(),
        focused: false,
        invalid: false,
        editable: true,
        leading: None,
        body: None,
        compact: false,
    }
}

impl Field {
    /// Whether the keyboard is in this field. Draws the ring.
    pub(crate) fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether what is in it was refused.
    pub(crate) fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// An icon before the text, for a field whose job is not obvious from its
    /// placeholder alone.
    pub(crate) fn leading(mut self, which: Icon) -> Self {
        self.leading = Some(which);
        self
    }

    /// A field that only *shows* a value — a project name a dialog already
    /// knows, a picker's current choice. No caret, no text cursor.
    pub(crate) fn read_only(mut self) -> Self {
        self.editable = false;
        self
    }

    /// Text laid out by the caller, replacing the value this would otherwise
    /// draw.
    ///
    /// A field with a real editing model behind it — see `input::TextInput` —
    /// knows where its caret goes and what is selected, and this cannot.
    /// `value` is still worth passing, because it is what decides whether the
    /// field reads as empty.
    /// The inline size: 22px, a tighter inset and corner, for renaming a tab
    /// or a row in place.
    pub(crate) fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub(crate) fn body(mut self, body: AnyElement) -> Self {
        self.body = Some(body);
        self
    }

    /// Builds the element. Attach `on_click` to what comes back.
    pub(crate) fn render(self, t: &Theme) -> Stateful<Div> {
        let empty = self.value.is_empty();
        let shown = if empty {
            self.placeholder.clone()
        } else {
            self.value.clone()
        };
        let border = if self.invalid {
            paint(t.status.failed)
        } else if self.focused {
            paint(t.focus_ring)
        } else {
            // A recessed well with no edge of its own at rest: the fill is
            // what says "type here", and the border is kept for the pointer,
            // the focus and a refusal to draw on.
            paint(t.sunken)
        };
        div()
            .id(self.id)
            // What is typed into a field is words more often than not, and
            // the placeholder beside it always is. Every text input reaches
            // the shell through this well, so the face is set once here.
            .prose()
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .h(if self.compact { COMPACT_H } else { FIELD_H })
            .px(if self.compact { px(5.0) } else { PAD_X })
            .rounded(if self.compact {
                super::RADIUS_SM
            } else {
                RADIUS_MD
            })
            .bg(paint(t.sunken))
            .border_1()
            .border_color(border)
            .text_size(FIELD_TEXT)
            .text_color(if empty {
                placeholder(t)
            } else {
                paint(t.text.primary)
            })
            // Editable and idle: the border answers the pointer, the way every
            // other control in the shell does.
            .when(self.editable && !self.focused && !self.invalid, |el| {
                el.hover(|el| el.border_color(hover_border(t)))
            })
            .when(self.editable, |el| el.cursor_text())
            .overflow_hidden()
            .children(self.leading.map(|which| icon(which, paint(t.text.dim))))
            .child(match self.body {
                Some(body) => body,
                None => shown.into_any_element(),
            })
    }
}

/// The message under a field that was refused.
pub(crate) fn error(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .prose()
        .mt(px(7.0))
        .text_size(CAPTION)
        .text_color(paint(t.status.failed))
        .child(text.into())
}
