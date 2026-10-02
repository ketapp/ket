//! Saving, and handing the file to a real editor at the caret.

use std::sync::Arc;

use gpui::{Context, SharedString};

use ket_core::surface::{EditorSurface, ExternalSurface, Position, ProcessSpawner};

use crate::Shell;

use super::syntax::SyntaxIndex;

impl Shell {
    /// Saves an existing file, or asks for an Untitled buffer's first path.
    pub(super) fn save_editor(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(state) = self.editors.get_mut(key) else {
            return;
        };
        if let Some(path) = state.path.clone() {
            match state.buffer.save() {
                Ok(stamp) => {
                    // What was just written is now the version this buffer is
                    // level with — without it the next poll would read the
                    // file back and "reload" the save onto itself.
                    state.stamp = Some(stamp);
                    self.note = Some(format!("saved {}", path.display()).into());
                    // What was uncommitted a moment ago is what was just
                    // written, so the rail is out of date the instant this
                    // returns.
                    self.refresh_editor_marks(cx);
                }
                Err(e) => self.note = Some(format!("could not save: {e}").into()),
            }
            return;
        }

        let directory = state.save_directory.clone();
        let editor_key: SharedString = key.to_owned().into();
        let chosen_path = cx.prompt_for_new_path(&directory, Some("untitled.txt"));
        cx.spawn(async move |this, cx| {
            let result = chosen_path.await;
            let _ = this.update(cx, |this, cx| {
                let path = match result {
                    Ok(Ok(Some(path))) => path,
                    Ok(Ok(None)) => return,
                    Ok(Err(error)) => {
                        this.note = Some(format!("could not choose a file: {error}").into());
                        cx.notify();
                        return;
                    }
                    Err(error) => {
                        this.note =
                            Some(format!("file chooser closed unexpectedly: {error}").into());
                        cx.notify();
                        return;
                    }
                };

                let Some(state) = this.editors.get_mut(editor_key.as_ref()) else {
                    return;
                };
                match state.buffer.save_as(&path) {
                    Ok(stamp) => {
                        state.stamp = Some(stamp);
                        state.path = Some(path.clone());
                        // The name it was just given is the first thing that
                        // says what language it is in.
                        state.language =
                            ket_core::syntax::language_for(&path.display().to_string());
                        state.syntax = SyntaxIndex::default();
                        state.save_directory = path.parent().unwrap_or(&path).to_path_buf();
                        let title: SharedString = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "untitled.txt".to_owned())
                            .into();
                        for space in this.spaces.values_mut() {
                            space.rename_editor(editor_key.as_ref(), title.clone());
                        }
                        this.note = Some(format!("saved {}", path.display()).into());
                        this.refresh_editor_marks(cx);
                        this.persist_layout();
                    }
                    Err(error) => this.note = Some(format!("could not save: {error}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hands the saved file at the cursor's position to `program` ("code" or "zed"),
    /// bypassing `$KET_EDITOR` deliberately: these are the two named,
    /// one-keystroke handoffs the pane offers regardless of what the user
    /// has configured for the generic external-editor escape hatch.
    pub(super) fn reveal_externally(&mut self, key: &str, program: &str) {
        let Some(state) = self.editors.get(key) else {
            return;
        };
        let Some(path) = state.path.as_deref() else {
            self.note = Some("save the file before opening it in another editor".into());
            return;
        };
        let (line, column) = state.cursor_line_col();
        let surface = match ExternalSurface::from_command_line(program, Arc::new(ProcessSpawner)) {
            Ok(surface) => surface,
            Err(e) => {
                self.note = Some(format!("{e}").into());
                return;
            }
        };
        let position = Position::file(path).with_line(line).with_column(column);
        self.note = Some(match surface.reveal(&position) {
            Ok(()) => format!("opened in {program}").into(),
            Err(e) => format!("{e}").into(),
        });
    }
}
