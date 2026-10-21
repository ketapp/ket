//! What ket's store says about the places the host's terminals belong to —
//! projects, worktrees, their figures and open backlog notes — read a few
//! seconds at a time, for snapshots and for the requests that name them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::{BACKLOG_TTL, FIGURES_TTL, PLACES_RECHECK, PLACES_TTL, Phones, lock};

/// What ket's store says about the projects and worktrees the host's
/// terminals belong to: their names, and the order a sidebar shows them in.
#[derive(Default)]
pub(super) struct Places {
    /// Projects, in the sidebar's order.
    pub(super) projects: Vec<ProjectPlace>,
    pub(super) worktrees: Vec<Place>,
}

pub(super) struct ProjectPlace {
    pub(super) id: String,
    /// The name a sidebar shows: the one somebody gave it, else the folder's.
    pub(super) name: String,
    /// The colour chosen for it, `#rrggbb`, or empty for ket's default.
    pub(super) color: String,
}

pub(super) struct Place {
    pub(super) id: String,
    pub(super) project: String,
    pub(super) name: String,
    pub(super) branch: String,
    /// Where it is checked out, for its figures.
    pub(super) path: std::path::PathBuf,
    /// The branch it was cut from; `None` for a repository's own checkout.
    pub(super) base: Option<String>,
    /// Pinned in the sidebar. Never a repository's own checkout.
    pub(super) pinned: bool,
}

/// Every worktree's figures, by id.
pub(super) type FigureMap = Arc<HashMap<String, Figures>>;

/// Open backlog notes by project id — see [`BACKLOG_TTL`].
pub(super) type BacklogCounts = Arc<HashMap<String, u32>>;

/// A worktree's figures, as the desktop's sidebar and merge dialog count them.
#[derive(Clone, Copy, Default)]
pub(super) struct Figures {
    pub(super) files_changed: u32,
    pub(super) lines_added: u32,
    pub(super) lines_removed: u32,
    pub(super) commits_ahead: u32,
}

impl Figures {
    /// Reads them from git. Anything that cannot be read is zero: a figure is
    /// a nicety, and a missing checkout or a deleted base is ordinary.
    fn read(place: &Place) -> Self {
        let Ok(status) = crate::status::of_worktree(&place.path, place.base.as_deref()) else {
            return Self::default();
        };
        let mut paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        // Only a dirty tree is worth a diff; a clean one has no lines to count.
        let (lines_added, lines_removed) = if status.total_changes > 0 {
            crate::git::Git::new(&place.path)
                .line_changes()
                .unwrap_or_default()
        } else {
            (0, 0)
        };
        Self {
            files_changed: u32::try_from(paths.len()).unwrap_or(u32::MAX),
            lines_added,
            lines_removed,
            commits_ahead: status.tracking.map_or(0, |tracking| tracking.ahead),
        }
    }
}

impl Places {
    /// Reads ket's store, or nothing if it cannot be read: names are a
    /// nicety, and a phone still gets every terminal without them.
    fn read() -> Self {
        let Ok(state) = crate::store::Store::open_default().and_then(|store| store.load()) else {
            return Self::default();
        };
        // The sidebar's own order, so a phone lists everything where the
        // desktop does.
        let projects = crate::workspace::sidebar_projects(&state);
        let mut worktrees: Vec<Place> = Vec::new();
        for project in &projects {
            // The repository's own checkout, under the id a window gives it.
            worktrees.push(Place {
                id: format!("primary-{}", project.id),
                project: project.id.to_string(),
                name: project.default_base.clone(),
                branch: project.default_base.clone(),
                path: project.root.clone(),
                base: None,
                pinned: false,
            });
        }
        for worktree in crate::workspace::sidebar_worktrees(&state) {
            worktrees.push(Place {
                id: worktree.id.to_string(),
                project: worktree.project_id.to_string(),
                name: worktree
                    .name
                    .clone()
                    .unwrap_or_else(|| worktree.branch.clone()),
                branch: worktree.branch,
                path: worktree.path,
                base: Some(worktree.base),
                pinned: worktree.pinned,
            });
        }
        let settings = state.project_settings;
        Self {
            projects: projects
                .into_iter()
                .map(|p| {
                    let chosen = settings.get(&p.id);
                    ProjectPlace {
                        id: p.id.to_string(),
                        name: chosen
                            .and_then(|s| s.display_name.clone())
                            .unwrap_or(p.name),
                        color: chosen.and_then(|s| s.color.clone()).unwrap_or_default(),
                    }
                })
                .collect(),
            worktrees,
        }
    }

    pub(super) fn get(&self, id: &str) -> Option<&Place> {
        self.worktrees.iter().find(|place| place.id == id)
    }
}

impl Phones {
    /// The figures of every worktree in `places` — see [`FIGURES_TTL`].
    pub(super) fn figures(&self, places: &Places) -> FigureMap {
        let mut figures = lock(&self.figures);
        match &*figures {
            Some((at, read)) if at.elapsed() < FIGURES_TTL => read.clone(),
            _ => {
                let read: FigureMap = Arc::new(
                    places
                        .worktrees
                        .iter()
                        .map(|place| (place.id.clone(), Figures::read(place)))
                        .collect(),
                );
                *figures = Some((Instant::now(), read.clone()));
                read
            }
        }
    }

    /// Each project's open backlog notes — see [`BACKLOG_TTL`].
    pub(super) fn backlog_counts(&self, places: &Places) -> BacklogCounts {
        let mut backlogs = lock(&self.backlogs);
        match &*backlogs {
            Some((at, read)) if at.elapsed() < BACKLOG_TTL => read.clone(),
            _ => {
                let read: BacklogCounts = Arc::new(
                    places
                        .projects
                        .iter()
                        .map(|project| {
                            let id = crate::id::ProjectId::from(project.id.as_str());
                            let open = crate::backlog::Backlog::load(&id)
                                .map(|backlog| backlog.open_count())
                                .unwrap_or(0);
                            (project.id.clone(), u32::try_from(open).unwrap_or(u32::MAX))
                        })
                        .collect(),
                );
                *backlogs = Some((Instant::now(), read.clone()));
                read
            }
        }
    }

    /// Recounts the backlogs on the next snapshot, after a phone changed one.
    pub(super) fn forget_backlogs(&self) {
        *lock(&self.backlogs) = None;
    }

    /// Reads ket's store again on the next snapshot, after a phone changed
    /// the worktrees in it.
    pub(super) fn forget_places(&self) {
        *lock(&self.places) = None;
    }

    pub(super) fn places(&self) -> Arc<Places> {
        self.places_knowing(&[])
    }

    /// [`Phones::places`], read again sooner when one of `worktrees` is not
    /// in it — see [`PLACES_RECHECK`].
    pub(super) fn places_knowing(&self, worktrees: &[&str]) -> Arc<Places> {
        let mut places = lock(&self.places);
        match &*places {
            Some((at, read))
                if at.elapsed() < PLACES_TTL
                    && (at.elapsed() < PLACES_RECHECK
                        || worktrees.iter().all(|id| read.get(id).is_some())) =>
            {
                read.clone()
            }
            _ => {
                let read = Arc::new(Places::read());
                *places = Some((Instant::now(), read.clone()));
                read
            }
        }
    }
}
