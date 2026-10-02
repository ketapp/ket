//! Starting notes as work: the agent, Economy level and base a note's
//! worktree is made with, the brief its agent is handed, and starting
//! several ticked notes at once through one sheet.
//!
//! The choices are the dialog's, not a note's: picked once in the open note
//! or the sheet, they hold for every Start until the dialog closes. They
//! open on what the Create Worktree dialog would offer — the project's
//! agent, the default Economy level, or Jev's when its beta is on, and the
//! project's base branch.

use gpui::{AnyElement, Context, MouseButton, Pixels, SharedString, div, prelude::*, px};
use ket_core::agents::{Catalogue, ShellProber};
use ket_core::backlog::{Note, attachment_path};
use ket_core::git::Git;
use ket_core::id::ProjectId;
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;

use super::{BODY_EDITOR, BODY_INSET, OPEN_PAD, PRIORITY_MARK, Pick, priority_mark_sized};
use crate::Shell;
use crate::paint::{alpha, paint};
use crate::phone_work::{NewWork, WorkOrigin, branch_seeded};
use crate::ui::agent::{agent_label, mark, provider};
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{caption, token_reduction_ring};
use crate::ui::dialog::{card, footer, header};
use crate::ui::field::label;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem};
use crate::ui::select::select;

/// The pickers in the open note, each its menu's origin.
const START_AGENT: &str = "backlog-start-agent";
const START_ECONOMY: &str = "backlog-start-economy";
const START_BASE: &str = "backlog-start-base";

/// The same pickers in the sheet, under origins of their own: the open note
/// is still drawn beneath it, and one menu must not hang from both.
const SHEET_AGENT: &str = "backlog-sheet-agent";
const SHEET_ECONOMY: &str = "backlog-sheet-economy";
const SHEET_BASE: &str = "backlog-sheet-base";

/// The pickers' widths in the open note: the agent's name, a level's, and a
/// branch's, which runs longest.
const AGENT_W: Pixels = px(220.0);
const ECONOMY_W: Pixels = px(170.0);
const BASE_W: Pixels = px(220.0);

/// The Economy menu's width: each level has a line saying what it changes.
const ECONOMY_MENU_W: Pixels = px(360.0);

/// The sheet's width.
const SHEET_W: Pixels = px(620.0);

/// How tall a brief may get before it scrolls.
const BRIEF_H: Pixels = px(200.0);

/// What Start makes a note's worktree with.
pub(super) struct StartOptions {
    /// The agents this desktop can run, by name.
    agents: Vec<String>,
    /// The one picked, an index into `agents`.
    agent: usize,
    /// The Economy level picked, or `None` for Jev's choice when its beta is
    /// on and the default level when it is not.
    economy: Option<u8>,
    /// Whether Jev chooses a level when none is picked.
    jev: bool,
    /// The branches to cut from, the project's base first.
    branches: Vec<String>,
    /// The one picked, an index into `branches`.
    base: usize,
}

impl StartOptions {
    /// What the project's new work would be made with. Off the window's
    /// thread: finding an agent can ask a login shell.
    fn read(project: &ProjectId) -> Self {
        let workspace = Workspace::open().ok();
        let agents: Vec<String> = workspace.as_ref().map_or_else(Vec::new, |workspace| {
            Catalogue::detect(workspace.config(), &ShellProber)
                .runnable()
                .map(|entry| entry.name.clone())
                .collect()
        });
        let agent = workspace
            .as_ref()
            .and_then(|workspace| {
                crate::worktree_dialog::preferred_agent(workspace, &agents, project)
            })
            .and_then(|wanted| agents.iter().position(|name| name == &wanted))
            .unwrap_or(0);
        let jev = workspace.as_ref().is_some_and(|workspace| {
            let jev = &workspace.config().jev;
            jev.enabled() && jev.auto_economy
        });
        let known = workspace
            .as_ref()
            .and_then(|workspace| workspace.projects().ok())
            .and_then(|projects| projects.into_iter().find(|known| &known.id == project));
        let branches = known.map_or_else(Vec::new, |known| {
            let others = Git::new(&known.root).local_branches().unwrap_or_default();
            std::iter::once(known.default_base.clone())
                .chain(
                    others
                        .into_iter()
                        .filter(|branch| *branch != known.default_base),
                )
                .collect()
        });
        Self {
            agents,
            agent,
            economy: None,
            jev,
            branches,
            base: 0,
        }
    }

    /// Takes what a menu picked.
    pub(super) fn take(&mut self, pick: Pick) {
        match pick {
            Pick::Agent(at) if at < self.agents.len() => self.agent = at,
            Pick::Economy(level) => self.economy = level,
            Pick::Base(at) if at < self.branches.len() => self.base = at,
            _ => {}
        }
    }

    fn agent(&self) -> Option<&str> {
        self.agents.get(self.agent).map(String::as_str)
    }

    /// The level a worktree is made at, as far as is known here: Jev's is
    /// only known once it has read the note.
    fn level(&self) -> u8 {
        self.economy
            .unwrap_or_else(ket_core::worktree::default_token_reduction)
    }

    /// What the Economy picker says.
    fn economy_name(&self) -> &str {
        match self.economy {
            None if self.jev => "Auto",
            _ => ket_core::worktree::token_reduction(self.level()).name(),
        }
    }

    /// The branch to cut from, when it is not the project's base — which is
    /// what making a worktree uses when it is given none.
    fn base(&self) -> Option<String> {
        (self.base > 0)
            .then(|| self.branches.get(self.base).cloned())
            .flatten()
    }

    fn base_name(&self) -> &str {
        self.branches.get(self.base).map_or("", String::as_str)
    }
}

/// The sheet that starts the ticked notes.
pub(super) struct Sheet {
    /// What its branch names end in, the first one's; each next note's is
    /// one on. Fixed when the sheet opens, so the names shown are the ones
    /// made.
    seed: u64,
    /// The note whose brief is shown under its line.
    previewing: Option<String>,
}

/// The rows of the start menu `origin` opens, the one under the keyboard as
/// it opens, and its width. `None` for an origin that is not a start menu.
pub(super) fn menu_items(
    options: &StartOptions,
    origin: &str,
    t: &Theme,
) -> Option<(Vec<MenuEntry<Pick>>, usize, Pixels)> {
    match origin {
        START_AGENT | SHEET_AGENT => {
            let items = options
                .agents
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let (which, colour) = provider(name, t);
                    let item = MenuItem::new(Pick::Agent(index), agent_label(name))
                        .tinted_icon(which, colour);
                    MenuEntry::Item(if index == options.agent {
                        item.checked()
                    } else {
                        item
                    })
                })
                .collect();
            Some((items, options.agent, AGENT_W))
        }
        START_ECONOMY | SHEET_ECONOMY => {
            let levels = crate::worktree_dialog::token_reduction_entries(
                options.economy,
                options.agent(),
                t,
                |level| Pick::Economy(Some(level)),
            );
            let (items, at) = if options.jev {
                let auto = MenuItem::new(Pick::Economy(None), "Auto")
                    .subtitle("Jev picks the level from the note")
                    .tinted_icon(Icon::Sliders, paint(t.accent));
                let auto = if options.economy.is_none() {
                    auto.checked()
                } else {
                    auto
                };
                let at = options.economy.map_or(0, |level| {
                    crate::worktree_dialog::token_reduction_index(level) + 1
                });
                (
                    std::iter::once(MenuEntry::Item(auto))
                        .chain(levels)
                        .collect(),
                    at,
                )
            } else {
                (
                    levels,
                    crate::worktree_dialog::token_reduction_index(options.level()),
                )
            };
            Some((items, at, ECONOMY_MENU_W))
        }
        START_BASE | SHEET_BASE => {
            let items = options
                .branches
                .iter()
                .enumerate()
                .map(|(index, branch)| {
                    let item =
                        MenuItem::new(Pick::Base(index), branch.clone()).icon(Icon::GitBranch);
                    let item = if index == 0 {
                        item.detail("base", None)
                    } else {
                        item
                    };
                    MenuEntry::Item(if index == options.base {
                        item.checked()
                    } else {
                        item
                    })
                })
                .collect();
            Some((items, options.base, BASE_W))
        }
        _ => None,
    }
}

/// The work starting `note` makes: a worktree named after its first words,
/// ending in `seed`'s characters, made as `options` say, whose agent is
/// handed the note's brief and its files. `None` when there are no words
/// to hand it.
fn note_work(
    project: &ProjectId,
    note: &Note,
    seed: u64,
    options: Option<&StartOptions>,
) -> Option<NewWork> {
    let prompt = note.brief()?;
    let branch = branch_seeded(first_words(note), "backlog", seed);
    let attachments = note
        .attachments
        .iter()
        .filter_map(|name| attachment_path(project, &note.id, name).ok())
        .collect();
    Some(NewWork {
        project: project.clone(),
        branch,
        prompt,
        attachments,
        agent: options.and_then(|options| options.agent().map(str::to_owned)),
        base: options.and_then(StartOptions::base),
        economy: options.and_then(|options| options.economy),
        origin: WorkOrigin::Backlog {
            note: note.id.clone(),
        },
    })
}

/// What a note's branch is named after: its title, else its description.
fn first_words(note: &Note) -> &str {
    [note.title.trim(), note.body.trim()]
        .into_iter()
        .find(|part| !part.is_empty())
        .unwrap_or_default()
}

/// What `note`'s agent is handed, as it will read it: the brief, the images
/// it is shown first, and the other files by path.
fn brief_text(project: &ProjectId, note: &Note) -> String {
    let mut text = note.brief().unwrap_or_default();
    let paths: Vec<std::path::PathBuf> = note
        .attachments
        .iter()
        .filter_map(|name| attachment_path(project, &note.id, name).ok())
        .collect();
    let (images, files): (Vec<_>, Vec<_>) = paths
        .iter()
        .partition(|path| crate::terminal::pastes_as_image(path));
    if !images.is_empty() {
        let names: Vec<String> = images
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        text = format!("[{}, shown first]\n\n{text}", names.join(", "));
    }
    if !files.is_empty() {
        let list: Vec<String> = files
            .iter()
            .map(|path| format!("- {}", path.display()))
            .collect();
        text = format!("{text}\n\nAttached files:\n{}", list.join("\n"));
    }
    text
}

impl Shell {
    /// Reads what Start will offer for `project`, off the window's thread,
    /// and gives it to the dialog if that is still open on the project.
    pub(super) fn read_start_options(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        let read = {
            let project = project.clone();
            cx.background_executor()
                .spawn(async move { StartOptions::read(&project) })
        };
        cx.spawn(async move |shell, cx| {
            let options = read.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Some(dialog) = shell.backlog.as_mut()
                    && dialog.project == project
                {
                    dialog.start = Some(options);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Starts the note `id` — the open one, saved first, or any row's: a new
    /// worktree named after it, made as the dialog's options say, with its
    /// agent handed the brief and the files.
    pub(super) fn start_backlog_note(&mut self, id: String, cx: &mut Context<Self>) {
        if self.backlog.as_ref().is_none_or(|dialog| dialog.starting) {
            return;
        }
        if !self.save_backlog_draft(false, cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(note) = dialog.saved(&id).cloned() else {
            dialog.error = Some("Write what the work is first.".into());
            return;
        };
        if note.done.is_some() {
            return;
        }
        // The open note's is the name its Start button showed; a row's has
        // shown none, so any will do.
        let seed = if dialog.selected.as_deref() == Some(id.as_str()) {
            dialog.branch_seed
        } else {
            ket_core::now_ms()
        };
        let Some(work) = note_work(&dialog.project, &note, seed, dialog.start.as_ref()) else {
            dialog.error = Some("Write what the work is first.".into());
            return;
        };
        dialog.starting = true;
        dialog.batch = 1;
        dialog.error = None;
        self.start_work(work, cx);
    }

    /// Starts the note `id` in `project` because a phone asked: the work the
    /// dialog's Start would make, with no dialog to report to.
    pub(crate) fn start_phone_note(
        &mut self,
        project: &ProjectId,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let note = ket_core::backlog::Backlog::load(project)
            .ok()
            .and_then(|backlog| backlog.notes.into_iter().find(|note| note.id == id));
        let work = note
            .filter(|note| note.done.is_none())
            .and_then(|note| note_work(project, &note, ket_core::now_ms(), None));
        match work {
            Some(work) => self.start_work(work, cx),
            None => self.toast(
                crate::ui::toast::Tone::Error,
                "A phone asked to start a note that is gone or already done",
                cx,
            ),
        }
    }

    /// What Start acts on from the keyboard: the open note, else the row the
    /// arrows are on.
    pub(super) fn backlog_start_target(&self) -> Option<String> {
        let dialog = self.backlog.as_ref()?;
        dialog.selected.clone().or_else(|| dialog.cursor.clone())
    }

    /// Ticks the note `id`, or unticks it.
    pub(super) fn toggle_backlog_pick(&mut self, id: &str) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if !dialog.saved(id).is_some_and(|note| note.done.is_none()) {
            return;
        }
        match dialog.picked.iter().position(|picked| picked == id) {
            Some(at) => {
                dialog.picked.remove(at);
            }
            None => dialog.picked.push(id.to_owned()),
        }
        dialog.cursor = Some(id.to_owned());
        dialog.confirming_picked = false;
    }

    /// ⇧-click: ticks every waiting note from the last one ticked, or the
    /// arrows' row, down or up to `id`.
    pub(super) fn pick_backlog_run(&mut self, id: &str, cx: &mut Context<Self>) {
        let query = self.backlog_query(cx);
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let open: Vec<String> = dialog
            .open_rows(&query)
            .iter()
            .map(|note| note.id.clone())
            .collect();
        let anchor = dialog.picked.last().or(dialog.cursor.as_ref());
        let from = anchor.and_then(|anchor| open.iter().position(|other| other == anchor));
        let Some(to) = open.iter().position(|other| other == id) else {
            return;
        };
        let Some(from) = from else {
            self.toggle_backlog_pick(id);
            return;
        };
        let (low, high) = (from.min(to), from.max(to));
        for other in &open[low..=high] {
            if !dialog.picked.contains(other) {
                dialog.picked.push(other.clone());
            }
        }
        dialog.cursor = Some(id.to_owned());
        dialog.confirming_picked = false;
    }

    /// ⌘A: ticks every waiting note the list shows, or unticks them all when
    /// they all are.
    pub(super) fn pick_all_backlog_notes(&mut self, cx: &mut Context<Self>) {
        let query = self.backlog_query(cx);
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let open: Vec<String> = dialog
            .open_rows(&query)
            .iter()
            .map(|note| note.id.clone())
            .collect();
        if open.iter().all(|id| dialog.picked.contains(id)) {
            dialog.picked.clear();
        } else {
            dialog.picked = open;
        }
        dialog.confirming_picked = false;
    }

    /// The ticked notes, in the order the list shows them — those already
    /// started, partway through several, among them.
    fn picked_notes(&self) -> Vec<Note> {
        let Some(dialog) = self.backlog.as_ref() else {
            return Vec::new();
        };
        let mut notes: Vec<&Note> = dialog
            .notes
            .iter()
            .filter(|note| dialog.picked.contains(&note.id))
            .collect();
        notes.sort_by(|a, b| ket_core::backlog::shown_order(a, b));
        notes.into_iter().cloned().collect()
    }

    /// Marks every ticked note done by hand.
    fn complete_picked_notes(&mut self, cx: &mut Context<Self>) {
        if !self.commit_backlog_draft(cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        for id in std::mem::take(&mut dialog.picked) {
            match ket_core::backlog::Backlog::mark_done(
                &dialog.project,
                &id,
                ket_core::backlog::Done::by_hand(),
            ) {
                Ok(backlog) => dialog.take(backlog),
                Err(error) => {
                    dialog.error = Some(error.to_string());
                    break;
                }
            }
        }
    }

    /// Deletes every ticked note and its files, once Delete has asked.
    fn delete_picked_notes(&mut self, cx: &mut Context<Self>) {
        if !self.commit_backlog_draft(cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        dialog.confirming_picked = false;
        for id in std::mem::take(&mut dialog.picked) {
            match ket_core::backlog::Backlog::remove(&dialog.project, &id) {
                Ok(backlog) => dialog.take(backlog),
                Err(error) => {
                    dialog.error = Some(error.to_string());
                    break;
                }
            }
            if dialog.cursor.as_deref() == Some(id.as_str()) {
                dialog.cursor = None;
            }
        }
    }

    /// Puts up the sheet that starts the ticked notes.
    pub(super) fn open_start_sheet(&mut self, cx: &mut Context<Self>) {
        if !self.commit_backlog_draft(cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if dialog.picked.is_empty() || dialog.starting {
            return;
        }
        dialog.menu = None;
        dialog.confirming_picked = false;
        dialog.sheet = Some(Sheet {
            seed: ket_core::now_ms(),
            previewing: None,
        });
    }

    /// Starts every ticked note, one worktree each, one after another: the
    /// next is made once the last exists — see [`Shell::backlog_started`].
    pub(super) fn start_picked_notes(&mut self, cx: &mut Context<Self>) {
        let notes = self.picked_notes();
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if dialog.starting {
            return;
        }
        let seed = dialog
            .sheet
            .as_ref()
            .map_or_else(ket_core::now_ms, |sheet| sheet.seed);
        let mut works: Vec<NewWork> = notes
            .iter()
            .enumerate()
            .filter(|(_, note)| note.done.is_none())
            .filter_map(|(at, note)| {
                note_work(
                    &dialog.project,
                    note,
                    seed + at as u64,
                    dialog.start.as_ref(),
                )
            })
            .collect();
        if works.is_empty() {
            dialog.error = Some("None of them says what the work is yet.".into());
            return;
        }
        let first = works.remove(0);
        dialog.batch = works.len() + 1;
        dialog.queue = works;
        dialog.starting = true;
        dialog.error = None;
        self.start_work(first, cx);
    }

    /// One labelled picker: `title` over `trigger`, with its menu.
    fn start_picker(
        &self,
        title: &'static str,
        origin: &'static str,
        trigger: crate::ui::select::Select,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = &self.theme;
        let open = self.backlog_menu_open(origin);
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(label(title, t))
            .child(self.backlog_picker(origin, trigger.open(open).render(t), cx))
    }

    /// The Agent, Economy and Base pickers under `origins`, or a line saying
    /// they are still being read.
    fn start_pickers(
        &self,
        origins: [&'static str; 3],
        cx: &mut Context<Self>,
    ) -> Result<[gpui::Div; 3], AnyElement> {
        let t = &self.theme;
        let Some(options) = self
            .backlog
            .as_ref()
            .and_then(|dialog| dialog.start.as_ref())
        else {
            return Err(caption("Finding the agents\u{2026}", t).into_any_element());
        };
        let [agent_origin, economy_origin, base_origin] = origins;
        let agent: SharedString = options
            .agent()
            .map_or_else(|| "No agent found".to_owned(), agent_label)
            .into();
        let agent = select(agent_origin, agent).leading(mark(options.agent().unwrap_or(""), t));
        let economy = select(economy_origin, options.economy_name().to_owned())
            .leading(token_reduction_ring(options.level(), t));
        let base = select(base_origin, options.base_name().to_owned())
            .leading(sized_icon(Icon::GitBranch, px(14.0), paint(t.text.dim)))
            .mono(crate::fonts::chrome());
        Ok([
            self.start_picker("Agent", agent_origin, agent, cx),
            self.start_picker("Economy", economy_origin, economy, cx),
            self.start_picker("Base", base_origin, base, cx),
        ])
    }

    /// What Start will make, in the open note: the three pickers, and the
    /// button that shows the brief.
    pub(super) fn backlog_start_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let previewing = self.backlog.as_ref().is_some_and(|dialog| dialog.preview);
        let row = div()
            .flex()
            .items_end()
            .gap(px(10.0))
            .px(OPEN_PAD)
            .pt(px(14.0));
        let row = match self.start_pickers([START_AGENT, START_ECONOMY, START_BASE], cx) {
            Ok([agent, economy, base]) => row
                .child(agent.w(AGENT_W).flex_none())
                .child(economy.w(ECONOMY_W).flex_none())
                .child(base.w(BASE_W).flex_none()),
            Err(waiting) => row.child(waiting),
        };
        row.child(div().flex_grow())
            .child(
                button(
                    "backlog-preview",
                    if previewing { "Hide brief" } else { "Preview" },
                )
                .ghost()
                .small()
                .leading(Icon::Eye)
                .render(t)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.preview = !dialog.preview;
                    }
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    /// The open note's brief as its agent will be handed it, from what is
    /// typed now: `title` is the title field's text.
    pub(super) fn backlog_brief(&self, note: &Note, title: String) -> AnyElement {
        let mut draft = note.clone();
        draft.title = title;
        draft.body = self.composer_text(BODY_EDITOR);
        self.brief_well("backlog-brief", &draft)
            .mx(BODY_INSET)
            .mt(px(10.0))
            .into_any_element()
    }

    /// A note's brief, set as it will be read, in a well of its own.
    fn brief_well(&self, id: &'static str, note: &Note) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        let faint = alpha(paint(t.text.dim), 0.72);
        let Some(dialog) = self.backlog.as_ref() else {
            return div().id(id);
        };
        let agent = dialog
            .start
            .as_ref()
            .and_then(StartOptions::agent)
            .map_or_else(|| "its agent".to_owned(), agent_label);
        let text = brief_text(&dialog.project, note);
        let text = if text.trim().is_empty() {
            "Nothing yet: write a title or a description.".to_owned()
        } else {
            text
        };
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .max_h(BRIEF_H)
            .overflow_y_scroll()
            .px(px(12.0))
            .py(px(10.0))
            .rounded(crate::ui::RADIUS_MD)
            .bg(paint(t.backdrop))
            .border_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(sized_icon(Icon::Eye, px(12.0), faint))
                    .child(caption(format!("What {agent} is handed"), t).text_color(faint)),
            )
            .child(
                div()
                    .font_family(crate::fonts::chrome())
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(paint(t.text.dim))
                    .child(text),
            )
    }

    /// The bar over the hints while notes are ticked: how many, and what
    /// can be done to them all.
    pub(super) fn backlog_picked_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        if dialog.picked.is_empty() {
            return None;
        }
        let t = &self.theme;
        let count = dialog.picked.len();
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(8.0))
            .rounded(crate::ui::RADIUS_LG)
            .bg(paint(t.surface))
            .border_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .font_family(crate::fonts::chrome())
                    .text_size(px(12.5))
                    .text_color(paint(t.text.primary))
                    .child(format!("{count} ticked")),
            )
            .child(div().flex_grow());
        let bar = if dialog.confirming_picked {
            bar.child(caption(
                if count == 1 {
                    "Delete it and its files?".to_owned()
                } else {
                    format!("Delete all {count} and their files?")
                },
                t,
            ))
            .child(
                button("backlog-picked-delete-cancel", "Cancel")
                    .ghost()
                    .small()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.backlog.as_mut() {
                            dialog.confirming_picked = false;
                        }
                        cx.notify();
                    })),
            )
            .child(
                button("backlog-picked-delete-confirm", "Delete")
                    .danger()
                    .small()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.delete_picked_notes(cx);
                        cx.notify();
                    })),
            )
        } else {
            bar.child(
                button("backlog-picked-done", "Mark done")
                    .ghost()
                    .small()
                    .leading(Icon::CircleCheck)
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.complete_picked_notes(cx);
                        cx.notify();
                    })),
            )
            .child(
                button("backlog-picked-delete", "Delete")
                    .danger()
                    .small()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.backlog.as_mut() {
                            dialog.confirming_picked = true;
                        }
                        cx.notify();
                    })),
            )
            .child(
                button("backlog-picked-start", format!("Start {count}\u{2026}"))
                    .primary()
                    .small()
                    .leading(Icon::Play)
                    .hint("\u{21e7}\u{2318}\u{21b5}")
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_start_sheet(cx);
                        cx.notify();
                    })),
            )
        };
        Some(bar.into_any_element())
    }

    /// The sheet over the dialog that starts the ticked notes: each one's
    /// branch, the three pickers they all share, and Start.
    pub(super) fn backlog_sheet(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.backlog.as_ref()?;
        let sheet = dialog.sheet.as_ref()?;
        let t = &self.theme;
        let notes = self.picked_notes();
        let count = notes.len();
        let faint = alpha(paint(t.text.dim), 0.72);

        let lines: Vec<_> = notes
            .iter()
            .enumerate()
            .map(|(at, note)| {
                // A note already started, partway through, shows the branch
                // it went to and a tick where its eye was.
                let started = note.done.as_ref().and_then(|done| done.branch.clone());
                let branch = started.clone().unwrap_or_else(|| {
                    branch_seeded(first_words(note), "backlog", sheet.seed + at as u64)
                });
                let shown = sheet.previewing.as_deref() == Some(note.id.as_str());
                let id = note.id.clone();
                let preview = icon_button(("backlog-sheet-preview", at), Icon::Eye)
                    .bare()
                    .small()
                    .pressed(shown)
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(sheet) = this
                            .backlog
                            .as_mut()
                            .and_then(|dialog| dialog.sheet.as_mut())
                        {
                            sheet.previewing = if sheet.previewing.as_deref() == Some(id.as_str()) {
                                None
                            } else {
                                Some(id.clone())
                            };
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let end = match started {
                    Some(_) => div()
                        .flex()
                        .flex_none()
                        .size(px(24.0))
                        .items_center()
                        .justify_center()
                        .child(sized_icon(
                            Icon::CircleCheck,
                            px(14.0),
                            paint(t.status.running),
                        ))
                        .into_any_element(),
                    None => preview,
                };
                let line = div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(crate::ui::RADIUS_MD)
                    .bg(paint(t.sunken))
                    .child(priority_mark_sized(note.priority, PRIORITY_MARK, false, t))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.5))
                            .text_color(paint(t.text.primary))
                            .child(note.title.trim().to_owned()),
                    )
                    .child(sized_icon(Icon::GitBranch, px(12.0), faint))
                    .child(
                        div()
                            .flex_none()
                            .font_family(crate::fonts::chrome())
                            .text_size(px(11.5))
                            .text_color(faint)
                            .child(branch),
                    )
                    .child(end);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(line)
                    .when(shown, |el| {
                        el.child(self.brief_well("backlog-sheet-brief", note))
                    })
            })
            .collect();

        let pickers = match self.start_pickers([SHEET_AGENT, SHEET_ECONOMY, SHEET_BASE], cx) {
            Ok([agent, economy, base]) => div()
                .flex()
                .gap(px(10.0))
                .child(agent.flex_1())
                .child(economy.flex_1())
                .child(base.flex_1())
                .into_any_element(),
            Err(waiting) => waiting,
        };

        let starting = dialog.starting;
        let made = dialog.batch.saturating_sub(dialog.queue.len());
        let start = button("backlog-sheet-start", format!("Start {count}"))
            .primary()
            .leading(Icon::Play)
            .hint("\u{21b5}")
            .loading_if(
                starting,
                format!("Starting {made} of {}\u{2026}", dialog.batch),
            )
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.start_picked_notes(cx);
                cx.notify();
            }));
        let cancel = button("backlog-sheet-cancel", "Cancel")
            .ghost()
            .enabled(!starting)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(dialog) = this.backlog.as_mut()
                    && !dialog.starting
                {
                    dialog.sheet = None;
                }
                cx.notify();
            }));
        let close = icon_button("backlog-sheet-close", Icon::Close)
            .bare()
            .small()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(dialog) = this.backlog.as_mut()
                    && !dialog.starting
                {
                    dialog.sheet = None;
                }
                cx.notify();
            }));

        let title = if count == 1 {
            "Start 1 worktree".to_owned()
        } else {
            format!("Start {count} worktrees")
        };
        let sheet = card("backlog-sheet", SHEET_W, t)
            .max_w(gpui::relative(0.9))
            .child(header(title, t).child(close))
            .child(
                div()
                    .id("backlog-sheet-notes")
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .max_h(px(300.0))
                    .overflow_y_scroll()
                    .children(lines),
            )
            .child(pickers)
            .child(caption(
                "Each note gets a worktree and an agent of its own, made one after another. \
                 The eye shows what an agent is handed.",
                t,
            ))
            .child(footer().child(cancel).child(start));

        Some(
            div()
                .absolute()
                .inset_0()
                .rounded(crate::ui::RADIUS_LG)
                .bg(crate::paint::scrim())
                .flex()
                .justify_center()
                .items_start()
                .pt(px(72.0))
                .occlude()
                // The dialog under it takes no presses while it is up; one
                // the sheet's pickers did not keep puts their menu away.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if let Some(dialog) = this.backlog.as_mut()
                            && dialog.menu.take().is_some()
                        {
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                )
                .child(sheet)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Pick, StartOptions};

    fn options() -> StartOptions {
        StartOptions {
            agents: vec!["claude".to_owned(), "codex".to_owned()],
            agent: 0,
            economy: None,
            jev: false,
            branches: vec!["main".to_owned(), "release".to_owned()],
            base: 0,
        }
    }

    #[test]
    fn the_projects_own_base_is_left_for_worktree_creation_to_fill_in() {
        let mut options = options();
        assert_eq!(options.base(), None);
        options.take(Pick::Base(1));
        assert_eq!(options.base().as_deref(), Some("release"));
    }

    #[test]
    fn a_pick_past_the_end_of_its_list_is_ignored() {
        let mut options = options();
        options.take(Pick::Agent(7));
        options.take(Pick::Base(7));
        assert_eq!(options.agent(), Some("claude"));
        assert_eq!(options.base(), None);
    }

    #[test]
    fn economy_reads_auto_only_while_jev_would_choose() {
        let mut options = options();
        assert_ne!(options.economy_name(), "Auto");
        options.jev = true;
        assert_eq!(options.economy_name(), "Auto");
        options.take(Pick::Economy(Some(
            ket_core::worktree::default_token_reduction(),
        )));
        assert_ne!(options.economy_name(), "Auto");
    }
}
