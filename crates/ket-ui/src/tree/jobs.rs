//! The rows under a worktree for what its agents left running in the
//! background — a build, a test run, a dev server — and, for a few minutes,
//! what of that has ended. See [`ket_core::jobs`] for how the host finds them.
//!
//! They hang from the subagent tree, after the subagents, on the same elbows
//! (`super::branch`): a running job's node is an open ring in the running
//! green, a server's a globe with its port beside its name, and an ended
//! job's a check, a cross for one that failed, or a square for one stopped.
//! The name is what the agent said the job was for, else its command.
//!
//! Each row is a control of its own — the shared row's surface, its hover and
//! its pointer — and only the row answers the pointer, never the worktree
//! above it, whose hover is kept for a card of its own. Under the pointer a
//! running job's time gives way to its Stop, and a server's to Open as well,
//! and the command it runs comes up beside it.

use gpui::{
    AnyElement, Context, ElementId, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use ket_core::id::WorktreeId;
use ket_core::jobs::{Job, Outcome};
use ket_core::theme::Theme;

use super::branch::branch_of;
use super::{BADGE, ELBOW_RUN, SUBAGENT_ROW_H, Selection, mark_axis};
use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::ui::ICON_GAP;
use crate::ui::button::icon_button;
use crate::ui::chip::signal_tag;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::row::sized_row;
use crate::ui::tooltip::{Side, tooltip};

/// A job's node: an icon where a subagent has its dot, a step larger so a
/// ring and a globe still read as themselves.
const NODE_ICON: Pixels = px(9.0);

impl Shell {
    /// The rows for `jobs`, numbered on from `first` in a tree whose last row
    /// is `last` — see `branch_of`. `end` is where their figures end, as the
    /// subagents' do.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn job_rows(
        &self,
        pi: usize,
        wi: usize,
        worktree: &WorktreeId,
        jobs: &[(String, Job)],
        first: usize,
        last: usize,
        end: Pixels,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = &self.theme;
        let label_x = mark_axis() + BADGE / 2.0 + ICON_GAP;
        let node_x = mark_axis() + ELBOW_RUN;
        let quiet = alpha(paint(t.text.dim), 0.7);

        jobs.iter()
            .enumerate()
            .map(|(n, (pane, job))| {
                let key = SharedString::from(format!(
                    "job-{worktree}-{pane}-{}-{}",
                    job.pid, job.started_at_ms
                ));
                let hovered = self.hovered_job.as_ref().is_some_and(|(at, _)| at == &key);
                let port = job.ports.first().copied().filter(|_| job.ended.is_none());
                let (node, ink) = match (job.ended, port) {
                    (None, Some(_)) => (Icon::Globe, paint(t.status.attention)),
                    (None, None) => (Icon::LoaderCircle, paint(t.status.running)),
                    (Some(ended), _) => match ended.outcome {
                        Outcome::Failed => (Icon::Close, paint(t.status.failed)),
                        Outcome::Stopped => (Icon::Stop, quiet),
                        Outcome::Completed | Outcome::Gone => (Icon::Check, quiet),
                    },
                };
                let words = if job.ended.is_some() {
                    quiet
                } else {
                    paint(t.text.dim)
                };
                let command = job.command.lines().next().unwrap_or_default().to_owned();
                let name = job.description.clone().unwrap_or_else(|| command.clone());

                let label = div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(5.0))
                    .children(port.map(|port| {
                        div()
                            .flex_none()
                            .text_color(paint(t.status.attention))
                            .child(format!(":{port}"))
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .prose()
                            .truncate()
                            .text_color(words)
                            .child(SharedString::from(name)),
                    );
                let label = tooltip(
                    ElementId::from(SharedString::from(format!("{key}-tip"))),
                    label.into_any_element(),
                    command,
                    Side::Right,
                    // The command, while the pointer is on the row and not on
                    // one of its buttons, which have tooltips of their own.
                    self.hovered_job.as_ref() == Some(&(key.clone(), None)),
                    t,
                )
                .flex()
                .flex_1()
                .min_w_0();

                let trailing = match (job.ended, hovered) {
                    (None, true) => self.job_actions(pi, wi, worktree, pane, job, &key, cx),
                    (None, false) => {
                        let age = now.saturating_sub(job.started_at_ms);
                        figure(
                            if age < 60_000 {
                                "<1m".to_owned()
                            } else {
                                crate::status_bar::compact_duration(age)
                            },
                            paint(t.text.dim),
                        )
                    }
                    (Some(ended), _) => figure(
                        match (ended.exit, ended.outcome) {
                            (Some(code), _) => format!("exit {code}"),
                            (None, Outcome::Completed) => "done".to_owned(),
                            (None, Outcome::Failed) => "failed".to_owned(),
                            (None, Outcome::Stopped) => "stopped".to_owned(),
                            (None, Outcome::Gone) => "ended".to_owned(),
                        },
                        quiet,
                    ),
                };

                let pane_key = pane.clone();
                let hover_key = key.clone();
                // The shared row's surface, its gap taken off: the node and
                // the label are placed against the worktree's own columns.
                let row = sized_row(ElementId::from(key), false, SUBAGENT_ROW_H, t).gap(px(0.0));
                branch_of(row, first + n, last, end, t)
                    .child(
                        div()
                            .flex_none()
                            .size(NODE_ICON)
                            .mr(label_x - node_x - NODE_ICON)
                            .child(sized_icon(node, NODE_ICON, ink)),
                    )
                    .child(label)
                    .child(div().flex().flex_none().ml(px(10.0)).child(trailing))
                    .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
                        let here = this
                            .hovered_job
                            .as_ref()
                            .is_some_and(|(at, _)| at == &hover_key);
                        if *is_hovered && !here {
                            this.hovered_job = Some((hover_key.clone(), None));
                        } else if !*is_hovered && here {
                            this.hovered_job = None;
                        }
                        cx.notify();
                    }))
                    // As a subagent's row does: to the worktree, and the pane
                    // whose agent started it.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(
                            Selection {
                                project: pi,
                                worktree: wi,
                            },
                            cx,
                        );
                        this.focus_agent_pane(Some(pane_key.as_str()));
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// A running job's Stop, and a server's Open before it: the shared dense
    /// icon button, each named by its tooltip.
    #[allow(clippy::too_many_arguments)]
    fn job_actions(
        &self,
        pi: usize,
        wi: usize,
        worktree: &WorktreeId,
        pane: &str,
        job: &Job,
        key: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let open = job.ports.first().copied().map(|port| {
            let worktree = worktree.clone();
            self.job_action(
                key,
                "open",
                Icon::ExternalLink,
                format!("Open localhost:{port}"),
                t,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_served(pi, wi, &worktree, port, cx);
                cx.stop_propagation();
            }))
        });
        let (pane, pid, started) = (pane.to_owned(), job.pid, job.started_at_ms);
        let stop = self
            .job_action(key, "stop", Icon::Stop, "Stop".to_owned(), t, cx)
            .on_click(move |_, _, cx| {
                ket_core::host::stop_job(&pane, pid, started);
                cx.stop_propagation();
            });
        div()
            .flex()
            .items_center()
            .children(open)
            .child(stop)
            .into_any_element()
    }

    /// One of a job row's buttons, with its tooltip.
    fn job_action(
        &self,
        key: &SharedString,
        which: &'static str,
        icon: Icon,
        label: String,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = SharedString::from(format!("{key}-{which}"));
        let shown = self.hovered_job.as_ref() == Some(&(key.clone(), Some(which)));
        let button = icon_button(ElementId::from(id.clone()), icon)
            .bare()
            .dense()
            .render(t)
            .into_any_element();
        let key = key.clone();
        tooltip(
            ElementId::from(SharedString::from(format!("{id}-tip"))),
            button,
            label,
            Side::Top,
            shown,
            t,
        )
        .flex_none()
        .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
            if *is_hovered {
                this.hovered_job = Some((key.clone(), Some(which)));
            } else if this.hovered_job.as_ref() == Some(&(key.clone(), Some(which))) {
                // Back to the row it is on, not off it: the row's own hover
                // says when the pointer has left, and may have said so first.
                this.hovered_job = Some((key.clone(), None));
            }
            cx.notify();
        }))
    }

    /// Opens a dev server's page in the worktree's own browser, in a new tab
    /// of the pane in front there.
    fn open_served(
        &mut self,
        pi: usize,
        wi: usize,
        worktree: &WorktreeId,
        port: u16,
        cx: &mut Context<Self>,
    ) {
        self.select(
            Selection {
                project: pi,
                worktree: wi,
            },
            cx,
        );
        let pane = self.spaces.entry(worktree.clone()).or_default().focused;
        let url = format!("http://localhost:{port}");
        if self
            .open_browser_tab_at(worktree, pane, Some(url), cx)
            .is_some()
        {
            self.persist_layout();
        }
        cx.notify();
    }

    /// The count a terminal tab wears while its agent has jobs running in the
    /// background — the shared signal tag, an open ring and the number in the
    /// running green — with what they are on hover. Nothing for a tab with
    /// nothing running: an ended job is the sidebar's to show.
    pub(crate) fn tab_jobs_mark(
        &self,
        terminal: crate::terminal::TerminalId,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The same two keys a tab's status is looked for under.
        let numeric = terminal.0.to_string();
        let status = [
            self.terminal_keys.get(&terminal).map(String::as_str),
            Some(numeric.as_str()),
        ]
        .into_iter()
        .flatten()
        .find_map(|key| self.activity.pane_status(key))?;
        let running: Vec<&Job> = status
            .jobs
            .iter()
            .filter(|job| job.ended.is_none())
            .collect();
        if running.is_empty() {
            return None;
        }
        let count = signal_tag(
            Icon::LoaderCircle,
            running.len().to_string(),
            paint(t.status.running),
        )
        .into_any_element();
        let what = running
            .iter()
            .map(|job| match job.ports.first() {
                Some(port) => format!("serving on :{port}"),
                None => job
                    .description
                    .clone()
                    .unwrap_or_else(|| job.command.lines().next().unwrap_or_default().to_owned()),
            })
            .collect::<Vec<_>>()
            .join(" \u{b7} ");
        let label = format!("{} in the background: {what}", running.len());
        let key = SharedString::from(format!("tab-jobs-{}", terminal.0));
        let shown = self.hovered_job.as_ref().is_some_and(|(at, _)| at == &key);
        Some(
            tooltip(
                ("tab-jobs", terminal.0),
                count,
                label,
                Side::Bottom,
                shown,
                t,
            )
            .flex_none()
            .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
                let here = this.hovered_job.as_ref().is_some_and(|(at, _)| at == &key);
                if *is_hovered {
                    this.hovered_job = Some((key.clone(), None));
                } else if here {
                    this.hovered_job = None;
                }
                cx.notify();
            }))
            .into_any_element(),
        )
    }
}

/// A row's figure at its far end: how long a job has run, or how it ended.
fn figure(words: String, ink: gpui::Rgba) -> AnyElement {
    div()
        .text_size(px(10.0))
        .text_color(ink)
        .child(words)
        .into_any_element()
}
