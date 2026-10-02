//! Find in a terminal: `Cmd-F` over the pane's whole scrollback.
//!
//! The search itself is `alacritty_terminal`'s — [`RegexSearch`] runs over
//! the grid's cells, wide characters and wrapped lines included, which is
//! work nobody should redo. What this owns is the query field, which match
//! navigation is on, and handing the pane what to highlight.
//!
//! Three decisions worth stating:
//!
//! - **The query is literal text.** It is escaped before it becomes a regex,
//!   so `foo(bar)` finds `foo(bar)`. Case follows the query, alacritty's
//!   own rule: all lower case matches either case, and one capital makes it
//!   exact.
//! - **Enter goes up.** Output is read bottom to top — the newest line is
//!   the last one — so the first match is the one nearest the bottom of what
//!   is on screen, and Enter walks back through history. Shift-Enter comes
//!   forward again.
//! - **Matches are found again every frame, not remembered.** A grid point
//!   names a line by its distance from the live screen, so every line of
//!   output a program writes moves every stored match one line out from
//!   under its text. The pane runs the search over just the rows on screen
//!   when it draws, which costs a screenful of cells, and the current match is
//!   lit only where it is still one of those.

use gpui::{AnyElement, Context, Entity, SharedString, Window, div, prelude::*, px};
use ket_core::terminal::alacritty_terminal::index::{Column, Direction, Line, Point as GridPoint};
use ket_core::terminal::alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};

use super::{TerminalHandle, TerminalId};
use crate::Shell;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::ui::button::icon_button;
use crate::ui::chip::caption;
use crate::ui::icon::Icon;

/// The most matches counted for the "3 of 12" read-out. Past this the count
/// says so rather than walking the rest of a very long scrollback on every
/// keystroke.
const MAX_COUNTED: usize = 9_999;

/// The query field's width in the strip.
const FIELD_W: gpui::Pixels = px(300.0);

/// One terminal's find session, on the shell's side of the pane.
pub(crate) struct TerminalFind {
    /// What has been typed.
    query: Entity<TextInput>,
    /// The text last searched for. The field notifies on a caret move too,
    /// and only a change of text should jump.
    searched: String,
    /// The match navigation last landed on.
    current: Option<Match>,
    /// Which one that is, counting from the top of the scrollback.
    index: Option<usize>,
    /// How many there are, up to [`MAX_COUNTED`].
    total: usize,
}

impl TerminalFind {
    /// Whether the keyboard is in the query field.
    pub(crate) fn query_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.query.read(cx).is_focused(window)
    }
}

/// What the pane needs to light the matches it draws: the compiled query and
/// the match navigation is on. Pushed to the view rather than read off the
/// shell — see `TerminalView`.
pub(crate) struct FindPaint {
    /// The query, compiled.
    pub(crate) regex: RegexSearch,
    /// The match to draw as the current one.
    pub(crate) current: Option<Match>,
}

/// `text` as a regex that matches exactly it.
fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Every match in the scrollback, top to bottom, up to [`MAX_COUNTED`].
fn all_matches<T>(
    term: &ket_core::terminal::alacritty_terminal::Term<T>,
    regex: &mut RegexSearch,
) -> Vec<Match> {
    use ket_core::terminal::alacritty_terminal::grid::Dimensions;
    let start = GridPoint::new(Line(-(term.history_size() as i32)), Column(0));
    let end = GridPoint::new(
        Line(term.screen_lines() as i32 - 1),
        Column(term.columns().saturating_sub(1)),
    );
    RegexIter::new(start, end, Direction::Right, term, regex)
        .take(MAX_COUNTED + 1)
        .collect()
}

/// Every match on screen, for the pane to light.
pub(crate) fn visible_matches<T>(
    term: &ket_core::terminal::alacritty_terminal::Term<T>,
    regex: &mut RegexSearch,
) -> Vec<Match> {
    use ket_core::terminal::alacritty_terminal::grid::Dimensions;
    let offset = term.grid().display_offset() as i32;
    let start = GridPoint::new(Line(-offset), Column(0));
    let end = GridPoint::new(
        Line(term.screen_lines() as i32 - 1 - offset),
        Column(term.columns().saturating_sub(1)),
    );
    RegexIter::new(start, end, Direction::Right, term, regex).collect()
}

/// Where navigation goes from.
#[derive(Clone, Copy)]
enum Step {
    /// A new query: the match nearest the bottom of the screen.
    Nearest,
    /// Enter: the one before the current match, wrapping to the last.
    Up,
    /// Shift-Enter: the one after it, wrapping to the first.
    Down,
}

impl Shell {
    /// Whether any terminal's find strip is open, and so holding the
    /// keyboard. Asked by `Shell::typing`.
    pub(crate) fn terminal_find_open(&self) -> bool {
        self.terminals.values().any(|handle| handle.find.is_some())
    }

    /// `Cmd-F` in a terminal: opens the strip on a focused field, or, when
    /// it is already open, puts the keyboard back in it with the query
    /// selected. What is selected in the grid, if it is one line, is what
    /// the field opens on.
    pub(crate) fn open_terminal_find(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        if let Some(find) = &handle.find {
            find.query.update(cx, |input, _| {
                input.select_all();
                input.request_focus();
            });
            return;
        }
        let seed = handle
            .term
            .with_term(|term| term.selection_to_string())
            .filter(|text| !text.is_empty() && !text.contains('\n'));
        let query = match &seed {
            Some(text) => TextInput::with_text("Find\u{2026}", text, cx),
            None => TextInput::new("Find\u{2026}", cx),
        };
        query.update(cx, |input, _| input.request_focus());
        cx.observe(&query, move |shell, query, cx| {
            let text = query.read(cx).text();
            let changed = shell
                .terminals
                .get(&id)
                .and_then(|handle| handle.find.as_ref())
                .is_some_and(|find| find.searched != text);
            if changed {
                shell.refresh_terminal_find(id, Step::Nearest, cx);
                cx.notify();
            }
        })
        .detach();
        handle.find = Some(TerminalFind {
            query,
            searched: String::new(),
            current: None,
            index: None,
            total: 0,
        });
        if seed.is_some() {
            self.refresh_terminal_find(id, Step::Nearest, cx);
        }
    }

    /// Closes the strip and puts the keyboard back in the grid.
    fn close_terminal_find(&mut self, id: TerminalId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        handle.find = None;
        handle.view.update(cx, |view, cx| {
            view.find = None;
            cx.notify();
        });
        window.focus(&self.focus);
    }

    /// Runs the query again and moves to the match `step` names, scrolling
    /// it into view.
    fn refresh_terminal_find(&mut self, id: TerminalId, step: Step, cx: &mut Context<Self>) {
        let Some(handle) = self.terminals.get_mut(&id) else {
            return;
        };
        let Some(find) = handle.find.as_mut() else {
            return;
        };
        let text = find.query.read(cx).text();
        find.searched = text.clone();
        let regex = (!text.is_empty())
            .then(|| RegexSearch::new(&literal(&text)).ok())
            .flatten();
        let Some(mut regex) = regex else {
            find.current = None;
            find.index = None;
            find.total = 0;
            handle.view.update(cx, |view, cx| {
                view.find = None;
                cx.notify();
            });
            return;
        };

        let previous = find.current.clone();
        let (current, index, total) = handle.term.with_term(|term| {
            let matches = all_matches(term, &mut regex);
            let total = matches.len();
            if matches.is_empty() {
                return (None, None, 0);
            }
            let last = matches.len() - 1;
            // Where the current match sits in the fresh list, or the first
            // one past it when output has moved it.
            let at = previous.as_ref().map(|current| {
                matches
                    .iter()
                    .position(|m| m.start() >= current.start())
                    .unwrap_or(matches.len())
            });
            let same = |at: usize| previous.as_ref() == matches.get(at);
            let index = match (step, at) {
                (Step::Nearest, _) | (_, None) => {
                    use ket_core::terminal::alacritty_terminal::grid::Dimensions;
                    let bottom =
                        term.screen_lines() as i32 - 1 - term.grid().display_offset() as i32;
                    matches
                        .iter()
                        .rposition(|m| m.start().line.0 <= bottom)
                        .unwrap_or(last)
                }
                (Step::Up, Some(at)) => at.checked_sub(1).unwrap_or(last),
                (Step::Down, Some(at)) if same(at) => (at + 1) % matches.len(),
                (Step::Down, Some(at)) => at % matches.len(),
            };
            let current = matches[index].clone();
            term.scroll_to_point(*current.start());
            (Some(current), Some(index), total)
        });
        find.current = current.clone();
        find.index = index;
        find.total = total;
        handle.view.update(cx, |view, cx| {
            view.find = Some(FindPaint { regex, current });
            cx.notify();
        });
    }

    /// Keys for a terminal with its strip open. `Cmd-G` and `Shift-Cmd-G`
    /// step wherever the keyboard is; the rest only while the field has it.
    /// Returns whether the key was taken.
    pub(crate) fn terminal_find_key(
        &mut self,
        id: TerminalId,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(query) = self
            .terminals
            .get(&id)
            .and_then(|handle: &TerminalHandle| handle.find.as_ref())
            .map(|find| find.query.clone())
        else {
            return false;
        };
        let m = keystroke.modifiers;
        if m.platform && keystroke.key == "g" {
            let step = if m.shift { Step::Down } else { Step::Up };
            self.refresh_terminal_find(id, step, cx);
            return true;
        }
        if !query.read(cx).is_focused(window) {
            return false;
        }
        self.caret_wake(cx);
        match keystroke.key.as_str() {
            "escape" => self.close_terminal_find(id, window, cx),
            "enter" => {
                let step = if m.shift { Step::Down } else { Step::Up };
                self.refresh_terminal_find(id, step, cx);
            }
            // Text is never taken here: it has to keep travelling until
            // macOS's input context sees it — see [`crate::input`].
            _ if is_text(keystroke) => return false,
            _ => {
                query.update(cx, |input, cx| input.key(keystroke, cx));
            }
        }
        true
    }

    /// The strip over a terminal with find open: the field, where navigation
    /// is, and the buttons for what Enter, Shift-Enter and Escape do.
    pub(crate) fn terminal_find_bar(
        &self,
        id: TerminalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let find = self.terminals.get(&id)?.find.as_ref()?;
        let t = &self.theme;
        let empty = find.query.read(cx).text().is_empty();
        let count: SharedString = match (find.index, find.total) {
            _ if empty => "".into(),
            (_, 0) => "No results".into(),
            (Some(index), total) if total > MAX_COUNTED => {
                format!("{} of {MAX_COUNTED}+", index + 1).into()
            }
            (Some(index), total) => format!("{} of {total}", index + 1).into(),
            (None, total) => format!("{total}").into(),
        };
        let found = find.total > 0;
        let key = id.0;

        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_end()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(6.0))
                .bg(paint(t.panel))
                .border_b_1()
                .border_color(paint(t.border))
                .child(caption(count, t).mr(px(4.0)))
                .child(
                    text_field(
                        &find.query,
                        ("terminal-find", key),
                        !empty && !found,
                        Style::new(t, self.caret.visible).leading(Icon::Search),
                        window,
                        cx,
                    )
                    .w(FIELD_W),
                )
                .child(
                    icon_button(("terminal-find-up", key), Icon::ChevronUp)
                        .bare()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.refresh_terminal_find(id, Step::Up, cx);
                            cx.notify();
                        })),
                )
                .child(
                    icon_button(("terminal-find-down", key), Icon::ChevronDown)
                        .bare()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.refresh_terminal_find(id, Step::Down, cx);
                            cx.notify();
                        })),
                )
                .child(
                    icon_button(("terminal-find-close", key), Icon::Close)
                        .bare()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_terminal_find(id, window, cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}
