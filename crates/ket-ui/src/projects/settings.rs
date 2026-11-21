//! A project's settings dialog, under the project's path: how the project
//! looks on the left, how it behaves on the right.
//!
//! The same [`ProjectForm`] the add dialog fills, in the same two columns
//! ([`Shell::form_columns`]), which the add dialog sets under its folder.
//! The badge's icon is a dropdown of its own, with a
//! filter over its glyphs, a picture picked from disk can stand in for it,
//! and its colour is always on show; an on/off setting is a switch; and
//! what an item means is in the tooltip on the ⓘ after its label rather
//! than in a caption under its control. Removing the
//! project is the project menu's and the command palette's to offer, not
//! this dialog's.
//!
//! This file holds the dialog, Save, the columns, and the tooltips both
//! dialogs use.
//! The columns are [`appearance`] and [`behaviour`], drawn from the shared
//! parts in [`crate::ui::section`].

use std::path::Path;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, MouseButton, Pixels, SharedString, Stateful, Window, div, prelude::*,
    px,
};
use ket_core::id::ProjectId;
use ket_core::workspace::Workspace;

use super::form::{Field, ImageDraft, ProjectForm, Seed};
use crate::Shell;
use crate::ui::button::button;
use crate::ui::chip::caption;
use crate::ui::dialog::{centered, footer, header};
use crate::ui::field::error as field_error;
use crate::ui::rule::vertical_rule;
use crate::ui::section::{info, info_label};
use crate::ui::tooltip::{Side, tooltip};

mod appearance;
mod behaviour;

/// Either dialog's width: two columns and the rule between them.
pub(super) const WIDTH: Pixels = px(920.0);

/// How wide the appearance column is, and every control at the end of a row
/// in the behaviour column, so the controls on both sides are one width. The
/// menus those controls open are as wide — see `ProjectForm::menu_width`.
pub(super) const CONTROL_W: Pixels = px(300.0);

/// Between the columns and the rule that parts them.
const COLUMN_GAP: Pixels = px(24.0);

/// Between a label and the control under it.
const STACK_GAP: Pixels = px(6.0);

/// Between one item and the next in the appearance column, and between the
/// behaviour column's sections and the rules between them.
pub(super) const ITEM_GAP: Pixels = px(14.0);

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
    /// The GitHub repository the project's issues go to, as its picked
    /// account sees it, or why there is none. `None` while it is asked.
    pub(crate) github_repo: Option<Result<String, String>>,
}

/// An item's ⓘ, and what its tooltip says — and the image's remove button,
/// which is a glyph with no word of its own.
///
/// One line each — the tooltip draws one — so each says what the item is
/// for, not everything about it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tip {
    /// The add dialog's folder.
    Folder,
    Name,
    Icon,
    /// The icon's ⓘ while a picture is shown instead of it.
    IconSetAside,
    Image,
    /// Not an ⓘ: the image field's remove button.
    RemoveImage,
    Colour,
    Base,
    HideDiscovered,
    Hooks,
    Agent,
    Command,
    StoreNotes,
    Sync,
    Account,
}

impl Tip {
    /// The tooltip's words.
    fn words(self) -> &'static str {
        match self {
            Self::Folder => "The repository to add. The fields below start from what is in it.",
            Self::Name => "What ket calls this project. Leave it blank to use the folder's name.",
            Self::Icon => "The mark on the project's badge: a glyph, a letter or an emoji.",
            Self::IconSetAside => "The image is shown instead. Remove it and this icon comes back.",
            Self::Image => "A favicon or logo to show on the badge instead of the icon.",
            Self::RemoveImage => "Remove the image",
            Self::Colour => "The badge's colour, and the rail on a worktree you select.",
            Self::Base => "The branch each new worktree is cut from.",
            Self::HideDiscovered => {
                "Leaves out worktrees made outside ket, by git or another tool."
            }
            Self::Hooks => "Lets this repository's .ket.toml run its hooks and post-command.",
            Self::Agent => "The agent for this project's worktrees. Settings picks until you do.",
            Self::Command => "How the agent is launched here. Blank runs the one in Settings.",
            Self::StoreNotes => {
                "Private stays on this Mac. In the repository shares it in .ket/backlog/."
            }
            Self::Sync => "Open issues arrive as notes. Done or deleted here closes the issue.",
            Self::Account => "The account ket uses for this project's issues. Default follows gh.",
        }
    }
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
        self.tucked_menu = None;
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
        let mut form = ProjectForm::new(
            &workspace,
            Seed {
                dir_name: &project.name,
                settings: &settings,
                base: &project.default_base,
                agent: project.preferred_agent.clone(),
                trusted: workspace.automation_trusted(id).unwrap_or(false),
                backlog_in_repo: Some(settings.backlog_in_repo),
                github_issues: Some(settings.github_issues),
                github_account: Some(settings.github_account.clone()),
            },
            focus,
            cx,
        );
        // Looked for once, here: a copy deleted by hand is no picture the
        // badge can show, so the dialog offers to choose one rather than
        // naming a file that is not there. The name stays stored until a
        // picture is chosen or taken off.
        form.kept_image = workspace
            .project_image_path(id, &settings)
            .filter(|path| path.is_file())
            .map(Arc::from);
        // An opener asking for an agent's command gets the one on show.
        let focus = form.visible(focus);
        self.watch_form(&form, focus, cx);
        self.adding_project = None;
        self.settings = Some(SettingsDialog {
            id: id.clone(),
            root: project.root.display().to_string().into(),
            form,
            backlog_was_in_repo: settings.backlog_in_repo,
            github_repo: None,
        });
        // Asked every time: an account may have been added since.
        self.read_github_accounts(cx);
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
        let picked = matches!(dialog.form.image, ImageDraft::Picked(_));

        let saved = Workspace::open()
            .and_then(|workspace| drafts.write(&workspace, &id))
            .and_then(|()| match move_backlog {
                Some(in_repo) => ket_core::backlog::Backlog::set_location(&id, in_repo).map(|_| ()),
                None => Ok(()),
            });
        match saved {
            Ok(()) => {
                self.settings = None;
                // Switched to follow GitHub issues, it should not wait minutes.
                self.issue_sync.forget(&id);
                self.reload(cx);
                // A copy is named for its contents, so the same picture
                // picked again lands where it was. Read afresh, or a copy
                // that was missing when the badge first asked would stay
                // missing to gpui's cache, which keeps a failure as long as
                // a success.
                if picked
                    && let Some(path) = self
                        .projects
                        .iter()
                        .find(|node| node.id == id)
                        .and_then(|node| node.image.clone())
                {
                    cx.remove_asset::<gpui::ImgResourceLoader>(&gpui::Resource::Path(path));
                }
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

        let dialog_footer = footer()
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

        let root = Path::new(dialog.root.as_ref());
        let columns = self.form_columns(&dialog.form, Some(root), window, cx);

        Some(
            centered(
                self.form_card("project-settings", WIDTH, cx)
                    .child(
                        header("settings-close", "Project settings")
                            .aside(
                                caption(dialog.root.clone(), t)
                                    .font_family(self.font_family.clone()),
                            )
                            .on_close(cx.listener(|this, _, _, cx| {
                                this.settings = None;
                                cx.notify();
                            }))
                            .render(t),
                    )
                    .child(columns)
                    .children(dialog.form.error.clone().map(|why| field_error(why, t)))
                    .child(dialog_footer),
            )
            // A press on the scrim, outside the card, is outside every panel
            // too. The card occludes, so a press on it never lands here.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.put_form_panels_away(window, cx) {
                        cx.notify();
                    }
                }),
            )
            .into_any_element(),
        )
    }

    /// Both dialogs' body: how the project looks on the left, how it
    /// behaves on the right, a rule between. `root` is the project's folder,
    /// which a picture picked from inside it is named from; `None` before a
    /// new project has one.
    pub(super) fn form_columns(
        &self,
        form: &ProjectForm,
        root: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .gap(COLUMN_GAP)
            .child(self.appearance(form, root, window, cx))
            .child(vertical_rule(&self.theme))
            .child(self.behaviour(form, window, cx))
    }

    /// An item with its label over it, the way the appearance column sets
    /// them out: the add dialog's folder sits this way too.
    pub(super) fn stacked_item(
        &self,
        words: &'static str,
        tip: Tip,
        shown: Option<Tip>,
        control: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> Div {
        stacked(self.item_label(words, tip, shown, cx), control)
    }

    // ---- labels ---------------------------------------------------------------

    /// An item's label: its words, then the ⓘ whose tooltip says what the
    /// item is for.
    fn item_label(
        &self,
        words: impl IntoElement,
        tip: Tip,
        shown: Option<Tip>,
        cx: &mut Context<Self>,
    ) -> Div {
        let mark = info(
            ("project-info", tip as usize),
            tip.words(),
            shown == Some(tip),
            &self.theme,
        );
        info_label(words, self.track_tip(tip, mark, cx), &self.theme)
    }

    /// `trigger` with `tip`'s tooltip up while the pointer is on it.
    fn with_project_tip(
        &self,
        tip: Tip,
        trigger: AnyElement,
        shown: Option<Tip>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tipped = tooltip(
            ("project-tip", tip as usize),
            trigger,
            tip.words(),
            Side::Bottom,
            shown == Some(tip),
            &self.theme,
        );
        self.track_tip(tip, tipped, cx)
    }

    /// `tipped` telling the dialog when the pointer is on it, so `tip`'s
    /// tooltip goes up and comes down with it.
    fn track_tip(&self, tip: Tip, tipped: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        tipped
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if let Some(form) = this.project_form_mut() {
                    if *hovered {
                        form.tip = Some(tip);
                    } else if form.tip == Some(tip) {
                        form.tip = None;
                    }
                }
                cx.notify();
            }))
            .into_any_element()
    }
}

/// A label over its control.
fn stacked(label: Div, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(STACK_GAP)
        .child(label)
        .child(control)
}
