//! The add-project dialog: a folder line over the project form, so a project
//! arrives named, coloured and running the right agent rather than being
//! fixed up after it lands in the sidebar.
//!
//! Nothing is registered until **Add project**. The folder is read when it is
//! chosen — its name and default branch fill the blanks — and registered only
//! with the drafts, so Cancel leaves the sidebar exactly as it was.

use std::path::Path;

use gpui::{AnyElement, Context, PathPromptOptions, Window, div, prelude::*, px};
use ket_core::id::ProjectId;
use ket_core::project::{Project, ProjectSettings};
use ket_core::workspace::Workspace;

use super::form::{Field, ProjectForm, Seed};
use crate::Shell;
use crate::ui::button::button;
use crate::ui::dialog::{centered, footer, header};
use crate::ui::field::field;
use crate::ui::icon::Icon;

/// The add-project dialog.
pub(crate) struct AddProjectDialog {
    /// The project in the chosen folder, read but not registered. `None`
    /// until a folder is chosen.
    pub(crate) found: Option<Project>,
    /// The fields.
    pub(crate) form: ProjectForm,
}

impl Shell {
    /// Opens the add-project dialog.
    pub(crate) fn add_project(&mut self, cx: &mut Context<Self>) {
        self.project_menu = None;
        let workspace = match Workspace::open() {
            Ok(workspace) => workspace,
            Err(e) => {
                self.note = Some(format!("could not open the workspace: {e}").into());
                return;
            }
        };
        let form = ProjectForm::new(
            &workspace,
            Seed {
                dir_name: "",
                settings: &ProjectSettings::default(),
                base: "",
                agent: None,
                trusted: false,
                backlog_in_repo: None,
            },
            Field::Name,
            cx,
        );
        self.watch_form(&form, Field::Name, cx);
        self.settings = None;
        self.adding_project = Some(AddProjectDialog { found: None, form });
    }

    /// Asks for the folder to add.
    ///
    /// The OS picker rather than a text field: a path is the one input a
    /// person should never have to type.
    fn choose_project_folder(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.take_project_folder(&path, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Fills the dialog from the repository containing `path`.
    ///
    /// Read, not registered: the folder's name becomes the name and initial a
    /// blank field falls back to, and its default branch the base. A folder
    /// that is not in a repository, or a project already in the sidebar, is
    /// refused here, where the folder was chosen, rather than on Add.
    fn take_project_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        let found = Project::discover(path);
        let known = |id: &ProjectId| self.projects.iter().any(|node| &node.id == id);
        let refusal = match &found {
            Ok(project) if known(&project.id) => Some(format!(
                "{} is already in the sidebar",
                project.root.display()
            )),
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        let Some(dialog) = self.adding_project.as_mut() else {
            return;
        };
        let form = &mut dialog.form;
        if let Some(why) = refusal {
            form.error = Some(why.into());
            return;
        }
        let Ok(project) = found else {
            return;
        };

        // The base follows the folder until someone types one of their own.
        let base = form.base.read(cx).text();
        let detected = dialog.found.as_ref().map(|old| old.default_base.as_str());
        if base.trim().is_empty() || Some(base.trim()) == detected {
            form.base
                .update(cx, |input, _| input.set_text(&project.default_base));
        }
        form.name
            .update(cx, |input, _| input.set_placeholder(project.name.clone()));
        form.dir_name = project.name.clone().into();
        form.error = None;
        dialog.found = Some(project);
    }

    /// Registers the chosen folder with the drafts, closing on success.
    ///
    /// A draft core refuses takes the project back out again, so the dialog
    /// stays up with the reason and Cancel still means nothing was added.
    pub(crate) fn add_project_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.adding_project.as_mut() else {
            return;
        };
        let Some(found) = dialog.found.as_ref() else {
            dialog.form.error = Some("Choose the folder to add first".into());
            return;
        };
        let root = found.root.clone();
        let id = found.id.clone();
        let Some(drafts) = dialog.form.drafts(cx) else {
            return;
        };

        let result = Workspace::open().and_then(|workspace| {
            // Checked again rather than trusted from when the folder was
            // chosen: the CLI may have added it since, and taking a refusal
            // back out must never remove a project someone else registered.
            if workspace.projects()?.iter().any(|p| p.id == id) {
                return Err(ket_core::KetError::Conflict(format!(
                    "{} is already in the sidebar",
                    root.display()
                )));
            }
            let project = workspace.add_project(&root)?;
            if let Err(e) = drafts.write(&workspace, &project.id) {
                let _ = workspace.remove_project(&project.id, false);
                return Err(e);
            }
            // Touching it sorts it to the top, which is where a project you
            // just added should be.
            let _ = workspace.touch_project(&project.id);
            Ok(project.id)
        });

        match result {
            Ok(id) => {
                self.adding_project = None;
                self.reload(cx);
                self.show_added(&id, cx);
                // A command that points `claude` at another config directory
                // needs ket's hooks there too, or this project's rows go dim.
                crate::tree::install_claude_hooks_in_background();
            }
            Err(e) => {
                if let Some(dialog) = self.adding_project.as_mut() {
                    dialog.form.error = Some(e.to_string().into());
                }
            }
        }
    }

    /// Brings a project that was just added into view: its first worktree
    /// when it already has some, else a note saying what to do next.
    fn show_added(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        self.note = None;
        let Some(index) = self.projects.iter().position(|p| &p.id == id) else {
            return;
        };
        if self.projects[index].worktrees.is_empty() {
            self.note = Some(
                format!(
                    "added {} — create a worktree to start an agent on it",
                    self.projects[index].name
                )
                .into(),
            );
        } else {
            self.select(
                crate::tree::Selection {
                    project: index,
                    worktree: 0,
                },
                cx,
            );
        }
    }

    /// The folder being added, above everything that defaults from it.
    ///
    /// A field that only shows the path — choosing is the OS picker's job —
    /// and a press on it or the button beside it opens the picker.
    fn folder_line(&self, dialog: &AddProjectDialog, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let mut path = field(
            "project-folder",
            dialog
                .found
                .as_ref()
                .map(|project| crate::header::tilde(&project.root))
                .unwrap_or_default(),
            "No folder chosen",
        )
        .read_only()
        .leading(Icon::Folder);
        if let Some(project) = &dialog.found {
            // One line, cut at the end: the field's height is fixed, and a
            // long path that wrapped would lose its second line instead.
            path = path.body(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(self.font_family.clone())
                    .child(crate::header::tilde(&project.root))
                    .into_any_element(),
            );
        }
        let path = path
            .render(t)
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.choose_project_folder(cx);
                cx.notify();
            }));
        let choose = button(
            "project-folder-choose",
            if dialog.found.is_some() {
                "Change…"
            } else {
                "Choose…"
            },
        )
        .render(t)
        .on_click(cx.listener(|this, _, _, cx| {
            this.choose_project_folder(cx);
            cx.notify();
        }));

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(path)
            .child(choose)
            .into_any_element()
    }

    /// The add-project dialog, when it is open.
    pub(crate) fn add_project_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.adding_project.as_ref()?;
        let t = &self.theme;

        let dialog_footer = footer()
            .child(
                button("add-project-cancel", "Cancel")
                    .ghost()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.adding_project = None;
                        cx.notify();
                    })),
            )
            .child(
                button("add-project-confirm", "Add project")
                    .primary()
                    .enabled(dialog.found.is_some())
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_project_confirmed(cx);
                        cx.notify();
                    })),
            );

        Some(
            centered(
                self.form_card("add-project", cx)
                    .child(header("Add project", t))
                    .child(self.folder_line(dialog, cx))
                    .child(self.form_identity(&dialog.form, window, cx))
                    .children(self.form_rows(&dialog.form, window, cx))
                    .child(dialog_footer),
            )
            .into_any_element(),
        )
    }
}
