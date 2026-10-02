//! Taking a note for a backlog from anywhere: ⇧⌘A puts up a small panel
//! over the window, on the project in context, with whatever the tab in
//! front has selected quoted under it — a failing test or a line an agent
//! wrote in a terminal, a few lines of a file, the lines picked in a diff,
//! a passage on a web page —
//! so it can be written down without leaving what is being read. The quote
//! says where it came from: the worktree, or the file and its lines. ↵ adds
//! the note and gets out of the way; ⌘↵ adds it and opens the backlog on it.

use gpui::{
    AnyElement, Context, Entity, FocusHandle, FontWeight, KeyDownEvent, MouseButton, Pixels,
    Window, div, prelude::*, px,
};
use ket_core::backlog::{Backlog, Note, Priority};
use ket_core::id::ProjectId;

use super::organise::split_tags;
use super::{priority_icon, priority_mark, priority_tint};
use crate::Shell;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::{alpha, paint};
use crate::ui::button::{icon_button, pressable};
use crate::ui::chip::{badge, caption, key_hint, tinted_tag};
use crate::ui::dialog::card;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;
use crate::ui::toast::Tone;

/// The panel's width.
const PANEL_W: Pixels = px(640.0);

/// How far down the window it sits: near the top, where a glance goes,
/// rather than over the middle of what was being read.
const PANEL_TOP: Pixels = px(96.0);

/// The most of a selection kept, in characters. A whole scrollback is not a
/// note.
const QUOTE_CHARS: usize = 6000;

/// How tall the quote may get before it scrolls.
const QUOTE_H: Pixels = px(150.0);

/// The two pickers, each its menu's origin.
const PROJECT_PICKER: &str = "capture-project";
const PRIORITY_PICKER: &str = "capture-priority";

/// What a menu in the panel picks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CapturePick {
    /// An index into `Shell::projects`.
    Project(usize),
    Priority(Priority),
}

/// What was selected when the panel opened, quoted under the note.
struct Quote {
    /// The words, cut at [`QUOTE_CHARS`].
    text: String,
    /// Where they came from: the worktree whose terminal it was, or the file
    /// and its lines.
    from: String,
    /// The language a code fence names for them: a file's extension, `diff`
    /// for a diff's lines, nothing for a terminal's.
    lang: String,
    /// What the panel marks the quote with.
    icon: Icon,
}

impl Quote {
    fn new(text: &str, from: String, lang: &str, icon: Icon) -> Self {
        let text = text.trim_end_matches(['\n', '\r']);
        let text = match text.char_indices().nth(QUOTE_CHARS) {
            Some((cut, _)) => format!("{}\u{2026}", &text[..cut]),
            None => text.to_owned(),
        };
        Self {
            text,
            from,
            lang: lang.to_owned(),
            icon,
        }
    }

    /// The quote as it goes in the note: where it came from, then a fenced
    /// block — fenced with one more backtick than the longest run in it, so
    /// a quote holding a fence of its own cannot close this one.
    fn markdown(&self) -> String {
        let longest = self
            .text
            .split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat((longest + 1).max(3));
        format!(
            "From {}:\n\n{fence}{}\n{}\n{fence}",
            self.from, self.lang, self.text
        )
    }
}

/// The panel, while it is up.
pub(crate) struct QuickCapture {
    /// Which project's backlog it goes in, an index into `Shell::projects`.
    project: usize,
    /// The note's title, `#tags` and all.
    title: Entity<TextInput>,
    priority: Priority,
    /// What was selected when it opened, quoted under the note.
    quote: Option<Quote>,
    /// A web page has been asked for its selection, which comes back a
    /// moment after the panel opens.
    asking_page: bool,
    /// A picker's menu, while one is open.
    menu: Option<OpenMenu<CapturePick>>,
    /// Where the keyboard waits while a menu is open.
    menu_focus: FocusHandle,
    /// Why the note could not be added.
    error: Option<String>,
}

impl Shell {
    /// What the tab in front has selected: a terminal's selection, an
    /// editor's, or the lines picked in a diff.
    fn selected_quote(&self) -> Option<Quote> {
        if let Some(terminal) = self.active_terminal_id() {
            let text = self.terminal_selection(terminal)?;
            let worktree = self.worktree_of_terminal(terminal);
            let from = self
                .projects
                .iter()
                .flat_map(|project| project.worktrees.iter())
                .find(|node| Some(&node.id) == worktree.as_ref())
                .map_or_else(|| "a terminal".to_owned(), |node| node.label().to_string());
            return Some(Quote::new(&text, from, "", Icon::Terminal));
        }
        if let Some((text, path, first, last)) = self.editor_selection() {
            let name = path.as_deref().map_or_else(
                || "an untitled file".to_owned(),
                |path| self.worktree_relative(path),
            );
            let lines = if first == last {
                format!("{first}")
            } else {
                format!("{first}\u{2013}{last}")
            };
            let lang = path
                .as_deref()
                .and_then(|path| path.extension())
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Some(Quote::new(
                &text,
                format!("{name}:{lines}"),
                &lang,
                Icon::FileLines,
            ));
        }
        let (key, text, lines) = self.diff_selection()?;
        let against = match &key.base {
            Some((base, _)) => format!("against {base}"),
            None => "uncommitted".to_owned(),
        };
        let from = match lines.is_empty() {
            true => format!("{} ({against})", key.rel),
            false => format!("{}, {lines} ({against})", key.rel),
        };
        Some(Quote::new(&text, from, "diff", Icon::GitCompare))
    }

    /// `path` from the selected worktree's root, when it is inside it.
    fn worktree_relative(&self, path: &std::path::Path) -> String {
        self.selection
            .and_then(|selection| {
                let node = self
                    .projects
                    .get(selection.project)?
                    .worktrees
                    .get(selection.worktree)?;
                path.strip_prefix(&node.path).ok()
            })
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// ⇧⌘A: puts up the panel for the project in context, quoting what the
    /// tab in front has selected. Does nothing with no project.
    pub(crate) fn open_quick_capture(&mut self, cx: &mut Context<Self>) -> bool {
        let project = self
            .active_terminal_id()
            .and_then(|terminal| self.project_of_terminal(terminal))
            .or_else(|| self.selection.map(|selection| selection.project))
            .or_else(|| self.projects.iter().position(|project| project.expanded))
            .or_else(|| (!self.projects.is_empty()).then_some(0));
        let Some(project) = project else {
            return false;
        };
        let quote = self.selected_quote();
        // A page's selection is the page's to say, and it answers a moment
        // later; the panel opens now and the quote follows. The keyboard
        // comes back from the page first, so the title gets what is typed.
        let page = quote.is_none().then(|| self.active_browser_tab()).flatten();
        if let Some(id) = page {
            self.focus_browser_parent(id, cx);
            self.quote_browser_selection(id, cx, |shell, text, address, _| {
                if let Some(capture) = shell.quick_capture.as_mut()
                    && capture.asking_page
                {
                    capture.asking_page = false;
                    let from = if address.is_empty() {
                        "a web page".to_owned()
                    } else {
                        address
                    };
                    capture.quote = Some(Quote::new(&text, from, "", Icon::BrowserWindow));
                }
            });
        }
        let title = TextInput::new("What needs doing?  #tag", cx);
        self.watch_field(&title, cx);
        title.update(cx, |input, _| input.request_focus());
        self.popup = None;
        self.menu = None;
        self.quick_capture = Some(QuickCapture {
            project,
            title,
            priority: Priority::default(),
            quote,
            asking_page: page.is_some(),
            menu: None,
            menu_focus: cx.focus_handle(),
            error: None,
        });
        true
    }

    /// Adds the note, and puts the panel away. With `open`, the project's
    /// backlog opens on it.
    fn add_quick_capture(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(capture) = self.quick_capture.as_mut() else {
            return;
        };
        let Some(node) = self.projects.get(capture.project) else {
            return;
        };
        let project: ProjectId = node.id.clone();
        let name = node.name.clone();
        let (mut title, tags) = split_tags(&capture.title.read(cx).text());
        if title.is_empty() {
            // A selection alone is a note: its first line names it.
            if let Some(first) = capture.quote.as_ref().and_then(|quote| {
                quote
                    .text
                    .lines()
                    .map(|line| line.trim().trim_start_matches(['+', '-']).trim())
                    .find(|line| !line.is_empty())
            }) {
                title = first.chars().take(80).collect();
            } else {
                capture.error = Some("Write what it is first.".into());
                return;
            }
        }
        let notes = Backlog::load(&project).map(|backlog| backlog.notes);
        let notes = match notes {
            Ok(notes) => notes,
            Err(error) => {
                capture.error = Some(error.to_string());
                return;
            }
        };
        let mut note = Note::new(&notes);
        note.title = title;
        note.tags = tags;
        note.priority = capture.priority;
        if let Some(quote) = capture.quote.as_ref() {
            note.body = quote.markdown();
        }
        if let Err(error) = Backlog::save_note(&project, &note) {
            capture.error = Some(error.to_string());
            return;
        }
        self.quick_capture = None;
        self.recount_backlog(&project);
        if open {
            self.open_backlog_at(&project, note.id, window, cx);
        } else {
            self.toast(Tone::Info, format!("Added to {name}'s backlog"), cx);
        }
    }

    /// Opens the menu `origin` names, or closes it when it is the one open.
    fn toggle_capture_menu(&mut self, origin: &'static str, window: &mut Window) {
        let t = &self.theme;
        let Some(capture) = self.quick_capture.as_mut() else {
            return;
        };
        if let Some(menu) = capture.menu.take()
            && menu.opened_by(origin)
        {
            return;
        }
        let (items, at, width): (Vec<MenuEntry<CapturePick>>, usize, Pixels) =
            if origin == PROJECT_PICKER {
                let items = self
                    .projects
                    .iter()
                    .enumerate()
                    .map(|(index, project)| {
                        let item = MenuItem::new(CapturePick::Project(index), project.name.clone())
                            .tinted_icon(Icon::Folder, paint(project.color));
                        MenuEntry::Item(if index == capture.project {
                            item.checked()
                        } else {
                            item
                        })
                    })
                    .collect();
                (items, capture.project, px(240.0))
            } else {
                let items = Priority::ALL
                    .iter()
                    .map(|&priority| {
                        let item = MenuItem::new(CapturePick::Priority(priority), priority.name())
                            .tinted_icon(priority_icon(priority), priority_tint(priority, t));
                        MenuEntry::Item(if priority == capture.priority {
                            item.checked()
                        } else {
                            item
                        })
                    })
                    .collect();
                let at = Priority::ALL
                    .iter()
                    .position(|&priority| priority == capture.priority)
                    .unwrap_or(0);
                (items, at, px(168.0))
            };
        capture.menu = Some(OpenMenu::new(origin.into(), None, width, items).selected(at));
        window.focus(&capture.menu_focus);
    }

    /// Closes the menu, taking `picked` when one was, and gives the title
    /// the keyboard back.
    fn settle_capture_menu(&mut self, picked: Option<CapturePick>, cx: &mut Context<Self>) {
        let Some(capture) = self.quick_capture.as_mut() else {
            return;
        };
        capture.menu = None;
        match picked {
            Some(CapturePick::Project(index)) => capture.project = index,
            Some(CapturePick::Priority(priority)) => capture.priority = priority,
            None => {}
        }
        capture.title.update(cx, |input, _| input.request_focus());
    }

    /// Keys while the panel is up. It takes every one.
    pub(crate) fn quick_capture_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(capture) = self.quick_capture.as_mut() else {
            return false;
        };
        if let Some(menu) = capture.menu.as_mut() {
            match menu.key(event) {
                MenuKey::Ignored => {}
                MenuKey::Consumed => return true,
                MenuKey::Close => {
                    self.settle_capture_menu(None, cx);
                    return true;
                }
                MenuKey::Run(pick) => {
                    self.settle_capture_menu(Some(pick), cx);
                    return true;
                }
            }
        }
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        match key.key.as_str() {
            "escape" => self.quick_capture = None,
            "enter" => self.add_quick_capture(command, window, cx),
            _ => {
                // Text travels on to the input context — see
                // [`crate::input`].
                let title = capture.title.clone();
                if !is_text(key) {
                    title.update(cx, |input, cx| input.key(key, cx));
                }
            }
        }
        true
    }

    /// A picker in the panel: `trigger`, with its menu while `origin`'s is
    /// open.
    fn capture_picker(
        &self,
        origin: &'static str,
        trigger: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        let panel = self
            .quick_capture
            .as_ref()
            .and_then(|capture| capture.menu.as_ref())
            .filter(|menu| menu.opened_by(origin))
            .map(|menu| {
                menu.view(
                    t,
                    cx,
                    |shell, pick, cx| {
                        shell.settle_capture_menu(Some(*pick), cx);
                        cx.notify();
                    },
                    |shell, cx| {
                        shell.settle_capture_menu(None, cx);
                        cx.notify();
                    },
                )
            });
        dropdown((origin, 0usize), trigger, panel)
            .flex_none()
            .font_weight(FontWeight::NORMAL)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.toggle_capture_menu(origin, window);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// The panel, while it is up.
    pub(crate) fn quick_capture_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let capture = self.quick_capture.as_ref()?;
        let t = &self.theme;
        let node = self.projects.get(capture.project)?;
        let faint = alpha(paint(t.text.dim), 0.72);
        let menu_open = |origin: &str| {
            capture
                .menu
                .as_ref()
                .is_some_and(|menu| menu.opened_by(origin))
        };

        let initial: String = node
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        let project = select(PROJECT_PICKER, node.name.clone())
            .leading(badge(node.color, initial, t))
            .inline()
            .open(menu_open(PROJECT_PICKER))
            .render(t);
        let priority = select(PRIORITY_PICKER, capture.priority.name())
            .leading(priority_mark(capture.priority, t))
            .inline()
            .open(menu_open(PRIORITY_PICKER))
            .render(t);
        let close = icon_button("capture-close", Icon::Close)
            .bare()
            .small()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.quick_capture = None;
                cx.notify();
            }));
        let heading = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(crate::ui::TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(paint(t.text.primary))
                    .child("Add to backlog"),
            )
            .child(self.capture_picker(PROJECT_PICKER, project, cx))
            .child(div().flex_grow())
            .child(close);

        let title = text_field(
            &capture.title,
            "capture-title",
            capture.error.is_some(),
            Style::new(t, self.caret.visible).leading(Icon::Plus),
            window,
            cx,
        )
        .h(px(44.0))
        .text_size(px(16.0))
        .child(self.capture_picker(PRIORITY_PICKER, priority, cx));

        let (_, tags) = split_tags(&capture.title.read(cx).text());
        let tags = (!tags.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(tags.into_iter().map(|tag| {
                    tinted_tag(format!("#{tag}"), paint(t.status.attention))
                        .font_family(crate::fonts::chrome())
                }))
        });

        let quote = capture.quote.as_ref().map(|quote| {
            let from = match quote.text.lines().count() {
                1 => format!("From {}", quote.from),
                lines => format!("From {} \u{b7} {lines} lines", quote.from),
            };
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(sized_icon(quote.icon, px(12.0), faint))
                        .child(caption(from, t).text_color(faint)),
                )
                .child(
                    div()
                        .id("capture-quote")
                        .max_h(QUOTE_H)
                        .overflow_y_scroll()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(crate::ui::RADIUS_MD)
                        .bg(paint(t.sunken))
                        .font_family(crate::fonts::chrome())
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(paint(t.text.dim))
                        .child(quote.text.clone()),
                )
        });

        let hint = |id: &'static str, keys: &'static str, what: &'static str| {
            pressable(id, t)
                .h(px(28.0))
                .px(px(6.0))
                .child(key_hint(keys, what, t))
        };
        let hints = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .pt(px(8.0))
            .border_t_1()
            .border_color(alpha(paint(t.border), 0.6))
            .child(
                hint("capture-hint-add", "\u{21b5}", "add").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.add_quick_capture(false, window, cx);
                        cx.notify();
                    },
                )),
            )
            .child(
                hint("capture-hint-open", "\u{2318}\u{21b5}", "add and open").on_click(
                    cx.listener(|this, _, window, cx| {
                        this.add_quick_capture(true, window, cx);
                        cx.notify();
                    }),
                ),
            )
            .child(div().px(px(6.0)).child(key_hint("#", "tag", t)))
            .child(
                hint("capture-hint-cancel", "esc", "cancel").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.quick_capture = None;
                        cx.notify();
                    },
                )),
            );

        let panel = card("quick-capture", PANEL_W, t)
            .max_w(gpui::relative(0.92))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(capture) = this.quick_capture.as_mut()
                        && capture.menu.take().is_some()
                    {
                        cx.notify();
                    }
                }),
            )
            .child(heading)
            .child(title)
            .children(tags)
            .children(quote)
            .children(
                capture
                    .error
                    .clone()
                    .map(|why| caption(why, t).text_color(paint(t.status.failed))),
            )
            .child(hints);

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(PANEL_TOP)
                .child(panel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{QUOTE_CHARS, Quote};
    use crate::ui::icon::Icon;

    #[test]
    fn a_quote_goes_in_the_note_fenced_and_saying_where_it_came_from() {
        let quote = Quote::new(
            "fn main() {}\n\n",
            "src/main.rs:3".to_owned(),
            "rs",
            Icon::File,
        );
        assert_eq!(
            quote.markdown(),
            "From src/main.rs:3:\n\n```rs\nfn main() {}\n```"
        );
    }

    #[test]
    fn a_quote_holding_a_fence_gets_a_longer_one() {
        let quote = Quote::new("```\ncode\n```", "notes".to_owned(), "", Icon::Terminal);
        assert!(quote.markdown().starts_with("From notes:\n\n````\n```"));
        assert!(quote.markdown().ends_with("```\n````"));
    }

    #[test]
    fn a_long_selection_is_cut() {
        let long = "x".repeat(QUOTE_CHARS + 50);
        let quote = Quote::new(&long, "a terminal".to_owned(), "", Icon::Terminal);
        assert_eq!(
            quote.text.chars().count(),
            QUOTE_CHARS + 1,
            "and marked with an ellipsis"
        );
        assert!(quote.text.ends_with('\u{2026}'));
    }
}
