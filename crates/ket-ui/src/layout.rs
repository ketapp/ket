//! Versioned persistence for the shell-owned pane schema.
//!
//! Core deliberately stores this as opaque JSON. Keeping every conversion in
//! this module prevents pane and tab concepts from leaking into the engine.

use std::collections::{HashMap, HashSet};

use gpui::{Axis, SharedString};
use ket_core::id::WorktreeId;
use ket_core::store::MAX_LAYOUT_BYTES;
use ket_core::workspace::Workspace;
use serde::{Deserialize, Serialize};

use crate::Shell;
use crate::tabs::{Pane, PaneId, Space, Tab, TabKind};

const LAYOUT_VERSION: u32 = 1;

fn fits(layout: &ProjectLayout) -> bool {
    serde_json::to_vec(layout).is_ok_and(|bytes| bytes.len() <= MAX_LAYOUT_BYTES)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectLayout {
    version: u32,
    worktrees: Vec<WorktreeLayout>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorktreeLayout {
    id: WorktreeId,
    space: SavedSpace,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedSpace {
    root: SavedPane,
    focused: u64,
    next_id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SavedPane {
    Leaf {
        id: u64,
        tabs: Vec<SavedTab>,
        active: usize,
    },
    Split {
        axis: SavedAxis,
        children: Vec<SavedPane>,
        sizes: Vec<f32>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SavedAxis {
    Horizontal,
    Vertical,
}

/// One saved tab.
///
/// Internally tagged with the path beside the tag rather than adjacently
/// tagged with the path *as* the content, so a variant can carry a second
/// field. The JSON is the same either way — `{"kind":"file","path":"…"}` — so
/// a layout written before `title` existed still loads, and a tab nobody has
/// renamed still writes exactly what it always did.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SavedTab {
    /// Retained only so layouts written by the removed review pane still load.
    AllChanges,
    /// Retained only so layouts written by the removed review pane still load.
    File {
        path: String,
        /// The title, when somebody typed it. Absent means the label is the
        /// one derived from the path, which the restore recomputes rather than
        /// storing a copy of something that could go stale.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// `pinned` is written only when set, for the reason `title` is written
    /// only when typed: an unpinned tab still writes what it always did.
    Editor {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pinned: bool,
    },
    Audio {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pinned: bool,
    },
    /// A terminal. The fields are for the ket host, which keeps terminals
    /// running across a restart: the terminal's key with it, the agent it ran
    /// a typed title and whether it is pinned. All optional and all skipped
    /// when absent, so a
    /// layout written without them still loads — and an older build reading
    /// one written with them ignores them rather than rejecting the layout.
    Terminal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pinned: bool,
    },
}

/// A terminal tab a saved layout wants back, in a worktree not opened since.
///
/// Only with the ket host (`ket_core::host`), which may still be running it:
/// the tab asks for its terminal by key when the worktree is opened, and a
/// terminal the host no longer has is started afresh in its place. Without
/// the host a terminal cannot outlive the app, and the old rule — the pane is
/// owed one shell — still applies.
#[derive(Debug, Clone)]
pub(crate) struct SavedTerminal {
    pub(crate) pane: PaneId,
    pub(crate) key: String,
    pub(crate) agent: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) pinned: bool,
}

impl SavedTerminal {
    fn tab(&self) -> SavedTab {
        SavedTab::Terminal {
            key: Some(self.key.clone()),
            agent: self.agent.clone(),
            title: self.title.clone(),
            pinned: self.pinned,
        }
    }
}

/// What restoring a space yields besides the space itself.
#[derive(Debug, Default)]
struct Owed {
    /// Panes owed one fresh shell.
    shells: Vec<PaneId>,
    /// Terminal tabs to reattach — see [`SavedTerminal`].
    terminals: Vec<SavedTerminal>,
}

impl SavedSpace {
    /// `owed` are panes this space is still short a shell for, so a debt
    /// outlives a save it was never paid across. Selecting one worktree
    /// persists every worktree of its project, and an owed pane of a worktree
    /// nobody has been to yet is empty — without this it would be written out
    /// as an ordinary empty pane and pruned by the next launch, losing the
    /// arrangement after a single restart in which it was not visited.
    fn capture(space: &Space, shell: &Shell, owed: &[PaneId], restores: &[SavedTerminal]) -> Self {
        Self {
            root: SavedPane::capture(&space.root, shell, owed, restores),
            focused: space.focused.0,
            next_id: space.next_id,
        }
    }

    /// Rebuilds a space, and says which of its panes are owed a shell.
    ///
    /// A pty does not survive the process that ran it, so a terminal tab
    /// cannot come back — but the pane it was in has to, or splitting a
    /// worktree's shell in two is an arrangement that is lost on every
    /// restart. Those panes are named here and filled by
    /// `Shell::select`, once the worktree is actually opened; starting a shell
    /// for every worktree in every project at launch would be a great many
    /// processes nobody has asked to see.
    #[cfg(test)]
    fn restore(self) -> Result<(Space, Vec<PaneId>), String> {
        self.restore_with(false)
            .map(|(space, owed)| (space, owed.shells))
    }

    /// As [`Self::restore`], and with `hosted` — the ket host in use — keyed
    /// terminal tabs are reported for reattaching instead of as shells owed.
    fn restore_with(self, hosted: bool) -> Result<(Space, Owed), String> {
        let mut ids = HashSet::new();
        let mut owed = Owed::default();
        let root = self.root.restore(&mut ids, &mut owed, hosted)?;
        if !ids.contains(&self.focused) {
            return Err("focused pane does not exist".to_owned());
        }
        let largest = ids.iter().copied().max().unwrap_or(0);
        let mut space = Space {
            root,
            focused: PaneId(self.focused),
            next_id: self.next_id.max(largest.saturating_add(1)),
        };
        // What is left empty and owed nothing goes: a pane of browser tabs,
        // which are deliberately not persisted at all, would otherwise come
        // back as a `+` button and the words "select a worktree". `next_id` is
        // left where it is — the ids of the panes dropped here are spent, and
        // reusing one would hand a stale layout's identity to a pane somebody
        // splits off later.
        let keep: Vec<PaneId> = owed
            .shells
            .iter()
            .copied()
            .chain(owed.terminals.iter().map(|t| t.pane))
            .collect();
        space.prune_empty_panes(&keep);
        // Anything pruned anyway — a pane owed a shell that lost a fight with
        // the last-leaf rule — must not be reported, or a terminal would be
        // opened in a pane that is no longer there.
        owed.shells.retain(|pane| space.contains_pane(*pane));
        owed.terminals.retain(|t| space.contains_pane(t.pane));
        Ok((space, owed))
    }
}

impl SavedPane {
    fn capture(pane: &Pane, shell: &Shell, owed: &[PaneId], restores: &[SavedTerminal]) -> Self {
        match pane {
            Pane::Leaf { id, tabs, active } => {
                let mut saved: Vec<SavedTab> = tabs
                    .iter()
                    .filter_map(|tab| SavedTab::capture(tab, shell))
                    .collect();
                // Terminals still waiting to be reattached, for the same reason.
                saved.extend(
                    restores
                        .iter()
                        .filter(|t| t.pane == *id)
                        .map(SavedTerminal::tab),
                );
                // Still owed a shell and still without one: write the debt back
                // out, so it survives a save made before it could be paid.
                if saved.is_empty() && owed.contains(id) {
                    saved.push(SavedTab::Terminal {
                        key: None,
                        agent: None,
                        title: None,
                        pinned: false,
                    });
                }
                Self::Leaf {
                    id: id.0,
                    active: if saved.is_empty() {
                        0
                    } else {
                        (*active).min(saved.len() - 1)
                    },
                    tabs: saved,
                }
            }
            Pane::Split {
                axis,
                children,
                sizes,
            } => Self::Split {
                axis: match axis {
                    Axis::Horizontal => SavedAxis::Horizontal,
                    Axis::Vertical => SavedAxis::Vertical,
                },
                children: children
                    .iter()
                    .map(|child| Self::capture(child, shell, owed, restores))
                    .collect(),
                sizes: sizes.clone(),
            },
        }
    }

    fn restore(
        self,
        ids: &mut HashSet<u64>,
        owed: &mut Owed,
        hosted: bool,
    ) -> Result<Pane, String> {
        match self {
            Self::Leaf { id, tabs, active } => {
                if !ids.insert(id) {
                    return Err("pane id is duplicated".to_owned());
                }
                // Noted before the tabs are filtered, because filtering is
                // exactly what loses the terminals this is about.
                let mut wants_shell = false;
                for tab in &tabs {
                    if let SavedTab::Terminal {
                        key,
                        agent,
                        title,
                        pinned,
                    } = tab
                    {
                        match key {
                            Some(key) if hosted => owed.terminals.push(SavedTerminal {
                                pane: PaneId(id),
                                key: key.clone(),
                                // Layouts saved before a tab's terminal had
                                // started lost its agent; the key still names it.
                                agent: agent.clone().or_else(|| agent_in_key(key)),
                                title: title.clone(),
                                pinned: *pinned,
                            }),
                            _ => wants_shell = true,
                        }
                    }
                }
                if wants_shell {
                    owed.shells.push(PaneId(id));
                }
                let mut tabs: Vec<Tab> = tabs.into_iter().filter_map(SavedTab::restore).collect();
                // Pinned tabs lead the strip, and a layout that says otherwise
                // was not written by this build; a stable sort puts them back
                // at the head in the order they were saved.
                tabs.sort_by_key(|tab| !tab.pinned);
                Ok(Pane::Leaf {
                    id: PaneId(id),
                    active: if tabs.is_empty() {
                        0
                    } else {
                        active.min(tabs.len() - 1)
                    },
                    tabs,
                })
            }
            Self::Split {
                axis,
                children,
                sizes,
            } => {
                if children.len() < 2 || children.len() != sizes.len() {
                    return Err("split shape is invalid".to_owned());
                }
                if sizes.iter().any(|size| !size.is_finite() || *size <= 0.0) {
                    return Err("split size is invalid".to_owned());
                }
                let children = children
                    .into_iter()
                    .map(|child| child.restore(ids, owed, hosted))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Pane::Split {
                    axis: match axis {
                        SavedAxis::Horizontal => Axis::Horizontal,
                        SavedAxis::Vertical => Axis::Vertical,
                    },
                    children,
                    sizes,
                })
            }
        }
    }
}

impl SavedTab {
    fn capture(tab: &Tab, shell: &Shell) -> Option<Self> {
        // Only a title somebody typed is worth saving: every other one is
        // derived from the path, and a stored copy of a derived name is one
        // more thing that can go stale.
        let title = tab.renamed.then(|| tab.title.to_string());
        let pinned = tab.pinned;
        match &tab.kind {
            TabKind::Editor(key) => shell
                .editors
                .get(key.as_ref())
                .and_then(|state| state.saved_path())
                .map(|path| Self::Editor {
                    path: path.display().to_string(),
                    title,
                    pinned,
                }),
            TabKind::Audio(path) => Some(Self::Audio {
                path: path.to_string(),
                title,
                pinned,
            }),
            TabKind::Terminal(terminal) => {
                let key = shell.terminal_keys.get(terminal).cloned();
                // A tab is saved the moment it opens, before its terminal has
                // started and has a handle to say whose it is. Its key was
                // made from the agent it opens, so that answers until then —
                // without it, quitting early wrote the agent tab out as a
                // shell, and the next launch opened a second session beside it.
                let agent = shell
                    .terminals
                    .get(terminal)
                    .and_then(|handle| handle.agent.clone())
                    .or_else(|| key.as_deref().and_then(agent_in_key));
                Some(Self::Terminal {
                    key,
                    agent,
                    title,
                    pinned,
                })
            }
            // Browser sessions are deliberately private and process-local.
            TabKind::Browser(_) => None,
            #[cfg(target_os = "macos")]
            TabKind::IosSimulator => None,
            TabKind::Android => None,
            // A diff is a view of a moment; the Git view reopens one in a click.
            TabKind::Diff(_) => None,
        }
    }

    fn restore(self) -> Option<Tab> {
        match self {
            // The review pane was removed after its entry point disappeared.
            // Accept its saved variants so an old layout remains valid, but
            // drop the tabs the same way process-local terminals and browsers
            // are dropped during restoration.
            Self::AllChanges | Self::File { .. } => None,
            Self::Editor {
                path,
                title,
                pinned,
            } => Some(named(title, pinned, path, TabKind::Editor)),
            Self::Audio {
                path,
                title,
                pinned,
            } => Some(named(title, pinned, path, TabKind::Audio)),
            // A terminal does not survive the process that ran it, so there
            // is nothing on the other side of a restart to restore. Keeping
            // the variant means an older layout still loads; returning `None`
            // means it no longer leaves a dead tab behind when it does. A
            // renamed terminal loses its name with it, which is the same thing
            // happening to the tab rather than a second loss.
            Self::Terminal { .. } => None,
        }
    }
}

/// The agent a terminal key names, if it names one: keys are made as
/// `worktree|agent|stamp`, with `shell` in the middle for a plain shell.
fn agent_in_key(key: &str) -> Option<String> {
    key.rsplit('|')
        .nth(1)
        .filter(|name| !name.is_empty() && *name != "shell" && key.matches('|').count() >= 2)
        .map(str::to_owned)
}

/// A restored tab wearing the title that was saved for it, or the one its path
/// gives it.
fn named(
    title: Option<String>,
    pinned: bool,
    path: String,
    kind: fn(SharedString) -> TabKind,
) -> Tab {
    Tab {
        renamed: title.is_some(),
        pinned,
        title: title.map_or_else(|| filename(&path), SharedString::from),
        kind: kind(path.into()),
    }
}

fn filename(path: &str) -> SharedString {
    path.rsplit('/').next().unwrap_or(path).to_owned().into()
}

impl Shell {
    /// Saves the selected project's spaces, dropping non-selected worktrees if
    /// the opaque blob reaches core's bound. The visible worktree is attempted
    /// first; if it alone is oversized, the empty persisted project restores
    /// that worktree to the safe single-pane default on the next launch.
    pub(crate) fn persist_layout(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(project) = self.projects.get(selection.project) else {
            return;
        };
        let project_id = project.id.clone();
        let selected = self.selected_id();
        let mut candidates: Vec<WorktreeId> = project
            .worktrees
            .iter()
            .map(|worktree| worktree.id.clone())
            .filter(|id| self.spaces.contains_key(id))
            .collect();
        candidates.sort_by_key(|id| Some(id) != selected.as_ref());

        let mut worktrees = Vec::new();
        for id in candidates {
            let Some(space) = self.spaces.get(&id) else {
                continue;
            };
            let owed = self.owed_terminals.get(&id).map_or(&[][..], Vec::as_slice);
            worktrees.push(WorktreeLayout {
                space: SavedSpace::capture(
                    space,
                    self,
                    owed,
                    self.owed_restores.get(&id).map_or(&[][..], Vec::as_slice),
                ),
                id,
            });
            let probe = ProjectLayout {
                version: LAYOUT_VERSION,
                worktrees,
            };
            let within_bound = fits(&probe);
            worktrees = probe.worktrees;
            if !within_bound {
                worktrees.pop();
                break;
            }
        }

        let value = match serde_json::to_value(ProjectLayout {
            version: LAYOUT_VERSION,
            worktrees,
        }) {
            Ok(value) => value,
            Err(error) => {
                self.note = Some(format!("could not encode the layout: {error}").into());
                return;
            }
        };
        match Workspace::open().and_then(|workspace| workspace.set_layout(&project_id, Some(value)))
        {
            Ok(()) => {}
            Err(error) => self.note = Some(format!("could not save the layout: {error}").into()),
        }
    }

    /// Restores every valid project layout independently, so one corrupt blob
    /// cannot prevent the application or another project from opening.
    pub(crate) fn restore_layouts(&mut self, workspace: &Workspace) {
        let mut restored = HashMap::new();
        let mut warning = None;
        for project in &self.projects {
            let state = match workspace.project_state(&project.id) {
                Ok(state) => state,
                Err(error) => {
                    warning = Some(format!("could not restore layout: {error}"));
                    continue;
                }
            };
            let Some(value) = state.layout else { continue };
            let layout = match serde_json::from_value::<ProjectLayout>(value) {
                Ok(layout) if layout.version == LAYOUT_VERSION => layout,
                Ok(_) => {
                    warning = Some("layout version is unsupported; using defaults".to_owned());
                    continue;
                }
                Err(error) => {
                    warning = Some(format!("layout is invalid; using defaults: {error}"));
                    continue;
                }
            };
            let live: HashSet<&WorktreeId> =
                project.worktrees.iter().map(|node| &node.id).collect();
            for item in layout.worktrees {
                if !live.contains(&item.id) {
                    continue;
                }
                match item.space.restore_with(ket_core::host::enabled()) {
                    Ok((space, owed)) => {
                        if !owed.shells.is_empty() {
                            self.owed_terminals.insert(item.id.clone(), owed.shells);
                        }
                        if !owed.terminals.is_empty() {
                            self.owed_restores.insert(item.id.clone(), owed.terminals);
                        }
                        restored.insert(item.id, space);
                    }
                    Err(error) => {
                        warning = Some(format!("layout is invalid; using defaults: {error}"))
                    }
                }
            }
        }
        self.spaces.extend(restored);
        if let Some(warning) = warning {
            self.note = Some(warning.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nontrivial_space_round_trips_through_json() {
        let saved = SavedSpace {
            root: SavedPane::Split {
                axis: SavedAxis::Horizontal,
                children: vec![
                    SavedPane::Leaf {
                        id: 0,
                        tabs: vec![SavedTab::Editor {
                            path: "README.md".to_owned(),
                            title: None,
                            pinned: false,
                        }],
                        active: 0,
                    },
                    SavedPane::Split {
                        axis: SavedAxis::Vertical,
                        children: vec![
                            // A pane of nothing but a terminal. It restores
                            // with no tabs, since a pty does not survive the
                            // process — but the pane is kept and reported as
                            // owed a shell, which is why the leaf count below
                            // is still three.
                            SavedPane::Leaf {
                                id: 1,
                                tabs: vec![SavedTab::Terminal {
                                    key: None,
                                    agent: None,
                                    title: None,
                                    pinned: false,
                                }],
                                active: 0,
                            },
                            SavedPane::Leaf {
                                id: 2,
                                tabs: vec![SavedTab::Editor {
                                    path: "src/main.rs".to_owned(),
                                    title: None,
                                    pinned: false,
                                }],
                                active: 0,
                            },
                        ],
                        sizes: vec![0.4, 0.6],
                    },
                ],
                sizes: vec![0.3, 0.7],
            },
            focused: 2,
            next_id: 3,
        };
        let json = serde_json::to_string(&saved).unwrap();
        let restored: SavedSpace = serde_json::from_str(&json).unwrap();
        let (restored, owed) = restored.restore().unwrap();
        assert_eq!(restored.focused, PaneId(2));
        assert_eq!(restored.next_id, 3);
        assert_eq!(restored.leaf_count(), 3);
        assert_eq!(owed, vec![PaneId(1)], "the terminal's pane wants a shell");
    }

    #[test]
    fn a_corrupt_split_falls_back_as_an_error_instead_of_panicking() {
        let saved = SavedSpace {
            root: SavedPane::Split {
                axis: SavedAxis::Horizontal,
                children: Vec::new(),
                sizes: Vec::new(),
            },
            focused: 0,
            next_id: 1,
        };
        assert!(saved.restore().is_err());
    }

    #[test]
    fn an_oversized_project_layout_is_detected_without_panicking() {
        let layout = ProjectLayout {
            version: LAYOUT_VERSION,
            worktrees: vec![WorktreeLayout {
                id: WorktreeId::new("large"),
                space: SavedSpace {
                    root: SavedPane::Leaf {
                        id: 0,
                        tabs: vec![SavedTab::File {
                            path: "x".repeat(MAX_LAYOUT_BYTES),
                            title: None,
                        }],
                        active: 0,
                    },
                    focused: 0,
                    next_id: 1,
                },
            }],
        };
        assert!(!fits(&layout));
    }
}
