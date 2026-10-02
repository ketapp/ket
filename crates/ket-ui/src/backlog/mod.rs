//! A project's backlog in the shell: the dialog its menu opens, and starting
//! a note as work, which makes a worktree for it and hands its agent the
//! note, files and all.
//!
//! The dialog is a queue. A field at the top takes a new note in one line —
//! the common case is writing something down to come back to — and the notes
//! are listed under it, most pressing first. A note opens in place, in the
//! list, for its description, tags and files; Save puts it back, and Start
//! sends it to a new worktree, from the open note or from its row.
//!
//! Every note has a priority, picked beside the capture field for a new one
//! and beside the open note's title, and drawn as a coloured mark at the
//! start of its row. The waiting notes are grouped by priority, and within
//! one they keep the order they were dragged into — see [`list`] and
//! [`organise`]. A search and the notes' tags narrow the list.
//!
//! Start makes the worktree with the agent, Economy level and base the open
//! note shows, and can show the brief its agent will be handed. Ticking
//! several notes starts them together, through one sheet — see [`start`].
//!
//! A note can also be taken from anywhere with ⇧⌘A, the selection and all —
//! see [`capture`].
//!
//! What is kept, and where, is `ket_core::backlog`: privately, or in the
//! project's repository for a team to share. Nothing written is lost:
//! leaving an open note saves it, the way Save does, and so does closing the
//! dialog.

mod capture;
mod composer;
mod list;
mod organise;
mod start;

pub(crate) use capture::QuickCapture;

use gpui::{
    AnyElement, App, Context, Div, Entity, ExternalPaths, FocusHandle, FontWeight, KeyDownEvent,
    MouseButton, PathPromptOptions, Pixels, Rgba, SharedString, Stateful, Window, div, prelude::*,
    px, relative,
};
use ket_core::backlog::{Backlog, Done, Note, Priority, normalised_tags, shown_order};
use ket_core::id::ProjectId;
use ket_core::theme::{Color, Theme};
use ket_core::worktree::Worktree;

use crate::Shell;
use crate::input::{TextInput, is_text};
use crate::paint::{alpha, paint};
use crate::phone_work::NewWork;
use crate::ui::banner::banner;
use crate::ui::button::{icon_button, pressable};
use crate::ui::chip::{badge, caption, key_hint, signal_tag};
use crate::ui::dialog::{card, centered};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;
use crate::ui::toast::Tone;

/// The mark a priority is drawn with, in a row and in its picker.
fn priority_icon(priority: Priority) -> Icon {
    match priority {
        Priority::Low => Icon::PriorityLow,
        Priority::Medium => Icon::PriorityMedium,
        Priority::High => Icon::PriorityHigh,
        Priority::Urgent => Icon::PriorityUrgent,
    }
}

/// A priority's colour: the quota meter's ramp from warm to critical, since
/// both say how pressing something is, with Low left in the dim ink so it
/// recedes.
fn priority_tint(priority: Priority, t: &Theme) -> Rgba {
    paint(match priority {
        Priority::Low => t.text.dim,
        Priority::Medium => t.quota.warm,
        Priority::High => t.quota.hot,
        Priority::Urgent => t.quota.critical,
    })
}

/// A priority's mark in its colour, at the size rows and pickers use.
fn priority_mark(priority: Priority, t: &Theme) -> Div {
    priority_mark_sized(priority, PRIORITY_MARK, false, t)
}

/// A priority's mark, `size` square: its lit bars in its colour over the
/// whole scale drawn faint, so Low reads as one of three rather than as a
/// lone bar — or, for Urgent, the square that is not a fourth bar. `faded`
/// for a note that is done.
fn priority_mark_sized(priority: Priority, size: Pixels, faded: bool, t: &Theme) -> Div {
    let tint = priority_tint(priority, t);
    let tint = if faded { alpha(tint, 0.5) } else { tint };
    let mark = div().relative().flex_none().size(size);
    if priority == Priority::Urgent {
        return mark.child(sized_icon(Icon::PriorityUrgent, size, tint));
    }
    let track = alpha(paint(t.text.dim), if faded { 0.12 } else { 0.22 });
    let layer = |which: Icon, colour: Rgba| {
        div()
            .absolute()
            .top_0()
            .left_0()
            .child(sized_icon(which, size, colour))
    };
    mark.child(layer(Icon::PriorityTrack, track))
        .child(layer(priority_icon(priority), tint))
}

/// A priority mark's size, in a row and in a picker's trigger.
const PRIORITY_MARK: Pixels = px(16.0);

/// The priority menu's width: the longest name, its mark and its tick.
const PRIORITY_MENU_W: Pixels = px(168.0);

/// The two priority pickers, each its menu's origin — see
/// [`OpenMenu::opened_by`]: the capture field's, for the next new note, and
/// the open note's.
const CAPTURE_PRIORITY: &str = "backlog-capture-priority";
const OPEN_PRIORITY: &str = "backlog-open-priority";

/// A row's ⋯ menu — whichever row [`BacklogDialog::row_menu`] names.
const ROW_MENU: &str = "backlog-row-menu";

/// The ⋯ menu's width: its longest row and the key beside it.
const ROW_MENU_W: Pixels = px(248.0);

/// The open note's description: the composer the quick prompt writes in,
/// under a key of its own.
const BODY_EDITOR: &str = "backlog:body";

/// A queue: wide enough that a row's title rarely truncates, and tall enough
/// for an open note with the list still under it.
const DIALOG_WIDTH: Pixels = px(1040.0);
const DIALOG_HEIGHT: Pixels = px(820.0);

/// The dialog's inset and the space between its parts: a step roomier than
/// a settings card, for a surface read as a list.
const DIALOG_PAD: Pixels = px(22.0);
const DIALOG_GAP: Pixels = px(16.0);

/// The header's title, a step over a card's.
const HEADING_TEXT: Pixels = px(17.0);

/// A row's title and first line.
const ROW_TITLE: Pixels = px(16.0);
const ROW_DETAIL: Pixels = px(13.5);

/// A row's figures — files, age — and a done row's branch.
const ROW_FIGURE: Pixels = px(12.0);

/// The open note's title, a step over the rows' so it reads as the one being
/// worked on.
const OPEN_TITLE: Pixels = px(16.0);

/// The open note's inset: its title, description and files all start here.
const OPEN_PAD: Pixels = px(16.0);

/// The description's well's inset from the open note's edges: the actions
/// row's, so the well's edge lines up over the delete button's.
const BODY_INSET: Pixels = px(10.0);

/// How tall the description is; it scrolls past this.
const BODY_HEIGHT: Pixels = px(260.0);

/// The composer's gutter before its first character — see `ui::textarea` —
/// which the drawn markdown stands in by too, so a click to edit does not
/// move the words.
const COMPOSER_GUTTER: Pixels = px(12.0);

/// The description's text size — the Snippets body's.
const BODY_TEXT: Pixels = px(14.0);

/// What a row's ⋯ menu does to its note.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowAction {
    Open,
    /// Opens the note, where Start shows the agent, Economy and base it
    /// will use.
    StartWithOptions,
    Priority(Priority),
    Tag,
    Delete,
}

/// What a menu in the dialog picks. One kind for every menu, so one menu is
/// open at a time and the keys reach whichever it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Priority(Priority),
    /// An index into [`start::StartOptions::agents`].
    Agent(usize),
    /// An Economy level, or `None` for Jev's choice.
    Economy(Option<u8>),
    /// An index into [`start::StartOptions::branches`].
    Base(usize),
    /// A row's ⋯ menu, for the note [`BacklogDialog::row_menu`] names.
    Row(RowAction),
}

/// The backlog dialog, while it is open.
pub(crate) struct BacklogDialog {
    project: ProjectId,
    /// The project's name and colour, for the header.
    name: SharedString,
    colour: Color,
    /// Whether the project keeps its backlog in its repository.
    in_repo: bool,
    /// What is saved, as the last read or write left it.
    notes: Vec<Note>,
    /// Why the file could not be read. Nothing is written over it then.
    load_error: Option<String>,
    /// The field a new note is written in.
    capture: Entity<TextInput>,
    /// The priority the capture field's next note is given.
    capture_priority: Priority,
    /// Whether the composer has its description open — see [`composer`].
    composing: bool,
    /// Whether that description has the keyboard. Remembered, as
    /// [`Self::body_active`] is.
    capture_body_active: bool,
    /// Files to send with the note being written, attached once it is added.
    capture_files: Vec<std::path::PathBuf>,
    /// The row under the pointer, which shows its actions as the arrows'
    /// row does.
    hovered: Option<String>,
    /// The note whose ⋯ menu is open.
    row_menu: Option<String>,
    /// The row button under the pointer, by its note and which button, for
    /// its tooltip.
    row_tip: Option<(String, &'static str)>,
    /// The open note's priority as picked: saved with its words.
    priority: Priority,
    /// The open note's tags as edited: saved with its words.
    tags: Vec<String>,
    /// The field a tag is typed into on the open note, while it is out.
    tag_input: Option<Entity<TextInput>>,
    /// A picker's menu, while one is open.
    menu: Option<OpenMenu<Pick>>,
    /// Where the keyboard waits while a menu is open, so what is typed
    /// filters the menu rather than landing in a field as well.
    menu_focus: FocusHandle,
    /// The row the arrow keys are on.
    cursor: Option<String>,
    /// The open note: a saved one's id, or `fresh`'s.
    selected: Option<String>,
    /// What the open note's branch name ends in: fixed when it opens, so the
    /// name Start shows is the one it makes, and does not change under the
    /// pointer as the dialog redraws.
    branch_seed: u64,
    /// A note that has not been saved yet — made to hold files dropped on
    /// the dialog while no note was open.
    fresh: Option<Note>,
    /// The open note's title field. Its description is the composer under
    /// [`BODY_EDITOR`].
    title: Option<Entity<TextInput>>,
    /// Whether the description has the keyboard. Remembered rather than
    /// read off a focus handle, for the reason the Snippets pane gives.
    body_active: bool,
    /// The note whose Delete is asking to be sure.
    confirming_delete: Option<String>,
    /// Whether the done notes are listed.
    show_done: bool,
    /// The search over the notes.
    search: Entity<TextInput>,
    /// The tag the list is narrowed to.
    tag_filter: Option<String>,
    /// The notes ticked to act on together, in the order they were ticked.
    picked: Vec<String>,
    /// The ticked notes' Delete is asking to be sure.
    confirming_picked: bool,
    /// What Start makes a worktree with. `None` until it has been read, off
    /// the window's thread — finding the agents can ask a login shell.
    start: Option<start::StartOptions>,
    /// Whether the open note shows the brief its agent will be handed.
    preview: bool,
    /// The sheet that starts the ticked notes, while it is up.
    sheet: Option<start::Sheet>,
    /// Where a dragged note would land.
    drop: Option<organise::Drop>,
    /// Why the last thing asked for did not happen.
    error: Option<String>,
    /// A worktree is being made for a note.
    starting: bool,
    /// The work still to start after the one being made: several notes
    /// started at once go one after another.
    queue: Vec<NewWork>,
    /// How many a batch started with, for "2 of 3".
    batch: usize,
}

impl BacklogDialog {
    fn saved(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    /// Whether `note` is in the list as it is narrowed: it answers the
    /// search and carries the tag picked. The open note always is, so
    /// typing into it never hides it.
    fn shows(&self, note: &Note, query: &str) -> bool {
        if self.selected.as_deref() == Some(note.id.as_str()) {
            return true;
        }
        note.matches(query)
            && self.tag_filter.as_ref().is_none_or(|wanted| {
                note.tags
                    .iter()
                    .any(|tag| tag.to_lowercase() == wanted.to_lowercase())
            })
    }

    /// Whether the search or a tag narrows the list.
    fn narrowed(&self, query: &str) -> bool {
        !query.trim().is_empty() || self.tag_filter.is_some()
    }

    /// The waiting notes as they are drawn: most pressing first, then in
    /// the order they were put in.
    fn open_rows(&self, query: &str) -> Vec<&Note> {
        let mut open: Vec<&Note> = self
            .notes
            .iter()
            .filter(|note| note.done.is_none() && self.shows(note, query))
            .collect();
        open.sort_by(|a, b| shown_order(a, b));
        open
    }

    /// The rows in the order they are drawn: a note being written, the open
    /// ones, then the done ones when they are shown, the latest first.
    fn rows(&self, query: &str) -> Vec<&Note> {
        let mut done: Vec<&Note> = self
            .notes
            .iter()
            .filter(|note| note.done.is_some() && self.show_done && self.shows(note, query))
            .collect();
        done.sort_by_key(|note| std::cmp::Reverse(note.done.as_ref().map_or(0, |done| done.at_ms)));
        self.fresh
            .iter()
            .chain(self.open_rows(query))
            .chain(done)
            .collect()
    }

    fn done_count(&self) -> usize {
        self.notes.iter().filter(|note| note.done.is_some()).count()
    }

    /// Takes a fresh read of the notes, and lets go of any tick on one that
    /// is gone or done — except while ticked notes are being started, when
    /// the sheet still lists every one of them.
    fn take(&mut self, backlog: Backlog) {
        self.notes = backlog.notes;
        if self.starting {
            return;
        }
        let notes = &self.notes;
        self.picked.retain(|id| {
            notes
                .iter()
                .any(|note| &note.id == id && note.done.is_none())
        });
        if self.picked.is_empty() {
            self.confirming_picked = false;
        }
    }
}

impl Shell {
    /// Notes `project` has not started, for the count on its sidebar heading.
    /// Read off the open dialog while there is one, so the count moves as
    /// notes are added and started rather than when the dialog closes.
    pub(crate) fn backlog_count(&self, project: &crate::tree::ProjectNode) -> usize {
        match self.backlog.as_ref() {
            Some(dialog) if dialog.project == project.id && dialog.load_error.is_none() => {
                dialog.notes.len() - dialog.done_count()
            }
            _ => project.backlog,
        }
    }

    /// Re-reads `project`'s count off disk once the dialog has let go of it.
    fn recount_backlog(&mut self, project: &ProjectId) {
        if let Some(node) = self.projects.iter_mut().find(|node| &node.id == project) {
            node.backlog_seen = Backlog::modified(project);
            node.backlog = Backlog::load(project)
                .map(|backlog| backlog.open_count())
                .unwrap_or(0);
        }
    }

    /// Recounts the backlogs whose files changed since they were counted —
    /// a phone's note, written by the host. A stat per project, on the tick.
    pub(crate) fn refresh_backlog_counts(&mut self) {
        let changed: Vec<ProjectId> = self
            .projects
            .iter()
            .filter(|node| Backlog::modified(&node.id) != node.backlog_seen)
            .map(|node| node.id.clone())
            .collect();
        for project in changed {
            self.recount_backlog(&project);
        }
    }

    /// Opens `project`'s backlog with the keyboard in the field a new note is
    /// written in.
    pub(crate) fn open_backlog(&mut self, project: &ProjectId, cx: &mut Context<Self>) {
        self.popup = None;
        self.menu = None;
        let (name, colour) = self
            .projects
            .iter()
            .find(|node| &node.id == project)
            .map_or_else(
                || (SharedString::default(), crate::projects::default_color()),
                |node| (node.name.clone(), node.color),
            );
        let (notes, load_error) = match Backlog::load(project) {
            Ok(backlog) => (backlog.notes, None),
            Err(error) => (
                Vec::new(),
                Some(format!("Could not read the backlog: {error}")),
            ),
        };
        let in_repo = ket_core::workspace::Workspace::open()
            .and_then(|workspace| workspace.project_settings(project))
            .is_ok_and(|settings| settings.backlog_in_repo);
        let capture = TextInput::new("Add to the backlog\u{2026}", cx);
        self.watch_field(&capture, cx);
        capture.update(cx, |input, _| input.request_focus());
        let search = TextInput::new("Search notes", cx);
        self.watch_field(&search, cx);
        self.backlog = Some(BacklogDialog {
            project: project.clone(),
            name,
            colour,
            in_repo,
            notes,
            load_error,
            capture,
            capture_priority: Priority::default(),
            composing: false,
            capture_body_active: false,
            capture_files: Vec::new(),
            hovered: None,
            row_menu: None,
            row_tip: None,
            priority: Priority::default(),
            tags: Vec::new(),
            tag_input: None,
            menu: None,
            menu_focus: cx.focus_handle(),
            cursor: None,
            selected: None,
            branch_seed: 0,
            fresh: None,
            title: None,
            body_active: false,
            confirming_delete: None,
            show_done: false,
            search,
            tag_filter: None,
            picked: Vec::new(),
            confirming_picked: false,
            start: None,
            preview: false,
            sheet: None,
            drop: None,
            error: None,
            starting: false,
            queue: Vec::new(),
            batch: 0,
        });
        self.read_start_options(project.clone(), cx);
    }

    /// Opens `project`'s backlog on the note `id`.
    pub(crate) fn open_backlog_at(
        &mut self,
        project: &ProjectId,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_backlog(project, cx);
        self.select_backlog_note(Some(id), Some(window), cx);
    }

    /// Closes the dialog, saving the open note. One that will not save keeps
    /// the dialog open, with its reason.
    fn close_backlog(&mut self, cx: &mut Context<Self>) {
        if !self.commit_backlog_draft(cx) {
            return;
        }
        if self.backlog_composing() {
            self.close_composer(composer::CAPTURE_EDITOR);
        }
        if let Some(dialog) = self.backlog.take() {
            self.recount_backlog(&dialog.project);
        }
    }

    /// What the search field holds.
    fn backlog_query(&self, cx: &App) -> String {
        self.backlog
            .as_ref()
            .map(|dialog| dialog.search.read(cx).text())
            .unwrap_or_default()
    }

    /// Saves what the composer holds as a new note, at the top of its
    /// priority — the title, its `#tags`, the description and files when
    /// there are any — and empties it for the next one.
    fn add_backlog_note(&mut self, cx: &mut Context<Self>) {
        let details = self.backlog_details_text();
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if dialog.load_error.is_some() {
            return;
        }
        let (title, tags) = organise::split_tags(&dialog.capture.read(cx).text());
        if title.is_empty() && tags.is_empty() && details.is_empty() {
            return;
        }
        let mut note = Note::new(&dialog.notes);
        note.title = title;
        note.body = details;
        note.tags = tags;
        note.priority = dialog.capture_priority;
        match Backlog::save_note(&dialog.project, &note) {
            Ok(backlog) => {
                dialog.take(backlog);
                dialog.cursor = Some(note.id.clone());
                dialog.error = None;
            }
            Err(error) => {
                dialog.error = Some(error.to_string());
                return;
            }
        }
        // The note exists now, so its files have somewhere to go.
        for path in std::mem::take(&mut dialog.capture_files) {
            match Backlog::attach(&dialog.project, &note.id, &path) {
                Ok(backlog) => dialog.take(backlog),
                Err(error) => {
                    dialog.error = Some(error.to_string());
                    break;
                }
            }
        }
        self.clear_backlog_composer(cx);
    }

    /// Opens the note `id` in place, or a new one for `None`. Leaving the one
    /// open saves it. `window` is given for a pick made by hand, which puts
    /// the keyboard in the description.
    fn select_backlog_note(
        &mut self,
        id: Option<String>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.backlog.as_ref() else {
            return;
        };
        if id.is_some() && dialog.selected == id {
            return;
        }
        if !self.commit_backlog_draft(cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_ref() else {
            return;
        };
        let (note, new) = match id {
            Some(id) => match dialog.saved(&id) {
                Some(note) => (note.clone(), false),
                None => return,
            },
            None => (Note::new(&dialog.notes), true),
        };
        let input = TextInput::with_text("What is it?", &note.title, cx);
        self.watch_field(&input, cx);
        self.open_composer(BODY_EDITOR, &note.body);
        self.set_composer_size(BODY_EDITOR, BODY_TEXT);
        let by_hand = window.is_some();
        let unwritten = note.body.trim().is_empty();
        if new {
            input.update(cx, |input, _| input.request_focus());
        } else if let Some(window) = window {
            window.focus(&self.focus);
        }
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.selected = Some(note.id.clone());
            dialog.priority = note.priority;
            dialog.tags = note.tags.clone();
            dialog.tag_input = None;
            dialog.menu = None;
            dialog.branch_seed = ket_core::now_ms();
            dialog.cursor = Some(note.id.clone());
            if new {
                dialog.fresh = Some(note);
            }
            dialog.title = Some(input);
            // Straight into the description only when there is none to read
            // yet; a written one opens drawn, and a click or ↵ edits it.
            dialog.body_active = !new && by_hand && unwritten;
            dialog.confirming_delete = None;
            dialog.error = None;
        }
    }

    /// The open note as typed.
    fn backlog_draft(&self, cx: &App) -> Option<Note> {
        let dialog = self.backlog.as_ref()?;
        let id = dialog.selected.as_ref()?;
        let base = dialog
            .fresh
            .as_ref()
            .filter(|note| &note.id == id)
            .or_else(|| dialog.saved(id))?;
        let mut note = base.clone();
        note.title = dialog.title.as_ref()?.read(cx).text();
        note.body = self.composer_text(BODY_EDITOR);
        note.priority = dialog.priority;
        note.tags = dialog.tags.clone();
        Some(note)
    }

    /// Whether the open note says anything the file does not.
    fn backlog_draft_is_dirty(&self, cx: &App) -> bool {
        let Some(draft) = self.backlog_draft(cx) else {
            return false;
        };
        match self
            .backlog
            .as_ref()
            .and_then(|dialog| dialog.saved(&draft.id))
        {
            Some(saved) => {
                saved.title != draft.title.trim()
                    || saved.body != draft.body.trim_end()
                    || saved.priority != draft.priority
                    || saved.tags != normalised_tags(&draft.tags)
            }
            None => !draft.is_blank() || !draft.tags.is_empty(),
        }
    }

    /// Writes the open note, when it changed or `force` says to — a new
    /// note about to be given a file must exist first. Returns whether what
    /// is shown is now saved.
    fn save_backlog_draft(&mut self, force: bool, cx: &mut Context<Self>) -> bool {
        let Some(mut draft) = self.backlog_draft(cx) else {
            return true;
        };
        if !force && !self.backlog_draft_is_dirty(cx) {
            return true;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return false;
        };
        if dialog.load_error.is_some() {
            return false;
        }
        draft.title = draft.title.trim().to_owned();
        // Trailing blank lines are the box's, not the note's: pasted into an
        // agent they would be Enters.
        draft.body = draft.body.trim_end().to_owned();
        match Backlog::save_note(&dialog.project, &draft) {
            Ok(backlog) => {
                dialog.take(backlog);
                if dialog
                    .fresh
                    .as_ref()
                    .is_some_and(|fresh| fresh.id == draft.id)
                {
                    dialog.fresh = None;
                }
                dialog.error = None;
                true
            }
            Err(error) => {
                dialog.error = Some(error.to_string());
                false
            }
        }
    }

    /// Saves the open note if it changed, then lets go of it. A new note
    /// with nothing in it is dropped. Returns whether it is safe to move on.
    fn commit_backlog_draft(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.save_backlog_draft(false, cx) {
            return false;
        }
        self.close_backlog_draft();
        true
    }

    fn close_backlog_draft(&mut self) {
        self.close_composer(BODY_EDITOR);
        if let Some(dialog) = self.backlog.as_mut() {
            dialog.selected = None;
            dialog.fresh = None;
            dialog.title = None;
            dialog.tag_input = None;
            dialog.body_active = false;
            // Every menu but the capture field's hangs off the open note.
            if dialog
                .menu
                .as_ref()
                .is_some_and(|menu| !menu.opened_by(CAPTURE_PRIORITY))
            {
                dialog.menu = None;
            }
        }
    }

    /// Save: the open note written and put back in the list, with the arrow
    /// keys on its row.
    fn save_backlog_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_backlog_draft(cx) {
            window.focus(&self.focus);
        }
    }

    /// Cancel: the open note put back as it was saved, whatever was typed
    /// into it since. A new note that was never saved goes with it.
    fn cancel_backlog_note(&mut self, window: &mut Window) {
        self.close_backlog_draft();
        window.focus(&self.focus);
    }

    /// Deletes the note `id` and its files, once Delete has asked.
    fn delete_backlog_note(&mut self, id: &str) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        dialog.confirming_delete = None;
        if dialog.saved(id).is_some() {
            match Backlog::remove(&dialog.project, id) {
                Ok(backlog) => dialog.take(backlog),
                Err(error) => {
                    dialog.error = Some(error.to_string());
                    return;
                }
            }
        }
        if dialog.cursor.as_deref() == Some(id) {
            dialog.cursor = None;
        }
        if dialog.selected.as_deref() == Some(id) {
            self.close_backlog_draft();
        }
    }

    /// Copies `paths` into the open note — a new one is made to hold them
    /// when none is open. Dropped while a note is being written with a
    /// description, they go with that one instead.
    fn attach_to_backlog_note(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        if self.backlog_composing() {
            self.hold_capture_files(paths);
            return;
        }
        if self
            .backlog
            .as_ref()
            .is_some_and(|dialog| dialog.selected.is_none())
        {
            self.select_backlog_note(None, None, cx);
        }
        if !self.save_backlog_draft(true, cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(id) = dialog.selected.clone() else {
            return;
        };
        for path in paths {
            if path.is_dir() {
                dialog.error = Some(format!(
                    "{} is a folder; attach the files in it instead.",
                    path.display()
                ));
                continue;
            }
            match Backlog::attach(&dialog.project, &id, &path) {
                Ok(backlog) => dialog.take(backlog),
                Err(error) => {
                    dialog.error = Some(error.to_string());
                    break;
                }
            }
        }
    }

    /// Asks for files to attach to the open note.
    fn choose_backlog_files(&mut self, cx: &mut Context<Self>) {
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
                this.attach_to_backlog_note(paths, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn detach_from_backlog_note(&mut self, name: &str) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(id) = dialog.selected.clone() else {
            return;
        };
        match Backlog::detach(&dialog.project, &id, name) {
            Ok(backlog) => dialog.take(backlog),
            Err(error) => dialog.error = Some(error.to_string()),
        }
    }

    /// A note's worktree exists: the note is done, and the dialog makes way
    /// for the worktree it went to — or, partway through several, starts
    /// the next.
    pub(crate) fn backlog_started(
        &mut self,
        project: &ProjectId,
        note: &str,
        created: &Worktree,
        cx: &mut Context<Self>,
    ) {
        let done = Done {
            at_ms: ket_core::now_ms(),
            worktree: Some(created.id.clone()),
            branch: Some(created.branch.clone()),
        };
        if let Err(error) = Backlog::mark_done(project, note, done) {
            self.toast_detail(
                Tone::Error,
                "Started, but the note could not be marked done",
                error.to_string(),
                cx,
            );
        }
        // The dialog makes way for the worktree only when it was its own
        // Start; one a phone started leaves it open, with the note done.
        match self.backlog.as_mut() {
            Some(dialog) if &dialog.project == project && dialog.starting => {
                if dialog.queue.is_empty() {
                    self.close_backlog_draft();
                    self.backlog = None;
                } else {
                    if let Ok(backlog) = Backlog::load(project) {
                        dialog.take(backlog);
                    }
                    let next = dialog.queue.remove(0);
                    self.start_work(next, cx);
                }
            }
            Some(dialog) if &dialog.project == project => {
                if let Ok(backlog) = Backlog::load(project) {
                    dialog.take(backlog);
                }
            }
            _ => {}
        }
        self.recount_backlog(project);
    }

    /// A note's worktree could not be made. Any still waiting behind it in a
    /// batch are not started either, and stay ticked.
    pub(crate) fn backlog_start_failed(&mut self, why: String, cx: &mut Context<Self>) {
        match self.backlog.as_mut() {
            Some(dialog) => {
                let left = dialog.queue.len();
                dialog.queue.clear();
                dialog.starting = false;
                if let Ok(backlog) = Backlog::load(&dialog.project) {
                    dialog.take(backlog);
                }
                dialog.error = Some(match left {
                    0 => format!("Could not start it: {why}"),
                    1 => format!("Could not start it: {why}. One more was not started."),
                    left => format!("Could not start it: {why}. {left} more were not started."),
                });
            }
            None => self.toast_detail(Tone::Error, "Could not start the note", why, cx),
        }
    }

    /// Marks the note `id` done by hand, for work that happened somewhere
    /// ket did not start it. The open note is saved first, and put back once
    /// it is done.
    fn complete_backlog_note(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.save_backlog_draft(false, cx) {
            return;
        }
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        match Backlog::mark_done(&dialog.project, &id, Done::by_hand()) {
            Ok(backlog) => {
                dialog.take(backlog);
                dialog.error = None;
            }
            Err(error) => {
                dialog.error = Some(error.to_string());
                return;
            }
        }
        if dialog.selected.as_deref() == Some(id.as_str()) {
            self.close_backlog_draft();
        }
    }

    /// Puts the open done note back among the waiting ones.
    fn reopen_backlog_note(&mut self) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(id) = dialog.selected.clone() else {
            return;
        };
        match Backlog::reopen(&dialog.project, &id) {
            Ok(backlog) => dialog.take(backlog),
            Err(error) => dialog.error = Some(error.to_string()),
        }
    }

    /// Goes to the worktree the open done note was started in.
    fn open_backlog_worktree(&mut self, cx: &mut Context<Self>) {
        let worktree = self.backlog.as_ref().and_then(|dialog| {
            let id = dialog.selected.as_ref()?;
            dialog
                .saved(id)?
                .done
                .as_ref()
                .and_then(|done| done.worktree.clone())
        });
        let Some(selection) = worktree.and_then(|id| self.position_of(&id)) else {
            return;
        };
        self.close_backlog(cx);
        if self.backlog.is_none() {
            self.select(selection, cx);
        }
    }

    /// The rows of the menu `origin` opens, and the one under the keyboard
    /// as it opens, with its width.
    fn backlog_menu_items(&self, origin: &str) -> Option<(Vec<MenuEntry<Pick>>, usize, Pixels)> {
        let dialog = self.backlog.as_ref()?;
        let t = &self.theme;
        if origin == CAPTURE_PRIORITY || origin == OPEN_PRIORITY {
            let current = if origin == CAPTURE_PRIORITY {
                dialog.capture_priority
            } else {
                dialog.priority
            };
            let items = Priority::ALL
                .iter()
                .map(|&priority| {
                    let item = MenuItem::new(Pick::Priority(priority), priority.name())
                        .tinted_icon(priority_icon(priority), priority_tint(priority, t));
                    MenuEntry::Item(if priority == current {
                        item.checked()
                    } else {
                        item
                    })
                })
                .collect();
            let at = Priority::ALL
                .iter()
                .position(|&priority| priority == current)
                .unwrap_or(0);
            return Some((items, at, PRIORITY_MENU_W));
        }
        if origin == ROW_MENU {
            let note = dialog.saved(dialog.row_menu.as_deref()?)?;
            let row = |action: RowAction, title: &'static str, which: Icon| {
                MenuItem::new(Pick::Row(action), title).icon(which)
            };
            let mut items = vec![
                MenuEntry::Item(
                    row(RowAction::Open, "Open", Icon::ExternalLink).detail("\u{21b5}", None),
                ),
                MenuEntry::Item(row(
                    RowAction::StartWithOptions,
                    "Start with options\u{2026}",
                    Icon::Sliders,
                )),
                MenuEntry::Heading("Priority".into()),
            ];
            items.extend(Priority::ALL.iter().rev().map(|&priority| {
                let item = MenuItem::new(Pick::Row(RowAction::Priority(priority)), priority.name())
                    .tinted_icon(priority_icon(priority), priority_tint(priority, t));
                MenuEntry::Item(if priority == note.priority {
                    item.checked()
                } else {
                    item
                })
            }));
            items.extend([
                MenuEntry::Separator,
                MenuEntry::Item(row(RowAction::Tag, "Add a tag", Icon::Tag)),
                MenuEntry::Separator,
                MenuEntry::Item(
                    row(RowAction::Delete, "Delete", Icon::Trash)
                        .danger()
                        .detail("\u{2318}\u{232b}", None),
                ),
            ]);
            return Some((items, 0, ROW_MENU_W));
        }
        start::menu_items(dialog.start.as_ref()?, origin, t)
    }

    /// Opens the menu `origin` names, or closes it when it is the one open.
    /// The keyboard moves to the menu, so typing filters it.
    fn toggle_backlog_menu(&mut self, origin: &'static str, window: &mut Window) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if let Some(menu) = dialog.menu.take()
            && menu.opened_by(origin)
        {
            return;
        }
        let Some((items, at, width)) = self.backlog_menu_items(origin) else {
            return;
        };
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let menu = OpenMenu::new(origin.into(), None, width, items).selected(at);
        dialog.menu = Some(if origin == CAPTURE_PRIORITY || origin == OPEN_PRIORITY {
            menu
        } else if origin == ROW_MENU {
            // Hung from the ⋯ at the row's right edge, so it opens leftwards
            // over the row rather than off the dialog.
            menu.offset(list::MORE_W - ROW_MENU_W, list::MORE_W + px(4.0))
        } else {
            menu.offset(px(0.0), px(6.0))
        });
        dialog.body_active = false;
        window.focus(&dialog.menu_focus);
    }

    /// Closes the menu, giving its picker `picked` when one was. The
    /// capture field's takes the keyboard back, so the note can be finished
    /// and saved without reaching for the field.
    fn settle_backlog_menu(&mut self, picked: Option<Pick>, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(menu) = dialog.menu.take() else {
            return;
        };
        let capture = menu.opened_by(CAPTURE_PRIORITY);
        let row = dialog.row_menu.take();
        match picked {
            Some(Pick::Priority(priority)) if capture => dialog.capture_priority = priority,
            Some(Pick::Priority(priority)) => dialog.priority = priority,
            Some(Pick::Row(action)) => {
                if let Some(id) = row {
                    self.run_backlog_row_action(id, action, cx);
                }
                return;
            }
            Some(pick) => {
                if let Some(options) = dialog.start.as_mut() {
                    options.take(pick);
                }
            }
            None => {}
        }
        if capture {
            dialog.capture.update(cx, |input, _| input.request_focus());
        }
    }

    /// Opens the ⋯ menu of the row for note `id`, or closes it when it is
    /// the one open.
    fn toggle_backlog_row_menu(&mut self, id: &str, window: &mut Window) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let same = dialog.row_menu.as_deref() == Some(id)
            && dialog
                .menu
                .as_ref()
                .is_some_and(|menu| menu.opened_by(ROW_MENU));
        dialog.menu = None;
        dialog.row_menu = None;
        if same {
            return;
        }
        dialog.row_menu = Some(id.to_owned());
        dialog.cursor = Some(id.to_owned());
        self.toggle_backlog_menu(ROW_MENU, window);
    }

    /// What a row's ⋯ menu asked of note `id`.
    fn run_backlog_row_action(&mut self, id: String, action: RowAction, cx: &mut Context<Self>) {
        match action {
            RowAction::Open | RowAction::StartWithOptions => {
                self.select_backlog_note(Some(id), None, cx);
            }
            RowAction::Tag => {
                self.select_backlog_note(Some(id), None, cx);
                self.open_backlog_tag_field(cx);
            }
            RowAction::Delete => {
                if let Some(dialog) = self.backlog.as_mut() {
                    dialog.confirming_delete = Some(id);
                }
            }
            RowAction::Priority(priority) => self.set_backlog_priority(&id, priority),
        }
    }

    /// Gives note `id` `priority`: the open note's draft, or a saved note
    /// written straight away.
    fn set_backlog_priority(&mut self, id: &str, priority: Priority) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if dialog.selected.as_deref() == Some(id) {
            dialog.priority = priority;
            return;
        }
        let Some(mut note) = dialog.saved(id).cloned() else {
            return;
        };
        if note.priority == priority {
            return;
        }
        note.priority = priority;
        match Backlog::save_note(&dialog.project, &note) {
            Ok(backlog) => dialog.take(backlog),
            Err(error) => dialog.error = Some(error.to_string()),
        }
    }

    /// Keys while a menu is open: it is the topmost thing in the dialog, so
    /// it has them first.
    fn backlog_menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self
            .backlog
            .as_mut()
            .and_then(|dialog| dialog.menu.as_mut())
        else {
            return false;
        };
        match menu.key(event) {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                self.settle_backlog_menu(None, cx);
                true
            }
            MenuKey::Run(pick) => {
                self.settle_backlog_menu(Some(pick), cx);
                true
            }
        }
    }

    /// A picker: `trigger`, with its menu under it while `origin`'s is the
    /// one open.
    fn backlog_picker(
        &self,
        origin: &'static str,
        trigger: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = &self.theme;
        let panel = self
            .backlog
            .as_ref()
            .and_then(|dialog| dialog.menu.as_ref())
            .filter(|menu| menu.opened_by(origin))
            .map(|menu| {
                menu.view(
                    t,
                    cx,
                    |shell, pick, cx| {
                        shell.settle_backlog_menu(Some(*pick), cx);
                        cx.notify();
                    },
                    |shell, cx| {
                        shell.settle_backlog_menu(None, cx);
                        cx.notify();
                    },
                )
            });
        dropdown((origin, 0usize), trigger, panel)
            .font_weight(FontWeight::NORMAL)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.toggle_backlog_menu(origin, window);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// The open ⋯ menu's panel, for the row it was opened on to hang.
    fn backlog_row_menu_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self
            .backlog
            .as_ref()?
            .menu
            .as_ref()
            .filter(|menu| menu.opened_by(ROW_MENU))?;
        Some(menu.view(
            &self.theme,
            cx,
            |shell, pick, cx| {
                shell.settle_backlog_menu(Some(*pick), cx);
                cx.notify();
            },
            |shell, cx| {
                shell.settle_backlog_menu(None, cx);
                cx.notify();
            },
        ))
    }

    /// Whether `origin`'s menu is the one open.
    fn backlog_menu_open(&self, origin: &str) -> bool {
        self.backlog
            .as_ref()
            .and_then(|dialog| dialog.menu.as_ref())
            .is_some_and(|menu| menu.opened_by(origin))
    }

    /// A priority picker showing `current`.
    fn backlog_priority_picker(
        &self,
        origin: &'static str,
        current: Priority,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = &self.theme;
        let trigger = select(origin, current.name())
            .leading(priority_mark(current, t))
            .inline()
            .open(self.backlog_menu_open(origin))
            .render(t);
        self.backlog_picker(origin, trigger, cx).flex_none()
    }

    /// Moves the arrow keys' row up or down the list. Up from the first row
    /// goes back to the capture field.
    fn step_backlog(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.backlog_query(cx);
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let rows: Vec<String> = dialog
            .rows(&query)
            .iter()
            .map(|note| note.id.clone())
            .collect();
        let at = dialog
            .cursor
            .as_ref()
            .and_then(|id| rows.iter().position(|row| row == id));
        let next = match (at, down) {
            (Some(at), true) => Some((at + 1).min(rows.len().saturating_sub(1))),
            (Some(0), false) | (None, false) => None,
            (Some(at), false) => Some(at - 1),
            (None, true) => Some(0),
        };
        match next.and_then(|at| rows.get(at).cloned()) {
            Some(id) => {
                dialog.cursor = Some(id);
                dialog.body_active = false;
                window.focus(&self.focus);
            }
            None => {
                dialog.cursor = None;
                dialog.body_active = false;
                let capture = dialog.capture.clone();
                window.focus(capture.read(cx).focus_handle());
            }
        }
    }

    /// Escape, one layer at a time: a field with the keyboard lets go of it
    /// first, then the ticks and the tag the list is narrowed to, and the
    /// next press closes the dialog, saving an open note the way closing
    /// always does. The capture field is the exception while it is empty —
    /// it has the keyboard from the moment the dialog opens, and letting go
    /// of nothing is a press that seems to do nothing.
    fn backlog_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        if dialog.sheet.is_some() {
            if !dialog.starting {
                dialog.sheet = None;
            }
            return;
        }
        if dialog.tag_input.take().is_some() {
            window.focus(&self.focus);
            return;
        }
        // A note being written with a description clears, from its title as
        // from its description: Escape is the composer's "start again".
        let in_details = dialog.capture_body_active && self.focus.is_focused(window);
        if dialog.composing && (in_details || dialog.capture.read(cx).is_focused(window)) {
            self.clear_backlog_composer(cx);
            return;
        }
        let editing_body = dialog.body_active && self.focus.is_focused(window);
        let on_title = dialog
            .title
            .as_ref()
            .is_some_and(|input| input.read(cx).is_focused(window));
        let capture = dialog.capture.read(cx);
        let on_capture = capture.is_focused(window) && !capture.is_empty();
        let on_search = dialog.search.read(cx).is_focused(window);
        if editing_body || on_title || on_capture || on_search {
            dialog.body_active = false;
            window.focus(&self.focus);
            return;
        }
        if dialog.confirming_picked {
            dialog.confirming_picked = false;
            return;
        }
        if !dialog.picked.is_empty() {
            dialog.picked.clear();
            return;
        }
        self.close_backlog(cx);
    }

    /// Keys while the dialog is up. It takes every one: typing a note must
    /// not also drive the shell.
    ///
    /// ⌘↵ saves — the capture field's line as a new note, or the open note —
    /// and ⇧⌘↵ starts the open note, the ticked ones, or the arrows' row.
    /// Escape lets go of the field that has the keyboard, and otherwise
    /// closes the dialog — see [`Shell::backlog_escape`]. The arrows walk
    /// the list, ⌥ and an arrow moves the row, ↵ opens it and space ticks
    /// it. ⌘F searches. Tab moves between the open note's title and
    /// description, which takes everything else while it has the keyboard,
    /// Enter included.
    pub(crate) fn backlog_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.backlog.is_none() {
            return false;
        }
        if self.backlog_menu_key(event, cx) {
            return true;
        }
        let Some(dialog) = self.backlog.as_ref() else {
            return false;
        };
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        if let Some(id) = dialog.confirming_delete.clone() {
            match key.key.as_str() {
                "escape" => {
                    if let Some(dialog) = self.backlog.as_mut() {
                        dialog.confirming_delete = None;
                    }
                }
                "enter" => self.delete_backlog_note(&id),
                _ => {}
            }
            return true;
        }
        if dialog.sheet.is_some() {
            match key.key.as_str() {
                "escape" => self.backlog_escape(window, cx),
                "enter" => self.start_picked_notes(cx),
                _ => {}
            }
            return true;
        }
        let open = dialog.selected.is_some();
        if key.key == "escape" {
            self.backlog_escape(window, cx);
            return true;
        }
        if key.key == "enter" && command && key.modifiers.shift {
            if !dialog.picked.is_empty() {
                self.open_start_sheet(cx);
            } else if let Some(id) = self.backlog_start_target() {
                self.start_backlog_note(id, cx);
            }
            return true;
        }
        if key.key == "f" && command {
            let search = dialog.search.clone();
            if let Some(dialog) = self.backlog.as_mut() {
                dialog.body_active = false;
            }
            search.update(cx, |input, _| input.select_all());
            window.focus(search.read(cx).focus_handle());
            return true;
        }

        let capture = dialog.capture.clone();
        let search = dialog.search.clone();
        let title = dialog.title.clone();
        let tag_input = dialog.tag_input.clone();
        let on_capture = capture.read(cx).is_focused(window);
        let on_search = search.read(cx).is_focused(window);
        let on_title = title
            .as_ref()
            .is_some_and(|input| input.read(cx).is_focused(window));
        let on_tag = tag_input
            .as_ref()
            .is_some_and(|input| input.read(cx).is_focused(window));
        let on_body = dialog.body_active && self.focus.is_focused(window);
        let on_details = dialog.capture_body_active && self.focus.is_focused(window);

        if key.key == "enter" && command {
            if on_capture || on_details {
                self.add_backlog_note(cx);
            } else if open {
                self.save_backlog_note(window, cx);
            }
            return true;
        }

        if on_details {
            self.backlog_details_key(key, window, cx);
            return true;
        }

        if on_body {
            if key.key == "tab" {
                if let Some(input) = title {
                    input.update(cx, |input, _| input.select_all());
                    window.focus(input.read(cx).focus_handle());
                }
                if let Some(dialog) = self.backlog.as_mut() {
                    dialog.body_active = false;
                }
            } else {
                self.composer_key(BODY_EDITOR, key, cx);
            }
            return true;
        }

        if on_tag && let Some(input) = tag_input {
            match key.key.as_str() {
                "enter" | "tab" => self.add_backlog_tag(cx),
                "backspace" if input.read(cx).is_empty() => self.drop_last_backlog_tag(),
                _ => {
                    // Text travels on to the input context — see
                    // [`crate::input`].
                    if !is_text(key) {
                        input.update(cx, |input, cx| input.key(key, cx));
                    }
                }
            }
            return true;
        }

        if on_title && let Some(input) = title {
            // Text is never taken here. It has to keep travelling until
            // macOS's input context sees it — see [`crate::input`].
            if is_text(key) || input.update(cx, |input, cx| input.key(key, cx)) {
                return true;
            }
            if matches!(key.key.as_str(), "enter" | "tab") {
                window.focus(&self.focus);
                if let Some(dialog) = self.backlog.as_mut() {
                    dialog.body_active = true;
                }
            }
            return true;
        }

        if on_capture {
            match key.key.as_str() {
                "enter" => self.add_backlog_note(cx),
                "tab" if !key.modifiers.shift => self.open_backlog_details(window),
                "down" => self.step_backlog(true, window, cx),
                _ => {
                    // As for the title: text travels on to the input context.
                    if !is_text(key) {
                        capture.update(cx, |input, cx| input.key(key, cx));
                    }
                }
            }
            return true;
        }

        if on_search {
            match key.key.as_str() {
                "enter" | "down" => {
                    if let Some(dialog) = self.backlog.as_mut() {
                        dialog.cursor = None;
                    }
                    self.step_backlog(true, window, cx);
                }
                _ => {
                    if !is_text(key) {
                        search.update(cx, |input, cx| input.key(key, cx));
                    }
                }
            }
            return true;
        }

        match key.key.as_str() {
            "up" if key.modifiers.alt => self.nudge_backlog_note(false, cx),
            "down" if key.modifiers.alt => self.nudge_backlog_note(true, cx),
            "up" => self.step_backlog(false, window, cx),
            "down" => self.step_backlog(true, window, cx),
            "n" if command => {
                window.focus(capture.read(cx).focus_handle());
            }
            "a" if command => self.pick_all_backlog_notes(cx),
            // The arrows' row: ⌘D done, ⌘⌫ delete, which asks first.
            "d" if command => {
                let cursor = self.backlog.as_ref().and_then(|d| d.cursor.clone());
                if let Some(id) = cursor {
                    self.complete_backlog_note(id, cx);
                }
            }
            "backspace" if command => {
                if let Some(dialog) = self.backlog.as_mut()
                    && let Some(id) = dialog.cursor.clone()
                    && dialog.saved(&id).is_some()
                {
                    dialog.confirming_delete = Some(id);
                }
            }
            // The open note's description, drawn: ↵ edits it.
            "enter" if open => {
                if let Some(dialog) = self.backlog.as_mut() {
                    dialog.body_active = true;
                }
            }
            "space" => {
                let cursor = self.backlog.as_ref().and_then(|d| d.cursor.clone());
                if let Some(id) = cursor {
                    self.toggle_backlog_pick(&id);
                }
            }
            "enter" => {
                let cursor = self.backlog.as_ref().and_then(|d| d.cursor.clone());
                if let Some(id) = cursor {
                    self.select_backlog_note(Some(id), Some(window), cx);
                }
            }
            _ => {}
        }
        true
    }

    /// The backlog dialog, when it is open.
    pub(crate) fn backlog_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.backlog.as_ref()?;
        let body = self.backlog_body_pane(window, cx);
        let details = self.backlog_details_pane(window, cx);
        let dialog = self.backlog.as_ref()?;
        let t = &self.theme;

        let close = icon_button("backlog-close", Icon::Close)
            .bare()
            .small()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_backlog(cx);
                cx.notify();
            }));
        let done = dialog.done_count();
        let waiting = dialog.notes.len() - done;
        let initial: String = dialog
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        let counts = match (waiting, done) {
            (0, 0) => None,
            (waiting, 0) => Some(format!("{waiting} open")),
            (waiting, done) => Some(format!("{waiting} open \u{b7} {done} done")),
        };
        let heading = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(11.0))
            .h(px(34.0))
            .child(badge(dialog.colour, initial, t))
            .child(
                div()
                    .text_size(HEADING_TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(paint(t.text.primary))
                    .child("Backlog"),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(paint(t.text.dim))
                    .child(dialog.name.clone()),
            )
            .children(counts.map(|counts| {
                div()
                    .ml(px(2.0))
                    .font_family(crate::fonts::chrome())
                    .text_size(px(11.5))
                    .text_color(alpha(paint(t.text.dim), 0.72))
                    .child(counts)
            }))
            // Shared with everyone who has the repository: worth knowing
            // before writing something down.
            .when(dialog.in_repo, |el| {
                el.child(signal_tag(
                    Icon::BookMarked,
                    ".ket/backlog",
                    paint(t.status.running),
                ))
            })
            .child(div().flex_1())
            .child(close);

        let capture = self.backlog_composer(details, window, cx)?;

        let (filters, content) = match dialog.load_error.as_ref() {
            Some(error) => (
                None,
                caption(error.clone(), t)
                    .text_color(paint(t.status.failed))
                    .into_any_element(),
            ),
            None => (
                self.backlog_filters(window, cx),
                self.backlog_list(body, window, cx),
            ),
        };
        let picked_bar = self.backlog_picked_bar(cx);
        let hints = self.backlog_hints(window, cx);
        let sheet = self.backlog_sheet(cx);
        let dialog = self.backlog.as_ref()?;
        let t = &self.theme;

        Some(
            centered(
                card("backlog", DIALOG_WIDTH, t)
                    .relative()
                    .p(DIALOG_PAD)
                    .gap(DIALOG_GAP)
                    .max_w(relative(0.92))
                    .h(DIALOG_HEIGHT)
                    .max_h(relative(0.9))
                    // Any press lets go of either description; one inside
                    // it takes it back, being deeper and so captured later.
                    .capture_any_mouse_down(cx.listener(|this, _, _, _| {
                        if let Some(dialog) = this.backlog.as_mut() {
                            dialog.body_active = false;
                            dialog.capture_body_active = false;
                        }
                    }))
                    // A press the menu and its picker did not keep puts the
                    // menu away.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if let Some(dialog) = this.backlog.as_mut()
                                && dialog.menu.take().is_some()
                            {
                                dialog.row_menu = None;
                                cx.notify();
                            }
                        }),
                    )
                    .drag_over::<ExternalPaths>({
                        let ring = paint(t.accent);
                        move |style, _, _, _| style.border_color(ring)
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.attach_to_backlog_note(paths.paths().to_vec(), cx);
                        cx.notify();
                    }))
                    .child(heading)
                    .child(capture)
                    .children(filters)
                    .child(content)
                    .children(
                        dialog
                            .error
                            .clone()
                            .map(|why| banner("backlog-error", Tone::Error, why, t)),
                    )
                    .children(picked_bar)
                    .child(hints)
                    .children(sheet),
            )
            .into_any_element(),
        )
    }

    /// The key hints along the foot, each also the button for what it
    /// names, so the pointer reaches each thing the keys do.
    fn backlog_hints(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let hints = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .pt(px(10.0))
            .border_t_1()
            .border_color(alpha(paint(t.border), 0.55));
        let Some(dialog) = self.backlog.as_ref() else {
            return hints;
        };
        let start = |what: &'static str, cx: &mut Context<Self>| {
            backlog_hint("backlog-hint-start", "\u{21e7}\u{2318}\u{21b5}", what, t).on_click(
                cx.listener(|this, _, _, cx| {
                    if let Some(id) = this.backlog_start_target() {
                        this.start_backlog_note(id, cx);
                    }
                    cx.notify();
                }),
            )
        };
        // Not buttons: these are ways of pointing, and the rows under the
        // pointer already answer to it.
        let quiet = |keys: &'static str, what: &'static str| {
            div().px(px(6.0)).child(key_hint(keys, what, t))
        };
        let close = |cx: &mut Context<Self>| {
            backlog_hint("backlog-hint-close", "esc", "close", t).on_click(cx.listener(
                |this, _, _, cx| {
                    this.close_backlog(cx);
                    cx.notify();
                },
            ))
        };
        // Writing a note with a description: the composer's own keys.
        let in_details = dialog.capture_body_active && self.focus.is_focused(window);
        let in_title = dialog.capture.read(cx).is_focused(window);
        if dialog.composing && (in_details || in_title) {
            let add = backlog_hint("backlog-hint-add", "\u{2318}\u{21b5}", "add", t).on_click(
                cx.listener(|this, _, _, cx| {
                    this.add_backlog_note(cx);
                    cx.notify();
                }),
            );
            let hints = if in_details {
                hints
                    .child(quiet("\u{21b5}", "next line"))
                    .child(add)
                    .child(quiet("\u{21e7}\u{21e5}", "back to title"))
            } else {
                hints
                    .child(quiet("\u{21b5}", "add"))
                    .child(quiet("\u{21e5}", "details"))
            };
            return hints.child(quiet("#", "tag")).child(div().flex_1()).child(
                backlog_hint("backlog-hint-clear", "esc", "clear", t).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.clear_backlog_composer(cx);
                        cx.notify();
                    },
                )),
            );
        }
        if !dialog.picked.is_empty() {
            return hints
                .child(quiet("\u{2318}-click", "tick"))
                .child(quiet("\u{21e7}-click", "tick a run"))
                .child(
                    backlog_hint("backlog-hint-all", "\u{2318}A", "all", t).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.pick_all_backlog_notes(cx);
                            cx.notify();
                        },
                    )),
                )
                .child(div().flex_1())
                .child(
                    backlog_hint("backlog-hint-clear", "esc", "clear", t).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(dialog) = this.backlog.as_mut() {
                                dialog.picked.clear();
                                dialog.confirming_picked = false;
                            }
                            cx.notify();
                        },
                    )),
                );
        }
        if dialog.selected.is_some() {
            return hints
                .child(
                    backlog_hint("backlog-hint-save", "\u{2318}\u{21b5}", "save", t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.save_backlog_note(window, cx);
                            cx.notify();
                        }),
                    ),
                )
                .child(start("start in a new worktree", cx))
                .child(div().flex_1())
                .child(
                    backlog_hint("backlog-hint-back", "esc", "put it back", t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.save_backlog_note(window, cx);
                            cx.notify();
                        }),
                    ),
                );
        }
        let on_cursor = |id: &'static str,
                         keys: &'static str,
                         what: &'static str,
                         run: fn(&mut Shell, String, &mut Context<Shell>),
                         cx: &mut Context<Self>| {
            backlog_hint(id, keys, what, t).on_click(cx.listener(move |this, _, _, cx| {
                let cursor = this.backlog.as_ref().and_then(|d| d.cursor.clone());
                if let Some(id) = cursor {
                    run(this, id, cx);
                }
                cx.notify();
            }))
        };
        hints
            .child(quiet("\u{2191}\u{2193}", "move"))
            .child(on_cursor(
                "backlog-hint-open",
                "\u{21b5}",
                "open",
                |this, id, cx| this.select_backlog_note(Some(id), None, cx),
                cx,
            ))
            .child(start("start", cx))
            .child(on_cursor(
                "backlog-hint-done",
                "\u{2318}D",
                "done",
                |this, id, cx| this.complete_backlog_note(id, cx),
                cx,
            ))
            .child(on_cursor(
                "backlog-hint-delete",
                "\u{2318}\u{232b}",
                "delete",
                |this, id, _| {
                    if let Some(dialog) = this.backlog.as_mut() {
                        dialog.confirming_delete = Some(id);
                    }
                },
                cx,
            ))
            .child(quiet("\u{2325}\u{2191}\u{2193}", "reorder"))
            .child(div().flex_1())
            .child(close(cx))
    }
}

/// A key hint that is also the button for what it names.
fn backlog_hint(
    id: &'static str,
    keys: &'static str,
    what: &'static str,
    t: &Theme,
) -> Stateful<Div> {
    pressable(id, t)
        .h(px(28.0))
        .px(px(6.0))
        .child(key_hint(keys, what, t))
}

/// How long ago `at_ms` was, in its largest unit: `2h`, `3d`.
fn age(at_ms: u64) -> String {
    let since = ket_core::now_ms().saturating_sub(at_ms);
    if since < 60_000 {
        return "now".to_owned();
    }
    crate::status_bar::compact_duration(since)
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned()
}
