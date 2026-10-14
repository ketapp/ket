//! The backlog's GitHub issues: making one from a note, and — for a project
//! that follows them — keeping the backlog in step with the repository's
//! open issues. The `gh` calls are `ket_core::github`'s; this is when they
//! run and what the dialog does with what they say.
//!
//! Following is both ways. Open issues arrive as notes, and a waiting note
//! whose issue closed is done. A note marked done here, or deleted while it
//! waits, closes its issue — deleted without that, the next read would bring
//! it back. Starting a note leaves its issue open: the pull request that
//! lands the work is what closes it.
//!
//! Every `gh` call runs off the window's thread, and a read of the issues
//! that overlapped one of ket's own changes is thrown away rather than
//! applied: it may predate an issue ket just made, and would add it twice.

use std::collections::BTreeMap;

use gpui::{Context, WeakEntity};
use ket_core::KetError;
use ket_core::backlog::{Backlog, Note};
use ket_core::github::{self, Checkout, CloseReason, OpenIssues};
use ket_core::id::ProjectId;
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::ui::toast::Tone;

/// How long a project's issues are trusted before they are read again.
const SYNC_EVERY_MS: u64 = 5 * 60 * 1000;

/// Where each project's issues were last read, and what ket is changing on
/// GitHub now.
#[derive(Default)]
pub(crate) struct IssueSync {
    /// When each project was last read, or asked whether it follows.
    read_at: BTreeMap<ProjectId, u64>,
    /// Changes to issues under way.
    busy: usize,
    /// Bumped as each change starts and ends, so a read can tell whether
    /// one overlapped it.
    changes: u64,
}

impl IssueSync {
    /// Reads `project` on the next tick — its settings just changed.
    pub(crate) fn forget(&mut self, project: &ProjectId) {
        self.read_at.remove(project);
    }
}

/// What a read of a project's issues found: nothing when it does not
/// follow them, else the open issues and when the read began.
type Read = ket_core::Result<Option<(OpenIssues, u64)>>;

impl Shell {
    /// Reads the issues of every project not read in the last few minutes.
    /// On the tick; a project that does not follow its issues costs a look
    /// at its settings, off the window's thread, as often.
    pub(crate) fn refresh_backlog_issues(&mut self, cx: &mut Context<Self>) {
        let now = ket_core::now_ms();
        let due: Vec<ProjectId> = self
            .projects
            .iter()
            .filter(|node| {
                let read = self.issue_sync.read_at.get(&node.id).copied();
                now.saturating_sub(read.unwrap_or(0)) >= SYNC_EVERY_MS
            })
            .map(|node| node.id.clone())
            .collect();
        for project in due {
            self.sync_backlog_issues(project, false, cx);
        }
    }

    /// Reads `project`'s open issues, when it follows them, and brings its
    /// backlog in step. `shown` says a failure in the dialog, for a read the
    /// person caused by opening it; the tick's are only logged.
    pub(super) fn sync_backlog_issues(
        &mut self,
        project: ProjectId,
        shown: bool,
        cx: &mut Context<Self>,
    ) {
        self.issue_sync
            .read_at
            .insert(project.clone(), ket_core::now_ms());
        let changes = self.issue_sync.changes;
        let read = cx.background_executor().spawn({
            let project = project.clone();
            async move {
                let (at, follows) = checkout(&project)?;
                if !follows {
                    return Ok(None);
                }
                let read_ms = ket_core::now_ms();
                github::open_issues(&at).map(|open| Some((open, read_ms)))
            }
        });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let read = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.take_backlog_issues(&project, changes, shown, read, cx);
            });
        })
        .detach();
    }

    /// Applies a read of `project`'s issues that began when
    /// [`IssueSync::changes`] was `changes`.
    fn take_backlog_issues(
        &mut self,
        project: &ProjectId,
        changes: u64,
        shown: bool,
        read: Read,
        cx: &mut Context<Self>,
    ) {
        let (open, read_ms) = match read {
            Ok(Some(read)) => read,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(project = project.as_str(), %error, "could not read GitHub issues");
                if shown && let Some(dialog) = self.open_backlog_of(project) {
                    dialog.error = Some(format!("Could not read GitHub issues: {error}"));
                }
                return;
            }
        };
        if self.issue_sync.busy > 0 || self.issue_sync.changes != changes {
            // Read again on the next tick, once ket's own change has landed.
            self.issue_sync.read_at.remove(project);
            return;
        }
        match Backlog::sync_issues(project, &open, read_ms) {
            Ok((backlog, synced)) => {
                if synced == ket_core::backlog::Synced::default() {
                    return;
                }
                if let Some(dialog) = self.open_backlog_of(project) {
                    dialog.take(backlog);
                }
                self.recount_backlog(project);
                cx.notify();
            }
            Err(error) => {
                tracing::warn!(project = project.as_str(), %error, "could not sync GitHub issues");
                if shown && let Some(dialog) = self.open_backlog_of(project) {
                    dialog.error = Some(format!("Could not add GitHub issues: {error}"));
                }
            }
        }
    }

    /// Makes the saved note `id` a GitHub issue, and links it.
    pub(super) fn make_backlog_issue(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_mut() else {
            return;
        };
        let Some(note) = dialog.saved(&id).filter(|note| note.issue.is_none()) else {
            return;
        };
        let title = issue_title(note);
        let body = note.body.trim().to_owned();
        let project = dialog.project.clone();
        dialog.error = None;
        self.change_issues(
            &project.clone(),
            move |at| github::create_issue(at, &title, &body),
            move |shell, made, cx| {
                let linked = made.and_then(|link| {
                    let number = link.number;
                    Backlog::link_issue(&project, &id, link).map(|backlog| (backlog, number))
                });
                match linked {
                    Ok((backlog, number)) => {
                        if let Some(dialog) = shell.open_backlog_of(&project) {
                            dialog.take(backlog);
                        }
                        shell.toast(Tone::Success, format!("Made GitHub issue #{number}"), cx);
                    }
                    Err(error) => shell.toast_detail(
                        Tone::Error,
                        "Could not make the GitHub issue",
                        error.to_string(),
                        cx,
                    ),
                }
            },
            cx,
        );
    }

    /// Closes the issues of `notes`, just marked done or deleted in the open
    /// dialog, when its project follows them. Only notes that were waiting:
    /// a started note's issue is closed by the work that lands it.
    pub(super) fn close_backlog_issues(
        &mut self,
        notes: &[Note],
        reason: CloseReason,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.backlog.as_ref().filter(|dialog| dialog.github) else {
            return;
        };
        let project = dialog.project.clone();
        let numbers: Vec<u64> = notes
            .iter()
            .filter(|note| note.done.is_none())
            .filter_map(|note| note.issue.as_ref().map(|issue| issue.number))
            .collect();
        for number in numbers {
            self.change_issues(
                &project,
                move |at| github::close_issue(at, number, reason),
                move |shell, closed, cx| {
                    if let Err(error) = closed {
                        shell.toast_detail(
                            Tone::Error,
                            format!("#{number} is still open on GitHub"),
                            error.to_string(),
                            cx,
                        );
                    }
                },
                cx,
            );
        }
    }

    /// Reopens the issue of `note`, just put back among the waiting ones in
    /// the open dialog, when it was done by hand — and so its issue closed —
    /// and the project follows its issues.
    pub(super) fn reopen_backlog_issue(&mut self, note: &Note, cx: &mut Context<Self>) {
        let Some(dialog) = self.backlog.as_ref().filter(|dialog| dialog.github) else {
            return;
        };
        let by_hand = note
            .done
            .as_ref()
            .is_some_and(|done| done.worktree.is_none());
        let Some(number) = note
            .issue
            .as_ref()
            .map(|issue| issue.number)
            .filter(|_| by_hand)
        else {
            return;
        };
        let project = dialog.project.clone();
        self.change_issues(
            &project,
            move |at| github::reopen_issue(at, number),
            move |shell, reopened, cx| {
                if let Err(error) = reopened {
                    shell.toast_detail(
                        Tone::Error,
                        format!("#{number} is still closed on GitHub"),
                        error.to_string(),
                        cx,
                    );
                }
            },
            cx,
        );
    }

    /// Runs `change` against `project`'s checkout off the window's thread,
    /// holding back reads of its issues until it is done, then hands what
    /// it returned to `then`.
    fn change_issues<T: Send + 'static>(
        &mut self,
        project: &ProjectId,
        change: impl FnOnce(&Checkout) -> ket_core::Result<T> + Send + 'static,
        then: impl FnOnce(&mut Shell, ket_core::Result<T>, &mut Context<Shell>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.issue_sync.busy += 1;
        self.issue_sync.changes += 1;
        let project = project.clone();
        let task = cx
            .background_executor()
            .spawn(async move { change(&checkout(&project)?.0) });
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let result = task.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.issue_sync.busy -= 1;
                shell.issue_sync.changes += 1;
                then(shell, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The backlog dialog, when it is open on `project`.
    fn open_backlog_of(&mut self, project: &ProjectId) -> Option<&mut super::BacklogDialog> {
        self.backlog
            .as_mut()
            .filter(|dialog| &dialog.project == project)
    }
}

/// `project`'s checkout, as the account it picked, and whether it follows
/// its GitHub issues.
fn checkout(project: &ProjectId) -> ket_core::Result<(Checkout, bool)> {
    let workspace = Workspace::open()?;
    let root = workspace
        .projects()?
        .into_iter()
        .find(|known| &known.id == project)
        .map(|known| known.root)
        .ok_or_else(|| KetError::UnknownProject(project.as_str().to_owned()))?;
    let settings = workspace.project_settings(project)?;
    let at = Checkout::new(root, settings.github_account);
    Ok((at, settings.github_issues))
}

/// What `note`'s issue is called: its title, else the first line of its
/// description. GitHub refuses an issue with no title.
fn issue_title(note: &Note) -> String {
    let title = note.title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    note.body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Untitled")
        .chars()
        .take(120)
        .collect()
}
