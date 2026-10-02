//! The snippet picker: ⌘/ from anywhere, a snippet picked, sent.
//!
//! The menus in [`crate::snippets`] hang off a worktree row or an agent tab,
//! which means reaching for the mouse and finding the row first. This is the
//! same pick from the keyboard, on the palette's surface: type to narrow,
//! Enter to send. The snippet goes to the selected worktree's agent the way
//! the worktree menu's **Send snippet** sends it — pasted into its open agent
//! tab and submitted, the focused one first — see
//! [`Shell::send_prompt_to_worktree`].

use gpui::{AnyElement, App, Context, KeyDownEvent, ScrollHandle, Window, div, prelude::*};
use ket_core::id::WorktreeId;
use ket_core::snippets::Snippet;

use crate::Shell;
use crate::input::{Style, is_text, text_line};
use crate::paint::paint;
use crate::palette::score;
use crate::snippets::preview;
use crate::ui::chip::caption;
use crate::ui::icon::{Icon, icon};
use crate::ui::picker::{hints, lit, overlay, panel, query_line, results, row};
use crate::ui::toast::Tone;

/// One row of the picker.
enum Candidate {
    /// The snippet at this index, and which characters of its name matched.
    Use { index: usize, hits: Vec<usize> },
    /// Settings, on the Snippets section — last, and the only row when
    /// nothing is saved yet.
    Manage,
}

/// The open picker.
pub(crate) struct SnippetPicker {
    /// Who a pick goes to, fixed when it opened: the selection can change
    /// behind the overlay only by a click that closes it, but a send must
    /// never land somewhere the header did not name.
    target: WorktreeId,
    /// The target's branch, for the query line.
    branch: gpui::SharedString,
    /// Read fresh on opening — see [`Shell::load_snippets`].
    snippets: Vec<Snippet>,
    candidates: Vec<Candidate>,
    pub(crate) selected: usize,
    scroll: ScrollHandle,
}

impl Shell {
    /// Opens the picker on the selected worktree, or says why it cannot.
    pub(crate) fn open_snippet_picker(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            self.note = Some("Select a worktree to send a snippet to".into());
            return;
        };
        let Some(worktree) = self
            .projects
            .get(selection.project)
            .and_then(|project| project.worktrees.get(selection.worktree))
        else {
            return;
        };
        let (target, branch) = (worktree.id.clone(), worktree.branch.clone());
        let snippets = self.load_snippets(cx);

        // One overlay at a time, as the palette has it.
        self.menu = None;
        self.popup = None;
        self.snippet_picker = Some(SnippetPicker {
            target,
            branch,
            snippets,
            candidates: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
        });
        // The overlay holds the keyboard itself, for the reason
        // `open_palette` gives.
        self.searches.snippet.update(cx, |input, cx| {
            input.clear();
            input.request_focus();
            cx.notify();
        });
        self.rank_snippet_picker(cx);
    }

    /// Closes it without sending anything.
    pub(crate) fn close_snippet_picker(&mut self, cx: &mut Context<Self>) {
        self.snippet_picker = None;
        self.searches.snippet.update(cx, |input, _| input.clear());
    }

    /// Re-ranks the rows against the query: saved order while it is empty,
    /// best match first once something is typed.
    pub(crate) fn rank_snippet_picker(&mut self, cx: &App) {
        let query = self.searches.snippet.read(cx).text();
        let Some(open) = self.snippet_picker.as_mut() else {
            return;
        };
        let mut scored: Vec<(i32, usize, Vec<usize>)> = open
            .snippets
            .iter()
            .enumerate()
            .filter_map(|(index, snippet)| {
                let (points, hits) = score(&query, &snippet.name)?;
                Some((points, index, hits))
            })
            .collect();
        // Stable, so ties keep the order the person arranged them in.
        scored.sort_by_key(|(points, _, _)| std::cmp::Reverse(*points));
        open.candidates = scored
            .into_iter()
            .map(|(_, index, hits)| Candidate::Use { index, hits })
            .collect();
        open.candidates.push(Candidate::Manage);
        open.selected = open.selected.min(open.candidates.len() - 1);
    }

    /// Sends the selected row's snippet, or goes to settings.
    fn run_snippet_picker(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.snippet_picker.take() else {
            return;
        };
        self.searches.snippet.update(cx, |input, _| input.clear());
        match open.candidates.get(open.selected) {
            Some(Candidate::Use { index, .. }) => {
                let Some(snippet) = open.snippets.get(*index) else {
                    return;
                };
                self.toast(
                    Tone::Info,
                    format!("Sent “{}” to {}", snippet.name, open.branch),
                    cx,
                );
                self.send_prompt_to_worktree(open.target, snippet.body.clone(), cx);
            }
            Some(Candidate::Manage) => self.manage_snippets(cx),
            None => {}
        }
    }

    /// Handles a key while the picker is open. Everything is swallowed, as
    /// the palette swallows it.
    pub(crate) fn snippet_picker_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.snippet_picker.is_none() {
            return false;
        }
        // The query line first, for the reason `palette_key` gives.
        let query = self.searches.snippet.clone();
        if is_text(&event.keystroke) {
            return true;
        }
        if query.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_snippet_picker(cx),
            "enter" => self.run_snippet_picker(cx),
            // ⌘/ again closes it, the way it toggles the quick prompt's menu.
            "/" if event.keystroke.modifiers.platform || event.keystroke.modifiers.control => {
                self.close_snippet_picker(cx)
            }
            "down" | "up" => {
                if let Some(open) = self.snippet_picker.as_mut() {
                    let last = open.candidates.len().saturating_sub(1);
                    open.selected = if event.keystroke.key == "down" {
                        (open.selected + 1).min(last)
                    } else {
                        open.selected.saturating_sub(1)
                    };
                    open.scroll.scroll_to_item(open.selected);
                }
            }
            _ => {}
        }
        true
    }

    /// The picker overlay, or nothing when it is closed.
    pub(crate) fn snippet_picker_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.snippet_picker.as_ref()?;
        let t = &self.theme;

        let query = query_line(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().child(text_line(
                    &self.searches.snippet,
                    "snippet-picker-query",
                    Style::new(t, self.caret.visible),
                    window,
                    cx,
                )))
                // Where Enter sends it, said before it is pressed.
                .child(
                    div()
                        .flex_none()
                        .font_family(crate::fonts::chrome())
                        .child(caption(format!("to {}", open.branch), t)),
                ),
            t,
        );

        let empty = open.snippets.is_empty();
        let rows =
            open.candidates
                .iter()
                .enumerate()
                .map(|(position, candidate)| {
                    let el = row(
                        ("snippet-picker-row", position),
                        position == open.selected,
                        t,
                    );
                    let el = match candidate {
                        Candidate::Use { index, hits } => {
                            let snippet = &open.snippets[*index];
                            el.child(div().flex_none().child(lit(&snippet.name, hits, t)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(caption(preview(&snippet.body), t)),
                                )
                        }
                        Candidate::Manage => el
                            .child(icon(Icon::Sliders, paint(t.text.dim)))
                            .child(if empty {
                                "No snippets yet — make one in settings"
                            } else {
                                "Manage snippets…"
                            }),
                    };
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(open) = this.snippet_picker.as_mut() {
                            open.selected = position;
                        }
                        this.run_snippet_picker(cx);
                        cx.notify();
                    }))
                })
                .collect::<Vec<_>>();

        Some(
            overlay(
                panel(t)
                    .child(query)
                    .child(results("snippet-picker-rows", &open.scroll).children(rows))
                    .child(hints(
                        &[
                            ("Enter", "Send"),
                            ("Esc", "Close"),
                            ("\u{2191}\u{2193}", "Move"),
                        ],
                        self.font_family.clone(),
                        t,
                    )),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_snippet_picker(cx);
                    cx.notify();
                }),
            )
            .into_any_element(),
        )
    }
}
