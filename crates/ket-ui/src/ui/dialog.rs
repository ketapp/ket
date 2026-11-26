//! The modal scaffold.
//!
//! Four dialogs built this by hand, in two houses that contradicted each
//! other: two were `rounded_lg` with a `selection` border on `surface`, two
//! were `rounded_md` with a `focus_ring` border on `panel`. All four also
//! re-declared the full-window absolute wrapper that `Shell::render` already
//! puts them inside, which is why the scrim was drawn twice.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Pixels, Rgba, SharedString, Stateful,
    Window, div, prelude::*, px,
};
use ket_core::theme::Theme;

use super::button::icon_button;
use super::icon::{Icon, icon};
use super::{CARD_PAD, RADIUS_LG, TITLE};
use crate::fonts::Prose;
use crate::paint::paint;

/// Centres a dialog in the window.
///
/// The scrim is not here: `Shell::render` washes everything behind whichever
/// modal is up, once, so a dialog never dims the window a second time.
pub(crate) fn centered(child: impl IntoElement) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(child)
}

/// The dialog's slab. Add the header, the body and the footer to it.
///
/// Set in the prose face, which everything inside inherits: a dialog is
/// sentences and choices to read, not a column of identifiers to compare. A
/// part that must stay monospace asks for it itself.
pub(crate) fn card(id: impl Into<ElementId>, width: Pixels, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .prose()
        .occlude()
        .flex()
        .flex_col()
        .w(width)
        .p(CARD_PAD)
        .gap(px(15.0))
        .rounded(RADIUS_LG)
        .border_1()
        .border_color(paint(t.border))
        .bg(paint(t.elevated))
        .shadow_lg()
}

/// The ✕ that closes a dialog, at the far end of its title row. Attach what
/// closing does to what comes back — the same as Cancel, and as Escape.
///
/// [`header`] draws it. A dialog whose heading is its own — a badge and a
/// count beside the name — sets this at that heading's end, so every dialog
/// is closed by the same mark in the same place.
pub(crate) fn close(id: impl Into<ElementId>, t: &Theme) -> Stateful<Div> {
    close_mark(id, true, t)
}

/// The ✕ itself, faded when it cannot be pressed.
fn close_mark(id: impl Into<ElementId>, enabled: bool, t: &Theme) -> Stateful<Div> {
    icon_button(id, Icon::Close)
        .circle()
        .enabled(enabled)
        .render(t)
}

/// A dialog's title, in the title's size and weight, with nothing else in
/// its row. For a pane that wears a dialog's look but cannot be closed — a
/// simulator's message about what it needs — where [`header`] would offer a
/// ✕ that has nowhere to go.
pub(crate) fn title(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(TITLE)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(paint(t.text.primary))
        .child(text.into())
}

/// What a dialog's ✕ does.
type OnClose = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A dialog's title row: its title, and at the far end the [`close`] ✕.
///
/// Every dialog has one, so a dialog is never closed only from its footer
/// or by a key the person has to know. Build it with [`header`].
pub(crate) struct Header {
    id: ElementId,
    title: SharedString,
    mark: Option<(Icon, Rgba)>,
    after: Option<AnyElement>,
    aside: Option<AnyElement>,
    on_close: Option<OnClose>,
    busy: bool,
}

/// A dialog's title row, `title` at its start and the ✕ at its end. `id`
/// names the ✕. Say what closing does with [`Header::on_close`].
pub(crate) fn header(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Header {
    Header {
        id: id.into(),
        title: title.into(),
        mark: None,
        after: None,
        aside: None,
        on_close: None,
        busy: false,
    }
}

impl Header {
    /// A mark leading the title, in `colour`: what kind of thing the dialog
    /// is asking about, for a question asked unbidden as a session is
    /// opened — see `crate::session_asks`. The dim ink of a label for a fact
    /// to weigh, a state's colour for a state.
    pub(crate) fn mark(mut self, which: Icon, colour: Rgba) -> Self {
        self.mark = Some((which, colour));
        self
    }

    /// Something set straight after the title rather than out at the far
    /// end — the project a worktree is being created in, so the title reads
    /// as one sentence. It takes the title's size and weight.
    pub(crate) fn after(mut self, after: impl IntoElement) -> Self {
        self.after = Some(after.into_any_element());
        self
    }

    /// Something at the far end, just before the ✕: what the dialog is
    /// about, when the title alone does not say — a project's folder.
    pub(crate) fn aside(mut self, aside: impl IntoElement) -> Self {
        self.aside = Some(aside.into_any_element());
        self
    }

    /// What the ✕ does: whatever Cancel does.
    pub(crate) fn on_close(
        mut self,
        close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Box::new(close));
        self
    }

    /// Held still while the dialog is doing something it cannot leave half
    /// done — a merge, a push. The ✕ is drawn faded and does nothing, as the
    /// footer's buttons are.
    pub(crate) fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Builds the row.
    pub(crate) fn render(self, t: &Theme) -> Div {
        let enabled = !self.busy && self.on_close.is_some();
        let mut close = close_mark(self.id, enabled, t);
        if let (true, Some(on_close)) = (enabled, self.on_close) {
            close = close.on_click(move |event, window, cx| on_close(event, window, cx));
        }
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .children(
                self.mark
                    .map(|(which, colour)| icon(which, colour).size(px(16.0)).flex_none()),
            )
            .child(title(self.title, t).min_w_0().children(self.after))
            .child(div().flex_grow())
            .children(self.aside)
            .child(close)
    }
}

/// The row a dialog's buttons sit in: trailing-aligned, with the cancel first
/// and the action last, which is the order a Mac reads them in.
pub(crate) fn footer() -> Div {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(10.0))
        .pt(px(6.0))
}

/// A quiet block explaining what the dialog is about to do.
pub(crate) fn body(text: impl Into<SharedString>, t: &Theme) -> AnyElement {
    div()
        .text_size(super::LABEL)
        .text_color(paint(t.text.dim))
        .child(text.into())
        .into_any_element()
}
