//! A project's backlog: notes on work not started yet — a title, a
//! description, files, tags and a [`Priority`] — kept until one is started as
//! a worktree with an agent on it, and then kept as done, with the branch it
//! went to. A note can also be marked done by hand, for work that happened
//! somewhere else.
//!
//! The waiting notes are kept in an order: most pressing first, and within a
//! priority wherever they were last moved to — see [`Backlog::sorted_open`].
//! A new note goes to the top of its priority, so a backlog nobody has
//! rearranged still reads newest first, as it did before notes could be
//! moved.
//!
//! By default a backlog is private to this person, like the rest of
//! [`paths::data_dir`]: one JSON file per project under
//! [`paths::backlog_dir`], and each note's files copied into a directory of
//! its own beside it, so a screenshot dropped in from the Desktop is still
//! there after the Desktop is tidied.
//!
//! ```text
//! backlog/<project-id>.json
//! backlog/<project-id>/<note-id>/<file>
//! ```
//!
//! A project can instead keep its backlog in the repository, where it travels
//! with the code and a team works from one list —
//! [`ProjectSettings::backlog_in_repo`], switched by
//! [`Backlog::set_location`]. There every note is a file of its own, so two
//! people editing different notes never touch the same file, and a merge of
//! their branches rarely conflicts:
//!
//! ```text
//! <repo>/.ket/backlog/<note-id>.json
//! <repo>/.ket/backlog/<note-id>/<file>
//! ```
//!
//! ket writes those files into the checkout and leaves committing them to
//! the person, like any other change there. A done note in a shared backlog
//! may name a worktree on someone else's machine — a [`WorktreeId`] only
//! means something where it was made — which is why [`Done`] keeps the branch
//! beside it: the branch is what reaches everyone else.
//!
//! Every function here finds the backlog wherever the project's setting says
//! it is. The `_in` variants take the root of a private store instead, so the
//! file handling is exercisable without a home or a state file.
//!
//! Removing a project leaves its backlog alone: a project's id comes from its
//! path, so adding the repository again brings the notes back. The choice of
//! where it is kept goes with the project's other settings, though, so a
//! backlog kept in the repository is found again by choosing the repository
//! again.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::config::write_atomically;
use crate::id::{ProjectId, WorktreeId};
use crate::project::{Project, ProjectSettings};
use crate::store::Store;
use crate::{KetError, Result, now_ms, paths};

/// Most tags a note keeps. Past this a tag stops sorting anything.
pub const MAX_TAGS: usize = 12;

/// Longest tag kept, in characters; a longer one is cut to this.
pub const MAX_TAG_CHARS: usize = 32;

/// How close two neighbouring ranks may get before [`Backlog::reorder`]
/// numbers their priority afresh rather than splitting the gap again.
const MIN_RANK_GAP: f64 = 1e-6;

/// One note.
///
/// `PartialEq` and not `Eq`, because [`Note::rank`] is a float.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Note {
    /// Stable across edits; names the note's directory of files, and in a
    /// repository its file.
    pub id: String,
    /// What the list shows, and the first line the agent is told.
    pub title: String,
    /// The description: what the agent is told, after the title.
    pub body: String,
    /// File names in the note's directory — see [`attachment_path`].
    pub attachments: Vec<String>,
    /// Words to find it by and group it with, without the `#` a search puts
    /// before one. Kept tidy by [`normalised_tags`] whenever it is saved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// How pressing it is. A note written before there was one is
    /// [`Priority::Medium`].
    pub priority: Priority,
    /// Where it sits among the open notes of its priority: lower is higher
    /// up. A float so that a move only has to change the note moved — see
    /// [`Backlog::reorder`]. A note written before there was one is 0, and
    /// equal ranks fall back to newest first, so an old file reads the same.
    pub rank: f64,
    /// When it was written.
    pub created_ms: u64,
    /// When its words or files last changed.
    pub updated_ms: u64,
    /// Set once the note has been started as work, or marked done.
    pub done: Option<Done>,
}

/// How pressing a note is. Ordered least to most, so the list can put the
/// most pressing first.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Whenever there is time.
    Low,
    /// The usual: what a new note is.
    #[default]
    Medium,
    /// Next, ahead of the usual.
    High,
    /// Before anything else.
    Urgent,
}

impl Priority {
    /// Every level, most pressing first — the order a picker lists them in.
    pub const ALL: [Self; 4] = [Self::Urgent, Self::High, Self::Medium, Self::Low];

    /// Its name, as a picker shows it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Urgent => "Urgent",
        }
    }
}

/// When a note was done, and where it went if it was started.
///
/// Both halves of the where are optional because a note marked done by hand
/// went nowhere ket made. A file written before that existed always has
/// them, and reads the same as it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// When it was started, or marked done.
    pub at_ms: u64,
    /// The worktree made for it. `None` when it was marked done by hand.
    ///
    /// Only meaningful on the machine that made it: in a backlog kept in the
    /// repository, it may be a teammate's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeId>,
    /// That worktree's branch, kept for when the worktree is gone — or was
    /// never on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl Done {
    /// Done by hand, now: no worktree, no branch.
    pub fn by_hand() -> Self {
        Self {
            at_ms: now_ms(),
            worktree: None,
            branch: None,
        }
    }
}

impl Note {
    /// A new, empty note with an id none of `taken` has.
    pub fn new(taken: &[Note]) -> Self {
        let now = now_ms();
        let mut stamp = now;
        let id = loop {
            let id = base36(stamp);
            if !taken.iter().any(|note| note.id == id) {
                break id;
            }
            stamp += 1;
        };
        Self {
            id,
            created_ms: now,
            updated_ms: now,
            ..Self::default()
        }
    }

    /// Nothing written in it yet.
    pub fn is_blank(&self) -> bool {
        self.title.trim().is_empty() && self.body.trim().is_empty() && self.attachments.is_empty()
    }

    /// Whether the note answers a search for `query`.
    ///
    /// Every word of the query has to be found, ignoring case: a `#word` as
    /// one of the note's tags, exactly, and any other word anywhere in its
    /// title, description or tags. A blank query, or a lone `#` typed on the
    /// way to a tag, leaves every note in.
    pub fn matches(&self, query: &str) -> bool {
        let title = self.title.to_lowercase();
        let body = self.body.to_lowercase();
        let tags: Vec<String> = self.tags.iter().map(|tag| tag.to_lowercase()).collect();
        query.split_whitespace().all(|term| {
            let term = term.to_lowercase();
            match term.strip_prefix('#') {
                Some(tag) => {
                    let tag = tag.trim_start_matches('#');
                    tag.is_empty() || tags.iter().any(|kept| kept == tag)
                }
                None => {
                    title.contains(&term)
                        || body.contains(&term)
                        || tags.iter().any(|kept| kept.contains(&term))
                }
            }
        })
    }

    /// What an agent is handed when the note is started: the title and the
    /// description, a blank line between them, trimmed. `None` when neither
    /// says anything, since there is then nothing to start.
    pub fn brief(&self) -> Option<String> {
        let words: Vec<&str> = [self.title.trim(), self.body.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        (!words.is_empty()).then(|| words.join("\n\n"))
    }
}

/// A project's notes, newest first. The order they are shown in is
/// [`Backlog::sorted_open`]'s and [`Backlog::sorted_done`]'s.
///
/// `PartialEq` and not `Eq`, because a [`Note`] is not.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backlog {
    /// Open and done alike.
    pub notes: Vec<Note>,
}

impl Backlog {
    /// The project's backlog. A missing file is an empty backlog; a malformed
    /// one is an error, and is never written over.
    pub fn load(project: &ProjectId) -> Result<Self> {
        Location::of(project)?.load()
    }

    /// The same, under an explicit root — so this is exercisable without a
    /// home. Mirrors [`crate::shell::ensure_wrappers`]/`ensure_wrappers_in`.
    pub fn load_in(root: &Path, project: &ProjectId) -> Result<Self> {
        Location::private(root, project).load()
    }

    /// Re-reads the file, applies `change` and writes it back, so an edit in
    /// one window never drops what another just saved. Returns the result.
    ///
    /// In a repository only the notes `change` touched are written, and a
    /// note it took out is deleted.
    pub fn update(project: &ProjectId, change: impl FnOnce(&mut Self)) -> Result<Self> {
        Location::of(project)?.update(|backlog| {
            change(backlog);
            Ok(())
        })
    }

    /// The same, under an explicit root.
    pub fn update_in(
        root: &Path,
        project: &ProjectId,
        change: impl FnOnce(&mut Self),
    ) -> Result<Self> {
        Location::private(root, project).update(|backlog| {
            change(backlog);
            Ok(())
        })
    }

    /// Saves `note`'s title, description, tags and priority, adding it at
    /// the top of its priority when it is new. Its files, done state and
    /// place in the list are left as the file has them.
    pub fn save_note(project: &ProjectId, note: &Note) -> Result<Self> {
        Location::of(project)?.update(|backlog| {
            backlog.save(note);
            Ok(())
        })
    }

    /// The same, under an explicit root.
    pub fn save_note_in(root: &Path, project: &ProjectId, note: &Note) -> Result<Self> {
        Location::private(root, project).update(|backlog| {
            backlog.save(note);
            Ok(())
        })
    }

    /// Deletes a note and its files.
    pub fn remove(project: &ProjectId, id: &str) -> Result<Self> {
        Self::remove_at(&Location::of(project)?, id)
    }

    /// The same, under an explicit root.
    pub fn remove_in(root: &Path, project: &ProjectId, id: &str) -> Result<Self> {
        Self::remove_at(&Location::private(root, project), id)
    }

    fn remove_at(at: &Location, id: &str) -> Result<Self> {
        let backlog = at.update(|backlog| {
            backlog.notes.retain(|note| note.id != id);
            Ok(())
        })?;
        remove_dir_all(&at.note_dir(id)?)?;
        Ok(backlog)
    }

    /// Copies `source` into the note's files, under its own name — or with
    /// `-2`, `-3` before the extension when the note has one by that name.
    /// The note must already be saved.
    pub fn attach(project: &ProjectId, id: &str, source: &Path) -> Result<Self> {
        Self::attach_at(&Location::of(project)?, id, source)
    }

    /// The same, under an explicit root.
    pub fn attach_in(root: &Path, project: &ProjectId, id: &str, source: &Path) -> Result<Self> {
        Self::attach_at(&Location::private(root, project), id, source)
    }

    fn attach_at(at: &Location, id: &str, source: &Path) -> Result<Self> {
        let dir = at.note_dir(id)?;
        // Before the directory is made: in a repository that has gone, making
        // it would put the repository's path back with nothing in it.
        at.present()?;
        std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| KetError::Conflict(format!("{} is not a file", source.display())))?;
        let name = unused_name(&dir, &name);
        let target = dir.join(&name);
        std::fs::copy(source, &target).map_err(|e| KetError::io(source, e))?;
        at.update(|backlog| {
            if let Some(note) = backlog.notes.iter_mut().find(|note| note.id == id) {
                note.attachments.push(name.clone());
                note.updated_ms = now_ms();
            }
            Ok(())
        })
    }

    /// Takes a file off a note and deletes its copy.
    pub fn detach(project: &ProjectId, id: &str, name: &str) -> Result<Self> {
        Self::detach_at(&Location::of(project)?, id, name)
    }

    /// The same, under an explicit root.
    pub fn detach_in(root: &Path, project: &ProjectId, id: &str, name: &str) -> Result<Self> {
        Self::detach_at(&Location::private(root, project), id, name)
    }

    fn detach_at(at: &Location, id: &str, name: &str) -> Result<Self> {
        let backlog = at.update(|backlog| {
            if let Some(note) = backlog.notes.iter_mut().find(|note| note.id == id) {
                note.attachments.retain(|kept| kept != name);
                note.updated_ms = now_ms();
            }
            Ok(())
        })?;
        let path = at.note_dir(id)?.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(KetError::io(&path, e)),
        }
        Ok(backlog)
    }

    /// Marks a note as done: started, or finished by hand — see [`Done`].
    pub fn mark_done(project: &ProjectId, id: &str, done: Done) -> Result<Self> {
        Location::of(project)?.update(|backlog| {
            backlog.finish(id, done);
            Ok(())
        })
    }

    /// The same, under an explicit root.
    pub fn mark_done_in(root: &Path, project: &ProjectId, id: &str, done: Done) -> Result<Self> {
        Location::private(root, project).update(|backlog| {
            backlog.finish(id, done);
            Ok(())
        })
    }

    /// Puts a done note back among the open ones, where its rank says.
    pub fn reopen(project: &ProjectId, id: &str) -> Result<Self> {
        Location::of(project)?.update(|backlog| {
            backlog.unfinish(id);
            Ok(())
        })
    }

    /// The same, under an explicit root.
    pub fn reopen_in(root: &Path, project: &ProjectId, id: &str) -> Result<Self> {
        Location::private(root, project).update(|backlog| {
            backlog.unfinish(id);
            Ok(())
        })
    }

    /// Moves the open note `id` to just before the open note `before`, or to
    /// the end of its priority when there is none — a drag in the list.
    ///
    /// Dropped before a note of another priority, it takes that priority:
    /// the list is one column, and where a note lands is what it means.
    /// Only the moved note's rank changes, to halfway between its new
    /// neighbours, unless they are too close to split; then that priority is
    /// numbered afresh, in the order it is shown.
    pub fn reorder(project: &ProjectId, id: &str, before: Option<&str>) -> Result<Self> {
        Location::of(project)?.update(|backlog| backlog.place(id, before))
    }

    /// The same, under an explicit root.
    pub fn reorder_in(
        root: &Path,
        project: &ProjectId,
        id: &str,
        before: Option<&str>,
    ) -> Result<Self> {
        Location::private(root, project).update(|backlog| backlog.place(id, before))
    }

    /// Keeps the project's backlog in its repository, or privately again,
    /// and moves every note and its files across. Returns the backlog where
    /// it now is.
    ///
    /// A note already on the other side is merged by id, and whichever copy
    /// was edited last is kept, so switching back and forth — or a second
    /// person switching to a repository that already has notes — loses
    /// nothing either side changed. The files are copied first and the old
    /// copies deleted last, after the setting has changed: a move cut short
    /// leaves notes in both places, never in neither, and choosing the same
    /// place again finishes it.
    ///
    /// Moving into a repository whose directory is gone is refused. Moving
    /// out of one only changes the setting, since there is nothing to read.
    ///
    /// The setting is written straight to the store, so no
    /// [`crate::event::Event::ProjectSettingsChanged`] is published: the
    /// caller holds the backlog this returns, and nothing else follows where
    /// a backlog is kept.
    pub fn set_location(project: &ProjectId, in_repo: bool) -> Result<Self> {
        let store = Store::open_default()?;
        let state = store.load()?;
        let root = state
            .projects
            .iter()
            .find(|known| &known.id == project)
            .map(|known| known.root.clone())
            .ok_or_else(|| KetError::UnknownProject(project.to_string()))?;
        let was_in_repo = state
            .project_settings
            .get(project)
            .is_some_and(|settings| settings.backlog_in_repo);

        let private = Location::private(&paths::backlog_dir()?, project);
        let repo = Location::Repo { root };
        let (from, to) = match in_repo {
            true => (&private, &repo),
            false => (&repo, &private),
        };
        to.present()?;
        let moved = match from.present() {
            Ok(()) => merge(from, to)?,
            Err(_) => Vec::new(),
        };

        if was_in_repo != in_repo {
            store.update(|state| {
                if !state.projects.iter().any(|known| &known.id == project) {
                    return Err(KetError::UnknownProject(project.to_string()));
                }
                let mut settings = state
                    .project_settings
                    .get(project)
                    .cloned()
                    .unwrap_or_default();
                settings.backlog_in_repo = in_repo;
                // Defaults are represented by absence, as
                // `Workspace::set_project_settings` keeps them.
                if settings == ProjectSettings::default() {
                    state.project_settings.remove(project);
                } else {
                    state.project_settings.insert(project.clone(), settings);
                }
                Ok(())
            })?;
        }

        from.forget(&moved)?;
        to.load()
    }

    /// Notes not started yet.
    pub fn open_count(&self) -> usize {
        self.notes.iter().filter(|note| note.done.is_none()).count()
    }

    /// The open notes in the order they are shown: most pressing first, then
    /// by [`Note::rank`], then newest first.
    pub fn sorted_open(&self) -> Vec<&Note> {
        let mut open: Vec<&Note> = self
            .notes
            .iter()
            .filter(|note| note.done.is_none())
            .collect();
        open.sort_by(|a, b| shown_order(a, b));
        open
    }

    /// The done notes, the most recently done first.
    pub fn sorted_done(&self) -> Vec<&Note> {
        let mut done: Vec<&Note> = self
            .notes
            .iter()
            .filter(|note| note.done.is_some())
            .collect();
        done.sort_by_key(|note| Reverse(note.done.as_ref().map_or(0, |done| done.at_ms)));
        done
    }

    /// When the project's backlog last changed on disk, or `None` when it has
    /// none — for a reader that recounts only when another process, a phone
    /// through the host, has written it since.
    ///
    /// In a repository that is the newest of its directory and its notes'
    /// files: the directory changes when a note is added or deleted, a file
    /// when one is edited in place — by hand, or by a pull.
    pub fn modified(project: &ProjectId) -> Option<SystemTime> {
        Location::of(project).ok()?.modified()
    }

    /// `note` saved over the one with its id, or added at the top of its
    /// priority — see [`Backlog::save_note`].
    fn save(&mut self, note: &Note) {
        let tags = normalised_tags(&note.tags);
        match self.notes.iter_mut().find(|saved| saved.id == note.id) {
            Some(saved) => {
                saved.title = note.title.clone();
                saved.body = note.body.clone();
                saved.tags = tags;
                saved.priority = note.priority;
                saved.updated_ms = now_ms();
            }
            None => {
                let top = self
                    .notes
                    .iter()
                    .filter(|open| open.done.is_none() && open.priority == note.priority)
                    .map(|open| open.rank)
                    .reduce(f64::min);
                let mut note = note.clone();
                note.tags = tags;
                note.rank = top.map_or(0.0, |top| top - 1.0);
                note.updated_ms = now_ms();
                self.notes.insert(0, note);
            }
        }
    }

    fn finish(&mut self, id: &str, done: Done) {
        if let Some(note) = self.notes.iter_mut().find(|note| note.id == id) {
            note.done = Some(done);
        }
    }

    fn unfinish(&mut self, id: &str) {
        if let Some(note) = self.notes.iter_mut().find(|note| note.id == id) {
            note.done = None;
            note.updated_ms = now_ms();
        }
    }

    /// The note `id` moved before `before` — see [`Backlog::reorder`].
    fn place(&mut self, id: &str, before: Option<&str>) -> Result<()> {
        let (own_priority, own_rank) = match self.notes.iter().find(|note| note.id == id) {
            None => return Err(KetError::Conflict(format!("no backlog note {id}"))),
            Some(note) if note.done.is_some() => {
                return Err(KetError::Conflict(format!(
                    "backlog note {id} is done; reopen it to move it"
                )));
            }
            Some(note) => (note.priority, note.rank),
        };
        if before == Some(id) {
            return Ok(());
        }
        let priority = match before {
            None => own_priority,
            Some(other) => self
                .notes
                .iter()
                .find(|note| note.id == other && note.done.is_none())
                .map(|note| note.priority)
                .ok_or_else(|| KetError::Conflict(format!("no open backlog note {other}")))?,
        };

        // The rest of that priority as it is shown, and where in it the note
        // goes.
        let mut order: Vec<(String, f64)> = self
            .sorted_open()
            .into_iter()
            .filter(|note| note.priority == priority && note.id != id)
            .map(|note| (note.id.clone(), note.rank))
            .collect();
        let at = before
            .and_then(|other| order.iter().position(|(each, _)| each == other))
            .unwrap_or(order.len());
        let above = at
            .checked_sub(1)
            .and_then(|index| order.get(index))
            .map(|(_, rank)| *rank);
        let below = order.get(at).map(|(_, rank)| *rank);
        let rank = match (above, below) {
            (Some(above), Some(below)) => {
                (below - above >= MIN_RANK_GAP).then_some(above + (below - above) / 2.0)
            }
            (Some(above), None) => Some(above + 1.0),
            (None, Some(below)) => Some(below - 1.0),
            (None, None) => Some(own_rank),
        };

        match rank {
            Some(rank) => {
                if let Some(note) = self.notes.iter_mut().find(|note| note.id == id) {
                    note.rank = rank;
                }
            }
            // Too close to split — or equal, as every note written before
            // ranks existed is — so the priority is numbered 0, 1, 2… in the
            // order it is shown, with the note in its new place.
            None => {
                order.insert(at, (id.to_owned(), own_rank));
                let mut next = 0.0;
                for (each, _) in &order {
                    if let Some(note) = self.notes.iter_mut().find(|note| &note.id == each) {
                        note.rank = next;
                    }
                    next += 1.0;
                }
            }
        }
        if let Some(note) = self.notes.iter_mut().find(|note| note.id == id)
            && note.priority != priority
        {
            note.priority = priority;
            note.updated_ms = now_ms();
        }
        Ok(())
    }
}

/// Where one of a note's files is kept.
pub fn attachment_path(project: &ProjectId, id: &str, name: &str) -> Result<PathBuf> {
    Ok(Location::of(project)?.note_dir(id)?.join(name))
}

/// The same, under an explicit root.
pub fn attachment_path_in(
    root: &Path,
    project: &ProjectId,
    id: &str,
    name: &str,
) -> Result<PathBuf> {
    Ok(Location::private(root, project).note_dir(id)?.join(name))
}

/// Where the repository at `root` keeps its backlog, when its project keeps
/// one there.
pub fn repo_dir(root: &Path) -> PathBuf {
    root.join(".ket").join("backlog")
}

/// The order open notes are shown in: most pressing first, then by
/// [`Note::rank`], then newest first — [`Backlog::sorted_open`]'s, for a
/// caller holding notes of its own.
pub fn shown_order(a: &Note, b: &Note) -> std::cmp::Ordering {
    b.priority
        .cmp(&a.priority)
        .then(a.rank.total_cmp(&b.rank))
        .then(b.created_ms.cmp(&a.created_ms))
}

/// `tags` the way a note keeps them: trimmed, without a leading `#`, spaces
/// inside one joined by `-`, and no blanks. A tag that differs from an
/// earlier one only in case is the same tag, so the first spelling stays.
/// At most [`MAX_TAGS`] of them, each cut to [`MAX_TAG_CHARS`].
///
/// Spaces are joined because a search splits on them: `#needs review` could
/// never find a tag that held one.
pub fn normalised_tags<S: AsRef<str>>(tags: impl IntoIterator<Item = S>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.as_ref().trim().trim_start_matches('#');
        let tag = tag.split_whitespace().collect::<Vec<_>>().join("-");
        let tag: String = tag.chars().take(MAX_TAG_CHARS).collect();
        let folded = tag.to_lowercase();
        if tag.is_empty() || kept.iter().any(|seen| seen.to_lowercase() == folded) {
            continue;
        }
        kept.push(tag);
        if kept.len() == MAX_TAGS {
            break;
        }
    }
    kept
}

/// Where a project's backlog is kept — see the module docs.
#[derive(Debug, Clone)]
enum Location {
    /// One file for the project under `root`: [`paths::backlog_dir`], or a
    /// test's own.
    Private { root: PathBuf, project: ProjectId },
    /// A file per note in the repository at `root` — see [`repo_dir`].
    Repo { root: PathBuf },
}

impl Location {
    /// Wherever `project`'s settings say.
    fn of(project: &ProjectId) -> Result<Self> {
        Ok(match kept_in_repo(project)? {
            Some(root) => Self::Repo { root },
            None => Self::private(&paths::backlog_dir()?, project),
        })
    }

    fn private(root: &Path, project: &ProjectId) -> Self {
        Self::Private {
            root: root.to_path_buf(),
            project: project.clone(),
        }
    }

    /// Refuses a repository whose directory has gone: an empty list would
    /// look like every note was lost, and a write would put the path back.
    fn present(&self) -> Result<()> {
        match self {
            Self::Repo { root } if !root.is_dir() => Err(KetError::Conflict(format!(
                "the backlog is kept in {}, which is not there",
                root.display()
            ))),
            _ => Ok(()),
        }
    }

    /// The directory holding note `id`'s files.
    fn note_dir(&self, id: &str) -> Result<PathBuf> {
        check_id(id)?;
        Ok(match self {
            Self::Private { root, project } => root.join(project.as_str()).join(id),
            Self::Repo { root } => repo_dir(root).join(id),
        })
    }

    fn load(&self) -> Result<Backlog> {
        self.present()?;
        match self {
            Self::Private { root, project } => load_file(&file_in(root, project)),
            Self::Repo { root } => load_notes(&repo_dir(root)),
        }
    }

    /// Re-reads the backlog, applies `change` and writes back what changed.
    /// Nothing is written when `change` fails.
    fn update(&self, change: impl FnOnce(&mut Backlog) -> Result<()>) -> Result<Backlog> {
        let mut backlog = self.load()?;
        match self {
            Self::Private { root, project } => {
                change(&mut backlog)?;
                let text = serde_json::to_string_pretty(&backlog)
                    .map_err(|e| KetError::Config(e.to_string()))?;
                write_atomically(&file_in(root, project), &text, false)?;
            }
            Self::Repo { root } => {
                let before = backlog.clone();
                change(&mut backlog)?;
                let dir = repo_dir(root);
                for note in &backlog.notes {
                    check_id(&note.id)?;
                }
                // Only what changed, so an edit is a one-file diff and a note
                // nobody touched never conflicts.
                for note in &backlog.notes {
                    if before.notes.iter().find(|old| old.id == note.id) == Some(note) {
                        continue;
                    }
                    let mut text = serde_json::to_string_pretty(note)
                        .map_err(|e| KetError::Config(e.to_string()))?;
                    // Committed, so it ends the way a text file in a
                    // repository is expected to.
                    text.push('\n');
                    write_atomically(&dir.join(format!("{}.json", note.id)), &text, false)?;
                }
                for old in &before.notes {
                    if !backlog.notes.iter().any(|note| note.id == old.id) {
                        remove_file(&dir.join(format!("{}.json", old.id)))?;
                    }
                }
            }
        }
        Ok(backlog)
    }

    fn modified(&self) -> Option<SystemTime> {
        let stamp = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified());
        match self {
            Self::Private { root, project } => stamp(&file_in(root, project)).ok(),
            Self::Repo { root } => {
                let dir = repo_dir(root);
                let mut newest = stamp(&dir).ok()?;
                for entry in std::fs::read_dir(&dir).ok()?.flatten() {
                    if entry.file_name().to_string_lossy().ends_with(".json")
                        && let Ok(changed) = entry.metadata().and_then(|meta| meta.modified())
                    {
                        newest = newest.max(changed);
                    }
                }
                Some(newest)
            }
        }
    }

    /// Deletes the notes `ids` and their files — the old copies, once
    /// [`Backlog::set_location`] has moved them. Leaves no empty file or
    /// directory behind.
    fn forget(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let left = self.update(|backlog| {
            backlog.notes.retain(|note| !ids.contains(&note.id));
            Ok(())
        })?;
        for id in ids {
            remove_dir_all(&self.note_dir(id)?)?;
        }
        if left.notes.is_empty() {
            // `remove_dir` only takes an empty directory, so anything else
            // kept there is left alone.
            match self {
                Self::Private { root, project } => {
                    remove_file(&file_in(root, project))?;
                    let _ = std::fs::remove_dir(root.join(project.as_str()));
                }
                Self::Repo { root } => {
                    let dir = repo_dir(root);
                    let _ = std::fs::remove_dir(&dir);
                    if let Some(parent) = dir.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Copies every note in `from` that `to` lacks, or has an older copy of, into
/// `to`, files first. Returns the ids of every note `from` had, all of which
/// `to` now holds as well or better.
fn merge(from: &Location, to: &Location) -> Result<Vec<String>> {
    let source = from.load()?;
    if source.notes.is_empty() {
        return Ok(Vec::new());
    }
    let target = to.load()?;
    let newer: Vec<Note> = source
        .notes
        .iter()
        .filter(|note| {
            target
                .notes
                .iter()
                .find(|kept| kept.id == note.id)
                .is_none_or(|kept| kept.updated_ms < note.updated_ms)
        })
        .cloned()
        .collect();
    // Files before the notes that list them, so a note never arrives
    // without its files.
    for note in &newer {
        replace_files(&from.note_dir(&note.id)?, &to.note_dir(&note.id)?)?;
    }
    to.update(|backlog| {
        for note in newer {
            match backlog.notes.iter_mut().find(|kept| kept.id == note.id) {
                Some(kept) => *kept = note,
                None => backlog.notes.push(note),
            }
        }
        backlog.notes.sort_by_key(|note| Reverse(note.created_ms));
        Ok(())
    })?;
    Ok(source.notes.into_iter().map(|note| note.id).collect())
}

/// Puts the files in `from` in place of whatever is in `to`.
fn replace_files(from: &Path, to: &Path) -> Result<()> {
    remove_dir_all(to)?;
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(KetError::io(from, e)),
    };
    std::fs::create_dir_all(to).map_err(|e| KetError::io(to, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| KetError::io(from, e))?;
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let source = entry.path();
            std::fs::copy(&source, to.join(entry.file_name()))
                .map_err(|e| KetError::io(&source, e))?;
        }
    }
    Ok(())
}

/// The repository root of `project` when its backlog is kept there.
///
/// Read off the state file through a view of the two fields it needs rather
/// than [`Store::load`], whose one-time migrations write — and this is read
/// by every backlog reader, the sidebar's on every tick. For the same reason
/// the answer is kept until the file changes.
fn kept_in_repo(project: &ProjectId) -> Result<Option<PathBuf>> {
    /// The state file read, its time and length then, and what it said.
    type Seen = (
        PathBuf,
        Option<(SystemTime, u64)>,
        BTreeMap<ProjectId, PathBuf>,
    );
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

    // The file `Store::open_default` reads.
    let path = paths::data_dir()?.join("state.json");
    let stamp = std::fs::metadata(&path)
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((seen_path, seen_stamp, roots)) = seen.as_ref()
        && seen_path == &path
        && seen_stamp == &stamp
    {
        return Ok(roots.get(project).cloned());
    }
    let roots = repo_roots(&path)?;
    let root = roots.get(project).cloned();
    *seen = Some((path, stamp, roots));
    Ok(root)
}

/// Every project in the state file at `path` that keeps its backlog in its
/// repository, with the repository's root.
fn repo_roots(path: &Path) -> Result<BTreeMap<ProjectId, PathBuf>> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct View {
        projects: Vec<Project>,
        project_settings: BTreeMap<ProjectId, ProjectSettings>,
    }

    let view: View = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => View::default(),
        Err(e) => return Err(KetError::io(path, e)),
    };
    Ok(view
        .projects
        .into_iter()
        .filter(|project| {
            view.project_settings
                .get(&project.id)
                .is_some_and(|settings| settings.backlog_in_repo)
        })
        .map(|project| (project.id, project.root))
        .collect())
}

fn load_file(path: &Path) -> Result<Backlog> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Backlog::default()),
        Err(e) => Err(KetError::io(path, e)),
    }
}

/// Every `<note-id>.json` in `dir`, newest first. The file's name is the
/// note's id, whatever the file says, so a note copied to a new name by hand
/// is a second note rather than a clash.
///
/// A file that will not parse is an error, as a malformed private backlog
/// is — after a merge, that is likely to be conflict markers, and naming the
/// file is what lets someone fix it.
fn load_notes(dir: &Path) -> Result<Backlog> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Backlog::default()),
        Err(e) => return Err(KetError::io(dir, e)),
    };
    let mut notes = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| KetError::io(dir, e))?.path();
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|id| check_id(id).is_ok())
        else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            // Deleted between the listing and the read — a pull, or another
            // window removing it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(KetError::io(&path, e)),
        };
        let mut note: Note = serde_json::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display())))?;
        id.clone_into(&mut note.id);
        notes.push(note);
    }
    notes.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(Backlog { notes })
}

fn file_in(root: &Path, project: &ProjectId) -> PathBuf {
    root.join(format!("{}.json", project.as_str()))
}

fn check_id(id: &str) -> Result<()> {
    // Ids are ket's own base36, but the file is editable by hand, and a note
    // id is joined onto a path that is later deleted.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(KetError::Conflict(format!("not a note id: {id:?}")));
    }
    Ok(())
}

/// Deletes `path`, which may already be gone.
fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(KetError::io(path, e)),
    }
}

/// Deletes the directory `path` and everything in it, which may already be
/// gone.
fn remove_dir_all(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(KetError::io(path, e)),
    }
}

/// `name`, or `stem-2.ext`, `stem-3.ext`… — the first `dir` has no file by.
fn unused_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_owned();
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| format!("{stem}-{n}{extension}"))
        .find(|candidate| !dir.join(candidate).exists())
        .unwrap_or_else(|| name.to_owned())
}

fn base36(mut n: u64) -> String {
    let mut digits = Vec::new();
    loop {
        let digit = u32::try_from(n % 36).unwrap_or(0);
        digits.push(char::from_digit(digit, 36).unwrap_or('0'));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    //! The store kept in a repository, and moving notes between it and the
    //! private one. Here rather than under `tests/`: which store a project
    //! uses is read off the real state file, and these reach the two stores
    //! directly instead.

    use super::*;

    /// A throwaway directory, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ket-backlog-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn note(id: &str, title: &str, updated_ms: u64) -> Note {
        Note {
            id: id.to_owned(),
            title: title.to_owned(),
            created_ms: updated_ms,
            updated_ms,
            ..Note::default()
        }
    }

    fn add(location: &Location, notes: Vec<Note>) {
        location
            .update(|backlog| {
                backlog.notes.extend(notes);
                Ok(())
            })
            .unwrap();
    }

    fn titles(location: &Location) -> Vec<String> {
        location
            .load()
            .unwrap()
            .notes
            .into_iter()
            .map(|note| note.title)
            .collect()
    }

    #[test]
    fn a_repository_keeps_one_file_per_note_ending_in_a_newline() {
        let scratch = Scratch::new("repo-files");
        let repo = Location::Repo {
            root: scratch.path("repo"),
        };
        std::fs::create_dir_all(scratch.path("repo")).unwrap();
        add(&repo, vec![note("a1", "first", 1), note("b2", "second", 2)]);

        let dir = repo_dir(&scratch.path("repo"));
        for id in ["a1", "b2"] {
            let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).unwrap();
            assert!(text.ends_with('\n'), "{id}.json: {text:?}");
        }
        assert_eq!(titles(&repo), ["second", "first"], "newest first");
    }

    #[test]
    fn an_edit_rewrites_only_the_note_that_changed() {
        let scratch = Scratch::new("repo-one-file");
        std::fs::create_dir_all(scratch.path("repo")).unwrap();
        let repo = Location::Repo {
            root: scratch.path("repo"),
        };
        add(&repo, vec![note("a1", "first", 1), note("b2", "second", 2)]);
        // Written by hand in another shape: rewriting it would change it.
        let untouched = repo_dir(&scratch.path("repo")).join("b2.json");
        let compact = serde_json::to_string(&note("b2", "second", 2)).unwrap();
        std::fs::write(&untouched, &compact).unwrap();

        repo.update(|backlog| {
            if let Some(first) = backlog.notes.iter_mut().find(|n| n.id == "a1") {
                first.title = "edited".to_owned();
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(std::fs::read_to_string(&untouched).unwrap(), compact);
        assert!(titles(&repo).contains(&"edited".to_owned()));
    }

    #[test]
    fn removing_a_note_deletes_its_file() {
        let scratch = Scratch::new("repo-remove");
        std::fs::create_dir_all(scratch.path("repo")).unwrap();
        let repo = Location::Repo {
            root: scratch.path("repo"),
        };
        add(&repo, vec![note("a1", "first", 1), note("b2", "second", 2)]);

        repo.update(|backlog| {
            backlog.notes.retain(|n| n.id != "a1");
            Ok(())
        })
        .unwrap();

        assert!(!repo_dir(&scratch.path("repo")).join("a1.json").exists());
        assert_eq!(titles(&repo), ["second"]);
    }

    #[test]
    fn a_note_file_that_will_not_parse_is_an_error_naming_it() {
        let scratch = Scratch::new("repo-malformed");
        let dir = repo_dir(&scratch.path("repo"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("zz9.json"), "<<<<<<< HEAD\n").unwrap();
        let repo = Location::Repo {
            root: scratch.path("repo"),
        };

        let error = repo.load().unwrap_err().to_string();
        assert!(error.contains("zz9.json"), "{error}");
    }

    #[test]
    fn a_repository_that_is_gone_is_refused_rather_than_read_as_empty() {
        let scratch = Scratch::new("repo-gone");
        let repo = Location::Repo {
            root: scratch.path("not-there"),
        };
        assert!(repo.load().is_err());
        assert!(
            repo.update(|_| Ok(())).is_err(),
            "and nothing writes it back"
        );
        assert!(!scratch.path("not-there").exists());
    }

    #[test]
    fn moving_notes_takes_their_files_and_leaves_nothing_behind() {
        let scratch = Scratch::new("move");
        let project = ProjectId::new("p1");
        let private = Location::private(&scratch.path("private"), &project);
        std::fs::create_dir_all(scratch.path("repo")).unwrap();
        let repo = Location::Repo {
            root: scratch.path("repo"),
        };
        let mut with_file = note("n1", "with a file", 1);
        with_file.attachments.push("log.txt".to_owned());
        add(&private, vec![with_file, note("n2", "plain", 2)]);
        let file = private.note_dir("n1").unwrap().join("log.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "the log").unwrap();

        let moved = merge(&private, &repo).unwrap();
        private.forget(&moved).unwrap();

        assert_eq!(titles(&repo), ["plain", "with a file"]);
        let copied = repo.note_dir("n1").unwrap().join("log.txt");
        assert_eq!(std::fs::read_to_string(copied).unwrap(), "the log");
        assert!(private.load().unwrap().notes.is_empty());
        assert!(!scratch.path("private/p1.json").exists());
        assert!(!scratch.path("private/p1").exists());
    }

    #[test]
    fn a_move_keeps_whichever_copy_was_edited_last() {
        let scratch = Scratch::new("move-newer");
        let project = ProjectId::new("p1");
        let private = Location::private(&scratch.path("private"), &project);
        std::fs::create_dir_all(scratch.path("repo")).unwrap();
        let repo = Location::Repo {
            root: scratch.path("repo"),
        };
        add(&repo, vec![note("n1", "edited in the repo", 200)]);
        add(
            &private,
            vec![note("n1", "older private copy", 100), note("n2", "new", 50)],
        );

        merge(&private, &repo).unwrap();
        let mut kept = titles(&repo);
        kept.sort();
        assert_eq!(kept, ["edited in the repo", "new"]);

        // And the other way: the private copy, edited since, wins.
        private
            .update(|backlog| {
                if let Some(n1) = backlog.notes.iter_mut().find(|n| n.id == "n1") {
                    n1.title = "edited privately".to_owned();
                    n1.updated_ms = 300;
                }
                Ok(())
            })
            .unwrap();
        merge(&private, &repo).unwrap();
        assert!(titles(&repo).contains(&"edited privately".to_owned()));
    }

    #[test]
    fn the_projects_kept_in_their_repositories_are_read_off_the_state_file() {
        #[derive(Serialize)]
        struct State {
            projects: Vec<Project>,
            project_settings: BTreeMap<ProjectId, ProjectSettings>,
        }
        let scratch = Scratch::new("repo-roots");
        let project = |id: &str| Project {
            id: ProjectId::new(id),
            name: id.to_owned(),
            root: scratch.path(id),
            default_base: "main".to_owned(),
            preferred_agent: None,
            last_opened_ms: 0,
        };
        let state = State {
            projects: vec![project("shared"), project("private")],
            project_settings: BTreeMap::from([(
                ProjectId::new("shared"),
                ProjectSettings {
                    backlog_in_repo: true,
                    ..ProjectSettings::default()
                },
            )]),
        };
        let path = scratch.path("state.json");
        std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();

        let roots = repo_roots(&path).unwrap();
        assert_eq!(
            roots.get(&ProjectId::new("shared")),
            Some(&scratch.path("shared"))
        );
        assert!(!roots.contains_key(&ProjectId::new("private")));
    }

    #[test]
    fn a_missing_state_file_keeps_every_backlog_private() {
        let scratch = Scratch::new("repo-roots-missing");
        assert!(repo_roots(&scratch.path("state.json")).unwrap().is_empty());
    }
}
