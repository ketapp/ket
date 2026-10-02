//! Prompt snippets in the shell: the menu that offers them, wherever it hangs.
//!
//! Three places offer a snippet — the quick prompt, a worktree row's menu and
//! an agent tab's menu — and each does something different with the pick, so
//! this module stops at the menu and the list it was built from. What a pick
//! *means* is the opener's call, the same rule [`crate::ui::menu`] follows.
//!
//! The list is read from disk each time a menu opens rather than kept on the
//! shell: the file is a few hundred bytes, and reading it fresh means an edit
//! made by hand shows up without a restart.

use gpui::{
    AnyElement, Context, Entity, FontWeight, KeyDownEvent, Pixels, SharedString, Window, div,
    prelude::*, px,
};
use ket_core::snippets::{Snippet, Snippets};
use ket_core::theme::Theme;

use crate::Shell;
use crate::input::{Style, TextInput, text_field};
use crate::paint::paint;
use crate::preferences::{Entry, Section};
use crate::ui::button::button;
use crate::ui::chip::{caption, tag, tinted_tag};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, OpenMenu};
use crate::ui::row::sized_row;
use crate::ui::textarea::textarea;
use crate::ui::toast::Tone;
use crate::ui::{RADIUS_LG, TITLE};

/// Wide enough for a name and a line of its body under it.
pub(crate) const SNIPPET_MENU_WIDTH: Pixels = px(320.0);

/// How much of a body a menu row previews before it trails off.
const PREVIEW_CHARS: usize = 60;

/// The settings pane's body box: the composer the quick prompt writes in,
/// under a key of its own.
const BODY_EDITOR: &str = "preferences:snippet-body";

/// The least the body box shrinks to. It takes whatever height the editor
/// has spare; this is a paragraph or two before it scrolls.
const BODY_HEIGHT: Pixels = px(200.0);

/// The body's text size. Under the quick prompt's: there the draft is the
/// one thing in the dialog, here it shares a pane with a list and a name.
const BODY_TEXT: Pixels = px(14.0);

/// How wide the list of saved snippets is beside the editor.
const LIST_WIDTH: Pixels = px(280.0);

/// A list row: a name over a first line.
const LIST_ROW_H: Pixels = px(52.0);

/// The least the list and editor shrink to together, so a short settings
/// window still leaves the body box room to write in.
const SPLIT_HEIGHT: Pixels = px(440.0);

/// What picking a row in a snippet menu asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnippetPick {
    /// The snippet at this index in the menu's own list.
    Use(usize),
    /// Settings, on the Snippets section.
    Manage,
}

/// An open snippet menu, and the snippets its rows were built from.
///
/// Carried together because a pick is an index: read against a list loaded
/// later, after an edit, it could name a different snippet.
pub(crate) struct SnippetMenu {
    /// The menu itself.
    pub(crate) menu: OpenMenu<SnippetPick>,
    /// What [`SnippetPick::Use`] indexes into.
    pub(crate) snippets: Vec<Snippet>,
}

impl SnippetMenu {
    /// A menu of `snippets`, named `origin`, searchable when `search` is set.
    ///
    /// `manage_blocked` greys out the way to settings, with the reason, for
    /// an opener that would lose something by closing — see
    /// [`crate::quick_prompt`].
    pub(crate) fn new(
        origin: impl Into<SharedString>,
        snippets: Vec<Snippet>,
        search: bool,
        manage_blocked: Option<&str>,
    ) -> Self {
        let placeholder = search.then(|| SharedString::from("Search snippets"));
        Self {
            menu: OpenMenu::new(
                origin.into(),
                placeholder,
                SNIPPET_MENU_WIDTH,
                entries(&snippets, manage_blocked),
            ),
            snippets,
        }
    }

    /// The snippet a pick names, if it names one.
    pub(crate) fn picked(&self, pick: SnippetPick) -> Option<&Snippet> {
        match pick {
            SnippetPick::Use(index) => self.snippets.get(index),
            SnippetPick::Manage => None,
        }
    }
}

/// One row per snippet, then the way to settings.
///
/// With nothing saved the menu still opens, on a disabled row saying so: a
/// menu item that leads nowhere is a dead end, and this one leads to where
/// snippets are made.
fn entries(snippets: &[Snippet], manage_blocked: Option<&str>) -> Vec<MenuEntry<SnippetPick>> {
    let mut entries: Vec<_> = snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| {
            MenuEntry::Item(
                MenuItem::new(SnippetPick::Use(index), snippet.name.clone())
                    .subtitle(preview(&snippet.body)),
            )
        })
        .collect();
    if entries.is_empty() {
        entries.push(MenuEntry::Item(
            MenuItem::new(SnippetPick::Use(0), "No snippets yet").disabled(),
        ));
    }
    let manage = MenuItem::new(SnippetPick::Manage, "Manage snippets…").icon(Icon::Sliders);
    entries.push(MenuEntry::Separator);
    entries.push(MenuEntry::Item(match manage_blocked {
        Some(reason) => manage.subtitle(reason.to_owned()).disabled(),
        None => manage,
    }));
    entries
}

/// The first line of a body with anything in it, cut short.
pub(crate) fn preview(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() > PREVIEW_CHARS {
        let cut: String = line.chars().take(PREVIEW_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_owned()
    }
}

impl Shell {
    /// The saved snippets, read fresh.
    ///
    /// A file that does not parse is said rather than swallowed, and offers
    /// nothing: guessing at half of a broken file would put text in a prompt
    /// that the person never saved.
    pub(crate) fn load_snippets(&mut self, cx: &mut Context<Self>) -> Vec<Snippet> {
        match Snippets::load() {
            Ok(snippets) => snippets.items,
            Err(error) => {
                self.toast_detail(
                    Tone::Error,
                    "Could not read snippets",
                    error.to_string(),
                    cx,
                );
                Vec::new()
            }
        }
    }

    /// Settings, on the section where snippets are made.
    pub(crate) fn manage_snippets(&mut self, cx: &mut Context<Self>) {
        self.open_preferences_at(Section::Snippets, cx);
    }
}

// ---------------------------------------------------------------------------
// The Snippets section of settings
// ---------------------------------------------------------------------------

/// Which snippet the Snippets pane's editor is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnippetSlot {
    /// The saved snippet at this index.
    Existing(usize),
    /// One being written, not saved yet.
    New,
}

/// The Snippets pane while settings are open.
#[derive(Default)]
pub(crate) struct SnippetsPane {
    /// What is saved, as it was read when settings opened and as each save
    /// has left it since.
    saved: Vec<Snippet>,
    /// False when the file could not be read: writing over it would destroy
    /// the text that needs repairing, the same rule the config follows.
    writable: bool,
    /// Why the file could not be read, when it could not.
    load_error: Option<String>,
    /// The snippet in the editor. Never empty for long while any are saved:
    /// [`Shell::snippet_body_pane`] shows the first when nothing else is.
    selected: Option<SnippetSlot>,
    /// The shown snippet's name field. The body is the composer under
    /// [`BODY_EDITOR`].
    pub(crate) name: Option<Entity<TextInput>>,
    /// Whether the body box has the keyboard.
    ///
    /// Remembered rather than read off a focus handle: the composer takes
    /// keys through the shell's own handle, as the editor does, so "the
    /// shell is focused" is all gpui can say. A press anywhere in settings
    /// clears it, and a press in the box sets it again.
    body_active: bool,
    /// Whether Delete is asking to be sure.
    confirming_delete: bool,
    /// Why the shown snippet would not save.
    error: Option<String>,
}

impl SnippetsPane {
    /// The pane, read from disk.
    pub(crate) fn load() -> Self {
        match Snippets::load() {
            Ok(snippets) => Self {
                saved: snippets.items,
                writable: true,
                ..Self::default()
            },
            Err(error) => Self {
                load_error: Some(format!("Could not read snippets: {error}")),
                ..Self::default()
            },
        }
    }
}

impl Shell {
    fn snippets_pane(&mut self) -> Option<&mut SnippetsPane> {
        self.preferences.as_mut().map(|view| &mut view.snippets)
    }

    /// Whether the body box is taking keys.
    fn snippet_body_active(&self, window: &Window) -> bool {
        self.preferences
            .as_ref()
            .is_some_and(|view| view.snippets.body_active)
            && self.focus.is_focused(window)
    }

    /// Shows `slot` in the editor. Leaving the one shown saves it, the way
    /// leaving an agent card does; one that will not save stays, with its
    /// reason. `take_keys` puts the keyboard in it — a pick made by hand,
    /// not the first snippet shown when the pane opens.
    fn select_snippet(
        &mut self,
        slot: SnippetSlot,
        take_keys: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .preferences
            .as_ref()
            .and_then(|view| view.snippets.selected);
        if current == Some(slot) || !self.commit_snippet_draft(cx) {
            return;
        }
        let seed = match slot {
            SnippetSlot::Existing(index) => self
                .preferences
                .as_ref()
                .and_then(|view| view.snippets.saved.get(index))
                .map(|snippet| (snippet.name.clone(), snippet.body.clone())),
            SnippetSlot::New => Some((String::new(), String::new())),
        };
        let Some((name, body)) = seed else {
            return;
        };
        let input = TextInput::with_text("What menus call it", &name, cx);
        self.watch_field(&input, cx);
        self.open_composer(BODY_EDITOR, &body);
        self.set_composer_size(BODY_EDITOR, BODY_TEXT);
        let new = slot == SnippetSlot::New;
        if new {
            input.update(cx, |input, _| input.request_focus());
        } else if take_keys {
            // Straight into the text, which is what gets edited: the name
            // is a click away.
            window.focus(&self.focus);
        }
        if let Some(pane) = self.snippets_pane() {
            pane.selected = Some(slot);
            pane.name = Some(input);
            pane.body_active = !new && take_keys;
            pane.confirming_delete = false;
            pane.error = None;
        }
    }

    /// The shown snippet's name and body, as typed.
    fn snippet_draft(&self, cx: &Context<Self>) -> Option<(SnippetSlot, String, String)> {
        let pane = &self.preferences.as_ref()?.snippets;
        let slot = pane.selected?;
        let name = pane.name.as_ref()?.read(cx).text();
        Some((slot, name, self.composer_text(BODY_EDITOR)))
    }

    /// Whether the shown snippet says anything different from what is saved.
    fn snippet_draft_is_dirty(&self, cx: &Context<Self>) -> bool {
        let Some((slot, name, body)) = self.snippet_draft(cx) else {
            return false;
        };
        match slot {
            SnippetSlot::New => !name.trim().is_empty() || !body.trim().is_empty(),
            SnippetSlot::Existing(index) => self
                .preferences
                .as_ref()
                .and_then(|view| view.snippets.saved.get(index))
                .is_none_or(|saved| saved.name != name.trim() || saved.body != body.trim_end()),
        }
    }

    /// Writes the shown snippet to disk. Returns whether it saved; one that
    /// did not has its reason beside Save.
    fn save_snippet(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((slot, name, body)) = self.snippet_draft(cx) else {
            return false;
        };
        let Some(pane) = self.snippets_pane() else {
            return false;
        };
        if !pane.writable {
            return false;
        }
        let name = name.trim().to_owned();
        // Trailing blank lines are the box's, not the snippet's: pasted into
        // an agent they would be Enters.
        let body = body.trim_end().to_owned();
        let at = match slot {
            SnippetSlot::Existing(index) => Some(index),
            SnippetSlot::New => None,
        };
        if let Err(reason) = Snippets::check(&pane.saved, at, &name, &body) {
            pane.error = Some(reason);
            return false;
        }
        let mut items = pane.saved.clone();
        let snippet = Snippet { name, body };
        let index = match at {
            Some(index) => {
                items[index] = snippet;
                index
            }
            None => {
                items.push(snippet);
                items.len() - 1
            }
        };
        let file = Snippets { items };
        match file.save() {
            Ok(()) => {
                pane.saved = file.items;
                pane.selected = Some(SnippetSlot::Existing(index));
                pane.error = None;
                true
            }
            Err(error) => {
                pane.error = Some(error.to_string());
                false
            }
        }
    }

    /// Saves the shown snippet if it changed, then lets go of it. Returns whether
    /// settings are safe to leave.
    pub(crate) fn commit_snippet_draft(&mut self, cx: &mut Context<Self>) -> bool {
        if self.snippet_draft_is_dirty(cx) && !self.save_snippet(cx) {
            return false;
        }
        self.close_snippet_draft();
        true
    }

    /// Lets go of the shown snippet without saving it. The pane shows the
    /// first saved one in its place on the next frame.
    fn close_snippet_draft(&mut self) {
        self.close_composer(BODY_EDITOR);
        if let Some(pane) = self.snippets_pane() {
            pane.selected = None;
            pane.name = None;
            pane.body_active = false;
            pane.confirming_delete = false;
            pane.error = None;
        }
    }

    /// Deletes the shown snippet, once Delete has asked.
    fn delete_snippet(&mut self) {
        let Some(pane) = self.snippets_pane() else {
            return;
        };
        let Some(SnippetSlot::Existing(index)) = pane.selected else {
            return;
        };
        if !pane.writable || index >= pane.saved.len() {
            return;
        }
        let mut items = pane.saved.clone();
        items.remove(index);
        let file = Snippets { items };
        match file.save() {
            Ok(()) => {
                pane.saved = file.items;
                self.close_snippet_draft();
            }
            Err(error) => {
                pane.confirming_delete = false;
                pane.error = Some(error.to_string());
            }
        }
    }

    /// Keys for the Snippets pane, ahead of the rest of settings. Returns
    /// whether it took the key.
    ///
    /// The body box takes everything while it has the keyboard, Enter
    /// included — it is a paragraph, not a field — except ⌘↵, which saves,
    /// Tab, which goes to the name, and Escape, which lets go of it.
    pub(crate) fn snippets_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.preferences.as_ref().map(|view| &view.snippets) else {
            return false;
        };
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        if key.key == "escape" && pane.confirming_delete {
            if let Some(pane) = self.snippets_pane() {
                pane.confirming_delete = false;
            }
            return true;
        }
        let name = pane.name.clone();
        let on_name = name
            .as_ref()
            .is_some_and(|input| input.read(cx).is_focused(window));

        if self.snippet_body_active(window) {
            match key.key.as_str() {
                "escape" => {
                    if let Some(pane) = self.snippets_pane() {
                        pane.body_active = false;
                    }
                }
                "enter" if command => {
                    self.save_snippet(cx);
                }
                "tab" => {
                    if let Some(input) = name {
                        input.update(cx, |input, _| input.select_all());
                        window.focus(input.read(cx).focus_handle());
                    }
                }
                _ => self.composer_key(BODY_EDITOR, key, cx),
            }
            return true;
        }

        if on_name {
            match key.key.as_str() {
                "enter" => {
                    self.save_snippet(cx);
                    return true;
                }
                "tab" => {
                    window.focus(&self.focus);
                    if let Some(pane) = self.snippets_pane() {
                        pane.body_active = true;
                    }
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Lets go of the body box. For a press anywhere in settings; one in the
    /// box itself takes it back straight after.
    pub(crate) fn release_snippet_body(&mut self) {
        if let Some(pane) = self.snippets_pane() {
            pane.body_active = false;
        }
    }

    /// The shown snippet's body box, built ahead of the pane because the
    /// editor it draws needs the shell mutably and the pane does not have it.
    ///
    /// Shows the first saved snippet when none is — on arrival, and after a
    /// delete or a discard — so the editor is never an empty half of the
    /// pane while there is something to put in it.
    pub(crate) fn snippet_body_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.preferences.as_ref()?;
        if view.section == Section::Snippets
            && view.snippets.selected.is_none()
            && !view.snippets.saved.is_empty()
        {
            self.select_snippet(SnippetSlot::Existing(0), false, window, cx);
        }
        self.preferences.as_ref()?.snippets.selected?;
        let active = self.snippet_body_active(window);
        // Only the box with the keyboard shows a caret: with the name field
        // taking keys, a blinking one here says typing would land in both.
        self.set_composer_caret(BODY_EDITOR, active);
        let editor = self.editor_pane(BODY_EDITOR, None, window, cx);
        let text = self.composer_text(BODY_EDITOR);
        let t = &self.theme;
        let footer = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.0))
            .child(
                caption(measure(&text), t)
                    .font_family(crate::fonts::chrome())
                    .text_color(faint(t)),
            )
            .child(div().flex_grow())
            .child(
                caption("\u{21b5} new line \u{b7} \u{2318}\u{21b5} save", t).text_color(faint(t)),
            );
        let area = textarea("snippet-body", editor, BODY_HEIGHT)
            .active(active)
            .footer(footer);
        let area = match text.is_empty() {
            true => area.placeholder("The text it puts into a prompt", BODY_TEXT),
            false => area,
        };
        Some(
            area.render(t)
                .flex_1()
                .min_h(BODY_HEIGHT)
                .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                    if let Some(pane) = this.snippets_pane() {
                        pane.body_active = true;
                    }
                    window.focus(&this.focus);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The Snippets pane: the saved snippets listed beside an editor for
    /// the one picked.
    pub(crate) fn snippet_settings(
        &self,
        t: &Theme,
        body: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entry> {
        let Some(view) = self.preferences.as_ref() else {
            return Vec::new();
        };
        let pane = &view.snippets;

        let heading = div()
            .flex()
            .items_start()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(TITLE)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(paint(t.text.primary))
                                    .child("Snippets"),
                            )
                            .when(pane.load_error.is_none(), |el| {
                                el.child(tag(format!("{} saved", pane.saved.len()), t))
                            }),
                    )
                    .child(caption(
                        "Saved text to drop into any prompt: \u{2318}/ in the quick prompt, or Send snippet on a worktree's or an agent tab's menu.",
                        t,
                    )),
            )
            .child(
                button("snippet-new", "New snippet")
                    .leading(Icon::Plus)
                    .enabled(pane.writable && pane.selected != Some(SnippetSlot::New))
                    .render(t)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_snippet(SnippetSlot::New, true, window, cx);
                        cx.notify();
                    })),
            );
        let mut content = vec![
            Entry::setting(
                "Prompt snippets saved text quick prompt worktree agent tab send new snippet add create",
                heading,
            ),
            Entry::layout(div().h(px(14.0))),
        ];

        if let Some(error) = pane.load_error.as_ref() {
            content.push(Entry::layout(
                caption(error.clone(), t)
                    .py(px(8.0))
                    .text_color(paint(t.status.failed)),
            ));
            return content;
        }
        if pane.saved.is_empty() && pane.selected.is_none() {
            content.push(Entry::layout(
                caption(
                    "None yet. A review checklist, the commands to run before committing \u{2014} anything typed into agents more than once.",
                    t,
                )
                .py(px(8.0)),
            ));
            return content;
        }

        // The shown snippet's row reads what is being typed, so a name
        // written into a new one appears in the list as it is written.
        let draft = self.snippet_draft(cx);
        let live = |slot: SnippetSlot, name: &str, body: &str| match draft.as_ref() {
            Some((at, typed, text)) if *at == slot => (typed.clone(), text.clone()),
            _ => (name.to_owned(), body.to_owned()),
        };
        let mut rows = Vec::new();
        if pane.selected == Some(SnippetSlot::New) {
            let (name, body) = live(SnippetSlot::New, "", "");
            rows.push(self.snippet_row(SnippetSlot::New, &name, &body, t, cx));
        }
        for (index, snippet) in pane.saved.iter().enumerate() {
            let slot = SnippetSlot::Existing(index);
            let (name, body) = live(slot, &snippet.name, &snippet.body);
            rows.push(self.snippet_row(slot, &name, &body, t, cx));
        }
        let list = div()
            .id("snippet-list")
            .flex()
            .flex_col()
            .flex_none()
            .w(LIST_WIDTH)
            .p(px(6.0))
            .gap(px(1.0))
            .border_r_1()
            .border_color(paint(t.rule))
            .overflow_y_scroll()
            .children(rows);

        let words = pane
            .saved
            .iter()
            .map(|snippet| format!("{} {}", snippet.name, snippet.body))
            .collect::<Vec<_>>()
            .join(" ");
        content.push(Entry::setting(
            words,
            div()
                .flex()
                .flex_1()
                .min_h(SPLIT_HEIGHT)
                .rounded(RADIUS_LG)
                .border_1()
                .border_color(paint(t.rule))
                .overflow_hidden()
                .child(list)
                .children(self.snippet_editor(body.take(), t, window, cx)),
        ));
        content
    }

    /// One snippet in the list: its name over its first line, marked while
    /// it is the one in the editor.
    fn snippet_row(
        &self,
        slot: SnippetSlot,
        name: &str,
        body: &str,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self
            .preferences
            .as_ref()
            .is_some_and(|view| view.snippets.selected == Some(slot));
        let (id, new) = match slot {
            SnippetSlot::Existing(index) => (index, false),
            SnippetSlot::New => (usize::MAX, true),
        };
        let name = match name.trim() {
            "" if new => "Untitled".to_owned(),
            name => name.to_owned(),
        };
        let first = preview(body);
        let detail = match (new, first.is_empty()) {
            (true, true) => "Not saved yet".to_owned(),
            (_, _) => first,
        };
        sized_row(("snippet-row", id), selected, LIST_ROW_H, t)
            .relative()
            .flex_col()
            .items_start()
            .justify_center()
            .gap(px(3.0))
            .pl(px(14.0))
            .when(selected, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.0))
                        .bottom(px(10.0))
                        .w(px(3.0))
                        .rounded(px(2.0))
                        .bg(paint(t.marker)),
                )
            })
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(match new {
                        true => tinted_tag("New", paint(t.accent)).into_any_element(),
                        false => caption(lines(body), t)
                            .flex_none()
                            .font_family(crate::fonts::chrome())
                            .text_color(faint(t))
                            .into_any_element(),
                    }),
            )
            .child(
                caption(detail, t)
                    .w_full()
                    .truncate()
                    .when(new, |el| el.text_color(faint(t))),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_snippet(slot, true, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The editor beside the list: the shown snippet's name and text, and
    /// what can be done with them.
    fn snippet_editor(
        &self,
        body: Option<AnyElement>,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pane = &self.preferences.as_ref()?.snippets;
        let slot = pane.selected?;
        let input = pane.name.as_ref()?;
        let id = match slot {
            SnippetSlot::Existing(index) => index,
            SnippetSlot::New => usize::MAX,
        };
        let name_field = text_field(
            input,
            ("snippet-name", id),
            pane.error.is_some(),
            Style::new(t, self.caret.visible),
            window,
            cx,
        );
        let dirty = self.snippet_draft_is_dirty(cx);
        let save = |label: &'static str| {
            button(("snippet-save", id), label)
                .primary()
                .enabled(dirty && pane.writable)
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.save_snippet(cx);
                    cx.notify();
                }))
        };
        // Why it would not save, or that what is shown is what is saved.
        let status = match pane.error.as_ref() {
            Some(error) => Some(
                caption(error.clone(), t)
                    .text_color(paint(t.status.failed))
                    .into_any_element(),
            ),
            None if !dirty && slot != SnippetSlot::New => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(paint(t.status.running)),
                    )
                    .child(caption("Saved", t))
                    .into_any_element(),
            ),
            None => None,
        };

        let actions = div().flex().items_center().gap(px(8.0));
        let actions = match slot {
            SnippetSlot::New => actions
                .child(div().flex_grow())
                .children(status)
                .child(
                    button(("snippet-discard", id), "Discard")
                        .ghost()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_snippet_draft();
                            cx.notify();
                        })),
                )
                .child(save("Save snippet")),
            SnippetSlot::Existing(_) if pane.confirming_delete => actions
                .child(caption("Delete this snippet?", t))
                .child(
                    button(("snippet-delete-cancel", id), "Cancel")
                        .ghost()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(pane) = this.snippets_pane() {
                                pane.confirming_delete = false;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    button(("snippet-delete-confirm", id), "Delete")
                        .danger()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.delete_snippet();
                            cx.notify();
                        })),
                ),
            SnippetSlot::Existing(_) => actions
                .child(
                    button(("snippet-delete", id), "Delete")
                        .danger()
                        .leading(Icon::Trash)
                        .enabled(pane.writable)
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(pane) = this.snippets_pane() {
                                pane.confirming_delete = true;
                            }
                            cx.notify();
                        })),
                )
                .child(div().flex_grow())
                .children(status)
                .when(dirty, |el| el.child(save("Save"))),
        };

        Some(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .p(px(16.0))
                .gap(px(8.0))
                .child(caption("Name", t))
                .child(name_field)
                .child(caption("Text", t).mt(px(6.0)))
                .children(body)
                .child(actions.pt(px(4.0)))
                .into_any_element(),
        )
    }
}

/// The ink for what is said about a snippet rather than in it: the counts
/// and the key hints, a step under the captions they sit beside.
fn faint(t: &Theme) -> gpui::Rgba {
    crate::paint::alpha(paint(t.text.dim), 0.72)
}

/// How long a body is, for the foot of the box it is written in.
fn measure(body: &str) -> String {
    let chars = body.chars().count();
    match body.is_empty() {
        true => "0 chars".to_owned(),
        false => format!("{chars} chars \u{b7} {}", lines(body)),
    }
}

/// A body's line count, as a list row gives it.
fn lines(body: &str) -> String {
    match body.lines().count().max(1) {
        1 => "1 line".to_owned(),
        n => format!("{n} lines"),
    }
}
