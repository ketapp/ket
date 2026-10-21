//! Worktrees' pull requests and their checks: read from GitHub about once a
//! minute, worn by each row's GitHub mark, passed on to phones through the
//! host, and handed to a worktree's agent by Fix checks with agent.
//!
//! One `gh pr list` per project reads every pull request open in it, with
//! its checks, and they are matched to the project's worktrees by branch —
//! so one an agent opened with `gh pr create` is found as well as one ket
//! opened. The `gh` calls are `ket_core::github`'s and run off the window's
//! thread; what the agent is told is `ket_core::checks`'s.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc;

use gpui::{Context, WeakEntity};
use ket_core::checks::checkout;
use ket_core::event::{AgentState, Event, SessionOutcome};
use ket_core::github::{self, PullRequestStatus};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::ui::toast::Tone;

/// How long a project's pull requests are trusted before they are read
/// again.
const READ_EVERY_MS: u64 = 60 * 1000;

/// How long a project whose read failed waits to be tried again. Most such
/// projects are not on GitHub at all, and fail the same way every time.
const RETRY_AFTER_MS: u64 = 10 * 60 * 1000;

/// What ket has read of the worktrees' pull requests.
#[derive(Default)]
pub(crate) struct PullRequests {
    /// When each project is next due to be read; one not in here is due now.
    due_at: BTreeMap<ProjectId, u64>,
    /// Projects being read now.
    reading: HashSet<ProjectId>,
    /// Each worktree's open pull request, as last read.
    open: HashMap<WorktreeId, PullRequestStatus>,
}

impl PullRequests {
    /// The pull request `worktree`'s branch is open as, as last read.
    pub(crate) fn of(&self, worktree: &WorktreeId) -> Option<&PullRequestStatus> {
        self.open.get(worktree)
    }
}

impl Shell {
    /// Reads the pull requests of `worktree`'s project on the next tick: ket
    /// just pushed its branch, and its checks have started.
    pub(crate) fn read_checks_soon(&mut self, worktree: &WorktreeId) {
        if let Some(project) = self.project_of_worktree(worktree) {
            self.pull_requests.due_at.remove(&project);
        }
    }

    /// The project `worktree` is in.
    pub(crate) fn project_of_worktree(&self, worktree: &WorktreeId) -> Option<ProjectId> {
        self.projects
            .iter()
            .find(|project| project.worktrees.iter().any(|w| &w.id == worktree))
            .map(|project| project.id.clone())
    }

    /// Reads the pull requests of every project that is due, on the tick. A
    /// project with no worktree of its own has none to read.
    pub(crate) fn refresh_pull_requests(&mut self, cx: &mut Context<Self>) {
        let now = ket_core::now_ms();
        let due: Vec<ProjectId> = self
            .projects
            .iter()
            .filter(|node| node.worktrees.iter().any(|w| !w.primary && !w.missing))
            .filter(|node| !self.pull_requests.reading.contains(&node.id))
            .filter(|node| {
                self.pull_requests
                    .due_at
                    .get(&node.id)
                    .is_none_or(|&at| now >= at)
            })
            .map(|node| node.id.clone())
            .collect();
        for project in due {
            self.read_pull_requests(project, cx);
        }
    }

    fn read_pull_requests(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.pull_requests.reading.insert(project.clone());
        let remembered: Vec<String> = self
            .projects
            .iter()
            .find(|node| node.id == project)
            .map(|node| {
                node.worktrees
                    .iter()
                    .filter(|worktree| !worktree.primary && !worktree.missing)
                    .filter(|worktree| {
                        self.pull_requests.open.contains_key(&worktree.id)
                            || worktree.pull_request.is_some()
                    })
                    .map(|worktree| worktree.branch.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let read = cx.background_executor().spawn({
            let project = project.clone();
            async move {
                let at = checkout(&project)?;
                let mut listed = github::open_pull_requests(&at)?;
                for branch in remembered {
                    if !listed.iter().any(|pr| pr.branch == branch)
                        && let Ok(Some(pr)) = github::pull_request_for_branch(&at, &branch)
                    {
                        listed.push(pr);
                    }
                }
                Ok(listed)
            }
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let read = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.take_pull_requests(&project, read, cx);
            });
        })
        .detach();
    }

    /// Applies a read of `project`'s open pull requests to its worktrees.
    /// A read that failed changes nothing it knew: GitHub being out of reach
    /// for a minute is not every check disappearing. One that worked is told
    /// to the host even when nothing here changed: a window that has just
    /// opened starts out knowing none, as does a host that has outlived the
    /// window before it, and finding none is news to such a host.
    fn take_pull_requests(
        &mut self,
        project: &ProjectId,
        read: ket_core::Result<Vec<PullRequestStatus>>,
        cx: &mut Context<Self>,
    ) {
        self.pull_requests.reading.remove(project);
        let now = ket_core::now_ms();
        let open = match read {
            Ok(open) => {
                self.pull_requests
                    .due_at
                    .insert(project.clone(), now + READ_EVERY_MS);
                open
            }
            Err(error) => {
                // Debug, not a warning: a project that is not on GitHub
                // fails like this every time it is tried.
                tracing::debug!(project = project.as_str(), %error, "could not read pull requests");
                self.pull_requests
                    .due_at
                    .insert(project.clone(), now + RETRY_AFTER_MS);
                return;
            }
        };
        let Some(node) = self.projects.iter().find(|node| &node.id == project) else {
            return;
        };
        let mut changed = false;
        for worktree in node.worktrees.iter().filter(|w| !w.primary) {
            let found = github::best_for_branch(&open, worktree.branch.as_ref()).cloned();
            changed |= match found {
                Some(pr) if self.pull_requests.open.get(&worktree.id) == Some(&pr) => false,
                Some(pr) => {
                    self.pull_requests.open.insert(worktree.id.clone(), pr);
                    true
                }
                None => self.pull_requests.open.remove(&worktree.id).is_some(),
            };
        }
        self.publish_pull_requests();
        if changed {
            cx.notify();
        }
    }

    /// Tells the host every worktree's pull request, for phones — sent only
    /// when they differ from what it was last told. Only the worktrees still
    /// in the sidebar: one deleted since it was read is not worth a row on a
    /// phone.
    fn publish_pull_requests(&self) {
        let open = self
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .filter_map(|worktree| {
                self.pull_requests
                    .of(&worktree.id)
                    .map(|pr| (worktree.id.to_string(), pr.clone()))
            })
            .collect();
        ket_core::host::publish_pull_requests(open);
    }

    /// Puts `status` on `id` at once, rather than waiting for the next poll,
    /// and tells the host.
    pub(crate) fn remember_pull_request(&mut self, id: WorktreeId, status: PullRequestStatus) {
        self.pull_requests.open.insert(id, status);
        self.publish_pull_requests();
    }

    /// Reads `id`'s pull request from GitHub now, after a merge or similar.
    pub(crate) fn refresh_pull_request(
        &mut self,
        id: WorktreeId,
        number: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project_of_worktree(&id) else {
            return;
        };
        let read = cx
            .background_executor()
            .spawn(async move { github::pull_request(&checkout(&project)?, number) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let read = read.await;
            let _ = shell.update(cx, |shell, cx| {
                if let Ok(status) = read {
                    shell.remember_pull_request(id, status);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Hands `id`'s unresolved review comments to its agent, from the
    /// worktree menu: reads them, and types a message naming each into the
    /// agent's terminal — opening an agent there first when none is running,
    /// as Fix checks does. The row is selected, so its agent is in view.
    pub(crate) fn address_comments(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let Some(project) = self.project_of_worktree(&id) else {
            self.toast(Tone::Error, "That worktree isn't here any more", cx);
            return;
        };
        let Some(status) = self
            .pull_requests
            .of(&id)
            .filter(|status| status.can_address_comments())
            .cloned()
        else {
            self.toast(
                Tone::Error,
                "This pull request has no review comments to address",
                cx,
            );
            return;
        };
        if let Some(selection) = self.position_of(&id) {
            self.select(selection, cx);
        }
        self.toast(
            Tone::Info,
            format!("Reading #{}'s review comments\u{2026}", status.number),
            cx,
        );
        let number = status.number;
        let handed = cx.background_executor().spawn({
            let id = id.clone();
            async move { ket_core::address_comments::hand_over(&checkout(&project)?, &id, &status) }
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let handed = handed.await;
            let _ = shell.update(cx, |shell, cx| match handed {
                Ok(prompt) => {
                    shell.send_prompt_to_worktree(id, prompt, cx);
                    shell.toast(
                        Tone::Info,
                        format!("Gave #{number}'s review comments to the agent"),
                        cx,
                    );
                }
                Err(error) => shell.toast(
                    Tone::Error,
                    format!("Couldn't read #{number}'s review comments: {error}"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Hands `id`'s failed checks to its agent, from the worktree menu: reads
    /// what the failed jobs printed, saves it, and types a message naming
    /// them into the agent's terminal — opening an agent there first when
    /// none is running, as the quick prompt does. The row is selected, so
    /// its agent is in view.
    pub(crate) fn fix_checks(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let Some(project) = self.project_of_worktree(&id) else {
            self.toast(Tone::Error, "That worktree isn't here any more", cx);
            return;
        };
        let Some(status) = self
            .pull_requests
            .of(&id)
            .filter(|status| status.failed().next().is_some())
            .cloned()
        else {
            self.toast(
                Tone::Error,
                "None of this pull request's checks have failed",
                cx,
            );
            return;
        };
        if let Some(selection) = self.position_of(&id) {
            self.select(selection, cx);
        }
        self.toast(
            Tone::Info,
            format!("Reading #{}'s failed checks\u{2026}", status.number),
            cx,
        );
        let number = status.number;
        let handed = cx.background_executor().spawn({
            let id = id.clone();
            async move {
                let logs = ket_core::checks::logs_dir()?;
                ket_core::checks::hand_over(&checkout(&project)?, &id, &status, &logs)
            }
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let handed = handed.await;
            let _ = shell.update(cx, |shell, cx| match handed {
                Ok(prompt) => {
                    shell.send_prompt_to_worktree(id, prompt, cx);
                    shell.toast(
                        Tone::Info,
                        format!("Gave #{number}'s failed checks to the agent"),
                        cx,
                    );
                }
                Err(error) => shell.toast(
                    Tone::Error,
                    format!("Couldn't read #{number}'s failed checks: {error}"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Types a phone's Fix checks with agent into `id`'s agent: the message
    /// the host made of its failed checks. The window is left where it is.
    pub(crate) fn fix_checks_from_phone(
        &mut self,
        req: u64,
        id: WorktreeId,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let said = self.pull_requests.of(&id).map_or_else(
            || "Gave the failed checks to the agent from your phone".to_owned(),
            |status| {
                format!(
                    "Gave #{}'s failed checks to the agent from your phone",
                    status.number
                )
            },
        );
        if let Some(terminal) = self.agent_terminal(&id) {
            self.submit_or_queue(terminal, &prompt, cx);
            ket_core::host::finish_phone_fix_checks(req, None);
            self.toast(Tone::Info, said, cx);
            return;
        }

        let started = cx.background_executor().spawn(async move {
            let (sender, receiver) = mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("ket-phone-checks-agent".into())
                .spawn(move || start_agent_for_phone(id, prompt, sender))
                .map_err(|error| format!("could not start the agent runner: {error}"))?;
            // However long setting up the worktree and starting the agent
            // take: giving up first would say it failed while it went on to
            // work. The host tells a phone that tires of waiting that the
            // agent is still starting.
            receiver
                .recv()
                .map_err(|_| "the agent runner stopped".to_owned())?
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let result = started.await;
            ket_core::host::finish_phone_fix_checks(req, result.as_ref().err().cloned());
            let _ = shell.update(cx, |shell, cx| match result {
                Ok(()) => shell.toast(Tone::Info, said, cx),
                Err(why) => shell.toast(Tone::Error, format!("Could not send prompt: {why}"), cx),
            });
        })
        .detach();
    }
}

/// Starts an agent session without holding the phone's answer until the
/// entire turn ends. Errors before its prompt is sent go back to the phone;
/// once the agent is thinking, the runner stays alive until the turn ends.
fn start_agent_for_phone(
    id: WorktreeId,
    prompt: String,
    sender: mpsc::SyncSender<std::result::Result<(), String>>,
) {
    let mut sender = Some(sender);
    let result = (|| -> std::result::Result<(), String> {
        let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        let workspace = Workspace::open().map_err(|e| e.to_string())?;
        let needs_provisioning = workspace
            .worktrees(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|worktree| worktree.id == id)
            .is_some_and(|worktree| !worktree.is_provisioned());
        if needs_provisioning {
            workspace
                .provision_worktree(&id)
                .map_err(|e| e.to_string())?;
        }
        let mut events = workspace.bus().subscribe();
        runtime.block_on(async {
            let run = workspace.run_agent(&id, None, &prompt);
            tokio::pin!(run);
            let mut session = None;
            loop {
                tokio::select! {
                    finished = &mut run => {
                        // A fast turn can finish before select reads the
                        // already-published Thinking event. Drain those
                        // events before deciding whether the prompt arrived.
                        while let Ok(report) = events.try_recv() {
                            match report.event {
                                Event::AgentSessionStarted { session_id, worktree_id, .. }
                                    if worktree_id == id => session = Some(session_id),
                                Event::AgentStateChanged { session_id, to: AgentState::Thinking, .. }
                                    if session.as_ref() == Some(&session_id) => return Ok(()),
                                _ => {}
                            }
                        }
                        return match finished.map_err(|e| e.to_string())? {
                            SessionOutcome::Failed { why } => Err(why),
                            SessionOutcome::Completed | SessionOutcome::Cancelled => {
                                Err("the agent ended before receiving the prompt".into())
                            }
                        };
                    }
                    report = events.recv() => {
                        if let Ok(report) = report {
                            match report.event {
                                Event::AgentSessionStarted { session_id, worktree_id, .. }
                                    if worktree_id == id => session = Some(session_id),
                                Event::AgentStateChanged { session_id, to: AgentState::Thinking, .. }
                                    if session.as_ref() == Some(&session_id) => {
                                        if let Some(sender) = sender.take() {
                                            let _ = sender.send(Ok(()));
                                        }
                                        let _ = run.await;
                                        return Ok(());
                                    }
                                _ => {}
                            }
                        }
                    }
                }
            }
        })
    })();
    if let Some(sender) = sender {
        let _ = sender.send(result);
    }
}
