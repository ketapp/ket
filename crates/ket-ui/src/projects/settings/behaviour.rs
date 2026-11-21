//! Both project dialogs' right column: how the project behaves.
//!
//! What happens in its worktrees, which agent runs in them, and where its
//! backlog lives, a section each with a rule between them.

use gpui::{Context, Div, Window, div, prelude::*, px};

use super::{CONTROL_W, ITEM_GAP, Tip};
use crate::Shell;
use crate::paint::paint;
use crate::projects::form::{Field, ProjectForm};
use crate::ui::group::{button_group, segment};
use crate::ui::icon::Icon;
use crate::ui::rule::rule;
use crate::ui::section::{section, setting_row};
use crate::ui::toggle::switch;

impl Shell {
    /// The right column: what happens in the project's worktrees, which agent
    /// runs in them, and where its backlog lives — a section each, ruled off.
    pub(super) fn behaviour(
        &self,
        form: &ProjectForm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = &self.theme;
        let shown = form.tip;
        let mono = self.font_family.clone();

        let base = self
            .form_field(form, Field::Base, false, Some(Icon::GitBranch), window, cx)
            .w(CONTROL_W)
            .flex_none()
            .font_family(mono.clone());
        let hide = switch("hide-discovered", form.hide_discovered, t).on_click(cx.listener(
            |this, _, _, cx| {
                if let Some(form) = this.project_form_mut() {
                    form.hide_discovered = !form.hide_discovered;
                }
                cx.notify();
            },
        ));
        let trust = switch("trust-automation", form.trust_automation, t).on_click(cx.listener(
            |this, _, _, cx| {
                if let Some(form) = this.project_form_mut() {
                    form.trust_automation = !form.trust_automation;
                }
                cx.notify();
            },
        ));
        let hooks = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child("Run hooks and post-command from")
            .child(
                div()
                    .font_family(mono)
                    .text_size(px(12.0))
                    .child(".ket.toml"),
            );
        let worktrees = section(
            "Worktrees",
            "Each task gets its own checkout of this repository.",
            t,
        )
        .child(setting_row(self.item_label("Base branch", Tip::Base, shown, cx)).child(base))
        .child(
            setting_row(self.item_label(
                "Hide worktrees ket did not create",
                Tip::HideDiscovered,
                shown,
                cx,
            ))
            .child(hide),
        )
        .child(setting_row(self.item_label(hooks, Tip::Hooks, shown, cx)).child(trust));

        let mut agent = section(
            "Agent",
            "The coding agent ket starts in each new worktree.",
            t,
        )
        .child(
            setting_row(self.item_label("Agent", Tip::Agent, shown, cx))
                .child(self.form_agent(form, cx).w(CONTROL_W).flex_none()),
        );
        if let Some(at) = form.command_row() {
            let field = self
                .command_field(form, at, window, cx)
                .w(CONTROL_W)
                .flex_none();
            agent = agent.child(
                setting_row(self.item_label("Command", Tip::Command, shown, cx)).child(field),
            );
            // Under the field they are about, at its width.
            let notes = self.command_notes(form, at, cx);
            if !notes.is_empty() {
                agent = agent.child(
                    div().flex().justify_end().child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(CONTROL_W)
                            .gap(px(7.0))
                            .children(notes),
                    ),
                );
            }
        }

        let backlog = self.backlog_section(form, cx);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(ITEM_GAP)
            .child(worktrees)
            .child(rule(t))
            .child(agent)
            .when_some(backlog, |el, backlog| el.child(rule(t)).child(backlog))
    }

    /// Where the backlog is kept, whether it follows GitHub issues, and the
    /// account those issues go through. `None` for a form that asks nothing
    /// about the backlog.
    fn backlog_section(&self, form: &ProjectForm, cx: &mut Context<Self>) -> Option<Div> {
        let t = &self.theme;
        let shown = form.tip;
        let in_repo = form.backlog_in_repo?;

        let store = button_group("project-backlog")
            .child(
                segment("project-backlog-private", "Private")
                    .leading(Icon::Lock, paint(t.text.dim))
                    .selected(!in_repo)
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        if let Some(form) = this.project_form_mut() {
                            form.backlog_in_repo = Some(false);
                        }
                        cx.notify();
                    })),
            )
            .child(
                segment("project-backlog-repo", "In the repository")
                    .leading(Icon::BookMarked, paint(t.status.running))
                    .selected(in_repo)
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                        if let Some(form) = this.project_form_mut() {
                            form.backlog_in_repo = Some(true);
                        }
                        cx.notify();
                    })),
            )
            .render(t);
        let mut backlog = section(
            "Backlog",
            "Notes on work to do later, each ready to start as a worktree.",
            t,
        )
        .child(
            setting_row(self.item_label("Store notes", Tip::StoreNotes, shown, cx)).child(store),
        );

        if let Some(on) = form.github_issues {
            let sync =
                switch("project-github-issues", on, t).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(form) = this.project_form_mut() {
                        form.github_issues = form.github_issues.map(|on| !on);
                    }
                    cx.notify();
                }));
            backlog = backlog.child(
                setting_row(self.item_label("Sync with GitHub issues", Tip::Sync, shown, cx))
                    .child(sync),
            );
        }

        // The account's label carries where its issues go, under it, so the
        // row reads as one fact: this account, that repository.
        let account = div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(self.item_label("GitHub account", Tip::Account, shown, cx))
            .children(self.github_status());
        let control = self
            .github_control(cx)
            .map(|control| div().flex_none().w(CONTROL_W).child(control));
        Some(
            backlog
                .child(setting_row(account).children(control))
                .children(self.github_refusal()),
        )
    }
}
