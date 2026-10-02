//! The editor as a composer: the quick prompt's draft and the other
//! dialogs that borrow the pane to take a paragraph of prose.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Keystroke, Pixels, Window};

use ket_core::buffer::Buffer;

use crate::Shell;

use super::QUICK_PROMPT_EDITOR;
use super::state::EditorState;

impl Shell {
    pub(crate) fn open_quick_prompt_editor(&mut self) {
        self.open_composer(QUICK_PROMPT_EDITOR, "");
    }

    pub(crate) fn close_quick_prompt_editor(&mut self) {
        self.close_composer(QUICK_PROMPT_EDITOR);
    }

    pub(crate) fn quick_prompt_text(&self) -> String {
        self.composer_text(QUICK_PROMPT_EDITOR)
    }

    pub(crate) fn quick_prompt_editor_key(
        &mut self,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) {
        self.composer_key(QUICK_PROMPT_EDITOR, keystroke, cx);
    }

    /// Opens a composer — the editor pane set in prose and wrapped, the box a
    /// prompt is written in — under `key`, holding `text` with the caret at
    /// its end.
    pub(crate) fn open_composer(&mut self, key: &str, text: &str) {
        let mut state = EditorState::empty(PathBuf::new());
        state.wrap = true;
        state.composer = true;
        if !text.is_empty() {
            state.buffer = Buffer::from_text(text);
            let end = state.buffer.len_chars();
            state.buffer.set_cursor(end);
        }
        self.editors.insert(key.to_owned().into(), state);
    }

    /// Sets the composer under `key` at `size` rather than [`COMPOSER_TEXT`]:
    /// for a draft that shares its surface with other controls instead of
    /// being the one thing in its dialog.
    ///
    /// [`COMPOSER_TEXT`]: super::COMPOSER_TEXT
    pub(crate) fn set_composer_size(&mut self, key: &str, size: Pixels) {
        if let Some(state) = self.editors.get_mut(key) {
            state.composer_size = size;
        }
    }

    /// Whether the composer under `key` draws its caret.
    pub(crate) fn set_composer_caret(&mut self, key: &str, shown: bool) {
        if let Some(state) = self.editors.get_mut(key) {
            state.caret = shown;
        }
    }

    pub(crate) fn close_composer(&mut self, key: &str) {
        self.editors.remove(key);
    }

    /// What is written in the composer under `key`.
    pub(crate) fn composer_text(&self, key: &str) -> String {
        self.editors
            .get(key)
            .map_or_else(String::new, |state| state.buffer.text())
    }

    /// Puts `text` at the composer's caret, over whatever is selected.
    pub(crate) fn composer_insert(&mut self, key: &str, text: &str) {
        if let Some(state) = self.editors.get_mut(key) {
            state.buffer.replace_selection(text);
        }
        self.ensure_cursor_visible(key);
    }

    /// Handles a keystroke for the composer under `key`.
    pub(crate) fn composer_key(
        &mut self,
        key: &str,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) {
        let m = keystroke.modifiers;
        let cmd = m.platform || m.control;
        match keystroke.key.as_str() {
            // A prompt is a text field, so Cmd-Delete does what it does in
            // one — not the code pane's whole-line delete.
            edge @ ("backspace" | "delete") if m.platform => {
                if let Some(state) = self.editors.get_mut(key) {
                    state.delete_to_row_edge(edge == "delete");
                }
            }
            "a" if cmd => {
                if let Some(state) = self.editors.get_mut(key) {
                    state.buffer.select_all();
                }
            }
            "c" if cmd => {
                self.copy_selection(key, cx);
            }
            "x" if cmd => {
                if self.copy_selection(key, cx)
                    && let Some(state) = self.editors.get_mut(key)
                {
                    state.buffer.backspace();
                }
            }
            "v" if cmd => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                    && let Some(state) = self.editors.get_mut(key)
                {
                    state
                        .buffer
                        .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                }
            }
            "z" if cmd && m.shift => {
                if let Some(state) = self.editors.get_mut(key) {
                    state.buffer.redo();
                }
            }
            "z" if cmd => {
                if let Some(state) = self.editors.get_mut(key) {
                    state.buffer.undo();
                }
            }
            _ => self.handle_buffer_key(key, keystroke),
        }
        self.ensure_cursor_visible(key);
    }

    /// Where dictation into the draft begins: the selection, if any, is
    /// cleared out of its way and the caret's position returned, along with
    /// whether the words need a space in front to keep off the word before.
    pub(crate) fn quick_prompt_dictation_start(&mut self) -> Option<(usize, bool)> {
        let state = self.editors.get_mut(QUICK_PROMPT_EDITOR)?;
        if state.buffer.has_selection() {
            state.buffer.replace_selection("");
        }
        let start = state.buffer.cursor();
        let spaced = start > 0
            && state
                .buffer
                .text()
                .chars()
                .nth(start - 1)
                .is_some_and(|before| !before.is_whitespace());
        Some((start, spaced))
    }

    /// Writes `text` over the `len` characters dictation last wrote at
    /// `start`, and returns how many it wrote this time.
    ///
    /// The first write is an ordinary insertion; every one after it revises
    /// that insertion in place, so one undo takes back everything that was
    /// heard rather than stepping through each guess the recogniser made.
    pub(crate) fn quick_prompt_dictate(&mut self, start: usize, len: usize, text: &str) -> usize {
        let Some(state) = self.editors.get_mut(QUICK_PROMPT_EDITOR) else {
            return len;
        };
        // Clamped, though nothing should edit the draft while it listens: a
        // range past the end of the rope is a panic, not a no-op.
        let total = state.buffer.len_chars();
        let start = start.min(total);
        let end = (start + len).min(total);
        if end == start {
            state.buffer.set_selection(start, start);
            state.buffer.replace_selection(text);
        } else {
            state.buffer.revise(start..end, text);
        }
        self.ensure_cursor_visible(QUICK_PROMPT_EDITOR);
        text.chars().count()
    }

    pub(crate) fn quick_prompt_editor_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.editor_pane(QUICK_PROMPT_EDITOR, None, window, cx)
    }
}
