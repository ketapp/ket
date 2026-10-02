//! The command palette — the discovery surface.
//!
//! Every command ket can perform is a named entry in `ket_core::command`'s
//! registry, and this is how a person finds one without already knowing its
//! keybinding. It complements the sidebar rather than replacing it: the tree is
//! where you *are*, the palette is how you *jump*.
//!
//! Two rules it exists to honour, both from the plan:
//!
//! - **Every command is reachable without a binding.** The registry is the
//!   single source of truth, so anything registered shows up here for free —
//!   including commands added later, without this file changing.
//! - **Keyboard-only operation.** That is an accessibility requirement, not a
//!   preference, so nothing here needs the mouse.
//!
//! It also shows each command's binding where one exists, because the palette
//! is where people *learn* the bindings rather than a substitute for them.

use gpui::Axis;
use gpui::{AnyElement, App, Context, KeyDownEvent, SharedString, Window, div, prelude::*};
use ket_core::command::{CommandId, Context as CommandContext, Invocation, Registry};
use ket_core::keybinding::Keymap;

use crate::Shell;
use crate::commands::Intent;
use crate::input::{Style, is_text, text_line};
use crate::tabs::TabKind;
use crate::ui::chip::{caption, kbd};
use crate::ui::picker::{lit, overlay, panel, query_line, results, row};

/// Most rows the palette will render.
///
/// The list is ranked, so past this the entries are ones nobody scrolls to —
/// and an unbounded list of elements would make every frame scale with entries
/// the reader cannot see.
const MAX_ROWS: usize = 40;

/// How many recently-run commands are remembered.
const MAX_RECENT: usize = 20;

/// One ranked candidate.
pub(crate) struct Candidate {
    /// The command this row runs.
    pub(crate) id: CommandId,
    /// Its title, as the registry gives it.
    pub(crate) title: SharedString,
    /// Its category, for the dim right-hand label.
    pub(crate) category: SharedString,
    /// Its keybinding, when it has one.
    pub(crate) binding: Option<SharedString>,
    /// Character indices in `title` that the query matched.
    ///
    /// Kept so the matched characters can be lit: a fuzzy matcher that will not
    /// show you *why* something matched is guessing at you, and the fix is to
    /// show the evidence rather than to explain the algorithm.
    pub(crate) hits: Vec<usize>,
}

/// The palette's state.
#[derive(Default)]
pub(crate) struct Palette {
    /// Whether it is showing.
    pub(crate) open: bool,
    /// Index into [`Palette::candidates`].
    pub(crate) selected: usize,
    /// The ranked matches for the current query.
    pub(crate) candidates: Vec<Candidate>,
    /// Where the list is scrolled to, so the arrow keys can pull the selected
    /// row back into view.
    pub(crate) scroll: gpui::ScrollHandle,
    /// Recently run commands, most recent first.
    pub(crate) recent: Vec<CommandId>,
}

/// Scores `query` against `text`, returning the match positions.
///
/// A subsequence match — every query character must appear in order — with
/// bonuses for the things that make a match feel deliberate rather than
/// coincidental: characters that are adjacent, characters that start a word,
/// and a match right at the beginning. `worktree.create` should beat
/// `project.remove` for "wc" because the letters land on word starts, not
/// because it happens to be shorter.
///
/// Returns `None` when the query is not a subsequence at all.
///
/// Shared with the menu component: two fuzzy matchers in one shell would
/// drift apart, and the menu filters with the same notion of a match that
/// ranks the palette.
pub(crate) fn score(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }

    let haystack: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().collect();

    let mut hits = Vec::with_capacity(needle.len());
    let mut total = 0;
    let mut at = 0;
    let mut previous_hit: Option<usize> = None;

    for wanted in &needle {
        let found = haystack[at..]
            .iter()
            .position(|c| c.eq_ignore_ascii_case(wanted))
            .map(|offset| at + offset)?;

        let mut points = 1;

        // Adjacent to the last match: the query is tracking a real run of
        // characters rather than scattering across the string.
        if previous_hit == Some(found.wrapping_sub(1)) {
            points += 8;
        }

        // The start of a word, where a person's eye and their abbreviation
        // both go first.
        let starts_word = found == 0
            || matches!(
                haystack.get(found - 1),
                Some(' ' | '.' | '_' | '-' | '/' | ':')
            );
        if starts_word {
            points += 6;
        }

        if found == 0 {
            points += 4;
        }

        total += points;
        hits.push(found);
        previous_hit = Some(found);
        at = found + 1;
    }

    // Prefer the shorter of two equally well matched titles: it has less in it
    // that the query did *not* explain.
    total -= (haystack.len() as i32) / 16;

    Some((total, hits))
}

impl Shell {
    /// Opens the palette, empty and showing everything applicable.
    pub(crate) fn open_palette(&mut self, cx: &mut Context<Self>) {
        // Only one overlay at a time: summoning the palette while a menu is
        // open would leave two things claiming the keyboard's first refusal.
        self.menu = None;
        self.popup = None;
        self.palette.open = true;
        self.palette.selected = 0;
        // Asked for here and granted on the first frame — see
        // `TextInput::request_focus`. The palette has to hold the keyboard
        // itself: a terminal pane registers a text-input handler against the
        // window's handle, so an overlay that leaves that handle focused is
        // one whose typing lands in the shell behind it.
        self.searches.palette.update(cx, |input, cx| {
            input.clear();
            input.request_focus();
            cx.notify();
        });
        self.rank(cx);
    }

    /// Closes the palette without running anything.
    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.palette.open = false;
        self.palette.candidates.clear();
        self.palette.selected = 0;
        self.searches.palette.update(cx, |input, _| input.clear());
    }

    /// Re-ranks the candidate list against the current query.
    ///
    /// Filters to commands that *apply* in the current context first, because
    /// `Command::applies_in` exists precisely so a palette does not offer things
    /// that cannot run — an entry that fails the moment you pick it is worse
    /// than one that was never offered.
    pub(crate) fn rank(&mut self, cx: &App) {
        let context = self.command_context();
        let query = self.searches.palette.read(cx).text();

        let mut scored: Vec<(i32, Candidate)> = self
            .registry
            .applicable(&context)
            .into_iter()
            .filter_map(|command| {
                let (mut points, hits) = score(&query, &command.title)?;

                // Recency, so what you just used is near the top. Ranked below
                // an actual text match rather than above it: when you have
                // typed something, what you typed is the stronger signal.
                if let Some(position) = self.palette.recent.iter().position(|id| id == &command.id)
                {
                    points += (MAX_RECENT - position) as i32;
                }

                Some((
                    points,
                    Candidate {
                        id: command.id.clone(),
                        title: command.title.clone().into(),
                        category: command.category.as_str().into(),
                        binding: binding_for(&self.keymap, &command.id),
                        hits,
                    },
                ))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
        scored.truncate(MAX_ROWS);

        self.palette.candidates = scored.into_iter().map(|(_, c)| c).collect();
        self.palette.selected = self
            .palette
            .selected
            .min(self.palette.candidates.len().saturating_sub(1));
    }

    /// Runs the selected command.
    pub(crate) fn run_selected(&mut self, cx: &mut Context<Self>) {
        let Some(candidate) = self.palette.candidates.get(self.palette.selected) else {
            return;
        };
        let id = candidate.id.clone();

        self.palette.recent.retain(|seen| seen != &id);
        self.palette.recent.insert(0, id.clone());
        self.palette.recent.truncate(MAX_RECENT);

        self.close_palette(cx);

        // Dispatch goes through the registry rather than around it — that is the
        // whole reason the registry exists, and a palette reaching past it would
        // be the second dispatch path the plan warns about.
        self.dispatch(id.as_str(), cx);
    }

    /// Runs a command by id, without going through the palette's own
    /// candidate list — for a keyboard shortcut that fires a known command
    /// directly. Shares the same registry dispatch as [`Self::run_selected`],
    /// so a shortcut and its palette entry can never drift apart.
    pub(crate) fn dispatch(&mut self, id: &str, cx: &mut Context<Self>) {
        let context = self.command_context();
        if let Err(e) = self.registry.dispatch(&Invocation::new(id), &context) {
            // A command left unbound on purpose says which purpose, rather than
            // reporting the registry's generic "has no implementation here".
            self.note = Some(crate::commands::explain(id, &e).into());
            return;
        }

        self.apply_intents(cx);
    }

    /// Applies whatever the handler asked the shell to do.
    ///
    /// Drained after dispatch rather than during it, because a handler runs
    /// inside the registry and cannot touch the shell — see [`crate::commands`].
    pub(crate) fn apply_intents(&mut self, cx: &mut Context<Self>) {
        let intents: Vec<Intent> = match self.outbox.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            // A poisoned outbox means a handler panicked holding it. The
            // operation already happened; losing the follow-up is better than
            // taking the window down over it.
            Err(_) => return,
        };

        let mut layout_changed = false;
        for intent in intents {
            match intent {
                Intent::Reload => self.reload(cx),
                Intent::ToggleSidebar => {
                    self.sidebar_open = !self.sidebar_open;
                    self.persist_view();
                }
                // Shown through `show_files_panel`, so the tree it reveals is
                // read now rather than whenever the next tick comes round —
                // the sweep is stopped while the panel is hidden.
                Intent::TogglePanel => match self.panel_open {
                    true => {
                        self.panel_open = false;
                        self.persist_view();
                    }
                    // Saves for itself, along with reading the tree it is
                    // about to show.
                    false => self.show_files_panel(),
                },
                Intent::SplitRight => {
                    if let Some(id) = self.selected_id()
                        && let Some(space) = self.spaces.get_mut(&id)
                    {
                        space.split(Axis::Horizontal);
                        layout_changed = true;
                    }
                }
                Intent::SplitDown => {
                    if let Some(id) = self.selected_id()
                        && let Some(space) = self.spaces.get_mut(&id)
                    {
                        space.split(Axis::Vertical);
                        layout_changed = true;
                    }
                }
                Intent::ClosePane => {
                    if let Some(id) = self.selected_id()
                        && let Some(space) = self.spaces.get_mut(&id)
                    {
                        space.close_focused_pane();
                        layout_changed = true;
                    }
                    self.prune_browsers();
                    #[cfg(target_os = "macos")]
                    self.prune_ios_simulator();
                    self.prune_android();
                }
                Intent::FocusPane(toward) => {
                    // No re-tiling: which pane has focus does not change what
                    // any pane is showing. It is persisted, though — where you
                    // were is part of the arrangement a restart restores.
                    let (axis, forward) = toward.along();
                    if let Some(id) = self.selected_id()
                        && let Some(space) = self.spaces.get_mut(&id)
                        && let Some(pane) = space.neighbour(axis, forward)
                    {
                        space.focused = pane;
                        layout_changed = true;
                    }
                    #[cfg(target_os = "macos")]
                    self.follow_ios_simulator_pane_focus();
                }
                Intent::Say(message) => self.note = Some(message.into()),
                Intent::AddProject => self.add_project(cx),
                Intent::OpenSettings => self.open_preferences(cx),
                Intent::OpenProjectSettings(id) => {
                    self.open_project_settings(&id, crate::projects::Field::Name, cx);
                }
                Intent::ConfirmRemoveProject(id) => self.confirm_remove_project(&id),
            }
        }
        if layout_changed {
            self.persist_layout();
        }
    }

    /// What the registry needs to know about where we are.
    pub(crate) fn command_context(&self) -> CommandContext {
        let pane = self
            .space()
            .and_then(|space| space.active_tab())
            .map(|tab| match &tab.kind {
                TabKind::Editor(_) => "editor",
                TabKind::Audio(_) => "audio",
                TabKind::Terminal(_) => "terminal",
                TabKind::Browser(_) => "browser",
                #[cfg(target_os = "macos")]
                TabKind::IosSimulator => "simulator",
                TabKind::Android => "android",
                TabKind::Diff(_) => "diff",
            })
            .map(str::to_owned);
        CommandContext {
            project: self
                .selection
                .and_then(|s| self.projects.get(s.project))
                .map(|p| p.id.clone()),
            worktree: self.selected_id(),
            session: None,
            pane,
        }
    }

    /// Handles a keystroke while the palette is open.
    ///
    /// Returns whether the palette consumed it. Everything is reachable from
    /// here without the mouse, which is the accessibility requirement rather
    /// than a nicety.
    pub(crate) fn palette_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if !self.palette.open {
            return false;
        }

        // The query line gets first refusal: motion, deletion, selection and
        // the clipboard belong to it, and what it declines is what the
        // palette itself uses. Printable keys are declined too and must stay
        // declined — it is the fall-through to macOS's input context that
        // produces dead keys and every input method. Reporting them consumed
        // here is still right: it stops the shell behind the overlay from
        // acting on them as well. See [`crate::input`].
        let query = self.searches.palette.clone();
        if is_text(&event.keystroke) {
            return true;
        }
        if query.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => self.close_palette(cx),
            "enter" => self.run_selected(cx),
            "down" => {
                let last = self.palette.candidates.len().saturating_sub(1);
                self.palette.selected = (self.palette.selected + 1).min(last);
                self.palette.scroll.scroll_to_item(self.palette.selected);
            }
            "up" => {
                self.palette.selected = self.palette.selected.saturating_sub(1);
                self.palette.scroll.scroll_to_item(self.palette.selected);
            }
            // Swallowed, like every key an overlay claiming the keyboard is
            // given: a chord typed over the palette must not also drive the
            // shell underneath it.
            _ => {}
        }

        true
    }

    /// The palette overlay, or nothing when it is closed.
    pub(crate) fn palette_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.palette.open {
            return None;
        }

        let t = &self.theme;
        let selected = self.palette.selected;
        let mono = self.font_family.clone();

        let query = query_line(
            text_line(
                &self.searches.palette,
                "palette-query",
                Style::new(t, self.caret.visible),
                window,
                cx,
            ),
            t,
        );

        let rows = self
            .palette
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                row(("palette-row", index), index == selected, t)
                    .child(lit(&candidate.title, &candidate.hits, t))
                    .child(div().flex_grow())
                    .child(caption(candidate.category.clone(), t))
                    .children(
                        candidate
                            .binding
                            .clone()
                            .map(|keys| kbd(keys, mono.clone(), t)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.palette.selected = index;
                        this.run_selected(cx);
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();

        Some(
            overlay(
                panel(t)
                    .child(query)
                    .child(results("palette-rows", &self.palette.scroll).children(rows)),
            )
            // The wash is part of the picker now, so a click on it is a click
            // outside — which dismisses, the way clicking off any overlay does.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_palette(cx);
                    cx.notify();
                }),
            )
            .into_any_element(),
        )
    }
}

/// The first binding for `id`, rendered the way a person would type it.
pub(crate) fn binding_for(keymap: &Keymap, id: &CommandId) -> Option<SharedString> {
    keymap
        .bindings()
        .iter()
        .find(|binding| &binding.command == id)
        .map(|binding| binding.chord.to_string().into())
}

/// The keymap, falling back to the defaults when the user's file will not load.
///
/// A broken `keybindings.toml` must not stop the app opening — losing your
/// custom bindings is recoverable, and an app that will not start is not.
pub(crate) fn keymap(registry: &Registry) -> Keymap {
    Keymap::load(registry).unwrap_or_else(|_| Keymap::builtin())
}
