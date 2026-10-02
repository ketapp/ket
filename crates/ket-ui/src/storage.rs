//! What each worktree costs on disk, measured rarely and remembered.
//!
//! The number is expensive — see `ket_core::storage`, which has to walk the
//! whole checkout to get it — so nothing here is on the tick. One
//! sweep shortly after launch fills the cache for every worktree, and clearing
//! a worktree's build output re-measures that worktree. Between those, the
//! sidebar draws whatever the cache holds and nothing at all when it holds
//! nothing, which is the honest state before the first sweep lands.
//!
//! Cached in memory only. Sizes go stale the moment an agent runs `cargo
//! build`, so a figure persisted to `state.json` would come back after a
//! restart claiming to know something it could not.

use std::collections::HashMap;

use gpui::Context;
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::storage::Footprint;
use ket_core::workspace::Workspace;

use crate::Shell;

impl Shell {
    /// What a worktree costs, when that has been measured.
    pub(crate) fn footprint(&self, id: &WorktreeId) -> Option<Footprint> {
        self.sizes.get(id).copied()
    }

    /// Measures every worktree ket knows about, once.
    ///
    /// Called at launch. The guard is not an optimisation: a second sweep
    /// started while the first is walking would put two processes through the
    /// same several hundred thousand files.
    pub(crate) fn measure_all(&mut self, cx: &mut Context<Self>) {
        self.measure(None, cx);
    }

    /// The sweep behind it.
    ///
    /// `None` measures everything. Results are merged into the cache by id
    /// rather than replacing it, so a project-scoped refresh cannot blank out
    /// what the launch sweep learned about the others.
    fn measure(&mut self, project: Option<ProjectId>, cx: &mut Context<Self>) {
        if self.measuring {
            return;
        }
        self.measuring = true;

        let read = cx.background_executor().spawn(async move {
            let Ok(workspace) = Workspace::open() else {
                return Vec::new();
            };
            let Ok(worktrees) = workspace.worktrees(project.as_ref()) else {
                return Vec::new();
            };

            worktrees
                .into_iter()
                .filter(|worktree| worktree.exists())
                .map(|worktree| {
                    let dirs = workspace.build_dirs(&worktree.project_id);
                    let footprint = ket_core::storage::measure(&worktree.path, &dirs);
                    (worktree.id, footprint)
                })
                .collect::<Vec<_>>()
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let rows = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.measuring = false;
                for (id, footprint) in rows {
                    shell.sizes.insert(id, footprint);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-measures one worktree now, after something changed what it holds.
    pub(crate) fn remeasure_worktree(&mut self, id: WorktreeId, cx: &mut Context<Self>) {
        let Some(path) = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .find(|node| node.id == id)
            .map(|node| node.path.clone())
        else {
            return;
        };

        let for_dirs = id.clone();
        let read = cx.background_executor().spawn(async move {
            let workspace = Workspace::open().ok()?;
            let worktree = workspace
                .worktrees(None)
                .ok()?
                .into_iter()
                .find(|w| w.id == for_dirs)?;
            let dirs = workspace.build_dirs(&worktree.project_id);
            Some(ket_core::storage::measure(&path, &dirs))
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let measured = read.await;
            let _ = shell.update(cx, |shell, cx| {
                match measured {
                    Some(footprint) => {
                        shell.sizes.insert(id, footprint);
                    }
                    None => {
                        shell.sizes.remove(&id);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The cache itself, as the shell holds it.
pub(crate) type Sizes = HashMap<WorktreeId, Footprint>;

/// The smallest size the sidebar states for a worktree.
///
/// Below it the figure is noise: every checkout has one, and a column of
/// "38 MB"s beside the branch names says nothing a person would act on. From
/// here up it is a worktree carrying build output worth clearing, which is
/// the one reason the number is on the row at all. Binary megabytes, the
/// unit `format_bytes` prints, so the threshold and the label agree.
pub(crate) const SHOWN_FROM: u64 = 500 << 20;

impl Shell {
    /// A project's total on disk and how much of it is build output, when
    /// any of its worktrees has been measured.
    ///
    /// The primary checkout is left out, as it is from every other storage
    /// figure: it is the repository itself, not something ket made and can
    /// take away again.
    pub(crate) fn project_footprint(&self, pi: usize) -> Option<(u64, u64)> {
        let project = self.projects.get(pi)?;
        let mut measured = project
            .worktrees
            .iter()
            .filter(|node| !node.primary && !node.missing)
            .filter_map(|node| self.footprint(&node.id))
            .peekable();
        measured.peek()?;
        Some(measured.fold((0, 0), |(total, build), footprint| {
            (total + footprint.total, build + footprint.build)
        }))
    }
}
