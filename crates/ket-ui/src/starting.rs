//! A worktree on its way: in the sidebar the moment Start is pressed, and
//! filled in as it is made, rather than after the fetch, the checkout and
//! the agent.
//!
//! Before the worktree exists its row is drawn here, under its project; the
//! pane keeps whatever it was showing. Once it exists the real row and tabs
//! take over, and while its agent starts and is given the prompt the row's
//! words are still this module's. A start that fails leaves its row, with
//! what went wrong, and a click on it brings the dialog back as it was.

use std::sync::{Arc, Mutex, PoisonError};

use gpui::{AnyElement, Context, Div, div, prelude::*, px};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::theme::Theme;
use ket_core::workspace::CreateStep;

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::ui::agent::{MARK, agent_label, provider};
use crate::ui::chip::{BADGE, Motion, status_dot};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::row::block_row_with_rail;
use crate::ui::{CAPTION, ICON_GAP};

/// What the New worktree dialog was given, kept so Edit can put it back.
#[derive(Clone)]
pub(crate) struct Draft {
    pub(crate) project: ProjectId,
    pub(crate) prompt: String,
    pub(crate) branch: String,
    /// Whether the branch was typed rather than taken from the prompt.
    pub(crate) branch_edited: bool,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) economy: u8,
}

/// Where ket-core's create has got to, written from the thread it runs on.
pub(crate) type Step = Arc<Mutex<Option<CreateStep>>>;

/// The three parts of making one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Fetch,
    Checkout,
    Agent,
}

/// One worktree being made.
pub(crate) struct Start {
    key: u64,
    draft: Draft,
    step: Step,
    /// The worktree, once it exists.
    worktree: Option<WorktreeId>,
    /// Why it could not be made, and at which part.
    failed: Option<(Stage, String)>,
}

/// Every worktree being made.
#[derive(Default)]
pub(crate) struct Starts {
    list: Vec<Start>,
    next: u64,
}

impl Start {
    fn stage(&self) -> Stage {
        if let Some((stage, _)) = &self.failed {
            return *stage;
        }
        if self.worktree.is_some() {
            return Stage::Agent;
        }
        match *self.step.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(CreateStep::CheckingOut) => Stage::Checkout,
            _ => Stage::Fetch,
        }
    }

    /// The row's detail: the part under way.
    fn words(&self) -> String {
        if let Some((_, why)) = &self.failed {
            return why.clone();
        }
        match self.stage() {
            Stage::Fetch => match &*self.step.lock().unwrap_or_else(PoisonError::into_inner) {
                Some(CreateStep::Fetching { remote }) => format!("Fetching {remote}"),
                _ => "Fetching".to_owned(),
            },
            Stage::Checkout => "Checking out".to_owned(),
            Stage::Agent => match &self.draft.agent {
                Some(agent) => format!("Starting {}", agent_label(agent)),
                None => "Opening".to_owned(),
            },
        }
    }

    /// The agent's mark, most of the way out until it is running.
    fn mark(&self, size: gpui::Pixels, t: &Theme) -> AnyElement {
        match &self.draft.agent {
            Some(agent) => {
                let (which, tint) = provider(agent, t);
                sized_icon(which, size, alpha(tint, 0.35)).into_any_element()
            }
            None => {
                sized_icon(Icon::GitBranch, size, alpha(paint(t.text.dim), 0.34)).into_any_element()
            }
        }
    }

    /// "starting", breathing in the running green, or "failed".
    fn state(&self, t: &Theme) -> Div {
        let (word, colour, motion) = match self.failed {
            Some(_) => ("failed", t.status.failed, Motion::Still),
            None => ("starting", t.status.running, Motion::Travel),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .text_size(px(11.5))
            .text_color(paint(colour))
            .child(status_dot(paint(colour), motion, px(6.0)))
            .child(word)
    }
}

impl Shell {
    /// Shows a worktree being made, in the sidebar, and returns where its
    /// creation should report its progress.
    pub(crate) fn begin_start(&mut self, draft: Draft) -> Step {
        // Started again after it failed: the new start takes the old one's
        // place rather than standing under it.
        self.starting.list.retain(|start| {
            start.failed.is_none()
                || start.draft.project != draft.project
                || start.draft.branch != draft.branch
        });
        let step = Step::default();
        let key = self.starting.next;
        self.starting.next += 1;
        self.starting.list.push(Start {
            key,
            draft,
            step: step.clone(),
            worktree: None,
            failed: None,
        });
        step
    }

    /// The live start for `branch` in `project`: not yet a worktree, and not
    /// one that failed, which stays up until it is clicked and must not take
    /// a later start's progress.
    fn unborn(&self, project: &ProjectId, branch: &str) -> Option<usize> {
        self.starting.list.iter().position(|start| {
            start.worktree.is_none()
                && start.failed.is_none()
                && &start.draft.project == project
                && start.draft.branch == branch
        })
    }

    /// Where the start for `branch` in `project` reports, if one is showing.
    pub(crate) fn start_step(&self, project: &ProjectId, branch: &str) -> Option<Step> {
        self.unborn(project, branch)
            .map(|at| self.starting.list[at].step.clone())
    }

    /// The worktree exists: its real row takes over, still saying it is
    /// starting until its agent has the prompt.
    pub(crate) fn start_made(&mut self, project: &ProjectId, branch: &str, id: WorktreeId) {
        if let Some(at) = self.unborn(project, branch) {
            self.starting.list[at].worktree = Some(id);
        }
    }

    /// The worktree is made and needs nothing more: no prompt to give.
    pub(crate) fn start_settled(&mut self, project: &ProjectId, branch: &str) {
        if let Some(at) = self.unborn(project, branch) {
            self.forget_start(self.starting.list[at].key);
        }
    }

    /// It could not be made. Returns whether a start was showing to say so.
    pub(crate) fn start_failed(&mut self, project: &ProjectId, branch: &str, why: String) -> bool {
        let Some(at) = self.unborn(project, branch) else {
            return false;
        };
        let start = &mut self.starting.list[at];
        let stage = start.stage();
        start.failed = Some((stage, why));
        true
    }

    /// The worktree's agent has its prompt, or will not get one: the real
    /// row shows through.
    pub(crate) fn start_done(&mut self, id: &WorktreeId) {
        self.starting
            .list
            .retain(|start| start.worktree.as_ref() != Some(id));
    }

    fn forget_start(&mut self, key: u64) {
        self.starting.list.retain(|start| start.key != key);
    }

    /// The start a made worktree's row speaks for while its agent starts.
    pub(crate) fn start_for(&self, id: &WorktreeId) -> Option<&Start> {
        self.starting
            .list
            .iter()
            .find(|start| start.worktree.as_ref() == Some(id))
    }

    /// The rows of `project`'s worktrees that do not exist yet.
    pub(crate) fn starting_rows(&self, project: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = &self.theme;
        let Some(node) = self.projects.get(project) else {
            return Vec::new();
        };
        let rail = crate::projects::rail_colour(node.color, t);
        self.starting
            .list
            .iter()
            .filter(|start| start.worktree.is_none() && start.draft.project == node.id)
            .map(|start| {
                let key = start.key;
                let ink = paint(match start.failed {
                    Some(_) => t.status.failed,
                    None => t.text.note.unwrap_or(t.text.dim),
                });
                block_row_with_rail(("starting-row", key as usize), false, rail, t)
                    .flex_row()
                    .items_center()
                    .gap(ICON_GAP)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(BADGE)
                            .child(start.mark(MARK, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(11.5))
                                    .truncate()
                                    .child(start.draft.branch.clone()),
                            )
                            .child(
                                div()
                                    .prose()
                                    .mt(px(1.0))
                                    .text_size(CAPTION)
                                    .text_color(ink)
                                    .truncate()
                                    .child(start.words()),
                            ),
                    )
                    .child(start.state(t))
                    // A failed one: the dialog back, holding what it was given.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_start(key, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// The row's detail, ink and state for a made worktree still starting.
    pub(crate) fn start_row_parts(&self, id: &WorktreeId) -> Option<(String, Div, AnyElement)> {
        let start = self.start_for(id)?;
        let t = &self.theme;
        Some((start.words(), start.state(t), start.mark(MARK, t)))
    }

    /// A click on a failed start's row: the row goes and the dialog comes
    /// back, holding what it was given. One still being made is left alone.
    fn edit_start(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(draft) = self
            .starting
            .list
            .iter()
            .find(|start| start.key == key && start.failed.is_some())
            .map(|start| start.draft.clone())
        else {
            return;
        };
        self.forget_start(key);
        self.reopen_new_worktree(&draft, cx);
    }
}
