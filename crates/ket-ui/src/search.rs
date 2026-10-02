//! The sidebar's search: everything ket knows about, except files.
//!
//! Three surfaces now take a query, and the difference between them is what
//! they are asking about rather than how they look. `Cmd-P` finds a *file* in
//! the selected worktree. `Cmd-K` runs a *command*. This one answers "where is
//! that", across worktrees, projects, the tabs already open, and settings — the
//! things the sidebar shows, plus the ones it has no room for.
//!
//! Two decisions worth stating, because both were arguable:
//!
//! - **It is not a modal.** The palette dims the window and takes the pointer;
//!   this hangs off the field it was typed into and leaves the shell behind it
//!   live, so a search is something you do *beside* your work rather than on
//!   top of it. That is the whole design, and it is why nothing here calls
//!   `picker::overlay`, which paints the wash.
//! - **It does not search files.** `Cmd-P` already does, over an index built
//!   for it, and folding a worktree's file tree in here would drown four
//!   worktrees and seven settings sections in ten thousand paths every time.
//!
//! There is no Help group. ket has no help content to index yet, and a category
//! whose rows go nowhere is worse than one that is honestly absent.

use gpui::{
    AnyElement, App, Bounds, Context, KeyDownEvent, Pixels, Rgba, SharedString, Window, canvas,
    div, prelude::*, px,
};
use ket_core::command::CommandId;
use ket_core::id::WorktreeId;
use ket_core::theme::Color;

use crate::Shell;
use crate::input::{Style, is_text, text_line};
use crate::paint::paint;
use crate::palette::score;
use crate::preferences::Section;
use crate::tabs::{Tab, TabKind};
use crate::tree::Selection;
use crate::ui::button::icon_button;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::picker::{ROW_HEIGHT, group_header, hints, lit_in, note, panel_w, results, row};
use crate::ui::{CAPTION, LABEL};

/// How wide the panel is.
///
/// Wide enough for a worktree, what it is doing and which project it belongs
/// to on one line — which the 300px the sidebar starts at is not, and is the
/// reason this hangs beside the sidebar rather than inside it.
pub(crate) const WIDTH: Pixels = px(470.0);

/// How far below the window's top edge the panel hangs before the field has
/// been painted once and measured — see [`Search::field`].
const TOP: Pixels = px(79.0);

/// How far in from the window's leading edge, on that same first frame.
const LEFT: Pixels = px(8.0);

/// The gap between the field's bottom edge and the panel.
const GAP: Pixels = px(6.0);

/// Most rows one group contributes while every group is showing.
///
/// A cap per group rather than one overall, because the overall list is what a
/// cap should not be spent on: twelve worktrees would push settings and
/// commands off a panel that is supposed to be the way you reach them. Lifted
/// entirely once a group is scoped — that is what scoping is *for*.
const PER_GROUP: usize = 4;

/// Most rows the panel draws at once, past which the rest are a footnote.
const MAX_ROWS: usize = 12;

/// What a hit is about, and what picking it does.
#[derive(Clone)]
pub(crate) enum Target {
    /// Go to a worktree, the way clicking its row does.
    Worktree(Selection),
    /// Go to a project — which means its last-visited worktree.
    Project(usize),
    /// Focus a tab that is already open somewhere.
    Tab {
        /// The worktree whose space holds it.
        worktree: WorktreeId,
        /// Which tab, by what it shows.
        kind: TabKind,
    },
    /// Open global settings at one section.
    Setting(Section),
    /// Open one project's settings dialog.
    ProjectSettings(usize),
    /// Run a command, through the registry like everything else.
    Command(CommandId),
}

/// Which list a hit belongs to.
///
/// The order is the order the groups are drawn in, and it is deliberate:
/// worktrees first because that is what the sidebar is mostly for, commands
/// last because they are the one group you can already reach another way.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    /// A checkout, in any project.
    Worktrees,
    /// A project.
    Projects,
    /// A tab already open in some worktree's space.
    Tabs,
    /// A settings section, global or per project.
    Settings,
    /// Anything in the command registry that applies here.
    Commands,
}

impl Group {
    /// Every group, in the order they are drawn.
    const ALL: [Group; 5] = [
        Group::Worktrees,
        Group::Projects,
        Group::Tabs,
        Group::Settings,
        Group::Commands,
    ];

    /// What the header above the group says.
    fn title(self) -> &'static str {
        match self {
            Group::Worktrees => "Worktrees",
            Group::Projects => "Projects",
            Group::Tabs => "Open tabs",
            Group::Settings => "Settings",
            Group::Commands => "Commands",
        }
    }
}

/// One ranked row.
pub(crate) struct Hit {
    /// Which list it is drawn under.
    group: Group,
    /// The name, and what the matched characters are lit in it.
    title: SharedString,
    /// Character indices in `title` the query matched.
    hits: Vec<usize>,
    /// The dim half of the row — what it is doing, or where it lives.
    context: Option<SharedString>,
    /// The same, for `context`: a worktree that matched on its *project's*
    /// name has nothing lit in its own, and a row that will not show you why
    /// it is in the list is guessing at you. See `picker::lit`.
    context_hits: Vec<usize>,
    /// The project this belongs to, drawn as a chip on the trailing edge.
    chip: Option<(Color, SharedString)>,
    /// A keybinding, where the row has one.
    binding: Option<SharedString>,
    /// The mark on the leading edge.
    mark: Icon,
    /// Its colour, when the row has one of its own — an agent's terracotta,
    /// say. `None` takes the dim ink every other mark in the chrome wears.
    tint: Option<Rgba>,
    /// What picking it does.
    target: Target,
}

/// The search's state.
#[derive(Default)]
pub(crate) struct Search {
    /// Whether the panel is showing.
    pub(crate) open: bool,
    /// Index into [`Search::hits`].
    pub(crate) selected: usize,
    /// The ranked rows for the current query and scope.
    pub(crate) hits: Vec<Hit>,
    /// The group the results are narrowed to, when one is.
    pub(crate) scope: Option<Group>,
    /// How many matches did not fit, for the footnote.
    pub(crate) more: usize,
    /// Where the list is scrolled to, so the arrow keys can pull the selected
    /// row back into view.
    pub(crate) scroll: gpui::ScrollHandle,
    /// Where the field was last painted, in window coordinates.
    ///
    /// The panel is drawn from the shell root rather than from inside the
    /// sidebar — a 470px surface anchored inside a 300px column that scrolls
    /// is one clipping rule away from being invisible — so it hangs from the
    /// field's measured edge. A fixed offset drifts every time the strip above
    /// the field changes height, and then the panel covers what you type.
    pub(crate) field: Option<Bounds<Pixels>>,
}

/// Scores `query` against a row's name, falling back to its context line.
///
/// A worktree called `seahorse` is a perfectly good answer to "ket" — because
/// it is *in* ket — and a search that only looked at names would refuse to say
/// so. So the context is a second haystack, ranked below the name: what you
/// typed landing in the title is the stronger claim, and `TITLE_BONUS` is what
/// keeps `ket` the project above `ket`'s four checkouts.
fn rank(query: &str, title: &str, context: Option<&str>) -> Option<(i32, Vec<usize>, Vec<usize>)> {
    /// How much a name match outranks a context one.
    ///
    /// Larger than any single character bonus `score` hands out, so the two
    /// kinds of match never interleave: every title match sorts above every
    /// context-only one, whatever the letters happened to land on.
    const TITLE_BONUS: i32 = 64;

    match score(query, title) {
        Some((points, hits)) => Some((points + TITLE_BONUS, hits, Vec::new())),
        None => {
            let (points, hits) = score(query, context?)?;
            Some((points, Vec::new(), hits))
        }
    }
}

/// The mark for a tab, from what it is showing.
fn tab_mark(kind: &TabKind) -> Icon {
    match kind {
        TabKind::Editor(_) => Icon::Pencil,
        TabKind::Audio(_) => Icon::Layers,
        TabKind::Terminal(_) => Icon::Terminal,
        TabKind::Browser(_) => Icon::ExternalLink,
        #[cfg(target_os = "macos")]
        TabKind::IosSimulator => Icon::Smartphone,
        TabKind::Android => Icon::Smartphone,
        TabKind::Diff(_) => Icon::GitCompare,
    }
}

impl Shell {
    /// Opens the panel, empty and showing everything.
    ///
    /// An empty query is not an empty panel: `score` matches everything at
    /// zero, so what comes back is a capped tour of each group — where you
    /// are, what is running, and the settings and commands you would otherwise
    /// have to already know the name of.
    pub(crate) fn open_search(&mut self, cx: &mut Context<Self>) {
        // One surface at a time, as everywhere else in the shell: two things
        // claiming the keyboard's first refusal is two things fighting over
        // Escape.
        self.menu = None;
        self.popup = None;
        self.palette.open = false;
        self.finder.open = false;

        // The panel hangs off a field in the sidebar, so a hidden sidebar is
        // a panel with nothing under it and, worse, a query line that is never
        // rendered and therefore never gets the keyboard. Opening the search
        // opens the sidebar with it.
        self.sidebar_open = true;
        // The field covers the strip, and a hidden button hears no hover-out:
        // left set, the search glyph's tooltip would be waiting on Escape.
        self.hovered_strip_action = None;
        self.search.open = true;
        self.search.selected = 0;
        self.search.scope = None;
        // Asked for here and granted on the first frame — see
        // `TextInput::request_focus`. Without it the window's handle keeps the
        // keyboard, and a terminal pane has an input handler registered
        // against exactly that handle: every character would land in the pane
        // behind the panel.
        self.searches.sidebar.update(cx, |input, cx| {
            input.clear();
            input.request_focus();
            cx.notify();
        });
        self.rank_search(cx);
    }

    /// Closes it without going anywhere.
    pub(crate) fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search.open = false;
        self.search.hits.clear();
        self.search.selected = 0;
        self.search.scope = None;
        self.search.more = 0;
        self.searches.sidebar.update(cx, |input, _| input.clear());
    }

    /// Re-ranks every source against the current query and scope.
    pub(crate) fn rank_search(&mut self, cx: &App) {
        // Closing clears the field, and clearing a field wakes the observer
        // that calls this. Without the guard, shutting the panel refills the
        // list it had just emptied — and leaves it there until the next open.
        if !self.search.open {
            return;
        }
        let query = self.searches.sidebar.read(cx).text();
        let scope = self.search.scope;

        let mut scored: Vec<(i32, Hit)> = Vec::new();
        for group in Group::ALL {
            if scope.is_some_and(|only| only != group) {
                continue;
            }
            self.gather(group, &query, &mut scored);
        }

        // By score, then by name, so a list nobody typed into is at least
        // alphabetical rather than in whatever order the maps iterated.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));

        let matched = scored.len();
        let mut kept: Vec<Hit> = Vec::new();
        let mut taken = [0usize; Group::ALL.len()];
        for (_, hit) in scored {
            if kept.len() == MAX_ROWS {
                break;
            }
            // Scoping lifts the per-group cap: a scoped list is a list of one
            // group, and capping it at four would leave the fifth worktree
            // unreachable by the very gesture meant to reach it.
            let slot = Group::ALL.iter().position(|g| *g == hit.group).unwrap_or(0);
            if scope.is_none() && taken[slot] == PER_GROUP {
                continue;
            }
            taken[slot] += 1;
            kept.push(hit);
        }

        // Then re-ordered into group order, which the score destroyed. Ranking
        // decides *which* rows survive; the groups decide where they sit.
        kept.sort_by_key(|hit| {
            Group::ALL
                .iter()
                .position(|g| *g == hit.group)
                .unwrap_or(usize::MAX)
        });

        self.search.more = matched.saturating_sub(kept.len());
        self.search.hits = kept;
        self.search.selected = self
            .search
            .selected
            .min(self.search.hits.len().saturating_sub(1));
    }

    /// Adds one group's matches to `out`.
    fn gather(&self, group: Group, query: &str, out: &mut Vec<(i32, Hit)>) {
        let t = &self.theme;
        match group {
            Group::Worktrees => {
                let now = ket_core::now_ms();
                for (pi, project) in self.projects.iter().enumerate() {
                    for (wi, node) in project.worktrees.iter().enumerate() {
                        let activity = self.activity.activity(&node.id, now);
                        let detail = activity.detail();
                        let context = if detail.is_empty() {
                            project.name.to_string()
                        } else {
                            format!("{} · {detail}", project.name)
                        };
                        let label = node.label();
                        let context = if label == node.branch {
                            context
                        } else {
                            format!("{} · {}", context, node.branch)
                        };
                        let Some((mut points, hits, context_hits)) =
                            rank(query, &label, Some(&context))
                        else {
                            continue;
                        };

                        // Two signals that only matter while nothing has been
                        // typed, and that are exactly what an empty panel
                        // should lead with: where you are, and what is
                        // running. Small enough that one typed character
                        // outranks both.
                        if self.selection
                            == Some(Selection {
                                project: pi,
                                worktree: wi,
                            })
                        {
                            points += 3;
                        }
                        if !detail.is_empty() {
                            points += 2;
                        }

                        let (mark, tint) = match node.agent.as_deref() {
                            Some(agent) => {
                                let (mark, tint) = crate::ui::agent::provider(agent, t);
                                (mark, Some(tint))
                            }
                            None => (Icon::GitBranch, None),
                        };

                        out.push((
                            points,
                            Hit {
                                group,
                                title: label,
                                hits,
                                context: Some(context.into()),
                                context_hits,
                                chip: Some((project.color, project.name.clone())),
                                binding: None,
                                mark,
                                tint,
                                target: Target::Worktree(Selection {
                                    project: pi,
                                    worktree: wi,
                                }),
                            },
                        ));
                    }
                }
            }

            Group::Projects => {
                for (pi, project) in self.projects.iter().enumerate() {
                    let count = project.worktrees.len();
                    let context = match count {
                        1 => "1 worktree".to_owned(),
                        n => format!("{n} worktrees"),
                    };
                    let Some((points, hits, context_hits)) =
                        rank(query, &project.name, Some(&context))
                    else {
                        continue;
                    };
                    out.push((
                        points,
                        Hit {
                            group,
                            title: project.name.clone(),
                            hits,
                            context: Some(context.into()),
                            context_hits,
                            chip: None,
                            binding: None,
                            mark: Icon::Folder,
                            tint: None,
                            target: Target::Project(pi),
                        },
                    ));
                }
            }

            Group::Tabs => {
                for project in &self.projects {
                    for node in &project.worktrees {
                        let Some(space) = self.spaces.get(&node.id) else {
                            continue;
                        };
                        for tab in space.all_tabs() {
                            let context = node.branch.to_string();
                            let Some((points, hits, context_hits)) =
                                rank(query, &tab.title, Some(&context))
                            else {
                                continue;
                            };
                            out.push((
                                points,
                                Hit {
                                    group,
                                    title: tab.title.clone(),
                                    hits,
                                    context: Some(context.into()),
                                    context_hits,
                                    chip: Some((project.color, project.name.clone())),
                                    binding: None,
                                    mark: tab_mark(&tab.kind),
                                    tint: None,
                                    target: Target::Tab {
                                        worktree: node.id.clone(),
                                        kind: tab.kind.clone(),
                                    },
                                },
                            ));
                        }
                    }
                }
            }

            Group::Settings => {
                for section in Section::ALL {
                    let Some((points, hits, context_hits)) =
                        rank(query, section.title(), Some("Settings"))
                    else {
                        continue;
                    };
                    out.push((
                        points,
                        Hit {
                            group,
                            title: section.title().into(),
                            hits,
                            context: Some("Settings".into()),
                            context_hits,
                            chip: None,
                            binding: None,
                            mark: section.icon(),
                            tint: None,
                            target: Target::Setting(section),
                        },
                    ));
                }
                for (pi, project) in self.projects.iter().enumerate() {
                    let title = format!("{} settings", project.name);
                    let Some((points, hits, context_hits)) =
                        rank(query, &title, Some("Project settings"))
                    else {
                        continue;
                    };
                    out.push((
                        points,
                        Hit {
                            group,
                            title: title.into(),
                            hits,
                            context: Some("Project settings".into()),
                            context_hits,
                            chip: Some((project.color, project.name.clone())),
                            binding: None,
                            mark: Icon::Sliders,
                            tint: None,
                            target: Target::ProjectSettings(pi),
                        },
                    ));
                }
            }

            Group::Commands => {
                let context = self.command_context();
                for command in self.registry.applicable(&context) {
                    let Some((mut points, hits, context_hits)) =
                        rank(query, &command.title, Some(command.category.as_str()))
                    else {
                        continue;
                    };
                    // The palette's own recency, read rather than duplicated:
                    // a command you just ran is the same command whichever
                    // surface you reach it from, and two histories of one
                    // thing would disagree within a minute.
                    if let Some(place) = self.palette.recent.iter().position(|id| id == &command.id)
                    {
                        points += (self.palette.recent.len() - place) as i32;
                    }
                    out.push((
                        points,
                        Hit {
                            group,
                            title: command.title.clone().into(),
                            hits,
                            context: Some(command.category.as_str().into()),
                            context_hits,
                            chip: None,
                            binding: crate::palette::binding_for(&self.keymap, &command.id),
                            mark: Icon::List,
                            tint: None,
                            target: Target::Command(command.id.clone()),
                        },
                    ));
                }
            }
        }
    }

    /// Goes wherever the selected row points, and closes the panel.
    pub(crate) fn activate_search_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.search.hits.get(index) else {
            return;
        };
        let target = hit.target.clone();
        // Closed first: every arm below opens something, and a panel still up
        // over what it just opened is a panel you have to dismiss to see the
        // thing you asked for.
        self.close_search(cx);

        match target {
            Target::Worktree(selection) => self.select(selection, cx),
            Target::Project(pi) => {
                // The project row's own rule, not a second one: a project is
                // reached through the worktree you were last in, falling back
                // to its first.
                let worktree = self.projects.get(pi).and_then(|project| {
                    self.project_focus
                        .get(&project.id)
                        .and_then(|wanted| {
                            project.worktrees.iter().position(|node| &node.id == wanted)
                        })
                        .or_else(|| (!project.worktrees.is_empty()).then_some(0))
                });
                if let Some(worktree) = worktree {
                    self.select(
                        Selection {
                            project: pi,
                            worktree,
                        },
                        cx,
                    );
                }
            }
            Target::Tab { worktree, kind } => {
                // Its worktree first: a tab lives in one space, and focusing it
                // without going there would leave the strip showing a tab the
                // content area is not rendering.
                let selection = self.projects.iter().enumerate().find_map(|(pi, project)| {
                    project
                        .worktrees
                        .iter()
                        .position(|node| node.id == worktree)
                        .map(|wi| Selection {
                            project: pi,
                            worktree: wi,
                        })
                });
                if let Some(selection) = selection {
                    self.select(selection, cx);
                }
                // `open_tab` focuses a tab already open rather than adding a
                // second one — see `Space::open_tab` — so the title it is given
                // here only matters for a tab that has since been closed, which
                // is a tab worth reopening under the name it had.
                if let Some(space) = self.spaces.get_mut(&worktree) {
                    let title = space
                        .tab_title(&kind)
                        .unwrap_or_else(|| SharedString::from("Untitled"));
                    space.open_tab(Tab {
                        title,
                        kind,
                        renamed: false,
                        pinned: false,
                    });
                }
            }
            Target::Setting(section) => self.open_preferences_at(section, cx),
            Target::ProjectSettings(pi) => {
                if let Some(project) = self.projects.get(pi) {
                    let id = project.id.clone();
                    self.open_project_settings(&id, crate::projects::Field::Name, cx);
                }
            }
            // Through the registry, like the palette: a second dispatch path
            // is the thing the registry exists to prevent.
            Target::Command(id) => self.dispatch(id.as_str(), cx),
        }
    }

    /// Narrows to the selected row's group, or widens again.
    fn scope_search(&mut self, cx: &App) {
        self.search.scope = match self.search.scope {
            // Already narrowed: Tab is how you get back out, so it is also how
            // you cycle past a group you did not want.
            Some(_) => None,
            None => self
                .search
                .hits
                .get(self.search.selected)
                .map(|hit| hit.group),
        };
        self.search.selected = 0;
        self.rank_search(cx);
    }

    /// Handles a keystroke while the panel is open. Returns whether it took it.
    pub(crate) fn search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.search.open {
            // A click can land in the field without going through
            // `open_search` — gpui gives it the keyboard on the press, and the
            // row's own click handler is not the only way in. A field holding
            // the keyboard with no panel under it would swallow every key and
            // show nothing for them, so the panel follows the focus.
            if !self.searches.sidebar.read(cx).is_focused(window) {
                return false;
            }
            self.search.open = true;
            self.search.selected = 0;
            self.rank_search(cx);
        }

        // Before the field sees it, and only in the one case the field would
        // swallow it for nothing: backspace on an empty query. The field
        // consumes every backspace whether or not there is anything to delete,
        // so a scope chip could never be reached by the key that should drop
        // it.
        if event.keystroke.key == "backspace"
            && self.search.scope.is_some()
            && self.searches.sidebar.read(cx).text().is_empty()
        {
            self.search.scope = None;
            self.search.selected = 0;
            self.rank_search(cx);
            return true;
        }

        // The query line gets first refusal on the rest: motion, deletion,
        // selection and the clipboard are its. Printable keys are declined and
        // must stay declined — the fall-through to macOS's input context is
        // what produces dead keys and every input method — but are still
        // reported consumed here, so the shell behind does not act on them
        // too. See `crate::input`.
        let query = self.searches.sidebar.clone();
        if is_text(&event.keystroke) {
            return true;
        }
        if query.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }

        match event.keystroke.key.as_str() {
            // A scope is a step you back out of before you leave altogether.
            "escape" => {
                if self.search.scope.is_some() {
                    self.search.scope = None;
                    self.search.selected = 0;
                    self.rank_search(cx);
                } else {
                    self.close_search(cx);
                }
            }
            "enter" => self.activate_search_hit(self.search.selected, cx),
            "tab" => self.scope_search(cx),
            "down" => {
                let last = self.search.hits.len().saturating_sub(1);
                self.search.selected = (self.search.selected + 1).min(last);
                self.scroll_to_selected();
            }
            "up" => {
                self.search.selected = self.search.selected.saturating_sub(1);
                self.scroll_to_selected();
            }
            // Swallowed like every other key while this holds the keyboard.
            _ => {}
        }

        true
    }

    /// Pulls the selected row back into view.
    ///
    /// Counting the group headers on the way, because they are children of the
    /// scrolling list too: a row index and a child index are the same number
    /// only in a list with nothing but rows in it, and this one has five
    /// headers scattered through it.
    fn scroll_to_selected(&self) {
        let mut child = 0;
        let mut last: Option<Group> = None;
        for (index, hit) in self.search.hits.iter().enumerate() {
            if last != Some(hit.group) {
                last = Some(hit.group);
                child += 1;
            }
            if index == self.search.selected {
                break;
            }
            child += 1;
        }
        self.search.scroll.scroll_to_item(child);
    }

    /// The panel, or nothing when it is closed.
    pub(crate) fn search_view(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.search.open {
            return None;
        }

        let t = &self.theme;
        let selected = self.search.selected;
        let mono = self.font_family.clone();

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last: Option<Group> = None;
        for (index, hit) in self.search.hits.iter().enumerate() {
            if last != Some(hit.group) {
                last = Some(hit.group);
                rows.push(group_header(hit.group.title(), t).into_any_element());
            }
            rows.push(self.search_row(index, hit, index == selected, mono.clone(), cx));
        }

        if rows.is_empty() {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .h(ROW_HEIGHT)
                    .px(px(10.0))
                    .text_size(LABEL)
                    .text_color(paint(t.text.dim))
                    .child("Nothing matches")
                    .into_any_element(),
            );
        }

        let more = (self.search.more > 0).then(|| note(format!("{} more", self.search.more), t));
        let keys: &[(&str, &str)] = if self.search.scope.is_some() {
            &[
                ("⏎", "Open"),
                ("↑↓", "Move"),
                ("⌫", "Drop filter"),
                ("esc", "Close"),
            ]
        } else {
            &[
                ("⏎", "Open"),
                ("↑↓", "Move"),
                ("⇥", "Filter"),
                ("esc", "Close"),
            ]
        };

        let (left, top) = self
            .search
            .field
            .map_or((LEFT, TOP), |field| (field.left(), field.bottom() + GAP));

        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                // Nothing behind it is dimmed and nothing behind it is
                // disabled, so the one thing the panel owes the shell is that a
                // click landing *on* it does not also reach through it — which
                // is what `occlude` is, and what keeps the root's
                // dismiss-on-click from closing the panel the moment you pick a
                // row in it.
                .occlude()
                .child(
                    panel_w(t, WIDTH)
                        .child(
                            results("search-rows", &self.search.scroll)
                                .max_h(ROW_HEIGHT * 12.0)
                                .children(rows),
                        )
                        .children(more)
                        .child(hints(keys, mono, t)),
                )
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element(),
        )
    }

    /// One result row.
    fn search_row(
        &self,
        index: usize,
        hit: &Hit,
        selected: bool,
        mono: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let mark = hit.tint.unwrap_or_else(|| paint(t.text.dim));

        let chip = hit.chip.as_ref().map(|(colour, name)| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.0))
                .px(px(7.0))
                .py(px(2.0))
                .rounded(crate::ui::RADIUS_LG)
                .bg(paint(t.hover))
                .text_size(px(11.0))
                .text_color(paint(t.text.dim))
                .child(
                    div()
                        .flex_none()
                        .size(px(7.0))
                        .rounded(crate::ui::RADIUS_SM)
                        .bg(paint(*colour)),
                )
                .child(name.clone())
        });

        row(("search-row", index), selected, t)
            .child(sized_icon(hit.mark, px(14.0), mark))
            .child(
                div()
                    .flex_none()
                    .max_w(px(210.0))
                    .overflow_hidden()
                    .child(lit_in(&hit.title, &hit.hits, paint(t.text.primary), t)),
            )
            .children(hit.context.as_ref().map(|context| {
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(CAPTION)
                    .child(lit_in(context, &hit.context_hits, paint(t.text.dim), t))
            }))
            // Only when there is no context to take the slack, so the trailing
            // chip is always hard against the panel's edge rather than drifting
            // in from it on some rows and not others.
            .when(hit.context.is_none(), |el| el.child(div().flex_grow()))
            .children(
                hit.binding
                    .clone()
                    .map(|keys| crate::ui::chip::kbd(keys, mono, t)),
            )
            .children(chip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activate_search_hit(index, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The sidebar's query field, drawn only while the search is open.
    ///
    /// At rest the search is a glyph at the end of the strip's action cluster
    /// — see `Shell::sidebar` — and pressing it (or ⌘O) lays this over that
    /// whole line, focused: the filter, the sort and the actions are hidden
    /// under it rather than moved, so the tree below stays where it was.
    pub(crate) fn search_field(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let scope = self.search.scope;

        let clear = icon_button("search-clear", Icon::Close)
            .bare()
            .dense()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_search(cx);
                cx.stop_propagation();
                cx.notify();
            }));

        let body = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(8.0))
            .children(scope.map(|group| crate::ui::chip::removable(group.title(), t)))
            .child(div().flex_1().min_w_0().child(text_line(
                &self.searches.sidebar,
                "sidebar-search-query",
                Style::new(t, self.caret.visible),
                window,
                cx,
            )))
            .child(clear)
            .into_any_element();

        // Measures the field for the panel to hang from, asking for one more
        // frame only when it actually moved, or this would repaint forever.
        let shell = cx.entity();
        let probe = canvas(
            move |bounds, _, cx| {
                shell.update(cx, |this, cx| {
                    if this.search.field != Some(bounds) {
                        this.search.field = Some(bounds);
                        cx.notify();
                    }
                });
            },
            |_, _: (), _, _| {},
        )
        // Sized to a wrapper of its own rather than to the field: gpui lays an
        // absolute child out from its parent's content box, so inside the
        // field it measured from past the padding and border, and the panel
        // hung 11px in from the field's edge. The wrapper has neither.
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        // The same well as the field it replaces at rest, with the ring on:
        // this is where the keyboard is.
        let field = crate::ui::field::field("sidebar-search", "", "")
            .leading(Icon::Search)
            .focused(true)
            .body(body)
            .render(t)
            .flex_1()
            .min_w_0();

        div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .child(field)
            .child(probe)
            // A press here must not reach the shell root, whose job is to close
            // whatever is floating when a click lands outside it — including
            // the panel this field opens.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }
}
