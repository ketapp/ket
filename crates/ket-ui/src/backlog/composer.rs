//! The line a new note is written in, at the top of the backlog.
//!
//! One line at rest: a title, its `#tags`, and ↵ to add it — writing
//! something down to come back to is what the dialog is for, and that has to
//! stay one keystroke. ⇥ opens a description under the title, with the
//! note's priority, its tags, files to send with it and Add, for a note worth
//! more than a line; ⇧⇥ goes back up to the title, and Escape clears the
//! lot. While it is open the whole composer is one well, ringed in the
//! accent, so it reads as the thing being written in.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, FontWeight, PathPromptOptions, Pixels, Window, div, prelude::*, px,
};
use ket_core::backlog::normalised_tags;

use super::organise::split_tags;
use super::{BODY_TEXT, CAPTURE_PRIORITY, COMPOSER_GUTTER, backlog_hint};
use crate::Shell;
use crate::input::{Style, text_field, text_line};
use crate::paint::{alpha, paint};
use crate::ui::button::button;
use crate::ui::chip::{key_hint, removable};
use crate::ui::field::placeholder;
use crate::ui::icon::{Icon, sized_icon};

/// The composer's description, under a key of its own: the open note's is
/// [`super::BODY_EDITOR`], and both can be open at once.
pub(super) const CAPTURE_EDITOR: &str = "backlog:capture";

/// The composer at rest: a size up from a settings field, since writing a
/// note is the thing this dialog is for.
pub(super) const CAPTURE_H: Pixels = px(50.0);
const CAPTURE_TEXT: Pixels = px(16.0);

/// How tall the description is: three lines, then it scrolls.
const DETAILS_H: Pixels = px(66.0);

/// Where the title's words start, past the `+`: the description and the
/// tool row line up under them.
const TEXT_INSET: Pixels = px(45.0);

/// The composer's corner: a step rounder than a field, as the larger well.
const COMPOSER_RADIUS: Pixels = px(11.0);

impl Shell {
    /// Whether the composer has its description open.
    pub(super) fn backlog_composing(&self) -> bool {
        self.backlog.as_ref().is_some_and(|dialog| dialog.composing)
    }

    /// ⇥ from the title: opens the description under it, with the keyboard
    /// in it.
    pub(super) fn open_backlog_details(&mut self, window: &mut Window) {
        let Some(dialog) = self.backlog.as_ref() else {
            return;
        };
        if !dialog.composing {
            self.open_composer(CAPTURE_EDITOR, "");
            self.set_composer_size(CAPTURE_EDITOR, BODY_TEXT);
        }
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.composing = true;
            dialog.capture_body_active = true;
            dialog.body_active = false;
        }
        window.focus(&self.focus);
    }

    /// ⇧⇥ from the description: back up to the title.
    pub(super) fn backlog_details_to_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        dialog.capture_body_active = false;
        let capture = dialog.capture.clone();
        window.focus(capture.read(cx).focus_handle());
    }

    /// Empties the composer — title, description, tags and files — and folds
    /// it back to a line, with the keyboard in it for the next note.
    pub(super) fn clear_backlog_composer(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let was_composing = dialog.composing;
        dialog.composing = false;
        dialog.capture_body_active = false;
        dialog.capture_files.clear();
        dialog.capture_priority = Default::default();
        dialog.capture.update(cx, |input, _| {
            input.clear();
            input.request_focus();
        });
        if was_composing {
            self.close_composer(CAPTURE_EDITOR);
        }
    }

    /// What the composer's description holds, trimmed of the trailing blank
    /// lines the box keeps.
    pub(super) fn backlog_details_text(&self) -> String {
        if !self.backlog_composing() {
            return String::new();
        }
        self.composer_text(CAPTURE_EDITOR).trim_end().to_owned()
    }

    /// Keys while the description has the keyboard. It takes every one but
    /// the chords the dialog answers to above it.
    pub(super) fn backlog_details_key(
        &mut self,
        key: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match key.key.as_str() {
            "tab" if key.modifiers.shift => self.backlog_details_to_title(window, cx),
            "escape" => self.clear_backlog_composer(cx),
            _ => self.composer_key(CAPTURE_EDITOR, key, cx),
        }
    }

    /// The Tag button: a `#` after the title, with the keyboard there to
    /// finish it.
    fn start_backlog_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        dialog.capture_body_active = false;
        let capture = dialog.capture.clone();
        capture.update(cx, |input, _| {
            let text = input.text();
            let joined = if text.is_empty() || text.ends_with(' ') {
                format!("{text}#")
            } else {
                format!("{text} #")
            };
            input.set_text(&joined);
        });
        window.focus(capture.read(cx).focus_handle());
    }

    /// Takes the tag `name` out of the title it was written in.
    fn remove_capture_tag(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_ref() else {
            return;
        };
        let wanted = name.to_lowercase();
        dialog.capture.update(cx, |input, _| {
            let kept: Vec<String> = input
                .text()
                .split_whitespace()
                .filter(|word| {
                    !word.starts_with('#')
                        || normalised_tags([*word])
                            .first()
                            .is_none_or(|tag| tag.to_lowercase() != wanted)
                })
                .map(str::to_owned)
                .collect();
            input.set_text(&kept.join(" "));
        });
    }

    /// The Attach button: files to send with the note once it is added.
    fn choose_capture_files(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.hold_capture_files(paths);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps `paths` for the note being written, which has no files of its
    /// own until it is added.
    pub(super) fn hold_capture_files(&mut self, paths: Vec<PathBuf>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        for path in paths {
            if path.is_dir() {
                dialog.error = Some(format!(
                    "{} is a folder; attach the files in it instead.",
                    path.display()
                ));
            } else if !dialog.capture_files.contains(&path) {
                dialog.capture_files.push(path);
            }
        }
    }

    /// The description, built ahead of the dialog because the editor it
    /// draws needs the shell mutably. `None` while the composer is a line.
    pub(super) fn backlog_details_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        if !dialog.composing {
            return None;
        }
        let active = dialog.capture_body_active && self.focus.is_focused(window);
        self.set_composer_caret(CAPTURE_EDITOR, active);
        let empty = self.composer_text(CAPTURE_EDITOR).is_empty();
        let pane = self.editor_pane(CAPTURE_EDITOR, None, window, cx);
        let t = &self.theme;
        Some(
            div()
                .id("backlog-details")
                .relative()
                .h(DETAILS_H)
                .pl(TEXT_INSET - COMPOSER_GUTTER)
                .pr(px(12.0))
                .cursor_text()
                .child(pane)
                // Over an empty description, where the typed words will
                // start.
                .when(empty, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top_0()
                            .left(TEXT_INSET)
                            .text_size(BODY_TEXT)
                            .text_color(placeholder(t))
                            .child("Details: what to change, where, and how to tell it worked"),
                    )
                })
                .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.capture_body_active = true;
                        dialog.body_active = false;
                    }
                    window.focus(&this.focus);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The composer: one line at rest, or the whole well while a note with
    /// a description is being written.
    pub(super) fn backlog_composer(
        &self,
        details: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        let t = &self.theme;
        let priority = self.backlog_priority_picker(CAPTURE_PRIORITY, dialog.capture_priority, cx);
        let Some(details) = details else {
            let quiet = div()
                .flex()
                .flex_none()
                .items_center()
                .px(px(6.0))
                .child(key_hint("\u{21e5}", "details", t));
            return Some(
                text_field(
                    &dialog.capture,
                    "backlog-capture",
                    false,
                    Style::new(t, self.caret.visible).leading(Icon::Plus),
                    window,
                    cx,
                )
                .h(CAPTURE_H)
                .pl(px(16.0))
                .pr(px(10.0))
                .gap(px(12.0))
                .rounded(COMPOSER_RADIUS)
                .text_size(CAPTURE_TEXT)
                .child(quiet)
                .child(priority)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.body_active = false;
                        dialog.capture_body_active = false;
                    }
                    cx.notify();
                }))
                .into_any_element(),
            );
        };

        let title = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .pt(px(14.0))
            .pb(px(6.0))
            .text_size(CAPTURE_TEXT)
            .font_weight(FontWeight::MEDIUM)
            .child(sized_icon(Icon::Plus, px(16.0), paint(t.accent)))
            .child(
                text_line(
                    &dialog.capture,
                    "backlog-capture-title",
                    Style::new(t, self.caret.visible),
                    window,
                    cx,
                )
                .flex_1()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.capture_body_active = false;
                        dialog.body_active = false;
                    }
                    cx.notify();
                })),
            );

        let (_, tags) = split_tags(&dialog.capture.read(cx).text());
        let tag_chips: Vec<_> = tags
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                removable(format!("#{name}"), t)
                    .id(("backlog-capture-tag", index))
                    .font_family(crate::fonts::chrome())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_capture_tag(&name, cx);
                        cx.notify();
                    }))
            })
            .collect();
        let file_chips: Vec<_> = dialog
            .capture_files
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let held = path.clone();
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                removable(name, t)
                    .id(("backlog-capture-file", index))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(dialog) = this.backlog.as_mut() {
                            dialog.capture_files.retain(|path| *path != held);
                        }
                        cx.notify();
                    }))
            })
            .collect();

        let tools = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .ml(TEXT_INSET)
            .mr(px(10.0))
            .mt(px(8.0))
            .mb(px(10.0))
            .pt(px(10.0))
            .border_t_1()
            .border_color(alpha(paint(t.border), 0.6))
            .child(priority)
            .children(tag_chips)
            .children(file_chips)
            .child(
                button("backlog-capture-tag-add", "Tag")
                    .ghost()
                    .small()
                    .leading(Icon::Tag)
                    .render(t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start_backlog_tag(window, cx);
                        cx.notify();
                    })),
            )
            .child(
                button("backlog-capture-attach", "Attach")
                    .ghost()
                    .small()
                    .leading(Icon::Paperclip)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.choose_capture_files(cx);
                        cx.notify();
                    })),
            )
            .child(div().flex_grow())
            .child(
                backlog_hint("backlog-capture-clear", "esc", "clear", t).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.clear_backlog_composer(cx);
                        cx.notify();
                    },
                )),
            )
            .child(
                button("backlog-capture-add", "Add")
                    .primary()
                    .small()
                    .hint("\u{2318}\u{21b5}")
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_backlog_note(cx);
                        cx.notify();
                    })),
            );

        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .rounded(COMPOSER_RADIUS)
                .bg(paint(t.sunken))
                .border_1()
                .border_color(alpha(paint(t.focus_ring), 0.55))
                .child(title)
                .child(details)
                .child(tools)
                .into_any_element(),
        )
    }
}
