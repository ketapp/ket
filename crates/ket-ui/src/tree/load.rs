//! The sidebar's rows: built from the registry on the window's thread, with
//! git's part of them — each row's status and each project's other checkouts
//! — read off it. See [`Shell::reload`].

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Context, SharedString};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::theme::Color;
use ket_core::workspace::Workspace;

use super::{ChangeMarks, DiscoveredNode, ProjectNode, WorktreeNode, changed_paths};
use crate::Shell;
use crate::projects::default_color;

/// A ket worktree's sidebar row, from its record and, when it was read, its
/// git status. Without one the row shows no changes, which is the truth for a
/// worktree just cut from its base.
fn worktree_row(
    worktree: &ket_core::worktree::Worktree,
    status: Option<ket_core::status::WorktreeStatus>,
) -> WorktreeNode {
    let dirty = status.as_ref().is_some_and(|s| s.total_changes > 0);
    let changes = status.as_ref().map(changed_paths).unwrap_or(0);
    let marks = Arc::new(status.as_ref().map(ChangeMarks::of).unwrap_or_default());
    let in_progress = status.and_then(|s| s.in_progress);
    let base = worktree.base_rev().map(|rev| {
        // A base named `HEAD` resolved to a commit when the worktree was cut;
        // the commit is all there is to call it by.
        let label = if worktree.base != "HEAD" && !worktree.base.is_empty() {
            worktree.base.clone()
        } else {
            rev.chars().take(7).collect()
        };
        (
            SharedString::from(label),
            SharedString::from(rev.to_owned()),
        )
    });
    WorktreeNode {
        id: worktree.id.clone(),
        branch: worktree.branch.clone().into(),
        name: worktree.name.clone().map(SharedString::from),
        line_changes: None,
        changes,
        marks,
        base,
        agent: worktree.agent.clone().map(SharedString::from),
        dirty,
        in_progress,
        token_reduction: ket_core::worktree::resolve_level(
            worktree.token_reduction,
            worktree.token_reduction_id.as_deref(),
            worktree.token_reduction_pack_id.as_deref(),
        ),
        pinned: worktree.pinned,
        missing: !worktree.exists(),
        path: worktree.path.clone(),
        primary: false,
        // Carried over by `Shell::reload`, so a reload does not blink every
        // badge off until the next sweep.
        merged: false,
        unlanded: false,
        pull_request: worktree.pull_request.clone(),
    }
}

fn promote_worked_in(
    workspace: &Workspace,
    project: &ProjectId,
    mut managed: Vec<ket_core::worktree::Worktree>,
    discovered: &mut Vec<ket_core::worktree::DiscoveredWorktree>,
) -> Vec<ket_core::worktree::Worktree> {
    let paths: Vec<&std::path::Path> = discovered
        .iter()
        .map(|found| found.path.as_path())
        .collect();
    if paths.is_empty() {
        return managed;
    }

    let worked_in = ket_core::sessions::Sessions::new().scan(&paths);
    if worked_in.is_empty() {
        return managed;
    }

    let mut adopted = Vec::new();
    discovered.retain(|found| {
        if !worked_in.contains_key(&found.path) {
            return true;
        }
        match workspace.adopt_worktree(project, &found.path) {
            Ok(worktree) => {
                adopted.push(worktree);
                false
            }
            Err(e) => {
                tracing::debug!(path = %found.path.display(), %e, "not promoting");
                true
            }
        }
    });

    managed.extend(adopted);
    managed.sort_by_key(|worktree| std::cmp::Reverse(worktree.created_at_ms));
    managed
}

impl Shell {
    /// The rows a window opens with, from the registry, and git's part of
    /// them read as soon as the window exists.
    pub(crate) fn first_load(workspace: &Workspace, cx: &mut Context<Self>) -> Vec<ProjectNode> {
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let _ = shell.update(cx, |shell, cx| {
                shell.sweep_git_status(cx);
                shell.load_discovered(cx);
            });
        })
        .detach();
        Self::load_projects(workspace)
    }

    /// Builds the tree from the registry alone. No git, so it is quick enough
    /// for the window's thread: every row starts with no changes and every
    /// project with no discovered checkouts. [`Shell::reload`] carries both
    /// over from the rows it replaces, and they are read off the window's
    /// thread — status by `crate::git_watch`, the checkouts by
    /// [`Shell::load_discovered`].
    fn load_projects(workspace: &Workspace) -> Vec<ProjectNode> {
        let Ok(projects) = workspace.projects() else {
            return Vec::new();
        };
        // One registry read for every project's rows, already in the
        // sidebar's order.
        let worktrees = workspace.worktrees(None).unwrap_or_default();

        let mut nodes: Vec<ProjectNode> = projects
            .into_iter()
            .map(|project| {
                let settings = workspace.project_settings(&project.id).unwrap_or_default();
                let mut node = ProjectNode {
                    name: settings.name_for(&project).to_owned().into(),
                    color: settings
                        .color
                        .as_deref()
                        .and_then(|hex| Color::from_hex(hex).ok())
                        .unwrap_or_else(default_color),
                    icon: settings
                        .icon
                        .as_deref()
                        .and_then(crate::ui::glyph::drawable)
                        .map(|icon| SharedString::from(icon.to_owned())),
                    image: workspace
                        .project_image_path(&project.id, &settings)
                        .map(Arc::from),
                    hide_discovered: settings.hide_discovered_worktrees,
                    pinned: settings.pinned,
                    hidden: settings.hidden,
                    agent_overrides: settings.agents.clone(),
                    expanded: true,
                    worktrees: std::iter::once(WorktreeNode {
                        // A stable id derived from the project, so its tabs
                        // and its terminal survive a reload the same way a
                        // real worktree's do. Nothing in core ever sees it.
                        id: WorktreeId::new(format!("primary-{}", project.id)),
                        line_changes: None,
                        changes: 0,
                        marks: Arc::default(),
                        base: None,
                        branch: project.default_base.clone().into(),
                        name: None,
                        agent: None,
                        dirty: false,
                        in_progress: None,
                        token_reduction: ket_core::worktree::default_token_reduction(),
                        pinned: false,
                        missing: !project.root.is_dir(),
                        path: project.root.clone(),
                        primary: true,
                        // The base branch is not a thing that gets merged
                        // away, and a badge on it would say nothing.
                        merged: false,
                        unlanded: false,
                        pull_request: None,
                    })
                    .chain(
                        worktrees
                            .iter()
                            .filter(|worktree| worktree.project_id == project.id)
                            .map(|worktree| worktree_row(worktree, None)),
                    )
                    .collect(),
                    discovered: Vec::new(),
                    discovered_expanded: false,
                    backlog_seen: ket_core::backlog::Backlog::modified(&project.id),
                    backlog: ket_core::backlog::Backlog::load(&project.id)
                        .map(|backlog| backlog.open_count())
                        .unwrap_or(0),
                    id: project.id,
                };
                // Pinned rows lead the project, behind only the repository's own
                // checkout. Stable, so everything else keeps the order it had.
                node.worktrees.sort_by_key(|w| (!w.primary, !w.pinned));
                node
            })
            .collect();
        // Pinned projects lead, as pinned rows lead a project. Stable, so each
        // side keeps the order it was dragged into.
        nodes.sort_by_key(|node| !node.pinned);
        nodes
    }

    /// Reloads the tree, keeping what the reader had expanded.
    ///
    /// Expansion is remembered by project **id**, not by position. Projects are
    /// ordered by recency, so acting on one reorders the list — keyed by index,
    /// a refresh would silently collapse the project you were working in and
    /// expand some other one.
    ///
    /// Only the registry is read here, on the window's thread. Git — every
    /// row's status and each project's other checkouts — is read off it, and
    /// until it lands each row keeps what git last said. On this thread that
    /// was a `git worktree list` per project and a status per checkout:
    /// dozens of processes, with the window frozen until they finished.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.rebuild_rows(cx);
        self.sweep_git_status(cx);
        self.load_discovered(cx);
    }

    /// [`Shell::reload`]'s own part: the rows rebuilt from the registry, each
    /// keeping what git said about it and each project its discovered
    /// checkouts until they are read again.
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let Ok(workspace) = Workspace::open() else {
            return;
        };
        let selected = self.selected_id();

        /// What git said about a row, which the registry can't say.
        struct Said {
            dirty: bool,
            changes: usize,
            marks: Arc<ChangeMarks>,
            line_changes: Option<(u32, u32)>,
            in_progress: Option<ket_core::status::GitOperation>,
            merged: bool,
            unlanded: bool,
        }
        let mut said: HashMap<WorktreeId, Said> = HashMap::new();
        let mut was: HashMap<ProjectId, (bool, bool, Vec<DiscoveredNode>)> = HashMap::new();
        for project in std::mem::take(&mut self.projects) {
            for row in project.worktrees {
                said.insert(
                    row.id,
                    Said {
                        dirty: row.dirty,
                        changes: row.changes,
                        marks: row.marks,
                        line_changes: row.line_changes,
                        in_progress: row.in_progress,
                        // Swept on its own slow cadence: dropped here, every
                        // badge would blink off for up to a minute.
                        merged: row.merged,
                        unlanded: row.unlanded,
                    },
                );
            }
            was.insert(
                project.id,
                (
                    project.expanded,
                    project.discovered_expanded,
                    project.discovered,
                ),
            );
        }

        self.projects = Self::load_projects(&workspace);

        for node in &mut self.projects {
            for row in &mut node.worktrees {
                if let Some(said) = said.remove(&row.id) {
                    row.dirty = said.dirty;
                    row.changes = said.changes;
                    row.marks = said.marks;
                    row.line_changes = said.line_changes;
                    row.in_progress = said.in_progress;
                    row.merged = said.merged;
                    row.unlanded = said.unlanded;
                }
            }
            let Some((expanded, discovered_expanded, discovered)) = was.remove(&node.id) else {
                continue;
            };
            node.expanded = expanded;
            node.discovered_expanded = discovered_expanded;
            if !node.hide_discovered && !discovered.is_empty() {
                // A checkout adopted since is a row now, not a discovered one.
                let rows: Vec<String> = node
                    .worktrees
                    .iter()
                    .map(|row| {
                        std::fs::canonicalize(&row.path)
                            .unwrap_or_else(|_| row.path.clone())
                            .display()
                            .to_string()
                    })
                    .collect();
                node.discovered = discovered
                    .into_iter()
                    .filter(|found| !rows.iter().any(|row| row == found.path.as_ref()))
                    .collect();
            }
        }

        // The selection is re-found by worktree id, not kept by position:
        // removing a project shifts every index after it, and a selection
        // kept by index would land on some other project's worktree.
        self.selection = selected.and_then(|id| self.position_of(&id));
        if let Some(selection) = self.selection {
            self.select(selection, cx);
        }
        if self
            .project_menu
            .as_ref()
            .is_some_and(|menu| menu.project >= self.projects.len())
        {
            self.project_menu = None;
            self.tucked_menu = None;
        }
        // A level just changed, or a worktree came or went: either moves what
        // the savings rollup lists.
        self.refresh_savings();
        // A worktree that went takes its browser tabs and profile with it.
        #[cfg(feature = "chromium")]
        self.chromium_registry_changed(&workspace, cx);
    }

    /// Reads git's other checkouts of every project — a `git worktree list`
    /// each — off the window's thread, and adopts the ones an agent has
    /// worked in as rows; see [`promote_worked_in`]. Only the latest call's
    /// answer is used: an older one can predate a row adopted since.
    fn load_discovered(&mut self, cx: &mut Context<Self>) {
        self.git_watch.discovered_read += 1;
        let read = self.git_watch.discovered_read;
        let found = cx.background_executor().spawn(async move {
            let workspace = Workspace::open().ok()?;
            let mut adopted = false;
            let found: Vec<(ProjectId, Vec<DiscoveredNode>)> = workspace
                .projects()
                .ok()?
                .into_iter()
                .map(|project| {
                    let settings = workspace.project_settings(&project.id).unwrap_or_default();
                    // Gated on the setting, and the order matters: promotion
                    // *writes* to the registry, so running it before the
                    // check let `hide_discovered_worktrees` hide the rows
                    // while still adopting every one of them. Someone who has
                    // said they do not want git's other checkouts in this
                    // project has said it about the record, not just the
                    // display.
                    if settings.hide_discovered_worktrees {
                        return (project.id, Vec::new());
                    }
                    let Ok(report) = workspace.worktree_report(Some(&project.id)) else {
                        return (project.id, Vec::new());
                    };
                    let mut discovered = report.discovered;
                    let before = report.managed.len();
                    adopted |=
                        promote_worked_in(&workspace, &project.id, report.managed, &mut discovered)
                            .len()
                            != before;
                    let nodes = discovered
                        .iter()
                        .map(|found| DiscoveredNode {
                            label: found
                                .branch
                                .clone()
                                .unwrap_or_else(|| "detached".to_owned())
                                .into(),
                            path: found.path.display().to_string().into(),
                        })
                        .collect();
                    (project.id, nodes)
                })
                .collect();
            Some((found, adopted))
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let Some((found, adopted)) = found.await else {
                return;
            };
            let _ = shell.update(cx, |shell, cx| {
                if shell.git_watch.discovered_read != read {
                    return;
                }
                if adopted {
                    // The adopted checkouts are in the registry now.
                    shell.rebuild_rows(cx);
                    shell.read_git_status(cx);
                }
                for (id, discovered) in found {
                    if let Some(node) = shell.projects.iter_mut().find(|node| node.id == id) {
                        node.discovered = discovered;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts a worktree that was just made into the sidebar in place, from the
    /// registry and with no git: one cut a moment ago has no changes, and
    /// `crate::git_watch` reads it from the next tick like any other row. A
    /// reload ran git for every project and worktree to add this one row.
    /// Whether the row is there afterwards.
    pub(crate) fn add_row(&mut self, id: &WorktreeId, cx: &mut Context<Self>) -> bool {
        if self.position_of(id).is_some() {
            return true;
        }
        let Ok(workspace) = Workspace::open() else {
            return false;
        };
        let Some(worktree) = workspace
            .worktrees(None)
            .ok()
            .and_then(|all| all.into_iter().find(|w| &w.id == id))
        else {
            return false;
        };
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == worktree.project_id)
        else {
            return false;
        };
        project.worktrees.push(worktree_row(&worktree, None));
        self.resort_worktrees(&worktree.project_id);
        self.refresh_savings();
        // So a later delete from the CLI or a phone is seen as one.
        #[cfg(feature = "chromium")]
        self.chromium_registry_changed(&workspace, cx);
        self.menu_dirty = true;
        cx.notify();
        true
    }

    /// Puts a project's rows back in the order a reload gives them — its own
    /// checkout, then pinned rows, each side in the registry's order — after
    /// a pin changed in place. Reads the registry and nothing else: no git.
    pub(crate) fn resort_worktrees(&mut self, project: &ProjectId) {
        let Ok(order) = Workspace::open().and_then(|workspace| workspace.worktrees(Some(project)))
        else {
            return;
        };
        let selected = self.selected_id();
        let Some(node) = self.projects.iter_mut().find(|node| &node.id == project) else {
            return;
        };
        let rank = |id: &WorktreeId| {
            order
                .iter()
                .position(|worktree| &worktree.id == id)
                .unwrap_or(usize::MAX)
        };
        node.worktrees
            .sort_by_key(|row| (!row.primary, !row.pinned, rank(&row.id)));
        self.selection = selected.and_then(|id| self.position_of(&id));
    }
}
