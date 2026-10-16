//! Permission rules in the window: the offer to remember an answer, and the
//! Permissions pane. The rules themselves, and the hook that enforces them,
//! are `ket_core::permissions`.
//!
//! When a person answers an agent's permission prompt, the strip above that
//! terminal offers to make the answer a rule for the project — "Always allow
//! `cargo test *` in ket" after an allow, "Never allow" after a refusal. It
//! is offered only when the rule would take effect: a stricter rule already
//! covering the call (an ask rule that raised the prompt, say) means an
//! allow in the project could never win, so nothing is offered. It goes away
//! after a minute, when answered, or when dismissed.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use ket_core::agent_hooks::PermissionQuestion;
use ket_core::permissions::{self, RepoRules, Request, Rule, RuleFile, RuleSet, Verdict};
use ket_core::theme::Theme;

use crate::Shell;
use crate::paint::paint;
use crate::preferences::{Entry, setting};
use crate::terminal::TerminalId;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{caption, tag};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::toast::Tone;
use crate::ui::toggle::switch;

/// How long an offer stays up.
const OFFER_FOR_MS: u64 = 60_000;

/// An answer, ready to be remembered as a rule.
#[derive(Debug, Clone)]
pub(crate) struct RuleOffer {
    /// The project the rule would be kept under.
    project: String,
    project_name: SharedString,
    /// The rule, already saying allow or deny.
    rule: Rule,
    /// When the answer was seen.
    at_ms: u64,
}

/// The window's offers, by terminal, and the last answer each terminal was
/// looked at for, so one answer is offered once.
#[derive(Debug, Default)]
pub(crate) struct RuleOffers {
    offers: HashMap<TerminalId, RuleOffer>,
    seen: HashMap<TerminalId, u64>,
}

/// A question's subject in the shape the rules read a tool's input.
fn input_of(question: &PermissionQuestion) -> serde_json::Value {
    let key = match question.tool.as_str() {
        "Bash" => "command",
        "WebFetch" => "url",
        "NotebookEdit" | "NotebookRead" => "notebook_path",
        _ => "file_path",
    };
    match question.subject.as_deref() {
        Some(subject) => serde_json::json!({ key: subject }),
        None => serde_json::json!({}),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl Shell {
    /// Looks for answers this window's terminals have seen since the last
    /// look, and drops offers that have run out. Cheap when nothing was
    /// answered: the rules are only read for a new answer.
    pub(crate) fn refresh_rule_offers(&mut self) {
        let now = ket_core::now_ms();
        self.rule_offers
            .offers
            .retain(|_, offer| now.saturating_sub(offer.at_ms) < OFFER_FOR_MS);

        let terminals: Vec<(TerminalId, String)> = self
            .terminal_keys
            .iter()
            .map(|(id, key)| (*id, key.clone()))
            .collect();
        for (id, key) in terminals {
            let Some(status) = self.activity.pane_status(&key) else {
                continue;
            };
            let Some(answered) = status.answered.clone() else {
                continue;
            };
            if self
                .rule_offers
                .seen
                .get(&id)
                .is_some_and(|seen| *seen >= answered.at_ms)
            {
                continue;
            }
            self.rule_offers.seen.insert(id, answered.at_ms);
            if status.agent != "claude" || now.saturating_sub(answered.at_ms) >= OFFER_FOR_MS {
                continue;
            }
            let worktree = status.worktree.clone();
            let Some((project, node)) = self.projects.iter().find_map(|project| {
                project
                    .worktrees
                    .iter()
                    .find(|node| node.id.as_str() == worktree)
                    .map(|node| (project, node))
            }) else {
                continue;
            };
            let decision = if answered.allowed {
                Verdict::Allow
            } else {
                Verdict::Deny
            };
            let Some(rule) = permissions::suggest(&answered.question, Some(&node.path), decision)
            else {
                continue;
            };
            // Offered only where it would take effect: anything already
            // ruling the call as strictly, or more, makes the rule moot.
            let root = project
                .worktrees
                .iter()
                .find(|node| node.primary)
                .map(|node| node.path.clone());
            let home = home();
            let input = input_of(&answered.question);
            let ruled = RuleSet::load(Some(project.id.as_str()), root.as_deref(), home.as_deref())
                .ok()
                .and_then(|rules| {
                    rules.evaluate(&Request {
                        tool: &answered.question.tool,
                        input: &input,
                        worktree: Some(&node.path),
                        home: home.as_deref(),
                    })
                });
            if ruled.is_some_and(|ruling| ruling.verdict >= decision) {
                continue;
            }
            self.rule_offers.offers.insert(
                id,
                RuleOffer {
                    project: project.id.to_string(),
                    project_name: project.name.clone(),
                    rule,
                    at_ms: answered.at_ms,
                },
            );
        }
    }

    /// Keeps an offer as a rule, and says so.
    fn accept_rule_offer(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        let Some(offer) = self.rule_offers.offers.remove(&id) else {
            return;
        };
        let mut rule = offer.rule;
        rule.added_by = Some("mac".to_owned());
        let subject = rule.subject();
        let decision = rule.decision;
        match permissions::remember(&offer.project, rule) {
            Ok(()) => {
                let said = match decision {
                    Verdict::Allow => format!("{subject} is allowed in {}.", offer.project_name),
                    _ => format!("{subject} is refused in {}.", offer.project_name),
                };
                self.toast(Tone::Success, said, cx);
            }
            Err(e) => self.toast(Tone::Error, format!("Could not save the rule: {e}"), cx),
        }
    }

    /// The strip above a terminal offering to remember its last answer, or
    /// `None` when there is nothing to offer.
    pub(crate) fn rule_offer_strip(
        &self,
        id: TerminalId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let offer = self.rule_offers.offers.get(&id)?;
        let t = &self.theme;
        let key = id.0;
        let subject = offer.rule.subject();
        let (said, action) = match offer.rule.decision {
            Verdict::Allow => (
                "Allowed once.",
                format!("Always allow {subject} in {}", offer.project_name),
            ),
            _ => (
                "Refused once.",
                format!("Never allow {subject} in {}", offer.project_name),
            ),
        };
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(6.0))
                .bg(paint(t.panel))
                .border_b_1()
                .border_color(paint(t.border))
                .child(sized_icon(Icon::ShieldCheck, px(13.0), paint(t.text.dim)))
                .child(caption(said, t).flex_none())
                .child(div().flex_1())
                .child(
                    button(("rule-offer-accept", key), action)
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.accept_rule_offer(id, cx);
                            cx.notify();
                        })),
                )
                .child(
                    icon_button(("rule-offer-dismiss", key), Icon::Close)
                        .bare()
                        .small()
                        .render(t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.rule_offers.offers.remove(&id);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// The Permissions pane: ket's guardrails, the person's rules by project
    /// and globally, and each project's repository rules.
    pub(crate) fn permission_settings(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<Entry> {
        let mut out = Vec::new();
        let file = match RuleFile::load() {
            Ok(file) => file,
            Err(e) => {
                out.push(setting(
                    "permissions.toml",
                    &format!("Could not be read, so no rule of yours applies: {e}"),
                    self.edit_permissions_button(t, cx),
                    t,
                ));
                return out;
            }
        };

        let builtins = file.builtins;
        out.push(setting(
            "Built-in guardrails",
            "Ask before an agent edits the files that decide what agents may do: \
             permissions.toml, .ket.toml, and ket's and the agents' hook and settings files.",
            switch("permissions-builtins", builtins, t)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Err(e) = permissions::set_builtins(!builtins) {
                        this.toast(Tone::Error, format!("Could not save: {e}"), cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
            t,
        ));
        out.push(setting(
            "Rules file",
            "Every rule lives in permissions.toml. The first matching rule in a list wins; \
             across lists the strictest wins. Enforced for Claude Code.",
            self.edit_permissions_button(t, cx),
            t,
        ));

        for project in &self.projects {
            let id = project.id.to_string();
            let personal = file
                .projects
                .get(&id)
                .map(|p| p.rules.as_slice())
                .unwrap_or_default();
            for (index, rule) in personal.iter().enumerate() {
                let project_id = id.clone();
                out.push(self.rule_row(
                    rule,
                    &format!("{} \u{b7} this project", project.name),
                    Some(Box::new(move |_| {
                        permissions::forget(Some(&project_id), index)
                    })),
                    format!("rule-project-{id}-{index}").into(),
                    t,
                    cx,
                ));
            }
            let root = project.worktrees.iter().find(|node| node.primary);
            let repo = root
                .map(|node| RepoRules::load(&node.path))
                .unwrap_or_default();
            if let Some(error) = repo.error.as_deref() {
                out.push(setting(
                    &format!("{} \u{b7} .ket.toml", project.name),
                    &format!("Its [permissions] table could not be read, so none of its rules apply: {error}"),
                    div().into_any_element(),
                    t,
                ));
            }
            for (index, rule) in repo.rules.iter().enumerate() {
                out.push(self.rule_row(
                    rule,
                    &format!("{} \u{b7} repository", project.name),
                    None,
                    format!("rule-repo-{id}-{index}").into(),
                    t,
                    cx,
                ));
            }
            for rule in &repo.ignored {
                out.push(setting(
                    &rule.subject(),
                    &format!(
                        "{} \u{b7} repository \u{b7} ignored: a repository may deny or ask, never allow",
                        project.name
                    ),
                    tag("ignored", t).into_any_element(),
                    t,
                ));
            }
        }

        for (index, rule) in file.rules.iter().enumerate() {
            out.push(self.rule_row(
                rule,
                "global",
                Some(Box::new(move |_| permissions::forget(None, index))),
                format!("rule-global-{index}").into(),
                t,
                cx,
            ));
        }
        out
    }

    /// One rule: what it covers, what it says and where it is kept, and a
    /// way to take it out when it is the person's own.
    #[allow(clippy::type_complexity)]
    fn rule_row(
        &self,
        rule: &Rule,
        scope: &str,
        forget: Option<Box<dyn Fn(&mut Self) -> ket_core::Result<()>>>,
        id: SharedString,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Entry {
        let mut words = format!(
            "{} \u{b7} {} \u{b7} {scope}",
            rule.decision.as_str(),
            rule.tool.0.join(", ")
        );
        if let Some(reason) = rule.reason.as_deref() {
            words.push_str(&format!(" \u{b7} {reason}"));
        }
        let control = match forget {
            Some(forget) => button(id, "Remove")
                .ghost()
                .small()
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Err(e) = forget(this) {
                        this.toast(Tone::Error, format!("Could not remove the rule: {e}"), cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
            None => tag(rule.decision.as_str(), t).into_any_element(),
        };
        setting(&rule.subject(), &words, control, t)
    }

    fn edit_permissions_button(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        button("permissions-edit", "Edit permissions.toml")
            .small()
            .render(t)
            .on_click(cx.listener(|this, _, window, cx| {
                let Ok(path) = ket_core::paths::permissions_file() else {
                    return;
                };
                if !path.exists() {
                    let _ = RuleFile::default().save_to(&path);
                }
                this.close_preferences(window, cx);
                this.open_editor_tab(path, cx);
                cx.notify();
            }))
            .into_any_element()
    }
}
