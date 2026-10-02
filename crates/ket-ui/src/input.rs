//! The shell's one single-line text field.
//!
//! Every surface that asks for a string — a branch name, a project's display
//! name, the command palette's query, the file finder's, the settings filter,
//! the file panel's search box, the editor's find bar — used to grow its own:
//! a `String` on the state, `push_str` on the way in, `pop` on Backspace, and
//! the text drawn as a label. That spelling has no caret, no way back to a
//! typo three characters ago, and no paste, and it went wrong differently in
//! each place.
//!
//! It went wrong in one way everywhere, too. A `String` filled from the
//! window's key handler needs no focus of its own, so the window's handle
//! stayed focused — and a terminal pane registers a text-input handler
//! against exactly that handle. Every character typed into the settings
//! filter was also being typed into the shell behind it.
//!
//! There are two spellings of the same field: [`text_field`] draws
//! [`crate::ui::field`]'s box around it, and [`text_line`] draws none, for the
//! surfaces that are a query line rather than a control. Both are the same
//! entity underneath, so neither can drift from the other in what a key or a
//! mouse does.
//!
//! The editing model is [`ket_core::buffer::Buffer`], the same one the editor
//! pane uses. It is a rope with undo and word motion, which is more than a
//! branch name needs — but the alternative is a second definition of "one
//! word to the left" in an app that already has one, and the two would drift.
//! What this adds on top is the single-line rule: no newline ever enters the
//! buffer, so Enter stays the dialog's to use as **submit**.
//!
//! # Why this is an element and not a `div`
//!
//! The first spelling of this drew the caret by splitting the text into two
//! `div`s and putting a one-pixel rule between them, and worked out which
//! characters were on screen by dividing the width by the advance of an `m`.
//! That has three consequences, all of which showed:
//!
//! - the field had to be monospaced, or the caret drifted from the text —
//!   which is why these fields were monospaced at all, against a shell whose
//!   chrome is otherwise sans-serif;
//! - the caret could not be *aimed* — a click could not place it, because
//!   nothing knew where any character was;
//! - blinking it meant adding and removing a flex sibling, which moved the
//!   text beside it a pixel every half second.
//!
//! So the text is laid out properly instead: [`TextElement`] shapes the line
//! with the text system and asks the resulting [`ShapedLine`] where things
//! are — `x_for_index` to place the caret and the selection, `index_for_x` to
//! turn a click back into an offset. The caret is painted as a quad rather
//! than laid out as a sibling, so blinking it moves nothing.
//!
//! # Why it is an entity
//!
//! macOS does not deliver text by handing you characters. It hands you an
//! `NSTextInputClient`: the field is asked what is selected, what is in a
//! range, and where a range is on screen, and is told to replace a range with
//! new text — which is how dead keys, the character palette, and every
//! non-Latin input method work. gpui exposes that as [`EntityInputHandler`],
//! which is implemented on an entity and registered during paint. So the
//! field is an `Entity<TextInput>` that dialogs hold, rather than a struct
//! they own inline.
//!
//! This is also why [`TextInput::key`] declines printable characters. gpui's
//! macOS window runs our key callback first and only falls through to the
//! input context if we did not consume the event — so a field that swallowed
//! `a` would be a field with no IME at all. It takes the editing *commands*
//! and lets the text itself arrive through [`EntityInputHandler`].

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, Div, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, InspectorElementId, IntoElement, Keystroke,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Stateful, Style as GpuiStyle, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, size,
};
use ket_core::buffer::Buffer;
use ket_core::theme::Theme;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::paint;
use crate::ui::field::{field, placeholder};
use crate::ui::icon::Icon;

/// The caret's width. Two device pixels on a retina display, which is what
/// every native field draws.
const CARET_W: Pixels = px(1.0);

/// A field's presentation, which is the caller's to choose and not the
/// field's to know.
pub(crate) struct Style<'a> {
    /// The theme the field is painted from.
    theme: &'a Theme,
    /// The caret's blink phase: false is the half-second it is not drawn.
    ///
    /// Here rather than on the field because the phase is the *shell's* — one
    /// blinker for every field, so two fields on screen cannot blink out of
    /// step. See `Shell::caret_blink`.
    blink: bool,
    /// An icon before the text, for a field whose job is not obvious from its
    /// placeholder alone. Ignored by [`text_line`], which has no box to hang
    /// one in.
    leading: Option<Icon>,
}

impl<'a> Style<'a> {
    /// The plain field: a theme and the shell's blink phase.
    pub(crate) fn new(theme: &'a Theme, blink: bool) -> Self {
        Self {
            theme,
            blink,
            leading: None,
        }
    }

    /// With an icon before the text.
    pub(crate) fn leading(mut self, which: Icon) -> Self {
        self.leading = Some(which);
        self
    }
}

/// What a dialog should do with a key after its field has seen it.
///
/// The distinction exists because of how macOS delivers text. gpui's window
/// runs our key callback first and only falls through to the input context if
/// nothing consumed the event — so a modal that swallows every key, which is
/// otherwise exactly what a modal should do, is a modal you cannot type a
/// dead key or a Japanese phrase into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Key {
    /// An editing command, or one of the dialog's own. Nothing else should
    /// see it, and propagation stops here.
    Taken,
    /// Text bound for the input method. The dialog is finished with it, but
    /// the event has to keep propagating or the character never arrives.
    Text,
}

/// Whether a keystroke is a character to be inserted rather than a command.
///
/// Control-bearing chords are commands however printable their key is, and a
/// key whose character is itself a control code — Return, Tab, Escape — is
/// the dialog's, not the field's.
pub(crate) fn is_text(keystroke: &Keystroke) -> bool {
    let m = keystroke.modifiers;
    !m.platform
        && !m.control
        && !m.function
        && keystroke
            .key_char
            .as_deref()
            .is_some_and(|c| !c.is_empty() && !c.chars().any(char::is_control))
}

/// A single-line text field: the text, the caret, and what to show when empty.
pub(crate) struct TextInput {
    /// The text, the caret and the selection, in `ket-core`'s editing model.
    ///
    /// Character-indexed, where everything gpui hands us is byte- or
    /// UTF-16-indexed. `Buffer::char_to_byte` and `byte_to_char` are the
    /// border, and every crossing of it is at the edge of this type.
    buffer: Buffer,
    /// Shown, dimmed, while the field is empty.
    placeholder: SharedString,
    /// The field's own keyboard focus. `handle_input` is registered against
    /// it, so this is what makes the field a text field as far as macOS is
    /// concerned.
    focus: FocusHandle,
    /// Text the input method has provisionally inserted and may still replace
    /// — the `¨` of a dead key, the Latin spelling of a Japanese phrase. Byte
    /// offsets, and underlined rather than painted as a selection.
    marked: Option<Range<usize>>,
    /// The last line the element shaped, and where it put it.
    ///
    /// Kept because the questions macOS asks — where is this range, what is
    /// under this point — arrive between frames, when the only honest answer
    /// is what was drawn last.
    layout: Option<ShapedLine>,
    /// Where that line was painted, for the same reason.
    bounds: Option<Bounds<Pixels>>,
    /// How far the text is scrolled left, when it is longer than the field.
    scroll: Pixels,
    /// Whether the pointer is down and dragging out a selection.
    selecting: bool,
    /// Whether the text is drawn as bullets rather than as itself.
    ///
    /// For a field holding a credential. Masking is a property of the *field*
    /// rather than of how one caller chose to draw it, because it has to hold
    /// for everything the field does — what it shapes, what a click maps onto,
    /// and what Cmd-C is allowed to take out of it. A "draw bullets" flag
    /// passed to the renderer would mask the pixels and leave the clipboard
    /// handing the secret over intact.
    masked: bool,
    /// Whether the field should take the keyboard on the next frame.
    ///
    /// A one-shot request rather than a flag the caller holds, because a
    /// dialog opens in a click handler and focusing needs a `Window`. Dialogs
    /// used to solve that by re-focusing their field on *every* frame, which
    /// worked until you tried to leave the field: the press moved focus and
    /// the next render put it straight back. Asking once is the whole
    /// difference between a field you can focus and a field you cannot blur.
    wants_focus: bool,
}

impl TextInput {
    /// An empty field.
    pub(crate) fn new(placeholder: impl Into<SharedString>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            buffer: Buffer::new(),
            placeholder: placeholder.into(),
            focus: cx.focus_handle(),
            marked: None,
            layout: None,
            bounds: None,
            scroll: px(0.0),
            selecting: false,
            masked: false,
            wants_focus: false,
        })
    }

    /// A field starting with `text`, with everything selected.
    ///
    /// Selected rather than with the caret parked at the end, because a field
    /// opened with a value in it is a value being *replaced* — typing should
    /// not append to a name that was already there.
    pub(crate) fn with_text(
        placeholder: impl Into<SharedString>,
        text: &str,
        cx: &mut App,
    ) -> Entity<Self> {
        let input = Self::new(placeholder, cx);
        input.update(cx, |input, _| {
            input.set_text(text);
            input.buffer.select_all();
        });
        input
    }

    /// A field whose text is drawn as bullets, for a credential.
    ///
    /// The caret still moves through the real characters and a paste still
    /// lands whole; only what is shaped, and what may be copied out, changes.
    /// Seeded unselected, unlike [`TextInput::with_text`]: select-all on a
    /// field of bullets puts the whole secret one keystroke from being
    /// replaced by whatever is typed next, with nothing on screen to show
    /// what was lost.
    pub(crate) fn masked(
        placeholder: impl Into<SharedString>,
        text: &str,
        cx: &mut App,
    ) -> Entity<Self> {
        let input = Self::new(placeholder, cx);
        input.update(cx, |input, _| {
            input.masked = true;
            input.set_text(text);
        });
        input
    }

    /// What has been typed.
    pub(crate) fn text(&self) -> String {
        self.buffer.text()
    }

    // ---- the masking border ---------------------------------------------
    //
    // The shaped line is indexed in bytes of *what was drawn*, which is not
    // the buffer's text when the field is masked. These three are the only
    // places that difference is allowed to be known; every call into the
    // layout goes through one of them.

    /// One bullet, and how many bytes of it there are.
    ///
    /// Fixed-width by construction, which is what lets an offset be converted
    /// by multiplication rather than by walking the string.
    const BULLET: &'static str = "\u{2022}";

    /// The text as it is drawn: bullets, one per character, when masked.
    fn shown(&self) -> String {
        if self.masked {
            Self::BULLET.repeat(self.buffer.text().chars().count())
        } else {
            self.buffer.text()
        }
    }

    /// A buffer byte offset, as an offset into what was drawn.
    fn shown_byte(&self, byte: usize) -> usize {
        if self.masked {
            self.buffer.byte_to_char(byte) * Self::BULLET.len()
        } else {
            byte
        }
    }

    /// An offset into what was drawn, back to a buffer byte offset.
    fn buffer_byte(&self, shown: usize) -> usize {
        if self.masked {
            self.buffer.char_to_byte(shown / Self::BULLET.len())
        } else {
            shown
        }
    }

    /// The handle the field takes the keyboard through.
    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Whether the keyboard is in this field.
    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Asks for the keyboard on the next frame.
    ///
    /// For the callers that have no `Window` — opening a dialog, refusing a
    /// name — where `window.focus` is not available to call directly.
    pub(crate) fn request_focus(&mut self) {
        self.wants_focus = true;
    }

    /// Replaces the contents, putting the caret at the end.
    pub(crate) fn set_text(&mut self, text: &str) {
        self.buffer.select_all();
        self.buffer.replace_selection(&flatten(text));
        self.buffer.move_end();
        self.marked = None;
    }

    /// Changes what an empty field shows, for a default that is only known
    /// once something else in the dialog has been chosen.
    pub(crate) fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    /// Selects everything, so the next keystroke replaces it.
    pub(crate) fn select_all(&mut self) {
        self.buffer.select_all();
    }

    /// Whether anything has been typed.
    pub(crate) fn is_empty(&self) -> bool {
        self.buffer.text().is_empty()
    }

    /// Empties it, for a picker that opens fresh every time.
    pub(crate) fn clear(&mut self) {
        self.set_text("");
    }

    /// Handles a keystroke. Returns whether the field consumed it.
    ///
    /// What it declines is as much of the contract as what it takes:
    ///
    /// - Enter, Escape, Tab and the vertical arrows are never a single-line
    ///   field's, so the dialog around it keeps submit, cancel, and moving
    ///   between controls.
    /// - **Printable characters are declined too.** They are not ours to
    ///   insert: consuming them would tell gpui's macOS window that the event
    ///   is finished, and it is the fall-through to the input context that
    ///   produces dead keys, the character palette and every input method
    ///   that is not a US keyboard. They come back through
    ///   [`EntityInputHandler::replace_text_in_range`].
    pub(crate) fn key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = keystroke.modifiers;
        let (cmd, ctrl, word, extend) = (m.platform, m.control, m.alt, m.shift);
        let b = &mut self.buffer;

        match keystroke.key.as_str() {
            // Motion. Cmd is line-wise, Alt is word-wise, Shift extends —
            // the macOS bindings, which are the ones in the muscle memory.
            "left" if cmd && extend => b.extend_home(),
            "left" if cmd => b.move_home(),
            "left" if word && extend => b.extend_word_left(),
            "left" if word => b.move_word_left(),
            "left" if extend => b.extend_left(),
            "left" => collapse_left(b),
            "right" if cmd && extend => b.extend_end(),
            "right" if cmd => b.move_end(),
            "right" if word && extend => b.extend_word_right(),
            "right" if word => b.move_word_right(),
            "right" if extend => b.extend_right(),
            "right" => collapse_right(b),
            "home" if extend => b.extend_home(),
            "home" => b.move_home(),
            "end" if extend => b.extend_end(),
            "end" => b.move_end(),

            // The emacs motions macOS honours in every text field, and that a
            // terminal-shaped app's users reach for without thinking.
            "a" if ctrl && !cmd => b.move_home(),
            "e" if ctrl && !cmd => b.move_end(),
            "b" if ctrl && !cmd => collapse_left(b),
            "f" if ctrl && !cmd => collapse_right(b),
            "k" if ctrl && !cmd => {
                b.extend_end();
                b.backspace();
            }
            "u" if ctrl && !cmd => {
                b.extend_home();
                b.backspace();
            }
            "w" if ctrl && !cmd => {
                b.extend_word_left();
                b.backspace();
            }

            // Deletion. Backspace with a selection deletes the selection,
            // which is why every widened case is "extend, then backspace".
            "backspace" if cmd => {
                b.extend_home();
                b.backspace();
            }
            "backspace" if word => {
                b.extend_word_left();
                b.backspace();
            }
            "backspace" => b.backspace(),
            "delete" if cmd => {
                b.extend_end();
                b.delete();
            }
            "delete" if word => {
                b.extend_word_right();
                b.delete();
            }
            "delete" => b.delete(),

            "a" if cmd => b.select_all(),
            "z" if cmd && extend => {
                b.redo();
            }
            "z" if cmd => {
                b.undo();
            }
            "c" if cmd => self.copy(cx),
            "x" if cmd => {
                self.copy(cx);
                self.buffer.backspace();
            }
            "v" if cmd => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                if let Some(text) = pasted {
                    self.buffer.insert(&flatten(&text));
                }
            }

            // Enter, Escape, Tab and the vertical arrows belong to the dialog;
            // everything else is text, and belongs to the input method.
            _ => return false,
        }

        // Any of the above may have moved the caret out of view.
        self.marked = None;
        // Said to the field's own watchers, not just to whoever routed the
        // key. Typing arrives through macOS's input context, which notifies
        // from `replace_text_in_range`; everything editing here — backspace,
        // Ctrl-W, undo, paste — arrives through this method instead, and used
        // to change the text without telling anyone. A picker that re-ranks
        // from an observer on its field (see `Shell::rank_finder`) therefore
        // kept the list it had ranked for the character you just deleted.
        cx.notify();
        true
    }

    /// Puts the selection on the clipboard, if there is one.
    ///
    /// A masked field refuses. Drawing bullets and then handing the plain text
    /// to Cmd-C would mask the screen and nothing else, which is worse than
    /// not masking at all: it looks protected.
    fn copy(&self, cx: &mut App) {
        if self.masked {
            return;
        }
        if self.buffer.has_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.selected_text()));
        }
    }

    /// The offset under `position`, for a click or a drag.
    ///
    /// Answered from the last line that was drawn, which is the only layout
    /// there is: a click arrives between frames.
    fn offset_at(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.bounds.as_ref(), self.layout.as_ref()) else {
            return self.buffer.cursor();
        };
        let x = position.x - bounds.left() + self.scroll;
        self.buffer
            .byte_to_char(self.buffer_byte(line.closest_index_for_x(x)))
    }

    // ---- the UTF-16 border ----------------------------------------------------
    //
    // macOS counts in UTF-16 code units, the buffer counts in characters, and
    // the shaped line counts in bytes. Everything crossing between them goes
    // through these four.

    /// A byte offset for a UTF-16 one.
    fn byte_of_utf16(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        let mut utf16 = 0;
        for (at, c) in text.char_indices() {
            if utf16 >= offset {
                return at;
            }
            utf16 += c.len_utf16();
        }
        text.len()
    }

    /// A UTF-16 offset for a byte one.
    fn utf16_of_byte(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        let mut utf16 = 0;
        for (at, c) in text.char_indices() {
            if at >= offset {
                break;
            }
            utf16 += c.len_utf16();
        }
        utf16
    }

    /// A byte range for a UTF-16 one.
    fn bytes_of_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.byte_of_utf16(range.start)..self.byte_of_utf16(range.end)
    }

    /// A UTF-16 range for a byte one.
    fn utf16_of_bytes(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_of_byte(range.start)..self.utf16_of_byte(range.end)
    }

    /// The selection as bytes, which is what the line layout is indexed in.
    fn selected_bytes(&self) -> Range<usize> {
        let selection = self.buffer.selection();
        self.buffer.char_to_byte(selection.start)..self.buffer.char_to_byte(selection.end)
    }

    /// Puts the buffer's selection where a byte range says.
    fn select_bytes(&mut self, range: Range<usize>) {
        let anchor = self.buffer.byte_to_char(range.start);
        let cursor = self.buffer.byte_to_char(range.end);
        self.buffer.set_selection(anchor, cursor);
    }
}

/// Moving left with a selection up collapses to its left edge rather than
/// stepping one further — which is what every text field does, and what
/// "extend, then move" would get wrong.
fn collapse_left(buffer: &mut Buffer) {
    if buffer.has_selection() {
        buffer.set_cursor(buffer.selection().start);
    } else {
        buffer.move_left();
    }
}

/// The same, rightwards.
fn collapse_right(buffer: &mut Buffer) {
    if buffer.has_selection() {
        buffer.set_cursor(buffer.selection().end);
    } else {
        buffer.move_right();
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.bytes_of_utf16(&range_utf16);
        actual.replace(self.utf16_of_bytes(&range));
        Some(self.buffer.text()[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let bytes = self.selected_bytes();
        Some(UTF16Selection {
            range: self.utf16_of_bytes(&bytes),
            reversed: self.buffer.cursor() < self.buffer.anchor(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let marked = self.marked.clone()?;
        Some(self.utf16_of_bytes(&marked))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| self.bytes_of_utf16(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected_bytes());

        self.select_bytes(range);
        // Flattened here as everywhere else: a paste carrying a newline is a
        // paste onto one line, not a way to get a control character into a
        // branch name.
        self.buffer.replace_selection(&flatten(text));
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| self.bytes_of_utf16(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected_bytes());

        let flat = flatten(text);
        self.select_bytes(range.clone());
        self.buffer.replace_selection(&flat);

        // What is still provisional, so the element can underline it.
        self.marked = (!flat.is_empty()).then(|| range.start..range.start + flat.len());

        // The input method may want the caret somewhere inside what it just
        // inserted — mid-composition, that is where the next keystroke goes.
        if let Some(selected) = selected_utf16 {
            let inside = self.bytes_of_utf16(&selected);
            self.select_bytes(range.start + inside.start..range.start + inside.end);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Where the candidate window hangs itself. Without this, an input
        // method's list of choices appears in the corner of the screen rather
        // than under what is being composed.
        let line = self.layout.as_ref()?;
        let range = self.bytes_of_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(self.shown_byte(range.start)) - self.scroll,
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(self.shown_byte(range.end)) - self.scroll,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.bounds?;
        let line = self.layout.as_ref()?;
        let x = position.x - bounds.left() + self.scroll;
        Some(self.utf16_of_byte(self.buffer_byte(line.closest_index_for_x(x))))
    }
}

/// The laid-out line, the caret and the selection.
pub(crate) struct TextElement {
    /// The field being drawn.
    input: Entity<TextInput>,
    /// Colours and the blink phase.
    text: gpui::Hsla,
    /// The selection's wash.
    selection: gpui::Hsla,
    /// The caret's colour.
    caret: gpui::Hsla,
    /// Whether to paint the caret this frame.
    blink: bool,
}

/// What `prepaint` worked out and `paint` draws.
pub(crate) struct Painted {
    /// The shaped line, or nothing if there was no room for it.
    line: Option<ShapedLine>,
    /// The caret, when the field has focus and is in its lit phase.
    caret: Option<PaintQuad>,
    /// The selection's wash, when there is one.
    selection: Option<PaintQuad>,
    /// How far the line is scrolled, so `paint` draws it where `prepaint`
    /// measured it.
    scroll: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = GpuiStyle::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.buffer.text();
        let focused = input.focus.is_focused(window);
        // Every offset below indexes the line that is actually shaped, which
        // is bullets rather than characters when the field is masked.
        let marked = input
            .marked
            .clone()
            .map(|marked| input.shown_byte(marked.start)..input.shown_byte(marked.end));
        let selected = {
            let bytes = input.selected_bytes();
            input.shown_byte(bytes.start)..input.shown_byte(bytes.end)
        };
        let caret_at = input.shown_byte(input.buffer.char_to_byte(input.buffer.cursor()));

        let empty = content.is_empty();
        let shown: SharedString = if empty {
            input.placeholder.clone()
        } else {
            input.shown().into()
        };

        let style = window.text_style();
        let run = TextRun {
            len: shown.len(),
            font: style.font(),
            color: self.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        // What the input method has provisionally inserted is underlined, the
        // way macOS underlines it in every other application — it is the only
        // sign that a keystroke has not landed yet.
        let runs = match marked.as_ref().filter(|_| !empty) {
            Some(marked) => vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: shown.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(shown, font_size, &runs, None);

        // Keep the caret in view. Derived from the caret and the width rather
        // than remembered as a scroll position, so there is no second piece of
        // state to fall out of step with the text.
        let caret_x = if empty {
            px(0.0)
        } else {
            line.x_for_index(caret_at)
        };
        let mut scroll = self.input.read(cx).scroll;
        let room = bounds.size.width;
        if line.width <= room {
            scroll = px(0.0);
        } else {
            scroll = scroll.min(line.width - room).max(px(0.0));
            if caret_x - scroll > room - CARET_W {
                scroll = caret_x - room + CARET_W;
            }
            if caret_x < scroll {
                scroll = caret_x;
            }
        }

        let origin = point(bounds.left() - scroll, bounds.top());
        let caret = (focused && self.blink).then(|| {
            fill(
                Bounds::new(
                    point(origin.x + caret_x, bounds.top()),
                    size(CARET_W, bounds.size.height),
                ),
                self.caret,
            )
        });
        let selection = (focused && !selected.is_empty() && !empty).then(|| {
            fill(
                Bounds::from_corners(
                    point(origin.x + line.x_for_index(selected.start), bounds.top()),
                    point(origin.x + line.x_for_index(selected.end), bounds.bottom()),
                ),
                self.selection,
            )
        });

        Painted {
            line: Some(line),
            caret,
            selection,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        painted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        // What makes this a text field rather than a picture of one: from here
        // on macOS asks *us* what is selected and tells us what to replace.
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        // The text is clipped to the field, so a value longer than the box
        // stops at the border instead of running out over the label beside it.
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(selection) = painted.selection.take() {
                window.paint_quad(selection);
            }
            if let Some(line) = painted.line.take() {
                let origin = point(bounds.left() - painted.scroll, bounds.top());
                line.paint(origin, window.line_height(), window, cx).ok();
                self.input.update(cx, |input, _| {
                    input.layout = Some(line);
                    input.bounds = Some(bounds);
                    input.scroll = painted.scroll;
                });
            }
            if let Some(caret) = painted.caret.take() {
                window.paint_quad(caret);
            }
        });
    }
}

/// Grants a pending focus request, once, and then forgets it.
///
/// Here rather than in the caller because every way of drawing a field has to
/// do it, and a field that is drawn without it is one `request_focus` never
/// reaches — see [`TextInput::request_focus`].
fn grant_focus(input: &Entity<TextInput>, window: &mut Window, cx: &mut App) {
    if input.read(cx).wants_focus {
        input.update(cx, |input, _| input.wants_focus = false);
        window.focus(input.read(cx).focus_handle());
    }
}

/// The line itself: the shaped text, the caret and the selection.
///
/// The placeholder is [`placeholder`]'s ink whether the line sits in a box or
/// on its own. The two spellings used to disagree — the boxed hint thinned,
/// the bare one at full `text.dim` — and the explorer's filter sat a few
/// pixels under the sidebar's search field in a visibly different grey.
fn laid_out(input: &Entity<TextInput>, style: &Style<'_>, cx: &App) -> TextElement {
    let t = style.theme;
    TextElement {
        input: input.clone(),
        text: if input.read(cx).buffer.text().is_empty() {
            placeholder(t).into()
        } else {
            paint(t.text.primary).into()
        },
        selection: paint(t.selection).into(),
        caret: paint(t.text.primary).into(),
        blink: style.blink,
    }
}

/// Attaches the pointer behaviour every text field shares: click to place the
/// caret, drag to select, and a press outside to give the keyboard up.
///
/// Chained onto whatever chrome the caller drew, so the box and the bare line
/// cannot drift apart in what a mouse does to them.
fn wired(element: Stateful<Div>, input: &Entity<TextInput>) -> Stateful<Div> {
    element
        // Click to place the caret, drag to select — the two things the old
        // field could not do at all, because nothing knew where a character
        // was.
        .on_mouse_down(MouseButton::Left, {
            let input = input.clone();
            move |event: &MouseDownEvent, window, cx| {
                window.focus(input.read(cx).focus_handle());
                input.update(cx, |input, cx| {
                    let at = input.offset_at(event.position);
                    match event.click_count {
                        // A second click takes the word, a third the lot —
                        // which is what a double-click means everywhere else.
                        1 if event.modifiers.shift => {
                            input.buffer.set_selection(input.buffer.anchor(), at);
                        }
                        1 => input.buffer.set_cursor(at),
                        2 => {
                            input.buffer.set_cursor(at);
                            input.buffer.move_word_left();
                            input.buffer.extend_word_right();
                        }
                        _ => input.buffer.select_all(),
                    }
                    input.selecting = event.click_count == 1;
                    cx.notify();
                });
            }
        })
        .on_mouse_move({
            let input = input.clone();
            move |event: &MouseMoveEvent, _, cx| {
                input.update(cx, |input, cx| {
                    if input.selecting {
                        let at = input.offset_at(event.position);
                        input.buffer.set_selection(input.buffer.anchor(), at);
                        cx.notify();
                    }
                });
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let input = input.clone();
            move |_: &MouseUpEvent, _, cx| {
                input.update(cx, |input, _| input.selecting = false);
            }
        })
        // A press anywhere else gives the keyboard up. The field's own
        // business, not the surrounding dialog's — a dialog that had to know
        // about this would need a state meaning "no field", and every dialog
        // with a field in it would need its own copy of that state.
        //
        // Blurring to nothing rather than to some fallback: with no focus,
        // gpui dispatches keys from the root, so the dialog's Escape and
        // Enter still land without anything being told where to send them.
        .on_mouse_down_out({
            let input = input.clone();
            move |_: &MouseDownEvent, window, cx| {
                if input.read(cx).is_focused(window) {
                    window.blur();
                }
            }
        })
        // Released outside the field too, or a drag that ends over the dialog
        // leaves the field selecting for ever.
        .on_mouse_up_out(MouseButton::Left, {
            let input = input.clone();
            move |_: &MouseUpEvent, _, cx| {
                input.update(cx, |input, _| input.selecting = false);
            }
        })
}

/// The field: the well, the ring, and the text laid out inside them.
///
/// The look is [`crate::ui::field`]'s, so this agrees with every other control
/// in the shell; only what is inside the box is this module's.
///
/// The returned element carries the click and drag handlers that place the
/// caret, but no `on_click` of the caller's — which field this is, is the
/// dialog's business. Chain `.on_click` onto it.
pub(crate) fn text_field(
    input: &Entity<TextInput>,
    id: impl Into<ElementId>,
    invalid: bool,
    style: Style<'_>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let t = style.theme;
    grant_focus(input, window, cx);
    let element = laid_out(input, &style, cx);
    let state = input.read(cx);

    // The masked text, not the real one. It only decides whether the well
    // considers itself empty, and the two agree on that — but a secret handed
    // to a widget that has no use for it is a secret in one more place.
    let mut well = field(id, state.shown(), state.placeholder.clone())
        .focused(state.focus.is_focused(window))
        .invalid(invalid);
    if let Some(which) = style.leading {
        well = well.leading(which);
    }

    let drawn = well
        .body(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(element)
                .into_any_element(),
        )
        .render(t)
        // Without this the field's handle is focused but reaches nothing: key
        // events dispatch along the tree of elements that track focus, so a
        // handle no element claims has no path for a key to travel.
        .track_focus(state.focus_handle());

    wired(drawn, input)
}

/// The same field with no box around it: one line of text, its caret and its
/// selection, sized and coloured by whatever the caller put them in.
///
/// For the surfaces that are a *query line* rather than a control — the
/// command palette, the file finder, a menu's search row. Those were the last
/// places still accumulating characters into a `String`, which is why none of
/// them could be pasted into or clicked to correct a typo. Giving them the
/// same [`TextInput`] costs them nothing visually: the chrome was never this
/// module's, only the text inside it.
pub(crate) fn text_line(
    input: &Entity<TextInput>,
    id: impl Into<ElementId>,
    style: Style<'_>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    grant_focus(input, window, cx);
    let element = laid_out(input, &style, cx);

    let drawn = div()
        .id(id)
        // The same face as the boxed field's: a query is typed as words,
        // wherever the caller has put the line.
        .prose()
        .flex()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .cursor_text()
        .child(element)
        .track_focus(input.read(cx).focus_handle());

    wired(drawn, input)
}

impl Shell {
    /// Repaints the shell whenever `input` changes.
    ///
    /// A field notifies its own entity when its text or its caret moves, and
    /// the shell is what draws it — nothing else would tell the window that
    /// what it painted last frame is now wrong. Without this a caret placed
    /// by a click sits where it was until some other notification happens to
    /// schedule a frame, which is the blink timer, half a second later.
    pub(crate) fn watch_field(&self, input: &Entity<TextInput>, cx: &mut Context<Self>) {
        cx.observe(input, |_, _, cx| cx.notify()).detach();
    }
}

/// The shell's own text fields, one for each surface that takes typing.
///
/// On the shell rather than inside the states they serve, because those are
/// rebuilt wholesale — [`crate::explorer::Explorer`] is replaced every time
/// the selection moves, and both pickers are emptied on every open — while a
/// text field cannot be. It is the entity macOS holds an input handler
/// against, so a fresh one is a field that has just lost the keyboard, and
/// the character that arrives next goes wherever the old focus pointed.
pub(crate) struct Searches {
    /// The command palette's query line.
    pub(crate) palette: Entity<TextInput>,
    /// The file finder's.
    pub(crate) finder: Entity<TextInput>,
    /// The ⌘/ snippet picker's — see [`crate::snippet_picker`].
    pub(crate) snippet: Entity<TextInput>,
    /// The file panel's search box.
    pub(crate) explorer: Entity<TextInput>,
    /// The sidebar's search — see [`crate::search`].
    pub(crate) sidebar: Entity<TextInput>,
    /// Workspace-wide text search in the right panel.
    pub(crate) workspace: Entity<TextInput>,
    /// Files included in workspace search.
    pub(crate) workspace_include: Entity<TextInput>,
    /// Files excluded from workspace search.
    pub(crate) workspace_exclude: Entity<TextInput>,
}

impl Searches {
    /// Builds the three, empty.
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            palette: TextInput::new("Type a command\u{2026}", cx),
            finder: TextInput::new("Go to file\u{2026}", cx),
            snippet: TextInput::new("Send a snippet\u{2026}", cx),
            explorer: TextInput::new("Find files", cx),
            sidebar: TextInput::new("Search", cx),
            workspace: TextInput::new("Search files", cx),
            workspace_include: TextInput::new("Files to include", cx),
            workspace_exclude: TextInput::new("Files to exclude", cx),
        }
    }

    /// Every field in it, for the caller that has to treat them alike —
    /// observing them, or asking whether any has the keyboard.
    pub(crate) fn all(&self) -> [&Entity<TextInput>; 8] {
        [
            &self.palette,
            &self.finder,
            &self.snippet,
            &self.explorer,
            &self.sidebar,
            &self.workspace,
            &self.workspace_include,
            &self.workspace_exclude,
        ]
    }
}

/// One line's worth of `text`: a paste or a dead-key sequence that carries a
/// newline gets it dropped rather than swallowing the rest of the line.
fn flatten(text: &str) -> String {
    text.replace(['\n', '\r'], "")
}
