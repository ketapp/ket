//! Native application menus and their state.

use gpui::{Context, Menu, MenuItem, SystemMenuType};
use ket_core::id::{ProjectId, WorktreeId};

/// A project in a native switcher; its id survives list reordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuProject {
    pub(crate) id: ProjectId,
    pub(crate) name: String,
}

/// A worktree in a native switcher; its id survives list reordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuWorktree {
    pub(crate) id: WorktreeId,
    pub(crate) name: String,
}

/// Switch a project by stable id rather than its position when the menu opened.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = shell, no_json)]
pub(crate) struct SwitchProject {
    pub(crate) id: ProjectId,
}

/// Switch a worktree by stable id rather than its position when the menu opened.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = shell, no_json)]
pub(crate) struct SwitchWorktree {
    pub(crate) id: WorktreeId,
}

/// Start a named runnable agent, checked again when selected.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = shell, no_json)]
pub(crate) struct LaunchAgent {
    pub(crate) name: String,
}

/// A device launcher offered by this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuDevice {
    Ios,
    Android,
}

/// Open a device tab after checking the launcher is still available.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = shell, no_json)]
pub(crate) struct LaunchDevice {
    pub(crate) device: MenuDevice,
}

/// Open the focused file in a detected external application.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = shell, no_json)]
pub(crate) struct OpenInExternal {
    pub(crate) program: Option<std::path::PathBuf>,
}

/// Change Economy for a specific selected worktree and active-pack level.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = shell, no_json)]
pub(crate) struct SetEconomy {
    pub(crate) worktree: WorktreeId,
    pub(crate) level: u8,
}

/// One Economy level from the active policy pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuEconomyLevel {
    pub(crate) level: u8,
    pub(crate) name: String,
}

/// A detected editor shown by the File menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuEditor {
    pub(crate) label: String,
    pub(crate) program: std::path::PathBuf,
}

use crate::ProjectStorage;
use crate::SendFeedback;
use crate::ToggleEditorWrap;
use crate::{
    AboutKet, BringAllToFront, CloseWindow, FocusPaneAbove, FocusPaneBelow, FocusPaneLeft,
    FocusPaneRight, HideKet, HideOthers, ShowAll,
};
use crate::{
    ActualSize, AddProject, AddToBacklog, ChangeTabTitle, ClearBuildOutput, CliReference,
    CloseOtherTabs, ClosePane, CloseTab, CloseTabsLeft, CloseTabsRight, CommitAndMerge,
    CopyRelativeTabPath, CopyTabPath, CopyWorktreePath, DeleteWorktree, FindFile, FindInTab,
    KetHelp, KeyboardShortcuts, MinimizeWindow, NewBlankDocument, NewBrowserTab, NewTerminalTab,
    NewWorktree, NextTab, OpenBacklog, OpenCommandPalette, OpenPullRequest, OpenQuickPrompt,
    OpenSettings, PreviousTab, ProjectSettings, Quit, RefreshProjects, ReleaseNotes, RemoveProject,
    RenameWorktree, ReportIssue, SaveDocument, SearchProjects, SearchWorktree, SendSnippet,
    ShowFilesPanel, ShowGitPanel, ShowSearchPanel, SplitDown, SplitRight,
    ToggleDiscoveredWorktrees, ToggleFullScreen, ToggleRightPanel, ToggleSidebar, ToggleStatusBar,
    ToggleTabPin, ToggleWorktreePin, ViewAvailableReleases, ZoomIn, ZoomOut, ZoomWindow,
};
use crate::{EconomySavings, RevealWorktree};
#[cfg(target_os = "macos")]
use crate::{EditCopy, EditCut, EditPaste, EditRedo, EditSelectAll, EditUndo};
use crate::{FindNext, FindPrevious};

/// State that controls native labels, checkmarks, and item availability.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuState {
    pub(crate) status_bar_open: bool,
    pub(crate) sidebar_open: bool,
    pub(crate) panel_open: bool,
    pub(crate) panel_view: crate::panel::PanelView,
    pub(crate) full_screen: bool,
    pub(crate) modal_open: bool,
    pub(crate) selected_project: Option<ProjectId>,
    pub(crate) selected_worktree: Option<WorktreeId>,
    pub(crate) has_tab: bool,
    pub(crate) has_multiple_panes: bool,
    pub(crate) can_save: bool,
    pub(crate) can_find: bool,
    pub(crate) can_native_edit: bool,
    pub(crate) can_new_worktree: bool,
    pub(crate) projects: Vec<MenuProject>,
    pub(crate) worktrees: Vec<MenuWorktree>,
    pub(crate) hide_discovered: bool,
    pub(crate) worktree_primary: bool,
    pub(crate) worktree_missing: bool,
    pub(crate) worktree_pinned: bool,
    pub(crate) merge_ready: bool,
    pub(crate) has_build_output: bool,
    pub(crate) agents: Vec<String>,
    pub(crate) ios_available: bool,
    pub(crate) android_available: bool,
    pub(crate) can_open_in: bool,
    pub(crate) editors: Vec<MenuEditor>,
    pub(crate) economy_level: Option<u8>,
    pub(crate) economy_levels: Vec<MenuEconomyLevel>,
    pub(crate) editor_wrap: Option<bool>,
}

impl MenuState {
    pub(crate) fn from_shell(shell: &crate::Shell) -> Self {
        let selection = shell.selection;
        let project = selection.and_then(|s| shell.projects.get(s.project));
        let worktree = selection.and_then(|s| project?.worktrees.get(s.worktree));
        let space = shell.space();
        Self {
            status_bar_open: shell.status_bar_open,
            sidebar_open: shell.sidebar_open,
            panel_open: shell.panel_open,
            panel_view: shell.panel_view,
            full_screen: shell.menu_fullscreen,
            modal_open: shell.modal_open(),
            selected_project: project.map(|node| node.id.clone()),
            selected_worktree: worktree.map(|node| node.id.clone()),
            has_tab: space.is_some_and(|space| space.active_tab().is_some()),
            has_multiple_panes: space.is_some_and(|space| space.pane_count() > 1),
            can_save: shell.editor_active()
                || space.is_some_and(|space| space.ios_simulator_focused()),
            can_find: shell.editor_active() || shell.active_terminal_id().is_some(),
            can_native_edit: shell.editor_active()
                || shell.active_browser_tab().is_some()
                || shell.modal_open(),
            can_new_worktree: shell
                .active_terminal_id()
                .and_then(|terminal| shell.project_of_terminal(terminal))
                .is_some()
                || shell.projects.iter().any(|project| project.expanded),
            projects: shell
                .projects
                .iter()
                .map(|node| MenuProject {
                    id: node.id.clone(),
                    name: node.name.to_string(),
                })
                .collect(),
            worktrees: project
                .map(|node| {
                    node.worktrees
                        .iter()
                        .map(|worktree| MenuWorktree {
                            id: worktree.id.clone(),
                            name: worktree.label().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            hide_discovered: project.is_some_and(|node| node.hide_discovered),
            worktree_primary: worktree.is_some_and(|node| node.primary),
            worktree_missing: worktree.is_some_and(|node| node.missing),
            worktree_pinned: worktree.is_some_and(|node| node.pinned),
            merge_ready: worktree.is_some_and(|node| {
                !node.primary && !node.missing && node.dirty && node.in_progress.is_none()
            }) && shell.merging.is_none(),
            has_build_output: worktree
                .and_then(|node| shell.footprint(&node.id))
                .is_some_and(|size| size.build > 0),
            agents: shell.menu_agents.clone(),
            ios_available: shell.menu_ios_available,
            android_available: shell.menu_android_available,
            can_open_in: shell.active_open_in_target().is_some(),
            editors: ket_core::surface::cached_installed_editors()
                .unwrap_or_default()
                .iter()
                .map(|found| MenuEditor {
                    label: found.profile.label.to_owned(),
                    program: found.program.clone(),
                })
                .collect(),
            economy_level: worktree.map(|node| node.token_reduction),
            economy_levels: ket_core::worktree::token_reduction_levels()
                .iter()
                .map(|level| MenuEconomyLevel {
                    level: level.level,
                    name: level.name().to_owned(),
                })
                .collect(),
            editor_wrap: shell.active_editor_wrap(),
        }
    }
}

impl crate::Shell {
    /// Refresh the tools offered by the dynamic File submenus.
    pub(crate) fn refresh_menu_tools(&mut self, cx: &mut Context<Self>) {
        if self.menu_tools_refreshing {
            return;
        }
        self.menu_tools_refreshing = true;
        let discovered = cx.background_executor().spawn(async {
            let agents = crate::terminal::runnable_specs()
                .into_iter()
                .map(|spec| spec.name)
                .collect();
            #[cfg(target_os = "macos")]
            let ios = crate::ios_simulator::xcode_installed();
            #[cfg(not(target_os = "macos"))]
            let ios = false;
            (agents, ios, ket_core::android::installed())
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Self>, cx| {
            let (agents, ios, android) = discovered.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.menu_agents = agents;
                shell.menu_ios_available = ios;
                shell.menu_android_available = android;
                shell.menu_tools_refreshing = false;
                shell.menu_dirty = true;
                cx.notify();
            });
        })
        .detach();
    }
    /// Select a project through the same worktree path as the sidebar.
    pub(crate) fn switch_project_id(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        let Some(project) = self.projects.iter().position(|node| &node.id == id) else {
            return;
        };
        let preferred = self.project_focus.get(id);
        let worktree = self.projects[project]
            .worktrees
            .iter()
            .position(|node| Some(&node.id) == preferred)
            .or_else(|| (!self.projects[project].worktrees.is_empty()).then_some(0));
        if let Some(worktree) = worktree {
            self.select(crate::tree::Selection { project, worktree }, cx);
        } else {
            self.projects[project].expanded = true;
            cx.notify();
        }
    }

    /// Resolve a worktree id at dispatch time so a stale menu cannot select another row.
    pub(crate) fn switch_worktree_id(&mut self, id: &WorktreeId, cx: &mut Context<Self>) {
        for (project, node) in self.projects.iter().enumerate() {
            if let Some(worktree) = node.worktrees.iter().position(|item| &item.id == id) {
                self.select(crate::tree::Selection { project, worktree }, cx);
                return;
            }
        }
    }

    /// Rebuild native menus after a change in selection or chrome visibility.
    pub(crate) fn refresh_native_menus(&mut self, cx: &mut Context<Self>) {
        let state = MenuState::from_shell(self);
        if self.menu_state.as_ref() != Some(&state) {
            cx.set_menus(app_menus(&state));
            self.menu_state = Some(state);
        }
        self.menu_dirty = false;
    }
}

/// The menu bar. macOS delivers Cmd-Q as a *menu* key equivalent, and gpui
/// installs no menu bar of its own — without one the chord reaches nothing
/// and the app cannot be quit from the keyboard at all.
///
/// Takes the current shell state for native labels and checkmarks.
pub(crate) fn app_menus(state: &MenuState) -> Vec<Menu> {
    let has_worktree = state.selected_worktree.is_some();
    let mut menus = vec![
        Menu {
            name: "ket".into(),
            items: vec![
                MenuItem::action("About ket", AboutKet),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("View Available Releases…", ViewAvailableReleases),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide ket", HideKet),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit ket", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Document", NewBlankDocument).enabled(has_worktree),
                MenuItem::action("New Terminal Tab", NewTerminalTab).enabled(has_worktree),
                MenuItem::action("New Browser Tab", NewBrowserTab).enabled(has_worktree),
                MenuItem::submenu(Menu {
                    name: "New Agent Session".into(),
                    items: state
                        .agents
                        .iter()
                        .enumerate()
                        .map(|(index, name)| {
                            let item =
                                MenuItem::action(name.clone(), LaunchAgent { name: name.clone() })
                                    .enabled(has_worktree);
                            match crate::shortcuts::agent_session_chord(index) {
                                Some(chord) => item.key_equivalent(chord),
                                None => item,
                            }
                        })
                        .collect(),
                }),
                MenuItem::submenu(Menu {
                    name: "New Device Tab".into(),
                    items: vec![
                        MenuItem::action(
                            "iOS Simulator",
                            LaunchDevice {
                                device: MenuDevice::Ios,
                            },
                        )
                        .enabled(has_worktree && state.ios_available),
                        MenuItem::action(
                            "Android Emulator",
                            LaunchDevice {
                                device: MenuDevice::Android,
                            },
                        )
                        .enabled(has_worktree && state.android_available),
                    ],
                }),
                MenuItem::separator(),
                MenuItem::action("Find File…", FindFile).enabled(has_worktree),
                MenuItem::submenu(Menu {
                    name: "Open In".into(),
                    items: state
                        .editors
                        .iter()
                        .map(|editor| {
                            MenuItem::action(
                                editor.label.clone(),
                                OpenInExternal {
                                    program: Some(editor.program.clone()),
                                },
                            )
                            .enabled(state.can_open_in)
                        })
                        .chain(std::iter::once(
                            MenuItem::action("Finder", OpenInExternal { program: None })
                                .enabled(state.can_open_in),
                        ))
                        .collect(),
                }),
                MenuItem::separator(),
                MenuItem::action("Save", SaveDocument).enabled(state.can_save),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab).enabled(state.has_tab),
                MenuItem::action("Close Other Tabs", CloseOtherTabs).enabled(state.has_tab),
                MenuItem::action("Close Tabs to the Right", CloseTabsRight).enabled(state.has_tab),
                MenuItem::action("Close Tabs to the Left", CloseTabsLeft).enabled(state.has_tab),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
        #[cfg(target_os = "macos")]
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", EditUndo, gpui::OsAction::Undo)
                    .enabled(state.can_native_edit),
                MenuItem::os_action("Redo", EditRedo, gpui::OsAction::Redo)
                    .enabled(state.can_native_edit),
                MenuItem::separator(),
                MenuItem::os_action("Cut", EditCut, gpui::OsAction::Cut)
                    .enabled(state.can_native_edit),
                MenuItem::os_action("Copy", EditCopy, gpui::OsAction::Copy),
                MenuItem::os_action("Paste", EditPaste, gpui::OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action("Select All", EditSelectAll, gpui::OsAction::SelectAll)
                    .enabled(state.can_native_edit),
                MenuItem::separator(),
                MenuItem::submenu(Menu {
                    name: "Find".into(),
                    items: vec![
                        MenuItem::action("Find in File / Find in Terminal", FindInTab)
                            .enabled(state.can_find),
                        MenuItem::action("Find Next", FindNext).enabled(state.can_find),
                        MenuItem::action("Find Previous", FindPrevious).enabled(state.can_find),
                        MenuItem::separator(),
                        MenuItem::action("Search Worktree…", SearchWorktree).enabled(has_worktree),
                    ],
                }),
                MenuItem::separator(),
                MenuItem::action("Copy Path", CopyTabPath).enabled(state.has_tab),
                MenuItem::action("Copy Relative Path", CopyRelativeTabPath).enabled(state.has_tab),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", OpenCommandPalette),
                MenuItem::action("Search Projects and Tabs…", SearchProjects),
                MenuItem::separator(),
                MenuItem::action("Show Sidebar", ToggleSidebar).checked(state.sidebar_open),
                MenuItem::action("Show Right Panel", ToggleRightPanel).checked(state.panel_open),
                MenuItem::submenu(Menu {
                    name: "Right Panel".into(),
                    items: vec![
                        MenuItem::action("Files", ShowFilesPanel)
                            .enabled(has_worktree)
                            .checked(
                                state.panel_open
                                    && state.panel_view == crate::panel::PanelView::Files,
                            ),
                        MenuItem::action("Search", ShowSearchPanel)
                            .enabled(has_worktree)
                            .checked(
                                state.panel_open
                                    && state.panel_view == crate::panel::PanelView::Search,
                            ),
                        MenuItem::action("Git", ShowGitPanel)
                            .enabled(has_worktree)
                            .checked(
                                state.panel_open
                                    && state.panel_view == crate::panel::PanelView::Git,
                            ),
                    ],
                }),
                MenuItem::action(
                    if state.status_bar_open {
                        "Hide Status Bar"
                    } else {
                        "Show Status Bar"
                    },
                    ToggleStatusBar,
                )
                .checked(state.status_bar_open),
                MenuItem::action("Wrap Editor Lines", ToggleEditorWrap)
                    .enabled(state.editor_wrap.is_some())
                    .checked(state.editor_wrap == Some(true)),
                MenuItem::separator(),
                // In the platform's own order — Safari's, Terminal's.
                MenuItem::action("Actual Size", ActualSize),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::separator(),
                MenuItem::action(
                    if state.full_screen {
                        "Exit Full Screen"
                    } else {
                        "Enter Full Screen"
                    },
                    ToggleFullScreen,
                ),
            ],
        },
        Menu {
            name: "Project".into(),
            items: vec![
                MenuItem::action("Add Project…", AddProject),
                MenuItem::submenu(Menu {
                    name: "Switch Project".into(),
                    items: state
                        .projects
                        .iter()
                        .map(|project| {
                            MenuItem::action(
                                project.name.clone(),
                                SwitchProject {
                                    id: project.id.clone(),
                                },
                            )
                            .checked(state.selected_project.as_ref() == Some(&project.id))
                        })
                        .collect(),
                }),
                MenuItem::separator(),
                MenuItem::action("Project Settings…", ProjectSettings)
                    .enabled(state.selected_project.is_some()),
                MenuItem::action("Open Backlog", OpenBacklog)
                    .enabled(state.selected_project.is_some()),
                MenuItem::action("Add to Backlog…", AddToBacklog)
                    .enabled(state.selected_project.is_some()),
                MenuItem::action("Storage…", ProjectStorage)
                    .enabled(state.selected_project.is_some()),
                MenuItem::action("Economy Savings…", EconomySavings)
                    .enabled(state.selected_project.is_some()),
                MenuItem::action("Show Non-ket Worktrees", ToggleDiscoveredWorktrees)
                    .checked(!state.hide_discovered)
                    .enabled(state.selected_project.is_some()),
                MenuItem::action("Refresh Projects", RefreshProjects),
                MenuItem::separator(),
                MenuItem::action("Remove Project…", RemoveProject)
                    .enabled(state.selected_project.is_some()),
            ],
        },
        Menu {
            name: "Worktree".into(),
            items: vec![
                MenuItem::action("New Worktree…", NewWorktree).enabled(state.can_new_worktree),
                MenuItem::submenu(Menu {
                    name: "Switch Worktree".into(),
                    items: state
                        .worktrees
                        .iter()
                        .map(|worktree| {
                            MenuItem::action(
                                worktree.name.clone(),
                                SwitchWorktree {
                                    id: worktree.id.clone(),
                                },
                            )
                            .checked(state.selected_worktree.as_ref() == Some(&worktree.id))
                        })
                        .collect(),
                }),
                MenuItem::action("Quick Prompt…", OpenQuickPrompt).enabled(has_worktree),
                MenuItem::action("Send Snippet…", SendSnippet).enabled(has_worktree),
                MenuItem::submenu(Menu {
                    name: "Economy".into(),
                    items: state
                        .selected_worktree
                        .iter()
                        .flat_map(|id| {
                            state.economy_levels.iter().map(move |level| {
                                MenuItem::action(
                                    level.name.clone(),
                                    SetEconomy {
                                        worktree: id.clone(),
                                        level: level.level,
                                    },
                                )
                                .checked(state.economy_level == Some(level.level))
                                .enabled(!state.worktree_primary && !state.worktree_missing)
                            })
                        })
                        .collect(),
                }),
                MenuItem::separator(),
                MenuItem::action("Review Changes", ShowGitPanel).enabled(has_worktree),
                MenuItem::action("Commit and Merge…", CommitAndMerge).enabled(state.merge_ready),
                MenuItem::action("Push and Open Pull Request…", OpenPullRequest)
                    .enabled(has_worktree && !state.worktree_primary && !state.worktree_missing),
                MenuItem::separator(),
                MenuItem::action("Rename Worktree…", RenameWorktree)
                    .enabled(has_worktree && !state.worktree_primary && !state.worktree_missing),
                MenuItem::action(
                    if state.worktree_pinned {
                        "Unpin Worktree"
                    } else {
                        "Pin Worktree"
                    },
                    ToggleWorktreePin,
                )
                .enabled(has_worktree && !state.worktree_primary),
                MenuItem::action("Copy Worktree Path", CopyWorktreePath).enabled(has_worktree),
                MenuItem::action("Reveal in Finder", RevealWorktree)
                    .enabled(has_worktree && !state.worktree_missing),
                MenuItem::action("Clear Build Output…", ClearBuildOutput)
                    .enabled(state.has_build_output),
                MenuItem::separator(),
                MenuItem::action("Delete Worktree…", DeleteWorktree)
                    .enabled(has_worktree && !state.worktree_primary),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", MinimizeWindow),
                MenuItem::action("Zoom", ZoomWindow),
                MenuItem::action("Bring All to Front", BringAllToFront),
                MenuItem::separator(),
                MenuItem::action("Previous Tab", PreviousTab).enabled(state.has_tab),
                MenuItem::action("Next Tab", NextTab).enabled(state.has_tab),
                MenuItem::action("Pin/Unpin Tab", ToggleTabPin).enabled(state.has_tab),
                MenuItem::action("Change Tab Title…", ChangeTabTitle).enabled(state.has_tab),
                MenuItem::separator(),
                MenuItem::action("Split Pane Right", SplitRight).enabled(has_worktree),
                MenuItem::action("Split Pane Down", SplitDown).enabled(has_worktree),
                MenuItem::submenu(Menu {
                    name: "Focus Pane".into(),
                    items: vec![
                        MenuItem::action("Left", FocusPaneLeft).enabled(state.has_multiple_panes),
                        MenuItem::action("Right", FocusPaneRight).enabled(state.has_multiple_panes),
                        MenuItem::action("Above", FocusPaneAbove).enabled(state.has_multiple_panes),
                        MenuItem::action("Below", FocusPaneBelow).enabled(state.has_multiple_panes),
                    ],
                }),
                MenuItem::action("Close Pane", ClosePane).enabled(state.has_multiple_panes),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("ket Help", KetHelp),
                MenuItem::action("Keyboard Shortcuts", KeyboardShortcuts),
                MenuItem::action("CLI Reference", CliReference),
                MenuItem::action("Release Notes", ReleaseNotes),
                MenuItem::separator(),
                MenuItem::action("Send Feedback…", SendFeedback),
                MenuItem::action("Report an Issue…", ReportIssue),
            ],
        },
    ];
    if state.modal_open {
        for menu in &mut menus {
            disable_behind_modal(&mut menu.items);
        }
    }
    menus
}

fn disable_behind_modal(items: &mut [MenuItem]) {
    for item in items {
        match item {
            MenuItem::Action {
                action, enabled, ..
            } => {
                let action = action.as_any();
                #[cfg(target_os = "macos")]
                let edit_field = action.is::<EditUndo>()
                    || action.is::<EditRedo>()
                    || action.is::<EditCut>()
                    || action.is::<EditCopy>()
                    || action.is::<EditPaste>()
                    || action.is::<EditSelectAll>();
                #[cfg(not(target_os = "macos"))]
                let edit_field = false;
                let application = action.is::<Quit>()
                    || action.is::<HideKet>()
                    || action.is::<HideOthers>()
                    || action.is::<ShowAll>();
                let window = action.is::<MinimizeWindow>()
                    || action.is::<ZoomWindow>()
                    || action.is::<ToggleFullScreen>();
                if !edit_field && !application && !window {
                    *enabled = false;
                }
            }
            MenuItem::Submenu(menu) => disable_behind_modal(&mut menu.items),
            MenuItem::Separator | MenuItem::SystemMenu(_) => {}
        }
    }
}
