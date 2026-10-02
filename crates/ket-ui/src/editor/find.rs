//! Find: the bar, its query field, and stepping through the matches.

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, Keystroke, SharedString, Window, div, prelude::*,
};

use ket_core::buffer::{Case, FindMatches};

use crate::Shell;
use crate::input::{Style, TextInput, is_text, text_line};
use crate::paint::paint;
use crate::ui::button::icon_button;
use crate::ui::icon::Icon;

/// Find-in-file state for one editor tab. Case-insensitive with no toggle —
/// see the `editor` module doc's "Known simplifications" section.
pub(super) struct Find {
    /// Whether the find bar is showing and consuming keys.
    pub(super) open: bool,
    /// What has been typed.
    ///
    /// Built when the bar opens rather than with the editor state, because a
    /// text field is an entity and building one needs the app context — which
    /// loading a file into a buffer does not have. `None` is a bar that has
    /// never been opened, and reads as an empty query.
    pub(super) query: Option<Entity<TextInput>>,
    /// The current query's matches, refreshed on every query change.
    pub(super) matches: FindMatches,
}

impl Find {
    /// What is in the field, or nothing when the bar has never been opened.
    pub(super) fn text(&self, cx: &App) -> String {
        self.query
            .as_ref()
            .map(|input| input.read(cx).text())
            .unwrap_or_default()
    }
}

impl Default for Find {
    fn default() -> Self {
        Self {
            open: false,
            query: None,
            matches: FindMatches {
                ranges: Vec::new(),
                total: 0,
                truncated: false,
            },
        }
    }
}

impl Shell {
    /// Whether any editor's find bar is open, and so holding the keyboard.
    ///
    /// Asked by `Shell::typing`, which is what keeps the window from taking
    /// the focus back out from under the field.
    pub(crate) fn find_open(&self) -> bool {
        self.editors.values().any(|state| state.find.open)
    }

    /// Opens the find bar on a fresh, focused query field.
    ///
    /// A new field each time rather than a cleared one, because the field is
    /// what the search is re-run from: watching it is how a typed character
    /// reaches [`Shell::refresh_find`] at all, and the character does not
    /// arrive through the key handler — it comes back later from macOS's
    /// input context. See [`crate::input`].
    pub(super) fn open_find(&mut self, editor_key: &str, cx: &mut Context<Self>) {
        // What is selected is what you are about to search for — every editor
        // works this way, and typing the word out again when it is already
        // highlighted is the sort of small tax that makes a find bar feel
        // hand-built. A selection crossing a line break is left alone: the
        // field is one line, and a query with a newline in it is not what
        // anyone selecting a paragraph meant to ask for.
        let seed = self
            .editors
            .get(editor_key)
            .filter(|state| state.buffer.has_selection())
            .map(|state| state.buffer.selected_text())
            .filter(|text| !text.is_empty() && !text.contains('\n'));

        let input = match &seed {
            // Selected, like any field opened onto a value that is there to be
            // replaced: the next thing typed searches for that instead of
            // appending to the word already in the box.
            Some(text) => TextInput::with_text("Find\u{2026}", text, cx),
            None => TextInput::new("Find\u{2026}", cx),
        };
        input.update(cx, |input, _| input.request_focus());
        let key: SharedString = editor_key.to_owned().into();
        cx.observe(&input, move |shell, _, cx| {
            shell.refresh_find(key.as_ref(), true, cx);
            cx.notify();
        })
        .detach();
        if let Some(state) = self.editors.get_mut(editor_key) {
            state.find.open = true;
            state.find.query = Some(input);
        }

        if seed.is_some() {
            // Run it now: the bar opens with its matches counted and lit,
            // which is the whole of what "search for what I have selected"
            // means. Without a jump — the selection *is* the current match,
            // and jumping from the cursor would step straight past it to the
            // next one, moving the view away from the word just pointed at.
            self.refresh_find(editor_key, false, cx);
        }
    }

    /// Closes and clears one editor's find session.
    fn close_find(&mut self, key: &str) {
        if let Some(state) = self.editors.get_mut(key) {
            state.find.open = false;
            state.find.query = None;
            state.find.matches = FindMatches {
                ranges: Vec::new(),
                total: 0,
                truncated: false,
            };
        }
    }

    /// Keys while the find bar is open: the field takes its own editing
    /// chords, Enter/Shift-Enter step forward and back, Escape closes.
    pub(super) fn handle_find_key(
        &mut self,
        key: &str,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) {
        let input = self
            .editors
            .get(key)
            .and_then(|state| state.find.query.clone());

        if let Some(input) = input {
            // Text is never taken: it has to keep travelling until macOS's
            // input context sees it — see [`crate::input`].
            if is_text(keystroke) {
                return;
            }
            if input.update(cx, |input, cx| input.key(keystroke, cx)) {
                return;
            }
        }

        match keystroke.key.as_str() {
            "escape" => self.close_find(key),
            "enter" => self.find_step(key, keystroke.modifiers.shift, cx),
            _ => {}
        }
    }

    /// Re-runs the search and, on `jump`, moves to the first match at or
    /// after the cursor — "type to search" jumping as you type.
    pub(super) fn refresh_find(&mut self, key: &str, jump: bool, cx: &App) {
        let query = self
            .editors
            .get(key)
            .map(|state| state.find.text(cx))
            .unwrap_or_default();
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        state.find.matches = state.buffer.find_all(&query, Case::Insensitive);
        if jump
            && !query.is_empty()
            && let Some(range) =
                state
                    .buffer
                    .find_next(&query, Case::Insensitive, state.buffer.cursor())
        {
            state.buffer.set_selection(range.start, range.end);
        }
        self.ensure_cursor_visible(key);
    }

    /// Moves to the next (or, with `backward`, previous) match from the
    /// current selection.
    fn find_step(&mut self, key: &str, backward: bool, cx: &App) {
        let query = self
            .editors
            .get(key)
            .map(|state| state.find.text(cx))
            .unwrap_or_default();
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        if query.is_empty() {
            return;
        }
        let range = if backward {
            state
                .buffer
                .find_previous(&query, Case::Insensitive, state.buffer.selection().start)
        } else {
            state
                .buffer
                .find_next(&query, Case::Insensitive, state.buffer.selection().end)
        };
        if let Some(range) = range {
            state.buffer.set_selection(range.start, range.end);
        }
        self.ensure_cursor_visible(key);
    }

    /// The find bar, or nothing when find is closed.
    pub(super) fn find_bar(
        &self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = self.theme;
        let state = self.editors.get(key)?;
        if !state.find.open {
            return None;
        }
        let count = state.find.matches.total;
        let query = state.find.query.as_ref()?;
        let editor_key: SharedString = key.to_owned().into();
        let close_id: SharedString = format!("editor-find-close:{key}").into();

        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .bg(paint(t.panel))
                .border_b_1()
                .border_color(paint(t.selection))
                .text_color(paint(t.text.primary))
                .child(text_line(
                    query,
                    SharedString::from(format!("editor-find:{key}")),
                    Style::new(&t, self.caret.visible),
                    window,
                    cx,
                ))
                .child(div().flex_grow())
                .child(div().text_xs().text_color(paint(t.text.dim)).child(format!(
                    "{count} match{}",
                    if count == 1 { "" } else { "es" }
                )))
                .child(
                    icon_button(close_id, Icon::Close)
                        .bare()
                        .small()
                        .render(&t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_find(editor_key.as_ref());
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}
