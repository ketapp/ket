//! Keys: movement, editing, the clipboard, and the shortcuts the pane
//! answers before the shell's own.

use std::path::PathBuf;

use gpui::{App, ClipboardItem, Context, KeyDownEvent, Keystroke, ScrollStrategy, Window};

use ket_core::buffer::LineCol;

use crate::Shell;
use crate::shortcuts::global_shortcut;

use super::selection::line_range;

impl Shell {
    /// Handles a keystroke for the editor pane. Returns whether it was
    /// consumed, the same convention as [`Shell::palette_key`], so
    /// `main.rs`'s `on_key_down` can fall through to the shell's own
    /// shortcuts when this returns `false`.
    pub(crate) fn editor_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = event.keystroke.clone();

        let Some(key) = self.active_editor_key() else {
            return false;
        };

        // Never trap the shell's own chords: the palette (Cmd-K), the file
        // finder (Cmd-P) and the terminal (Ctrl-`) stay reachable with the
        // editor focused. `main.rs` handles the rest of the table ahead of
        // this pane; these are the ones that come after it, so a buffer with
        // focus would otherwise swallow them as unrecognised chords.
        if global_shortcut(&keystroke).is_some() {
            return false;
        }

        if self
            .editors
            .get(key.as_ref())
            .is_some_and(|s| s.load_error.is_some())
        {
            // A file that failed to load takes no further keys; the tab's
            // own close button is still how you leave it.
            return true;
        }

        // A character must never land during the half of the blink where the
        // caret is not drawn, so every keystroke restarts the phase.
        self.caret_wake(cx);

        let find_focused = self
            .editors
            .get(key.as_ref())
            .and_then(|state| state.find.query.as_ref())
            .is_some_and(|input| input.read(cx).is_focused(window));
        if find_focused {
            self.handle_find_key(key.as_ref(), &keystroke, cx);
        } else {
            self.handle_editor_key(key.as_ref(), &keystroke, cx);
        }
        true
    }

    /// Mode-switching and whole-buffer chords: find, save, undo/redo,
    /// select-all, indenting, the clipboard, wrap, and the external handoffs. Anything
    /// else falls to [`Shell::handle_buffer_key`].
    fn handle_editor_key(
        &mut self,
        editor_key: &str,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) {
        let input_key = keystroke.key.as_str();
        let m = keystroke.modifiers;
        let cmd = m.platform || m.control;

        match input_key {
            "f" if cmd && !m.shift => {
                self.open_find(editor_key, cx);
                return;
            }
            "s" if cmd => return self.save_editor(editor_key, cx),
            "w" if cmd && m.alt => {
                if let Some(state) = self.editors.get_mut(editor_key) {
                    state.wrap = !state.wrap;
                }
                return;
            }
            // Whole-line delete, not macOS's own delete-to-start-of-line:
            // asked for deliberately, so it is not a bug to be corrected
            // back to the platform's habit. The Mac's `Delete` key arrives
            // as `backspace`; `delete` is the forward one, and takes the
            // line as well rather than leaving the chord half-working on a
            // keyboard that has both.
            "backspace" | "delete" if cmd => self.delete_line(editor_key),
            "pageup" | "pagedown" if !cmd => {
                self.page(editor_key, input_key == "pagedown", m.shift);
            }
            "v" if cmd && m.alt => return self.reveal_externally(editor_key, "code"),
            "e" if cmd && m.alt => return self.reveal_externally(editor_key, "zed"),
            "z" if cmd && m.shift => {
                if let Some(state) = self.editors.get_mut(editor_key) {
                    state.buffer.redo();
                }
            }
            "z" if cmd => {
                if let Some(state) = self.editors.get_mut(editor_key) {
                    state.buffer.undo();
                }
            }
            "a" if cmd => {
                if let Some(state) = self.editors.get_mut(editor_key) {
                    state.buffer.select_all();
                }
            }
            // ⌘] and ⌘[: every line the selection touches, one level in or
            // out, by what this file indents with — tabs, or its usual run
            // of spaces. See `Buffer::indent_unit`.
            "]" if cmd => {
                if let Some(state) = self.editors.get_mut(editor_key) {
                    let unit = state.buffer.indent_unit();
                    state.buffer.indent_lines(&unit);
                }
            }
            "[" if cmd => {
                if let Some(state) = self.editors.get_mut(editor_key) {
                    let unit = state.buffer.indent_unit();
                    state.buffer.outdent_lines(&unit);
                }
            }
            "c" if cmd => {
                self.copy_selection(editor_key, cx);
                return;
            }
            "x" if cmd => {
                if self.copy_selection(editor_key, cx)
                    && let Some(state) = self.editors.get_mut(editor_key)
                {
                    state.buffer.backspace();
                }
            }
            "v" if cmd => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                    && let Some(state) = self.editors.get_mut(editor_key)
                {
                    // Line endings are normalised on the way in: the buffer
                    // counts lines by `\n`, so a carriage return pasted from
                    // a Windows file would otherwise sit inside a line as an
                    // invisible character.
                    state
                        .buffer
                        .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                }
            }
            _ => self.handle_buffer_key(editor_key, keystroke),
        }

        // Every path that reaches here may have changed the text, and the ones
        // that cannot — a cursor move, a selection — cost only the timer, which
        // the next keystroke replaces anyway.
        self.schedule_editor_marks(cx);
        self.ensure_cursor_visible(editor_key);
    }

    /// What the editor tab in front has selected: the text, the file, and
    /// the first and last lines it touches, counted from 1. `None` with no
    /// selection.
    pub(crate) fn editor_selection(&self) -> Option<(String, Option<PathBuf>, u32, u32)> {
        let state = self.editors.get(&self.active_editor_key()?)?;
        let buffer = &state.buffer;
        if !buffer.has_selection() {
            return None;
        }
        let range = buffer.selection();
        let first = buffer.line_col_at(range.start).line;
        let end = buffer.line_col_at(range.end);
        // A selection that runs to the start of a line stops on the one
        // before it, which is the last line any of it is on.
        let last = if end.column == 1 && end.line > first {
            end.line - 1
        } else {
            end.line
        };
        Some((buffer.selected_text(), state.path.clone(), first, last))
    }

    /// `Cmd-C`: the selection to the clipboard. Answers whether there was
    /// one, which is what tells `Cmd-X` whether it has anything to cut —
    /// with nothing selected both are no-ops rather than a cut of the
    /// character behind the cursor.
    pub(super) fn copy_selection(&self, editor_key: &str, cx: &mut App) -> bool {
        let Some(state) = self.editors.get(editor_key) else {
            return false;
        };
        if !state.buffer.has_selection() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(state.buffer.selected_text()));
        true
    }

    /// Movement, editing, and plain typing.
    pub(super) fn handle_buffer_key(&mut self, key: &str, keystroke: &Keystroke) {
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        let buffer = &mut state.buffer;
        let key = keystroke.key.as_str();
        let m = keystroke.modifiers;
        let cmd = m.platform || m.control;
        let extend = m.shift;
        let word = m.alt;

        match key {
            "left" if word && extend => buffer.extend_word_left(),
            "left" if word => buffer.move_word_left(),
            "left" if extend => buffer.extend_left(),
            "left" => buffer.move_left(),
            "right" if word && extend => buffer.extend_word_right(),
            "right" if word => buffer.move_word_right(),
            "right" if extend => buffer.extend_right(),
            "right" => buffer.move_right(),
            "up" if cmd && extend => buffer.extend_document_start(),
            "up" if cmd => buffer.move_document_start(),
            "up" if extend => buffer.extend_up(),
            "up" => buffer.move_up(),
            "down" if cmd && extend => buffer.extend_document_end(),
            "down" if cmd => buffer.move_document_end(),
            "down" if extend => buffer.extend_down(),
            "down" => buffer.move_down(),
            "home" if cmd && extend => buffer.extend_document_start(),
            "home" if cmd => buffer.move_document_start(),
            "home" if extend => buffer.extend_home(),
            "home" => buffer.move_home(),
            "end" if cmd && extend => buffer.extend_document_end(),
            "end" if cmd => buffer.move_document_end(),
            "end" if extend => buffer.extend_end(),
            "end" => buffer.move_end(),
            // Option-Delete takes the word, as in any Mac text field: back
            // to the start of the word before the caret, or on to the end of
            // the one after it. A selection goes instead, as with Delete.
            "backspace" | "delete" if word => {
                if !buffer.has_selection() {
                    if key == "delete" {
                        buffer.extend_word_right();
                    } else {
                        buffer.extend_word_left();
                    }
                }
                buffer.backspace();
            }
            "backspace" => buffer.backspace(),
            "delete" => buffer.delete(),
            "enter" => buffer.newline(),
            "tab" => buffer.insert("\t"),
            "escape" => buffer.set_cursor(buffer.cursor()),
            _ => {
                if let Some(typed) = keystroke.key_char.as_deref()
                    && !typed.is_empty()
                    && !cmd
                {
                    buffer.insert(typed);
                }
            }
        }
    }

    /// Moves the cursor a page — what the pane is showing, less one row of
    /// overlap so the eye keeps its place — and extends the selection with
    /// it when asked.
    ///
    /// Counted in *display* rows rather than buffer lines, which is the
    /// whole reason this is not two calls to [`Buffer::move_up`]: with soft
    /// wrap on, a screen of prose is far fewer lines than rows, and paging by
    /// lines would jump several screens at a time.
    ///
    /// [`Buffer::move_up`]: ket_core::buffer::Buffer::move_up
    fn page(&mut self, key: &str, down: bool, extend: bool) {
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        let rows = state.visible_rows.len().saturating_sub(1).max(1);
        state.ensure_lines();

        let LineCol { line, column } = state.buffer.line_col_at(state.buffer.cursor());
        let (line_idx, column_idx) = (line as usize - 1, column as usize - 1);

        let target = if state.wrap {
            let row = state.line_index.wrap_row_for(line_idx, column_idx);
            let row_start = state
                .line_index
                .wrap_rows
                .get(row)
                .map_or(0, |(_, col)| *col as usize);
            // Kept: how far into its own row the cursor was, so a page lands
            // under where it started rather than at the left margin.
            let offset = column_idx.saturating_sub(row_start);
            let last = state.line_index.wrap_rows.len().saturating_sub(1);
            let next = if down {
                (row + rows).min(last)
            } else {
                row.saturating_sub(rows)
            };
            let (next_line, next_col) = state
                .line_index
                .wrap_rows
                .get(next)
                .copied()
                .unwrap_or((line_idx as u32, 0));
            LineCol {
                line: next_line + 1,
                column: (next_col as usize + offset + 1) as u32,
            }
        } else {
            let next_line = if down {
                line_idx.saturating_add(rows)
            } else {
                line_idx.saturating_sub(rows)
            };
            LineCol {
                line: next_line as u32 + 1,
                column,
            }
        };

        let at = state.buffer.char_at_line_col(target);
        if extend {
            let anchor = state.buffer.anchor();
            state.buffer.set_selection(anchor, at);
        } else {
            state.buffer.set_cursor(at);
        }
    }

    /// `Cmd-Delete`: removes the line the cursor is on, or every line a
    /// selection touches, leaving the cursor at the start of whichever line
    /// moves up to take their place.
    fn delete_line(&mut self, key: &str) {
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        let selection = state.buffer.selection();
        // A selection ending exactly on a line's first character has not
        // reached into that line — selecting a whole line leaves the cursor
        // there — so the last line to go is the one before it.
        let furthest = if selection.end > selection.start {
            selection.end - 1
        } else {
            selection.end
        };
        let first = line_range(&state.buffer, selection.start);
        let last = line_range(&state.buffer, furthest);

        let mut start = first.start;
        // Only the file's very last line has no line break of its own to
        // take, so only that one takes the break above it instead; without
        // that, deleting it would leave behind the blank line it sat on.
        // Every other line's range already ends in its own break, and
        // reaching back for another would take the line before it too.
        let on_last_line =
            state.buffer.line_col_at(furthest).line as usize == state.buffer.line_count();
        if on_last_line && start > 0 {
            start -= 1;
        }

        state.buffer.set_selection(start, last.end);
        // Never coalesced with the edit before it, so one undo brings the
        // line back and nothing else with it.
        state.buffer.replace_selection("");
    }

    /// Scrolls the cursor into view if [`EditorState::visible_rows`] says it
    /// is not already showing, rather than recentring on every keystroke.
    ///
    /// [`EditorState::visible_rows`]: super::state::EditorState::visible_rows
    pub(super) fn ensure_cursor_visible(&mut self, key: &str) {
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        state.ensure_lines();

        let LineCol { line, column } = state.buffer.line_col_at(state.buffer.cursor());
        let row = if state.wrap {
            state
                .line_index
                .wrap_row_for(line as usize - 1, column as usize - 1)
        } else {
            line as usize - 1
        };
        if !state.visible_rows.contains(&row) {
            state.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
    }
}
