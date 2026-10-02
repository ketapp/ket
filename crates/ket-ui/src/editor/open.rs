//! Opening an editor tab, and keeping open ones current with disk and
//! with `HEAD`.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, SharedString, WeakEntity};

use ket_core::buffer::LineCol;
use ket_core::id::WorktreeId;
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::tabs::{PaneId, Tab, TabKind};

use super::state::EditorState;

/// How long the pane waits after the last keystroke before re-reading which
/// lines differ from `HEAD`.
///
/// Long enough that typing a word costs one read rather than one per
/// character, short enough that the rail has caught up by the time somebody
/// looks away from what they were writing.
const MARKS_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

impl Shell {
    /// Whether an editor pane is the surface taking keys, and so whether its
    /// caret should blink. `main.rs` asks this alongside [`Shell::typing`],
    /// which the editor deliberately does not answer to: the shell's own
    /// focus handle is what feeds it keys, and claiming to be typing would
    /// hand the keyboard to a field that does not exist.
    pub(crate) fn editor_active(&self) -> bool {
        self.active_editor_key().is_some() && !self.modal_open()
    }

    /// The stable in-process key of the active editor tab.
    pub(super) fn active_editor_key(&self) -> Option<SharedString> {
        match self.space()?.active_tab()?.kind.clone() {
            TabKind::Editor(key) => Some(key),
            _ => None,
        }
    }

    /// The absolute path of `id`'s worktree.
    ///
    /// Looked up fresh each time by reopening the workspace — spike-grade,
    /// matching `Selection`-handling's own `Workspace::open` per click in
    /// `tree.rs`, until Epic 5 gives the shell a workspace handle it can
    /// hold for its lifetime.
    pub(crate) fn worktree_root(id: &WorktreeId) -> Option<PathBuf> {
        let workspace = Workspace::open().ok()?;
        workspace
            .worktrees(None)
            .ok()?
            .into_iter()
            .find(|w| &w.id == id)
            .map(|w| w.path)
    }

    /// Opens `abs_path` as an editor tab in the active space, loading its
    /// buffer the first time or focusing the existing tab and buffer after.
    pub(crate) fn open_editor_tab(&mut self, abs_path: PathBuf, cx: &mut Context<Self>) {
        if crate::audio::is_audio_path(&abs_path) {
            self.open_audio_tab(abs_path);
            return;
        }
        let Some(worktree_id) = self.selected_id() else {
            return;
        };
        let key: SharedString = abs_path.display().to_string().into();
        let space = self.spaces.entry(worktree_id).or_default();

        let title = abs_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_owned());
        space.open_tab(Tab {
            title: title.into(),
            kind: TabKind::Editor(key.clone()),
            renamed: false,
            pinned: false,
        });

        // A state outlives its tab, so this is as often a file opened earlier
        // in the session as a new one — and disk may have moved on since,
        // which opening the file is precisely when a reader would notice.
        match self.editors.get_mut(&key) {
            Some(state) => {
                state.refresh_from_disk();
            }
            None => {
                self.editors.insert(key, EditorState::open(&abs_path));
            }
        }
        // Opening a file is exactly when somebody looks at what is changed in
        // it, and the tick that keeps the rail current is ten seconds wide.
        self.refresh_editor_marks(cx);
        self.persist_layout();
    }

    /// Opens a search result and places the cursor at its one-based position.
    pub(crate) fn open_editor_tab_at(
        &mut self,
        path: PathBuf,
        line: u32,
        column: u32,
        cx: &mut Context<Self>,
    ) {
        self.open_editor_tab(path.clone(), cx);
        let key: SharedString = path.display().to_string().into();
        let Some(state) = self.editors.get_mut(&key) else {
            return;
        };
        let cursor = state.buffer.char_at_line_col(LineCol { line, column });
        state.buffer.set_cursor(cursor);
        self.ensure_cursor_visible(&key);
    }

    /// Brings every buffer held this session back in line with disk — the
    /// shell's answer to "the agent just rewrote the file I am looking at".
    ///
    /// Polled from `main.rs`'s tick rather than driven by
    /// [`ket_core::surface::FileWatcher`]. The watcher is the better instrument
    /// and this should grow into it, but it watches a tree, so the shell would
    /// have to raise and drop one as the selected worktree moves and reconcile
    /// its overflow signal; the cost being avoided meanwhile is one `stat` per
    /// file opened this run, which is not a cost. The price of the poll is that
    /// a change can be up to one tick old, and only ever in a tab already on
    /// screen — opening one refreshes it there and then.
    ///
    /// Every state, not only the ones with a tab: a state outlives its tab, so
    /// a file closed and reopened is the same buffer, and it may as well have
    /// stayed true in the meantime.
    pub(crate) fn refresh_editors(&mut self, cx: &mut Context<Self>) {
        let reloaded: Vec<SharedString> = self
            .editors
            .iter_mut()
            .filter_map(|(key, state)| state.refresh_from_disk().then(|| key.clone()))
            .collect();
        // The match list a reload cleared, rebuilt against the new text. Here
        // rather than in `refresh_from_disk` because the query lives in a
        // `TextInput` entity, which takes a context to read.
        for key in &reloaded {
            if self
                .editors
                .get(key.as_ref())
                .is_some_and(|state| state.find.open)
            {
                self.refresh_find(key.as_ref(), false, cx);
            }
        }
        // Unconditionally, not only for what reloaded: the marks answer a
        // question about git as much as about the buffer, and a commit made in
        // the terminal beside this pane changes every one of them without
        // touching a file.
        self.refresh_editor_marks(cx);
    }

    /// Re-reads which lines of every open buffer differ from `HEAD`.
    ///
    /// **Off the window's thread**, like the sidebar's status sweep and for the
    /// same reason: each file means opening a repository, reading a blob out of
    /// it and running a diff — milliseconds apiece, and not work a frame can
    /// afford while somebody is typing into it.
    ///
    /// Against `HEAD` rather than the worktree's base, and against the buffer
    /// rather than the file on disk. [`ket_core::diff::line_marks`] carries why
    /// for both.
    ///
    /// Every state, not only the ones with a tab on screen: a state outlives
    /// its tab, and marks that went stale while a tab was closed would be drawn
    /// the moment it reopened, before the next tick corrected them.
    pub(super) fn refresh_editor_marks(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(SharedString, PathBuf, String)> = self
            .editors
            .iter()
            // Nothing for a buffer that is not the file it names: an Untitled
            // one, which git has never seen, and one whose read failed, whose
            // empty buffer would diff as "the whole file was deleted".
            .filter(|(_, state)| state.load_error.is_none())
            .filter_map(|(key, state)| {
                Some((key.clone(), state.path.clone()?, state.buffer.text()))
            })
            .collect();
        if wanted.is_empty() {
            return;
        }

        let read = cx.background_executor().spawn(async move {
            wanted
                .into_iter()
                .map(|(key, path, text)| {
                    // A file git cannot be asked about — one outside a
                    // repository, one in a repository that will not open — has
                    // no marks, which is the same as a file with nothing
                    // changed in it. Neither is worth a note in the window.
                    let marks = ket_core::diff::line_marks(&path, &text).unwrap_or_default();
                    (key, Arc::new(marks))
                })
                .collect::<Vec<_>>()
        });

        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let marks = read.await;
            let _ = shell.update(cx, |shell, cx| {
                // Written back by key rather than by position: a tab may have
                // been closed and its state dropped while this was reading.
                for (key, marks) in marks {
                    if let Some(state) = shell.editors.get_mut(&key) {
                        state.git = marks;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Schedules a marks read a short moment after the last keystroke.
    ///
    /// One field, so each keystroke replaces the timer the one before it set —
    /// dropping a `gpui::Task` cancels it — and a burst of typing costs one
    /// read at the end rather than one per character.
    pub(super) fn schedule_editor_marks(&mut self, cx: &mut Context<Self>) {
        self.editor_marks = Some(cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            cx.background_executor().timer(MARKS_DEBOUNCE).await;
            let _ = shell.update(cx, |shell, cx| shell.refresh_editor_marks(cx));
        }));
    }

    /// Opens a new in-memory plain-text editor in `pane` without touching disk.
    pub(crate) fn new_plain_text_file(&mut self, pane: PaneId) {
        let Some(worktree_id) = self.selected_id() else {
            return;
        };
        // The selected row's own path first. `worktree_root` asks the
        // workspace, which has never heard of the repository's own checkout.
        let Some(root) = self
            .selected_path()
            .or_else(|| Self::worktree_root(&worktree_id))
        else {
            self.note = Some("could not resolve the worktree's location".into());
            return;
        };
        if !self
            .spaces
            .get(&worktree_id)
            .is_some_and(|space| space.contains_pane(pane))
        {
            return;
        }

        let number = self.next_untitled;
        self.next_untitled += 1;
        let key: SharedString = format!("untitled:{number}").into();
        let title = if number == 1 {
            "Untitled".to_owned()
        } else {
            format!("Untitled {number}")
        };
        let tab = Tab {
            title: title.into(),
            kind: TabKind::Editor(key.clone()),
            renamed: false,
            pinned: false,
        };
        let Some(space) = self.spaces.get_mut(&worktree_id) else {
            return;
        };
        if space.open_tab_in(pane, tab) {
            self.editors.insert(key, EditorState::empty(root));
            self.persist_layout();
        }
    }
}
