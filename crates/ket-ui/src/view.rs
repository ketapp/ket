//! What the window looked like when you left it.
//!
//! The pane arrangement is `crate::layout`'s, and it is stored per project
//! because that is whose it is. This is everything else — which panels are
//! open and how wide, what is expanded in the sidebar and the file tree, which
//! worktree was selected, where the window itself sat — and none of it belongs
//! to a project. It is the window's, and it is written once for the whole of
//! it, into [`ket_core::store::State::view`].
//!
//! # Written on a comparison, not on a change
//!
//! Every one of these moves without a discrete moment to hang a save on: a
//! divider drag changes a width every frame, and the window's own size changes
//! while it is being dragged. So rather than a call at each of the dozen
//! places that can move one, the shell captures the whole shape and writes
//! only when the capture differs from what was last written —
//! [`Shell::persist_view`] is cheap to call and almost always does nothing.
//!
//! Two callers, and between them they cover the cases: the ten-second tick
//! that keeps the rest of the window current, so a change is at most a tick
//! from being durable, and the quit hook in `main`, so the change somebody
//! made in the last few seconds of a run is not the one that gets lost.
//!
//! # What is deliberately not kept
//!
//! Anything that is *about* the session rather than about the window: which
//! menu is open, what is being dragged, what a search box holds. Those are
//! answers to "what are you in the middle of", and coming back to them a day
//! later would be a window that had not finished a thought nobody remembers
//! having.

use std::collections::BTreeMap;

use gpui::{App, Bounds, Pixels, point, px, size};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::workspace::Workspace;
use serde::{Deserialize, Serialize};

use crate::Shell;
use crate::tree::Selection;

/// Bumped when a field changes meaning rather than when one is added: unknown
/// fields are ignored on read and missing ones take their default, so growing
/// the shape costs nothing and does not strand a window opened by an older
/// build.
const VIEW_VERSION: u32 = 1;

/// Most directories remembered as open, per worktree.
///
/// The whole blob is bounded by core at 64 KiB — see
/// [`ket_core::store::MAX_LAYOUT_BYTES`] — and a file tree is the only thing
/// here that grows with what somebody does rather than with what they have.
/// Opening a hundred directories in one worktree is a tree nobody is reading
/// as a tree any more, so the newest hundred is as much as is worth keeping.
const MAX_EXPANDED_DIRS: usize = 100;

/// Most worktrees whose file tree is remembered at all.
const MAX_EXPLORER_TREES: usize = 32;

/// The window's shape, as it is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct WindowView {
    version: u32,
    sidebar_open: bool,
    /// Whether the status bar was showing.
    status_bar_open: bool,
    sidebar_width: f32,
    panel_open: bool,
    panel_width: f32,
    /// Which of the right panel's views was showing, by name — see
    /// `crate::panel::PanelView::from_name` for what an unknown one becomes.
    panel_view: String,
    /// Where the window itself was. Absent until a window has reported its
    /// bounds, which is its first frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<SavedBounds>,
    /// Which projects were expanded in the sidebar, and whether the group of
    /// worktrees ket did not create was open under them.
    projects: BTreeMap<ProjectId, SavedProject>,
    /// The worktree that was selected. A worktree that has since been removed
    /// is ignored on restore rather than cleaned up here.
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<WorktreeId>,
    /// Whether the sidebar was showing worktrees by activity rather than
    /// grouped under their projects.
    activity_view: bool,
    /// Whether the header was folded to its compact strip.
    header_compact: bool,
    /// The span the Savings popup totals over, by name — see
    /// `crate::savings::SavingsPeriod::name`. An unknown or missing name reads
    /// as the default.
    savings_period: String,
    /// Which directories were open in each worktree's file tree.
    explorer: BTreeMap<WorktreeId, Vec<String>>,
}

impl Default for WindowView {
    fn default() -> Self {
        Self {
            version: VIEW_VERSION,
            sidebar_open: true,
            status_bar_open: true,
            panel_open: true,
            sidebar_width: 300.0,
            panel_width: 280.0,
            panel_view: String::new(),
            window: None,
            projects: BTreeMap::new(),
            selected: None,
            activity_view: false,
            header_compact: false,
            savings_period: String::new(),
            explorer: BTreeMap::new(),
        }
    }
}

/// A window's rectangle in screen coordinates, as `gpui` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedBounds {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// What a project's row remembers between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SavedProject {
    expanded: bool,
    discovered_expanded: bool,
    /// The worktree this project was last on, which is where clicking its
    /// heading goes. Absent for a project nobody has opened.
    #[serde(skip_serializing_if = "Option::is_none")]
    focus: Option<WorktreeId>,
}

/// Reads the stored shape, or `None` when there is none to read.
///
/// A blob that will not parse is treated as one that is not there: the window
/// opens on defaults, and the next save replaces it. There is nothing in here
/// worth refusing to open a window over.
fn stored() -> Option<WindowView> {
    let view = Workspace::open().ok()?.view_state().ok()??;
    serde_json::from_value::<WindowView>(view)
        .ok()
        .filter(|view| view.version == VIEW_VERSION)
}

/// Where the window was when it was last closed, if that is still a place a
/// window can be.
///
/// Read before the shell exists, because `gpui` wants a window's bounds when
/// it opens the window rather than after. A rectangle whose origin is not on
/// any display connected *now* is discarded — restoring a window onto a screen
/// that has gone home for the weekend opens it where nobody can see it, and
/// the only way back would be to edit the file it was restored from.
pub(crate) fn saved_window_bounds(cx: &App) -> Option<Bounds<Pixels>> {
    let saved = stored()?.window?;
    let bounds = Bounds {
        origin: point(px(saved.x), px(saved.y)),
        size: size(px(saved.width), px(saved.height)),
    };
    let on_a_display = cx
        .displays()
        .iter()
        .any(|display| display.bounds().contains(&bounds.origin));

    (on_a_display && bounds.size.width >= px(480.0) && bounds.size.height >= px(360.0))
        .then_some(bounds)
}

impl Shell {
    /// The window's shape as it is right now.
    fn capture_view(&self) -> WindowView {
        let projects = self
            .projects
            .iter()
            .map(|project| {
                (
                    project.id.clone(),
                    SavedProject {
                        expanded: project.expanded,
                        discovered_expanded: project.discovered_expanded,
                        focus: self.project_focus.get(&project.id).cloned(),
                    },
                )
            })
            .collect();

        WindowView {
            version: VIEW_VERSION,
            sidebar_open: self.sidebar_open,
            status_bar_open: self.status_bar_open,
            sidebar_width: f32::from(self.sidebar_width),
            panel_open: self.panel_open,
            panel_width: f32::from(self.panel_width),
            panel_view: self.panel_view.name().to_owned(),
            window: self.window_bounds.map(|bounds| SavedBounds {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            }),
            projects,
            selected: self.selected_id(),
            activity_view: self.activity_view,
            header_compact: self.header_compact,
            savings_period: self.savings_period.name().to_owned(),
            explorer: self.expanded_trees(),
        }
    }

    /// What every worktree's file tree has open, live tree included.
    ///
    /// The tree on screen is the only one whose expansion is in the `Explorer`
    /// — switching worktrees builds a new one — so the memory is what the
    /// others left behind and this folds the live one back into it.
    fn expanded_trees(&self) -> BTreeMap<WorktreeId, Vec<String>> {
        let mut trees: BTreeMap<WorktreeId, Vec<String>> = self
            .explorer_memory
            .iter()
            .map(|(id, dirs)| {
                let mut dirs: Vec<String> = dirs.iter().cloned().collect();
                dirs.sort();
                (id.clone(), dirs)
            })
            .filter(|(_, dirs)| !dirs.is_empty())
            .collect();

        if let Some(id) = self.explorer.worktree.clone() {
            let mut dirs: Vec<String> = self.explorer.expanded.iter().cloned().collect();
            dirs.sort();
            match dirs.is_empty() {
                true => trees.remove(&id),
                false => trees.insert(id, dirs),
            };
        }

        // Bounded at both ends: a tree nobody has opened a directory in is not
        // in here at all, and the ones that are keep a hundred directories
        // each. Which hundred is arbitrary — they are a set, and there is no
        // order in it that says which a reader cares about.
        for dirs in trees.values_mut() {
            dirs.truncate(MAX_EXPANDED_DIRS);
        }
        while trees.len() > MAX_EXPLORER_TREES {
            let Some(first) = trees.keys().next().cloned() else {
                break;
            };
            trees.remove(&first);
        }
        trees
    }

    /// Writes the window's shape, if it has moved since it was last written.
    ///
    /// Cheap to call and meant to be called often — see the module doc. The
    /// comparison is against what this shell last wrote rather than against
    /// the file, so a second ket running beside this one is not read back on
    /// every tick; the last window to move is the one whose shape is stored,
    /// which is the same rule two windows have always followed here.
    pub(crate) fn persist_view(&mut self) {
        let view = self.capture_view();
        if self.view_saved.as_ref() == Some(&view) {
            return;
        }

        let value = match serde_json::to_value(&view) {
            Ok(value) => value,
            Err(error) => {
                self.note = Some(format!("could not encode the window's state: {error}").into());
                return;
            }
        };

        match Workspace::open().and_then(|workspace| workspace.set_view_state(Some(value))) {
            // Held even when the write failed would mean never trying again;
            // held only when it succeeded means the next tick retries.
            Ok(()) => self.view_saved = Some(view),
            Err(error) => self.note = Some(format!("could not save the window: {error}").into()),
        }
    }

    /// Puts the window back the way it was left, and says which worktree was
    /// selected so the caller can open on it.
    ///
    /// Everything here is applied over the defaults the shell was built with,
    /// so a stored shape that is missing a field — written by a build that did
    /// not have it — leaves that part of the window at its default rather than
    /// at zero.
    pub(crate) fn restore_view(&mut self) -> Option<WorktreeId> {
        let view = stored()?;

        self.sidebar_open = view.sidebar_open;
        self.status_bar_open = view.status_bar_open;
        self.panel_open = view.panel_open;
        self.sidebar_width = px(view.sidebar_width);
        self.panel_width = px(view.panel_width);
        self.panel_view = crate::panel::PanelView::from_name(&view.panel_view);
        self.activity_view = view.activity_view;
        self.header_compact = view.header_compact;
        self.savings_period = crate::savings::SavingsPeriod::from_name(&view.savings_period);

        for project in &mut self.projects {
            let Some(saved) = view.projects.get(&project.id) else {
                continue;
            };
            project.expanded = saved.expanded;
            project.discovered_expanded = saved.discovered_expanded;
            // Only for a worktree that is still there: this is what clicking
            // the project's heading goes to, and a stale id would send it to
            // the first worktree instead, which is where it goes anyway when
            // there is nothing remembered.
            if let Some(focus) = saved
                .focus
                .clone()
                .filter(|id| project.worktrees.iter().any(|node| &node.id == id))
            {
                self.project_focus.insert(project.id.clone(), focus);
            }
        }

        self.explorer_memory = view
            .explorer
            .iter()
            .map(|(id, dirs)| (id.clone(), dirs.iter().cloned().collect()))
            .collect();

        let selected = view.selected.clone();
        // What was read is what is stored, so the first tick after launch has
        // nothing to write unless something has actually moved since.
        self.view_saved = Some(view);
        selected
    }

    /// Where a worktree sits in the sidebar, if it is still there.
    pub(crate) fn locate(&self, id: &WorktreeId) -> Option<Selection> {
        self.projects
            .iter()
            .enumerate()
            .find_map(|(project, node)| {
                node.worktrees
                    .iter()
                    .position(|worktree| &worktree.id == id)
                    .map(|worktree| Selection { project, worktree })
            })
    }
}
