//! Which of the shell's surfaces own the window: the modals, one at a time,
//! and every surface that takes typing.

use gpui::{App, Window};

use crate::Shell;

impl Shell {
    /// Whether some field of the shell's is taking typed characters.
    ///
    /// Two things read this. The shell takes the window's focus back the
    /// moment it goes false, and the caret's blink timer runs only while it is
    /// true — so a surface that takes typing has to be named here, or the
    /// field it just focused is blurred on the very next frame and the
    /// characters go wherever the shell's handle points. That is a terminal
    /// pane, which registers a text-input handler against exactly this handle:
    /// the settings filter used to type into the shell behind the modal for
    /// precisely this reason.
    ///
    /// The modals and overlays are named by whether they are *open*, not by
    /// whether their field has the keyboard yet: focus is asked for when one
    /// opens and granted a frame later, and a predicate that waited for the
    /// grant would blur the field in between. The panel's search box has no
    /// such gap — nothing but a click ever focuses it — so it answers for
    /// itself.
    pub(crate) fn typing(&self, window: &Window, cx: &App) -> bool {
        self.new_worktree.is_some()
            || self.settings.is_some()
            || self.adding_project.is_some()
            || self.preferences.is_some()
            || self.palette.open
            || self.snippet_picker.is_some()
            || self.finder.open
            || self.search.open
            || self.workspace_search_typing(window, cx)
            || self.find_open()
            || self.terminal_find_open()
            || self.tab_rename_open()
            || self.worktree_rename.is_some()
            || self.quick_prompt.is_some()
            || self.backlog.is_some()
            || crate::automations::editing(self)
            || self.quick_capture.is_some()
            || self.review_draft_focused()
            || self.feedback.typing()
            || self.browser_typing(window, cx)
            || self.chromium_dialog_typing_any(window, cx)
            || self.searches.explorer.read(cx).is_focused(window)
    }

    /// Whether a modal currently owns the window.
    pub(crate) fn modal_open(&self) -> bool {
        self.storage_dialog.is_some()
            || self.release_dialog.is_some()
            || self.preferences.is_some()
            || self.new_worktree.is_some()
            || self.settings.is_some()
            || self.adding_project.is_some()
            || self.confirm_remove.is_some()
            || self
                .confirm_remove_worktree
                .as_ref()
                .is_some_and(|confirm| !confirm.quiet)
            || self.confirm_clear_build.is_some()
            || self.confirm_discard.is_some()
            || self.confirm_close_session.is_some()
            || self.cold_resume.dialog.is_some()
            || self.confirm_merge_worktree.is_some()
            || self.confirm_pull_request.is_some()
            || self.confirm_merge_pull_request.is_some()
            || self.worktree_rename.is_some()
            || self.quick_prompt.is_some()
            || self.backlog.is_some()
            || self.automations.is_some()
            || self.quick_capture.is_some()
    }
}
