//! A project's settings dialog: the project form under the project's path,
//! with Remove beside Save.

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*};
use ket_core::id::ProjectId;
use ket_core::workspace::Workspace;

use super::form::{Field, ProjectForm, Seed};
use crate::Shell;
use crate::ui::button::button;
use crate::ui::chip::caption;
use crate::ui::dialog::{centered, footer, header};

/// The settings dialog, holding drafts of every field until **Save**.
pub(crate) struct SettingsDialog {
    /// The project being edited.
    pub(crate) id: ProjectId,
    /// Where it lives, shown so two projects with one name can be told apart.
    pub(crate) root: SharedString,
    /// The fields.
    pub(crate) form: ProjectForm,
    /// Whether the backlog was kept in the repository when the dialog
    /// opened: Save moves the notes only when the form changed it.
    pub(crate) backlog_was_in_repo: bool,
}

impl Shell {
    /// Opens the settings dialog for a project, with `focus` holding the keyboard.
    pub(crate) fn open_project_settings(
        &mut self,
        id: &ProjectId,
        focus: Field,
        cx: &mut Context<Self>,
    ) {
        self.project_menu = None;
        let Ok(workspace) = Workspace::open() else {
            return;
        };
        let Ok(projects) = workspace.projects() else {
            return;
        };
        let Some(project) = projects.into_iter().find(|p| &p.id == id) else {
            return;
        };
        let settings = workspace.project_settings(id).unwrap_or_default();
        let form = ProjectForm::new(
            &workspace,
            Seed {
                dir_name: &project.name,
                settings: &settings,
                base: &project.default_base,
                agent: project.preferred_agent.clone(),
                trusted: workspace.automation_trusted(id).unwrap_or(false),
                backlog_in_repo: Some(settings.backlog_in_repo),
            },
            focus,
            cx,
        );
        self.watch_form(&form, focus, cx);
        self.adding_project = None;
        self.settings = Some(SettingsDialog {
            id: id.clone(),
            root: project.root.display().to_string().into(),
            form,
            backlog_was_in_repo: settings.backlog_in_repo,
        });
    }

    /// Hands the dialog's drafts to core, closing on success. A refusal leaves
    /// the dialog open with the reason.
    pub(crate) fn save_project_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings.as_mut() else {
            return;
        };
        let Some(drafts) = dialog.form.drafts(cx) else {
            return;
        };
        let id = dialog.id.clone();
        // Moved after the rest is written, and only when it changed: moving
        // copies every note and its files across.
        let move_backlog = dialog
            .form
            .backlog_in_repo
            .filter(|&in_repo| in_repo != dialog.backlog_was_in_repo);

        let saved = Workspace::open()
            .and_then(|workspace| drafts.write(&workspace, &id))
            .and_then(|()| match move_backlog {
                Some(in_repo) => ket_core::backlog::Backlog::set_location(&id, in_repo).map(|_| ()),
                None => Ok(()),
            });
        match saved {
            Ok(()) => {
                self.settings = None;
                self.reload(cx);
                // A command that points `claude` at another config directory
                // needs ket's hooks there too, or this project's rows go dim.
                crate::tree::install_claude_hooks_in_background();
            }
            Err(e) => {
                if let Some(dialog) = self.settings.as_mut() {
                    dialog.form.error = Some(e.to_string().into());
                }
            }
        }
    }

    /// The settings dialog, when it is open.
    pub(crate) fn settings_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.settings.as_ref()?;
        let t = &self.theme;

        let remove_id = dialog.id.clone();
        let dialog_footer = footer()
            .justify_start()
            .child(
                button("settings-remove", "Remove project…")
                    .danger()
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_remove_project(&remove_id);
                        cx.notify();
                    })),
            )
            .child(div().flex_grow())
            .child(
                button("settings-cancel", "Cancel")
                    .ghost()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings = None;
                        cx.notify();
                    })),
            )
            .child(
                button("settings-save", "Save")
                    .primary()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_project_settings(cx);
                        cx.notify();
                    })),
            );

        Some(
            centered(
                self.form_card("project-settings", cx)
                    .child(header("Project settings", t).child(
                        caption(dialog.root.clone(), t).font_family(self.font_family.clone()),
                    ))
                    .child(self.form_identity(&dialog.form, window, cx))
                    .children(self.form_rows(&dialog.form, window, cx))
                    .child(dialog_footer),
            )
            .into_any_element(),
        )
    }
}
