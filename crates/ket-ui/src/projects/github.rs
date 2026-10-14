//! A project's GitHub account: which of the Mac's accounts its issues are
//! read and written as, and the repository that makes them go to.
//!
//! The accounts themselves are added and removed in Settings → GitHub (see
//! [`crate::github`]); a project only picks one. With none signed in yet,
//! the same Install and Sign in buttons appear here, so a project can be set
//! up without leaving its settings.

use std::path::PathBuf;

use gpui::{AnyElement, Context, KeyDownEvent, MouseButton, WeakEntity, div, prelude::*, px};
use ket_core::github::{self, Checkout};

use crate::Shell;
use crate::paint::paint;
use crate::ui::chip::{caption, tag};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::select::select;

impl Shell {
    /// Has the open settings check which repository the project's issues go
    /// to, as the account its form has picked. Asking as that account also
    /// says whether it can see the repository at all.
    pub(crate) fn read_project_repository(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings.as_mut() else {
            return;
        };
        if !self.github.connected() {
            dialog.github_repo = None;
            return;
        }
        let at = Checkout::new(
            PathBuf::from(dialog.root.as_ref()),
            dialog.form.github_account.clone().flatten(),
        );
        dialog.github_repo = None;
        let id = dialog.id.clone();
        let asked = at.account.clone();
        let read = cx
            .background_executor()
            .spawn(async move { github::repository(&at).map_err(|e| e.to_string()) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let repo = read.await;
            let _ = shell.update(cx, |shell, cx| {
                // Only the answer to what the form still asks.
                if let Some(dialog) = shell.settings.as_mut()
                    && dialog.id == id
                    && dialog.form.github_account.clone().flatten() == asked
                {
                    dialog.github_repo = Some(repo);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The GitHub line under the sync switch: the account picker and where
    /// issues go, or what it takes to get there. `None` for a project being
    /// added, which picks its account once it has settings.
    pub(super) fn github_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.settings.as_ref()?;
        let t = &self.theme;
        if !self.github.connected() {
            return Some(self.github_setup("project-github", cx));
        }
        let repo = match &dialog.github_repo {
            None => caption("Checking the repository\u{2026}", t),
            Some(Ok(repo)) => caption(format!("Issues go to {repo}"), t),
            Some(Err(why)) => caption(why.clone(), t).text_color(paint(t.status.failed)),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .min_w_0()
                .child(self.form_github_account(cx))
                .child(repo)
                .into_any_element(),
        )
    }

    /// The account dropdown: "Default", which follows whichever account `gh`
    /// has active, then each account signed in on this Mac.
    fn form_github_account(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let Some(form) = self.settings.as_ref().map(|dialog| &dialog.form) else {
            return div().into_any_element();
        };
        let picked = form.github_account.clone().flatten();
        let shown = match &picked {
            Some(login) => format!("@{login}"),
            None => "Default".to_owned(),
        };
        let mut trigger = select("project-github-account", shown)
            .leading(crate::ui::icon::icon(Icon::GitHub, paint(t.text.dim)))
            .open(form.github_menu.is_some());
        if picked.is_none()
            && let Some(active) = self.github.active()
        {
            trigger = trigger.trailing(tag(format!("@{}", active.login), t));
        }
        let panel = form.github_menu.as_ref().map(|menu| {
            menu.view(
                t,
                cx,
                |shell, row, cx| {
                    shell.pick_form_github_account(*row, cx);
                    cx.notify();
                },
                |shell, cx| {
                    if let Some(form) = shell.project_form_mut() {
                        form.github_menu = None;
                    }
                    cx.notify();
                },
            )
        });
        dropdown("project-github-account-select", trigger.render(t), panel)
            .w_full()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.toggle_form_github_menu();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Opens or closes the account dropdown.
    fn toggle_form_github_menu(&mut self) {
        let logins: Vec<String> = self
            .github
            .pickable()
            .into_iter()
            .map(|account| account.login.clone())
            .collect();
        let active = self.github.active().map(|account| account.login.clone());
        let Some(form) = self.project_form_mut() else {
            return;
        };
        if form.github_menu.take().is_some() {
            return;
        }
        form.agent_menu = None;
        form.palette_open = false;
        let picked = form.github_account.clone().flatten();
        form.github_menu = Some(account_menu(&logins, active, picked.as_deref()));
        form.github_logins = logins;
    }

    /// Takes a pick from the account dropdown: row 0 is the default.
    fn pick_form_github_account(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(form) = self.project_form_mut() {
            let picked = row
                .checked_sub(1)
                .and_then(|row| form.github_logins.get(row).cloned());
            form.github_account = Some(picked);
            form.github_menu = None;
            form.error = None;
        }
        self.read_project_repository(cx);
    }

    /// Lets the account dropdown take keys before the form does.
    pub(super) fn form_github_menu_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self
            .project_form_mut()
            .and_then(|form| form.github_menu.as_mut())
        else {
            return false;
        };
        match menu.key(event) {
            MenuKey::Ignored => false,
            MenuKey::Consumed => true,
            MenuKey::Close => {
                if let Some(form) = self.project_form_mut() {
                    form.github_menu = None;
                }
                true
            }
            MenuKey::Run(row) => {
                self.pick_form_github_account(row, cx);
                true
            }
        }
    }
}

/// The account dropdown's rows: "Default" with the account it means now,
/// then each account.
fn account_menu(
    logins: &[String],
    active: Option<String>,
    picked: Option<&str>,
) -> OpenMenu<usize> {
    let current = picked
        .and_then(|login| logins.iter().position(|known| known == login))
        .map_or(0, |row| row + 1);
    let checked = |item: MenuItem<usize>, row: usize| {
        MenuEntry::Item(if current == row { item.checked() } else { item })
    };
    let mut entries = Vec::with_capacity(logins.len() + 2);
    let default = MenuItem::new(0, "Default");
    let default = match active {
        Some(active) => default.detail(format!("@{active}"), None),
        None => default,
    };
    entries.push(checked(default, 0));
    entries.push(MenuEntry::Separator);
    for (row, login) in logins.iter().enumerate() {
        entries.push(checked(
            MenuItem::new(row + 1, format!("@{login}")),
            row + 1,
        ));
    }
    OpenMenu::new(
        "project-github-account".into(),
        None,
        super::form::control_width(),
        entries,
    )
    .selected(current)
    .offset(px(0.0), px(6.0))
}
