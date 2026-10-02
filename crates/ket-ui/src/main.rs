//! ket's native shell.
//!
//! A window over `ket-core`. The sidebar is the primary interaction — a tree of
//! project → worktree that you live in, rather than a switcher you summon — and
//! the content area holds that worktree's terminals, editors and previews.
//!
//! **On scrolling.** Large file and search views use virtualised lists rather
//! than tall stacks of elements in a scroll container. `uniform_list` asks only
//! for the visible range, which keeps per-frame work proportional to the
//! viewport rather than the document or result set.
//!
//! **On colour.** Every colour comes from a [`Theme`] token, never a literal at
//! the call site. A renderer naming `0x7ee08a` cannot be re-themed; one naming
//! `theme.status.running` can. That rule is only cheap while the shell is small.
//!
//! **On tabs.** Each worktree is a *space* with its own tabs, so switching
//! worktrees swaps the tab strip rather than accumulating everything you have
//! ever opened into one row. They live in memory for now; persisting them
//! belongs in the per-project layout blob from Epic 2, which is what
//! `MAX_LAYOUT_BYTES` exists to bound.
//!
//! **On the terminal.** `Ctrl-\`` opens a worktree's terminal tab; see
//! `crate::terminal` for how its pane stays fast (only the visible screen is
//! ever read) and for what it deliberately leaves out. Still to come in
//! Epic 5: splits, and agent sessions as a third level in the tree.

mod agent_override;
mod android_emulator;
mod appearance;
mod audio;
mod backlog;
mod browser;
// The workspace's fifth `unsafe` carve-out; see its lint block and the
// module doc for why.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod browser_snapshot;
mod clear_build;
mod commands;
mod device_capture;
mod diff_view;
mod discard_changes;
mod editor;
mod explorer;
mod feedback;
mod fonts;
mod frametrace;
mod git_panel;
mod git_watch;
mod handset;
mod header;
mod horizon;
mod input;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod ios_simulator;
mod layout;
mod merge_worktree;
mod motion;
mod orbit;
mod paint;
mod palette;
mod panel;
mod phone_work;
mod phones;
mod preferences;
mod projects;
mod projects_view;
mod quick_prompt;
mod region;
mod release;
mod savings;
mod search;
mod shortcuts;
mod snippet_picker;
mod snippets;
mod status_bar;
mod storage;
mod tabs;
mod terminal;
mod toasts;
mod tree;
mod ui;
mod usage_card;
mod view;
// The workspace's third `unsafe` carve-out; see its lint block and the
// module doc for why.
#[allow(unsafe_code)]
mod voice;
mod window_drag;
mod workspace_search;
mod worktree_dialog;
mod worktree_menu;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, KeyBinding, KeyDownEvent, Menu,
    MenuItem, MouseButton, Pixels, SharedString, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowOptions, actions, deferred, div, point, prelude::*, px, size,
};

use ket_core::activity::Tracker;
use ket_core::agent_hooks::Listener;
use ket_core::command::Registry;
use ket_core::config::{Config, ThemeConfig};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::keybinding::Keymap;
use ket_core::rate_limits::{BackgroundRefresh, RateLimitCache};
use ket_core::theme::Theme;
use ket_core::workspace::Workspace;

use crate::audio::AudioEngine;
use crate::browser::{BrowserHandle, BrowserId};
use crate::commands::Outbox;
use crate::editor::EditorState;
use crate::explorer::{Explorer, Finder, PanelResize};
use crate::input::Searches;
#[cfg(target_os = "macos")]
use crate::ios_simulator::IosSimulatorHandle;
use crate::paint::paint;
use crate::palette::Palette;
use crate::projects::{AddProjectDialog, ProjectMenu, RemoveConfirm, SettingsDialog};
use crate::shortcuts::{GlobalShortcut, global_shortcut};
use crate::tabs::{PaneId, Space, Toward};
use crate::terminal::{TerminalHandle, TerminalId};
use crate::tree::{MIN_CONTENT_SIZE, ProjectNode, Selection, SidebarResize};
use crate::workspace_search::WorkspaceSearch;

actions!(
    shell,
    [
        /// Quit the application. Dispatched by the app menu's Quit item — the
        /// key equivalent macOS resolves Cmd-Q against — and by the key
        /// binding in `main` on platforms where no menu has claimed the chord.
        Quit,
        /// Open the global settings view. Dispatched by the app menu's
        /// Settings item — the key equivalent macOS resolves ⌘, against — and
        /// handled on the shell's root element, which is where it can reach
        /// the shell at all.
        OpenSettings,
        /// Close the active tab without closing the application window.
        CloseTab,
        /// Open a fresh terminal tab in the focused pane.
        NewTerminalTab,
        /// Open a new untitled text editor in the focused pane.
        NewBlankDocument,
        /// Show or hide the status bar along the bottom of the window.
        /// Dispatched by the View menu, whose item says which it will do.
        ToggleStatusBar,
        /// The File menu's Add to Backlog: the ⇧⌘A panel. The chord is the
        /// shell's own — see `shortcuts::CAPTURE_CHORD` — and is taken there
        /// before this; the item is its way in when something native, a web
        /// page, has the keyboard and passes the chord on to the menu.
        AddToBacklog,
        /// The Edit menu's Cut — see [`EDIT_CONTEXT`]. A native item: AppKit
        /// sends `cut:` to whatever has the keyboard, which is how a browser
        /// pane's web page gets the chord. Nothing in ket handles the action
        /// itself; it arrives only when nothing that took the selector was
        /// focused, and then does nothing, as the chord always did.
        EditCut,
        /// The Edit menu's Copy: `copy:` to whatever has the keyboard. See
        /// [`EditCut`].
        EditCopy,
        /// The Edit menu's Paste: `paste:` to whatever has the keyboard. See
        /// [`EditCut`].
        EditPaste,
        /// The Edit menu's Select All: `selectAll:` to whatever has the
        /// keyboard. See [`EditCut`].
        EditSelectAll,
    ]
);

/// The key context the Edit menu's bindings live under, which no element in
/// ket ever sets.
///
/// A web page in a browser pane is a WKWebView, and WKWebView takes ⌘C and ⌘V
/// only as the Edit menu's key equivalents — with no Edit menu they reached
/// nothing, so text selected on a page could not be copied. gpui gives a
/// menu item its key equivalent from the keymap, so the chords are bound; but
/// bound under a context nothing sets, so gpui itself never dispatches them,
/// and ket's own handling of ⌘C and ⌘V in terminals, the editor and every
/// text field — which comes first and stops the key — is untouched. Only a
/// chord nothing in ket took goes on to the menu.
#[cfg(target_os = "macos")]
const EDIT_CONTEXT: &str = "KetNativeEditMenu";

/// The reusable non-modal popup currently open in this window.
///
/// A closed enum keeps the shell to one popup at a time without making the
/// shared popup component know what any popup means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupKind {
    /// Detailed provider quota and reset information.
    Usage,
    /// What token reduction has saved, by project — see `crate::savings`.
    Savings,
    /// The worktrees ready to merge, from the header's Merge figure.
    Merge,
}

/// Outcome of the managed Economy pack check performed at launch.
#[derive(Debug, Clone)]
pub(crate) enum PackRefreshStatus {
    /// No managed source is configured, so no download was attempted.
    NotConfigured,
    /// The configured source is being downloaded and validated.
    Checking,
    /// The source returned a valid pack. Which one is the staged file's to
    /// say — see `ket_core::pack::staged`.
    Succeeded,
    /// The download, validation, or cache write failed.
    Failed(String),
}

/// The chord that opens settings.
///
/// ⌘, is the macOS convention and every Mac application uses it. It is a
/// *menu* key equivalent like ⌘Q: the shell's own keymap lists chords for the
/// palette to display but dispatches none of them, so a binding that lives
/// only there reaches nothing.
#[cfg(target_os = "macos")]
const SETTINGS_CHORD: &str = "cmd-,";
#[cfg(not(target_os = "macos"))]
const SETTINGS_CHORD: &str = "ctrl-,";

/// The chord that quits: ⌘Q on macOS, Alt-F4 everywhere gpui runs besides.
#[cfg(target_os = "macos")]
const QUIT_CHORD: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT_CHORD: &str = "alt-f4";

/// The platform-standard chord for closing the active document tab.
#[cfg(target_os = "macos")]
const CLOSE_TAB_CHORD: &str = "cmd-w";
#[cfg(not(target_os = "macos"))]
const CLOSE_TAB_CHORD: &str = "ctrl-w";

/// The platform-standard chord for a new terminal tab.
#[cfg(target_os = "macos")]
const NEW_TERMINAL_TAB_CHORD: &str = "cmd-t";
#[cfg(not(target_os = "macos"))]
const NEW_TERMINAL_TAB_CHORD: &str = "ctrl-t";

/// The platform-standard chord for a new blank document.
#[cfg(target_os = "macos")]
const NEW_BLANK_DOCUMENT_CHORD: &str = "cmd-n";
#[cfg(not(target_os = "macos"))]
const NEW_BLANK_DOCUMENT_CHORD: &str = "ctrl-n";

/// The menu bar. macOS delivers Cmd-Q as a *menu* key equivalent, and gpui
/// installs no menu bar of its own — without one the chord reaches nothing
/// and the app cannot be quit from the keyboard at all.
///
/// Takes what the View menu toggles, because gpui's menu items carry no
/// checkmark: the item names what choosing it will do, the way macOS's own
/// "Show Tab Bar" and "Hide Toolbar" do, and the bar is rebuilt when that
/// changes — see [`Shell::set_status_bar`].
fn app_menus(status_bar_open: bool) -> Vec<Menu> {
    vec![
        Menu {
            name: "ket".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit ket", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Blank Document", NewBlankDocument),
                MenuItem::action("New Terminal Tab", NewTerminalTab),
                MenuItem::separator(),
                MenuItem::action("Add to Backlog\u{2026}", AddToBacklog),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        #[cfg(target_os = "macos")]
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Cut", EditCut, gpui::OsAction::Cut),
                MenuItem::os_action("Copy", EditCopy, gpui::OsAction::Copy),
                MenuItem::os_action("Paste", EditPaste, gpui::OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action("Select All", EditSelectAll, gpui::OsAction::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action(
                if status_bar_open {
                    "Hide Status Bar"
                } else {
                    "Show Status Bar"
                },
                ToggleStatusBar,
            )],
        },
    ]
}

/// How ket's own editor draws text.
///
/// A pair rather than two loose fields on the shell, because they are changed
/// together, saved together, and a size without the line height that goes with
/// it has never been a useful thing to pass anywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EditorType {
    /// Point size, already resolved against the shared code size.
    pub(crate) font_size: u16,
    /// Line height, as a multiple of that size.
    pub(crate) line_height: f32,
}

/// How many ticks pass between runs while the window is not focused.
///
/// At the ten-second tick this is thirty seconds: a background window is
/// still looked at, just not as often as one somebody is typing in.
const UNFOCUSED_EVERY: u32 = 3;

/// How often the window looks for agent reports between ticks.
///
/// A status changes on a keystroke — a turn starting, a permission prompt, a
/// turn cancelled — and waiting for the ten-second tick to show it made the
/// sidebar lag the pane beside it. Looking is a non-blocking read of a
/// channel, so doing it often costs nothing while nothing is there, and the
/// window redraws only when something was.
const STATUS_WAKE: std::time::Duration = std::time::Duration::from_millis(250);

/// The shell.
struct Shell {
    /// Resolved colours. Never read at a call site — see `crate::paint`.
    pub(crate) theme: Theme,
    /// The preference the theme was resolved from.
    ///
    /// Kept because following the system means re-resolving when the system
    /// changes, and the preference is the only thing that survives that.
    pub(crate) theme_config: ThemeConfig,
    /// Whether audio file previews may play inside the shell.
    pub(crate) audio_enabled: bool,
    /// How ket's own editor draws text: the point size, and the line height as
    /// a multiple of it.
    ///
    /// Held resolved rather than read out of the config on every frame — the
    /// size is the override or the shared code size, and working that out is
    /// the settings pane's job at the moment it is changed, not the render
    /// loop's sixty times a second.
    pub(crate) editor_type: EditorType,
    /// Whether the sidebar is showing.
    pub(crate) sidebar_open: bool,
    /// Whether the status bar is showing. Hidden, it is neither built nor
    /// fed: see [`Shell::set_status_bar`].
    pub(crate) status_bar_open: bool,
    /// The sidebar, as a view gpui can reuse between frames — see
    /// `crate::region`. Made on the first frame that shows it.
    sidebar_region: Option<gpui::Entity<crate::region::Region>>,
    /// Whether the right panel is showing. See `crate::explorer`.
    pub(crate) panel_open: bool,
    /// Its width, kept across hide/show like the sidebar's.
    pub(crate) panel_width: Pixels,
    /// Which view the right panel is showing. See `crate::panel`.
    pub(crate) panel_view: crate::panel::PanelView,
    /// The right panel's Git view. See `crate::git_panel`.
    pub(crate) git_panel: crate::git_panel::GitPanel,
    /// Open diff tabs' state, by the comparison each shows. See
    /// `crate::diff_view`.
    pub(crate) diffs: crate::diff_view::Diffs,
    /// The file tree, for the selected worktree.
    pub(crate) explorer: Explorer,
    /// The `Cmd-P` file finder.
    pub(crate) finder: Finder,
    /// Width retained across hide/show; dragging never destroys the preference.
    pub(crate) sidebar_width: Pixels,
    /// Every project ket knows about.
    pub(crate) projects: Vec<ProjectNode>,
    /// The selected worktree, if any.
    pub(crate) selection: Option<Selection>,
    /// Last worktree visited per project, so the project row restores its tabs.
    pub(crate) project_focus: HashMap<ProjectId, WorktreeId>,
    /// Open tabs per worktree. A worktree absent here has never been opened.
    pub(crate) spaces: HashMap<WorktreeId, Space>,
    /// The open dropdown menu, if any. One at a time: a menu is the topmost
    /// thing on screen and answers keys before anything under it.
    ///
    /// See [`crate::ui::menu`] for the component and [`crate::tabs`] for the `+`
    /// button's entries.
    pub(crate) menu: Option<crate::ui::menu::OpenMenu<crate::tabs::MenuAction>>,
    /// A tab's right-click menu, including the tab it was opened for.
    pub(crate) tab_context_menu: Option<crate::tabs::TabContextMenu>,
    /// Where a link clicked in a terminal should open, while that is asked.
    pub(crate) link_menu: Option<crate::browser::LinkMenu>,
    /// The **Open in** branch of the tab menu, while it is up.
    ///
    /// Its own field rather than a second use of `tab_context_menu`: the two
    /// are open at the same time and hand back different vocabularies, which is
    /// exactly the reason the worktree menu does not share `menu` either.
    pub(crate) tab_open_in: Option<crate::tabs::TabOpenInMenu>,
    /// The tab menu's **Send snippet** branch, while it is up.
    pub(crate) tab_snippets: Option<crate::tabs::TabSnippetMenu>,
    /// The merge dialog for one worktree row, while it is up.
    pub(crate) confirm_merge_worktree: Option<crate::merge_worktree::MergeWorktreeConfirm>,
    /// The worktree whose merge is in flight, if any.
    ///
    /// One at a time, and also what the row draws its in-flight state from.
    pub(crate) merging: crate::merge_worktree::Merging,
    /// The tab whose title is being edited in the strip, if any. One at a
    /// time: the field is what holds the keyboard, and two of them would be
    /// two places a character could land.
    pub(crate) tab_rename: Option<crate::tabs::TabRename>,
    /// The worktree display-name dialog, while open.
    pub(crate) worktree_rename: Option<crate::worktree_menu::WorktreeRename>,
    /// The global quick-prompt dialog, while open.
    pub(crate) quick_prompt: Option<crate::quick_prompt::QuickPrompt>,
    /// Whether a device tab's screenshot is being taken — see
    /// `device_capture`.
    pub(crate) capturing_device: bool,
    /// The device strip's button under the pointer, whose tooltip is showing.
    pub(crate) device_strip_hovered: Option<&'static str>,
    /// A project's backlog, while open.
    pub(crate) backlog: Option<crate::backlog::BacklogDialog>,
    /// The ⇧⌘A panel that takes a note for a backlog, while it is up.
    pub(crate) quick_capture: Option<crate::backlog::QuickCapture>,
    /// A worktree row's right-click menu, while it is open.
    ///
    /// Its own field rather than a second use of `menu`: the two menus hand
    /// back different vocabularies, and one field would mean one enum that
    /// knew about both the tab strip and the sidebar.
    pub(crate) worktree_menu:
        Option<crate::ui::menu::OpenMenu<crate::worktree_menu::WorktreeAction>>,
    /// The right-click menu of a folder in the file tree or a file in the Git
    /// view's change list, while it is open. See `crate::explorer`.
    pub(crate) path_menu: Option<crate::explorer::PathMenu>,
    /// The token-reduction level picker a worktree's menu opens onto, while it
    /// is up. See `crate::worktree_menu`.
    pub(crate) token_reduction_menu: Option<crate::ui::menu::OpenMenu<u8>>,
    /// The snippet picker a worktree's menu opens onto, while it is up.
    pub(crate) worktree_snippet_menu: Option<crate::snippets::SnippetMenu>,
    /// The one reusable non-modal popup that may be open in this window.
    pub(crate) popup: Option<PopupKind>,
    /// The feedback popover over the status bar's right end, and the draft
    /// it keeps between openings. See `crate::feedback`.
    pub(crate) feedback: crate::feedback::Feedback,
    /// The span the Savings item and popup total over.
    pub(crate) savings_period: crate::savings::SavingsPeriod,
    /// The savings rollup, rebuilt on the poll tick rather than per frame —
    /// see `Shell::refresh_savings`.
    pub(crate) savings: crate::savings::Savings,
    /// The latest context-and-cost reading for each worktree, from the status
    /// line payloads its agents draw anyway — see `crate::status_bar` and
    /// [`ket_core::usage::from_statusline`].
    ///
    /// Grouped by worktree rather than keyed by it: a worktree is what the
    /// sidebar, the token-reduction level and the reader all address, but it
    /// can hold several agents at once and each of them reports separately.
    /// See [`crate::usage_card::WorktreeSessions`], which keeps them apart.
    pub(crate) session_usage: HashMap<WorktreeId, crate::usage_card::WorktreeSessions>,
    /// One rollout tailer per worktree that has had a Codex session in it.
    ///
    /// Codex publishes no status line, so nothing pushes its numbers at ket the
    /// way Claude's do — they have to be read out of the transcript it is
    /// already writing. See [`ket_core::sessions::CodexRollout`].
    ///
    /// Created on demand rather than per worktree up front: finding the right
    /// rollout means walking Codex's dated store, which is too much to do for a
    /// worktree that has never run one. A hook report naming the agent is the
    /// signal that there is something to follow.
    pub(crate) codex_rollouts: HashMap<WorktreeId, ket_core::sessions::CodexRollout>,
    /// What each worktree's own instruction files cost every session in it.
    ///
    /// Filled once per worktree, the first tick a session there reports, and
    /// never on a repaint: it is a filesystem read whose answer only changes
    /// when someone edits the file — which does not reach a running session
    /// anyway, since both agents load these at session start.
    pub(crate) context_weights: HashMap<WorktreeId, ket_core::worktree::ContextWeight>,
    /// The pack whose levels are in force, when one is.
    ///
    /// Held so the usage popup can say which table the levels came from, and
    /// tell a staged version apart from the one already adopted.
    pub(crate) active_pack: Option<ket_core::pack::Pack>,
    /// Latest launch-time download state shown at the top of Economy settings.
    pub(crate) pack_refresh_status: PackRefreshStatus,
    /// The telemetry variables every Claude pane is launched with.
    ///
    /// Built once, from config, rather than per launch: opening a terminal
    /// would otherwise read and parse the config file, and the answer cannot
    /// change without a restart anyway — a running agent's environment is fixed
    /// when its process starts. Empty on every install that has not configured
    /// a collector, which is the default.
    pub(crate) telemetry_env: std::collections::BTreeMap<String, String>,
    /// Every session ket has watched spending, kept across runs so a level can
    /// be compared with the default rather than only observed.
    pub(crate) usage_history: ket_core::usage::History,
    /// When the history was last written, so a file that is rewritten whole is
    /// not rewritten on every repaint. See `Shell::save_usage_history`.
    pub(crate) usage_saved_at_ms: u64,
    /// Which worktree that menu — and any dialog it opens — is about.
    pub(crate) worktree_target: Option<crate::worktree_menu::WorktreeTarget>,
    /// The worktree whose token-reduction ring is under the pointer, if any —
    /// what tells that row's `ui::tooltip` to show. A single field rather
    /// than one per row: only one ring can be hovered at a time, and a row
    /// clears it on the way out rather than a timer, so nothing lingers.
    pub(crate) hovered_ring: Option<WorktreeId>,
    /// The header mark's release card: where the pointer is, and whether it
    /// is open. See `crate::release`.
    pub(crate) release_hover: crate::release::ReleaseHover,
    /// The worktree whose `primary` mark is under the pointer, if any. Kept
    /// apart from `hovered_ring` for the same reason the marks are apart: two
    /// things on one row, each with its own word to say, and sharing one field
    /// would make hovering either show whichever was asked for last.
    pub(crate) hovered_primary: Option<WorktreeId>,
    /// Whether a git status sweep is already walking the worktrees.
    ///
    /// One at a time: a read runs off the window's thread, and a second one
    /// started while the first is still going is two walks of the same trees.
    /// See `crate::git_watch`.
    pub(crate) reading_git_status: bool,
    /// Which worktrees' git status to read, and when. See `crate::git_watch`.
    pub(crate) git_watch: crate::git_watch::GitWatch,
    /// Whether a scan of the agents' session files is already running.
    ///
    /// The scan reads transcripts off disk and took 150ms on the window's
    /// thread, so it runs off it, one at a time.
    pub(crate) reading_records: bool,
    /// Whether ket is the focused window, kept by the activation observer.
    ///
    /// The tick slows to every [`UNFOCUSED_EVERY`]th run while this is false,
    /// and runs at once when it turns true again.
    pub(crate) window_active: bool,
    /// Whether a merged-state sweep is already running.
    ///
    /// One at a time, for the same reason `reading_git_status` is — and it
    /// matters more here, because this sweep spawns several git processes per
    /// worktree rather than one walk.
    pub(crate) reading_merged: bool,
    /// When the last merged-state sweep started, in epoch milliseconds.
    ///
    /// Debounced against [`crate::tree::MERGED_SWEEP_INTERVAL`] the same way
    /// the usage history is against its own, rather than given a second timer.
    pub(crate) merged_swept_at_ms: u64,
    /// The worktree whose merge mark is under the pointer, if any. Its own
    /// field for the same reason `hovered_primary` is.
    pub(crate) hovered_merge: Option<WorktreeId>,
    /// The worktree whose size on disk is under the pointer, for its
    /// tooltip.
    pub(crate) hovered_footprint: Option<WorktreeId>,
    /// The worktree row under the pointer, for the facts its trailing
    /// column shows only then.
    pub(crate) hovered_worktree: Option<WorktreeId>,
    /// Worktrees whose subagent rows are folded away. Open by default, and
    /// not persisted: a subagent is gone long before a restart.
    pub(crate) collapsed_subagents: std::collections::HashSet<WorktreeId>,
    /// How wide each worktree row's trailing column was last drawn — the
    /// fold, merge and footprint after the state word — so its subagent rows
    /// can end their figures under that word rather than out past the fold.
    pub(crate) meta_widths: std::collections::HashMap<WorktreeId, Pixels>,
    /// The open "clear build output?" question, if there is one.
    pub(crate) confirm_clear_build: Option<crate::clear_build::ClearBuildConfirm>,
    /// The open "discard changes?" question for a file in the Git view.
    pub(crate) confirm_discard: Option<crate::discard_changes::DiscardConfirm>,
    /// What each worktree costs on disk. See `crate::storage`.
    pub(crate) sizes: crate::storage::Sizes,
    /// Whether a size sweep is already walking. One at a time: two would put
    /// two walks through the same few hundred thousand files.
    pub(crate) measuring: bool,
    /// The transient reports currently on screen. See `crate::toasts`.
    pub(crate) toasts: Vec<crate::toasts::Toast>,
    /// The next toast's id. Never reused while its toast is up.
    pub(crate) next_toast_id: u64,
    /// Which corner they stack in.
    ///
    /// Top right by default: it is the corner the sidebar, the tab strip and
    /// the status bar all keep clear, so a toast there covers nothing a reader
    /// is likely to be looking at while it is up.
    pub(crate) toast_corner: crate::ui::toast::Corner,
    /// Which button in the sidebar's "Projects" strip is under the pointer, by
    /// its element id — what tells that button's `ui::tooltip` to show. The
    /// ids are fixed strings written at the call site, and only one of the
    /// handful of them can be hovered at a time, so one field covers the strip.
    pub(crate) hovered_strip_action: Option<&'static str>,
    /// Which button on a project heading is under the pointer — the button's
    /// fixed id and the project's index — what tells that button's
    /// `ui::tooltip` to show. One field for all headings, as
    /// `hovered_strip_action` is for the strip: only one can be hovered at a
    /// time, and the row clears it on the way out.
    pub(crate) hovered_project_action: Option<(&'static str, usize)>,
    /// Which of the header's layout buttons — the fold and the two panel
    /// switches — is under the pointer, by its element id: what tells that
    /// button's `ui::tooltip` to show. One field for the cluster, as
    /// `hovered_strip_action` is for the sidebar's strip.
    pub(crate) hovered_header_action: Option<&'static str>,
    /// Whether the sidebar shows worktrees globally, ordered by activity.
    pub(crate) activity_view: bool,
    /// Whether the header is folded to its compact strip — see
    /// `crate::header`.
    pub(crate) header_compact: bool,
    /// Which height the traffic lights were last placed for, so they are
    /// moved when the header changes rather than on every frame.
    pub(crate) lights_placed: std::cell::Cell<Option<bool>>,
    /// Which worktrees the sidebar lists. Not kept across runs: a filter
    /// left on "Waiting" is a sidebar that looks empty the next morning.
    pub(crate) sidebar_filter: crate::tree::SidebarFilter,
    /// The gap a row being dragged in the sidebar would drop into, if any —
    /// what the insertion line is drawn in. See `crate::tree::DropTarget`.
    ///
    /// Only meaningful while a drag is up, and read that way: a drag released
    /// over nothing tells no one, so the sidebar asks `cx.has_active_drag()`
    /// rather than trusting a field nothing was able to clear.
    pub(crate) drop_target: Option<crate::tree::DropTarget>,
    /// Where a tab being dragged over the pane tree would land, if anywhere —
    /// what the insertion rule and the drop wash are drawn from. See
    /// `crate::tabs::TabDrop`.
    ///
    /// Kept apart from `drop_target` rather than folded into it: the sidebar
    /// and the pane tree are separate drop surfaces with separate vocabularies,
    /// and one field would have each clearing the other's target on every move.
    pub(crate) tab_drop: Option<crate::tabs::TabDrop>,
    /// Panes that came back from a saved layout owing a shell, by worktree.
    ///
    /// A pty does not survive the process that ran it, so a restored terminal
    /// pane is empty until something starts one in it. Filled by
    /// `Shell::select`, so that opening ket does not spawn a shell for every
    /// pane of every worktree of every project at once — only for the worktree
    /// somebody actually goes to. Drained as it is used: a pane is owed a
    /// shell once.
    pub(crate) owed_terminals: HashMap<WorktreeId, Vec<PaneId>>,
    /// Terminal tabs a saved layout wants reattached, by worktree — see
    /// [`crate::layout::SavedTerminal`]. Only with the ket host. Drained the
    /// same way as `owed_terminals`, when the worktree is opened.
    pub(crate) owed_restores: HashMap<WorktreeId, Vec<crate::layout::SavedTerminal>>,
    /// Each open terminal's key with the ket host: unique to the tab, saved in
    /// the layout, and what the tab asks for to get its terminal back after a
    /// restart. Also what its agent's hooks report as their pane.
    pub(crate) terminal_keys: HashMap<TerminalId, String>,
    /// The worktrees last handed to the status owners — see
    /// `Shell::sync_status_worktrees`.
    pub(crate) status_worktrees: Vec<(String, std::path::PathBuf)>,
    /// Whether a tab is in the air right now.
    ///
    /// Only the native browser views need to know: they sit above everything
    /// GPUI draws, so a pane showing one can neither report the pointer
    /// crossing it nor be covered by a drop wash until it is hidden. See
    /// `Shell::browser_occluded`.
    pub(crate) tab_dragging: bool,
    /// The delete-worktree confirmation, while it is up.
    pub(crate) confirm_remove_worktree: Option<crate::worktree_menu::RemoveWorktreeConfirm>,
    /// The close-tab confirmation for a terminal running a CLI agent session,
    /// while it is up. See `crate::tabs::CloseSessionConfirm`.
    pub(crate) confirm_close_session: Option<crate::tabs::CloseSessionConfirm>,
    /// Open editor tabs' buffers and UI state, keyed by absolute path. See
    /// `editor.rs`'s module doc for why a buffer lives here rather than on
    /// the `Tab` itself.
    pub(crate) editors: HashMap<SharedString, EditorState>,
    /// The window's own rectangle, as of the last frame drawn.
    ///
    /// Stashed from the render rather than observed: `gpui` reports a window's
    /// bounds to whoever has the window, and the only thing the shell is
    /// handed one in is its render. Read by `Shell::persist_view`, which runs
    /// on a tick and so sees where a resize ended rather than every pixel of
    /// it on the way.
    pub(crate) window_bounds: Option<gpui::Bounds<Pixels>>,
    /// The window's shape as it was last written to disk, so a tick that finds
    /// nothing moved writes nothing. See `crate::view`.
    pub(crate) view_saved: Option<crate::view::WindowView>,
    /// Which directories were open in the file tree of each worktree other
    /// than the one on screen.
    ///
    /// Selecting a worktree builds a new `Explorer` — the old tree's listings
    /// are another worktree's files — so without this, going to a worktree and
    /// back collapses everything that was open in it. Seeded at launch from
    /// `crate::view`, and folded back into what is saved there.
    pub(crate) explorer_memory: HashMap<WorktreeId, std::collections::HashSet<String>>,
    /// The pending re-read of which editor lines differ from `HEAD`, while a
    /// keystroke's debounce is still running. Held so the next keystroke can
    /// cancel it — see `Shell::schedule_editor_marks`.
    pub(crate) editor_marks: Option<gpui::Task<()>>,
    /// One lazy output stream and the current audio track.
    pub(crate) audio: AudioEngine,
    /// Wakes the player UI only while a track is advancing.
    pub(crate) audio_tick: Option<gpui::Task<()>>,
    /// Ephemeral in-app browser tabs. Their WKWebViews deliberately do not
    /// participate in layout persistence or share Safari's browsing data.
    pub(crate) browsers: HashMap<BrowserId, BrowserHandle>,
    /// The next browser tab's process-local identity.
    pub(crate) next_browser: u64,
    /// The single process-local SimulatorKit surface, while its tab is open.
    #[cfg(target_os = "macos")]
    pub(crate) ios_simulator: Option<IosSimulatorHandle>,
    /// The Android emulator tab's state, while it is open.
    pub(crate) android: Option<crate::android_emulator::AndroidHandle>,
    /// Monotonic identity for in-memory files, so several Untitled tabs never alias.
    pub(crate) next_untitled: u64,
    /// What each worktree is doing, merged from ket-driven sessions and from
    /// what is actually running in each worktree's terminal.
    ///
    /// Polled rather than subscribed: sessions live in the shared store, which
    /// another `ket` process may also write, and a terminal's foreground is a
    /// thing you ask the pty rather than a thing it tells you. See
    /// [`ket_core::activity`].
    pub(crate) activity: Tracker,
    /// Where agents' own hooks report to, and the token that proves a report
    /// came from something ket launched.
    ///
    /// `None` when the listener could not bind, which costs live status and
    /// nothing else — the scanner and the terminal's foreground still answer.
    pub(crate) hooks: Option<Listener>,
    /// Minted once per run and handed to every agent ket starts.
    pub(crate) hook_token: String,
    /// The next terminal's id. Only ever counts up.
    pub(crate) next_terminal: u64,
    /// Running terminals, one per terminal *tab*. A tab absent here has never
    /// opened one; see `crate::terminal`.
    pub(crate) terminals: HashMap<TerminalId, TerminalHandle>,
    /// Status shown while a terminal tab is waiting for its backend, or when
    /// opening that backend failed. Kept separately so the tab can appear in
    /// the first frame instead of waiting on filesystem scans and pty setup.
    pub(crate) terminal_notes: HashMap<TerminalId, SharedString>,
    /// Why the content area is empty, when it is.
    pub(crate) note: Option<SharedString>,
    /// The menu open beside a project heading, if one is. See `crate::projects`.
    pub(crate) project_menu: Option<ProjectMenu>,
    /// The project settings dialog, while it is open.
    pub(crate) settings: Option<SettingsDialog>,
    /// The add-project dialog, while it is open.
    pub(crate) adding_project: Option<AddProjectDialog>,
    /// The global settings view, when it is open. Distinct from `settings`,
    /// which is one *project's*.
    pub(crate) preferences: Option<crate::preferences::Preferences>,
    /// The create-worktree dialog, while it is open.
    pub(crate) new_worktree: Option<crate::worktree_dialog::NewWorktree>,
    /// The remove-project confirmation, while it is up.
    pub(crate) confirm_remove: Option<RemoveConfirm>,
    /// The command catalogue. Built once — it does not change at runtime.
    pub(crate) registry: Registry,
    /// Key chords to command ids, for showing bindings beside commands.
    pub(crate) keymap: Keymap,
    /// The command palette.
    pub(crate) palette: Palette,
    /// The ⌘/ snippet picker, while it is up.
    pub(crate) snippet_picker: Option<crate::snippet_picker::SnippetPicker>,
    /// The sidebar's search — worktrees, projects, tabs, settings, commands.
    pub(crate) search: crate::search::Search,
    /// Workspace-wide text search shown in the right panel.
    pub(crate) workspace_search: WorkspaceSearch,
    /// The shell's own text fields — the palette's query line, the finder's,
    /// and the file panel's search box. See [`crate::input::Searches`].
    pub(crate) searches: Searches,
    /// Keyboard focus for the window, so key events reach us at all.
    pub(crate) focus: FocusHandle,
    /// The monospace family the terminal and editor draw with: the first
    /// preferred one that is actually installed. See `crate::fonts`.
    pub(crate) font_family: SharedString,
    /// What the chrome defaults to, which is a different mono from the
    /// editor's — the window root wears it and identifiers keep it. Its
    /// other half, the proportional face the chrome sets prose in, is not a
    /// field: it is reached through `fonts::prose`. See `crate::fonts`.
    pub(crate) chrome_family: SharedString,
    /// Provider quota, refreshed on its own threads and read synchronously.
    pub(crate) rate_limits: Arc<RateLimitCache>,
    /// Keeps the background quota refresh alive for the window's life: the
    /// header shows the figures it feeds, and the header is always on screen.
    /// Dropping it stops the loop.
    _quota_refresh: BackgroundRefresh,
    /// Where command handlers post what they want the shell to do.
    ///
    /// A registry handler cannot borrow the shell, so it writes here and the
    /// shell drains it after dispatch — one dispatch path rather than two.
    pub(crate) outbox: Outbox,
    /// The text caret's blink phase, shared by every field.
    ///
    /// One blinker rather than one per field: only one field has the keyboard
    /// at a time, and two timers started at different moments would blink out
    /// of step the moment a second field appeared. See [`Shell::caret_blink`].
    pub(crate) caret: Caret,
}

/// The blink behind every text caret in the shell.
///
/// A caret that does not blink reads as a rule drawn between two characters
/// rather than as an insertion point.
#[derive(Default)]
pub(crate) struct Caret {
    /// Whether the caret is in its visible phase.
    pub(crate) visible: bool,
    /// The timer flipping the phase. `None` while no field has the keyboard.
    task: Option<gpui::Task<()>>,
}

/// How long each phase of the caret's blink lasts: macOS's own interval, and
/// the one the terminal cursor already blinks at.
const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(530);

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Shell {
    /// Whether some field of the shell's is taking typed characters.
    ///
    /// Two things read this. The shell takes the window's focus back the
    /// moment it goes false, and the caret's blink timer runs only while it is
    /// true — so a surface that takes typing has to be named here, or the
    /// field it just focused is blurred on the very next frame and the
    /// characters go wherever the shell's handle points. That is a terminal
    /// pane, which registers a text-input handler against exactly this handle:
    /// the settings filter used to type into the shell behind the modal for
    /// precisely this reason.
    ///
    /// The modals and overlays are named by whether they are *open*, not by
    /// whether their field has the keyboard yet: focus is asked for when one
    /// opens and granted a frame later, and a predicate that waited for the
    /// grant would blur the field in between. The panel's search box has no
    /// such gap — nothing but a click ever focuses it — so it answers for
    /// itself.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.new_worktree.is_some()
            || self.settings.is_some()
            || self.adding_project.is_some()
            || self.preferences.is_some()
            || self.palette.open
            || self.snippet_picker.is_some()
            || self.finder.open
            || self.search.open
            || self.workspace_search_typing(window, cx)
            || self.find_open()
            || self.terminal_find_open()
            || self.tab_rename_open()
            || self.worktree_rename.is_some()
            || self.quick_prompt.is_some()
            || self.backlog.is_some()
            || self.quick_capture.is_some()
            || self.feedback.typing()
            || self.browser_typing(window, cx)
            || self.searches.explorer.read(cx).is_focused(window)
    }

    /// Whether a modal currently owns the window.
    pub(crate) fn modal_open(&self) -> bool {
        self.preferences.is_some()
            || self.new_worktree.is_some()
            || self.settings.is_some()
            || self.adding_project.is_some()
            || self.confirm_remove.is_some()
            || self
                .confirm_remove_worktree
                .as_ref()
                .is_some_and(|confirm| !confirm.quiet)
            || self.confirm_clear_build.is_some()
            || self.confirm_discard.is_some()
            || self.confirm_close_session.is_some()
            || self.confirm_merge_worktree.is_some()
            || self.worktree_rename.is_some()
            || self.quick_prompt.is_some()
            || self.backlog.is_some()
            || self.quick_capture.is_some()
    }

    /// The periodic refresh: everything that changes under the window without
    /// an event to say so. Runs on the tick, and once when the window regains
    /// focus.
    fn poll_tick(&mut self, cx: &mut Context<Self>) {
        let _tick = crate::frametrace::tick("total");
        {
            let _t = crate::frametrace::tick("drain_hook_reports");
            self.drain_hook_reports(true);
        }
        {
            let _t = crate::frametrace::tick("refresh_activity");
            self.refresh_activity(cx);
        }
        // Files change under ket constantly — the agent in the terminal is
        // editing the same worktree. See `Shell::refresh_editors`.
        {
            let _t = crate::frametrace::tick("refresh_editors");
            self.refresh_editors(cx);
        }
        // Widths, window bounds and the selection all move without a moment to
        // hang a save on. This writes only when something actually moved — see
        // `crate::view`.
        {
            let _t = crate::frametrace::tick("persist_view");
            self.persist_view();
        }
        self.refresh_savings();
        // The theme the window is drawn in, for phones to draw in too. Sent
        // only when it changed — the system flipping to dark, a pick in
        // Settings — so asking every tick costs a comparison.
        ket_core::host::publish_theme(self.theme);
        // What the usage popover shows, for phones: the host passes it on.
        ket_core::host::publish_usage(
            ket_core::rate_limits::Provider::ALL
                .map(|provider| self.rate_limits.snapshot(provider))
                .to_vec(),
        );
        cx.notify();
    }

    /// Loads everything the window needs from `ket-core`.
    fn load(cx: &mut Context<Self>) -> Self {
        // Warms the shell-probe cache off the window's thread. Resolving an
        // agent whose command is an alias costs a whole login-shell startup
        // (see `ket_core::shell`), and the first pane opened would otherwise
        // pay it with the window frozen. Detached and discarded: the value is
        // the cache it fills, and a machine with no such agent does no work.
        std::thread::spawn(|| {
            if let Ok(workspace) = ket_core::workspace::Workspace::open() {
                let _ = ket_core::agents::Catalogue::detect(
                    workspace.config(),
                    &ket_core::agents::ShellProber,
                );
            }
        });

        // And the editors a tab's Open in menu can offer, for the same reason
        // and at the same cost: one login shell, to learn a `PATH` that a GUI
        // application does not inherit. On its own thread rather than after the
        // catalogue, because neither waits on the other and the menu should be
        // answerable by the time anyone can right-click a tab.
        std::thread::spawn(|| {
            let _ = ket_core::surface::installed_editors();
        });

        // And where Claude keeps its transcripts, which is the login shell's
        // to say when the launch line is an alias that sets it. The first
        // session scan and the first rate-limit fetch both want it; the scan
        // runs on the window's thread and would otherwise start the shell
        // there, so it is asked here first and the scan finds the answer
        // waiting.
        std::thread::spawn(|| {
            let _ = ket_core::sessions::claude_config_dirs();
        });

        // And the checkouts a previous run renamed aside but was killed before
        // it could delete — see `ket_core::worktree_trash`. On a thread for the
        // same reason the deletions themselves are on one: these are whole Rust
        // checkouts, and a launch that stopped to remove several gigabytes of
        // them would be exactly the stall the trash exists to avoid.
        // Both roots, when Settings names one of its own: trash left under the
        // default before the change is still there to sweep.
        std::thread::spawn(|| {
            let chosen = Config::load().ok().and_then(|c| c.worktrees_dir().ok());
            let mut roots: Vec<_> = [ket_core::paths::worktrees_dir().ok(), chosen]
                .into_iter()
                .flatten()
                .collect();
            roots.dedup();
            for root in roots {
                ket_core::worktree_trash::sweep_stale(&root);
            }
        });

        let (registry, outbox) = crate::commands::registry();
        let keymap = crate::palette::keymap(&registry);
        let focus = cx.focus_handle();
        let installed = cx.text_system().all_font_names();
        // Resolved together, and this is the only call: `resolve` is also
        // what hands the prose face to `fonts::prose`, which every component
        // reads it from.
        let (font_family, chrome_family, _prose) = crate::fonts::resolve(&installed);

        let rate_limits = Arc::new(RateLimitCache::with_real_sources());
        let refresh = rate_limits.spawn_background_refresh();

        // The cache updates off-thread, so the window has to look. A tick rather
        // than a subscription because `rate_limits` deliberately does not
        // publish on the event bus — see its module docs.
        cx.spawn(async move |shell, cx| {
            let mut skipped = 0;
            loop {
                cx.background_executor()
                    .timer(crate::status_bar::POLL)
                    .await;
                if shell
                    .update(cx, |shell, cx| {
                        // Every run while focused; one in `UNFOCUSED_EVERY`
                        // otherwise. The window turning active runs it at
                        // once, so what is on screen is never stale for long.
                        if !shell.window_active && skipped + 1 < UNFOCUSED_EVERY {
                            skipped += 1;
                            return;
                        }
                        skipped = 0;
                        shell.poll_tick(cx);
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        // Agent status, as it arrives — see `STATUS_WAKE`.
        cx.spawn(async move |shell, cx| {
            loop {
                cx.background_executor().timer(STATUS_WAKE).await;
                if shell
                    .update(cx, |shell, cx| {
                        if shell.drain_hook_reports(false) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        // The user's preference, resolved against what macOS is currently set
        // to. A config that will not load falls back to defaults rather than
        // refusing to open a window — see `Config::load`.
        let config = Config::load().unwrap_or_default();
        // Read before the theme is moved out: the editor's size falls back to
        // the code size, so it needs the whole config still standing.
        let editor_type = EditorType {
            font_size: config.editor_font_size(),
            line_height: config.editor.line_height,
        };
        let theme_config = config.theme;
        let audio_enabled = config.viewer.audio;
        let theme = crate::appearance::current(&theme_config, cx);

        // Watched, not merely held. A field notifies its own entity when the
        // text changes; the shell is what draws the list ranked against it,
        // and nothing else would tell it that what it drew last frame is now
        // stale. The pickers re-rank from here rather than from their key
        // handlers because a typed character does not arrive through one: it
        // comes back later, from macOS's input context. See `crate::input`.
        let searches = Searches::new(cx);
        for input in searches.all() {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        cx.observe(&searches.palette, |shell, _, cx| {
            shell.palette.selected = 0;
            shell.rank(cx);
        })
        .detach();
        cx.observe(&searches.snippet, |shell, _, cx| {
            if let Some(open) = shell.snippet_picker.as_mut() {
                open.selected = 0;
            }
            shell.rank_snippet_picker(cx);
        })
        .detach();
        cx.observe(&searches.finder, |shell, _, cx| {
            shell.finder.selected = 0;
            shell.rank_finder(cx);
        })
        .detach();
        cx.observe(&searches.sidebar, |shell, _, cx| {
            shell.search.selected = 0;
            shell.rank_search(cx);
        })
        .detach();
        for input in [
            &searches.workspace,
            &searches.workspace_include,
            &searches.workspace_exclude,
        ] {
            cx.observe(input, |shell, _, cx| {
                shell.queue_workspace_search(cx);
            })
            .detach();
        }

        // Loaded before the shell is built, because what each worktree last
        // spent is seeded from it — see `usage_card::restored_sessions`.
        let usage_history = ket_core::usage::History::load();

        let workspace = match Workspace::open() {
            Ok(workspace) => workspace,
            Err(e) => {
                return Self {
                    theme,
                    theme_config,
                    audio_enabled,
                    editor_type,
                    sidebar_open: true,
                    status_bar_open: true,
                    sidebar_region: None,
                    panel_open: true,
                    panel_width: px(280.0),
                    panel_view: Default::default(),
                    git_panel: Default::default(),
                    diffs: Default::default(),
                    explorer: Explorer::default(),
                    finder: Finder::default(),
                    sidebar_width: px(300.0),
                    projects: Vec::new(),
                    selection: None,
                    project_focus: HashMap::new(),
                    spaces: HashMap::new(),
                    menu: None,
                    tab_context_menu: None,
                    link_menu: None,
                    tab_open_in: None,
                    tab_snippets: None,
                    confirm_merge_worktree: None,
                    merging: None,
                    tab_rename: None,
                    worktree_rename: None,
                    quick_prompt: None,
                    capturing_device: false,
                    device_strip_hovered: None,
                    backlog: None,
                    quick_capture: None,
                    worktree_menu: None,
                    path_menu: None,
                    token_reduction_menu: None,
                    worktree_snippet_menu: None,
                    popup: None,
                    feedback: Default::default(),
                    savings_period: Default::default(),
                    savings: Default::default(),
                    session_usage: crate::usage_card::restored_sessions(&usage_history),
                    codex_rollouts: HashMap::new(),
                    context_weights: HashMap::new(),
                    active_pack: None,
                    pack_refresh_status: PackRefreshStatus::NotConfigured,
                    telemetry_env: std::collections::BTreeMap::new(),
                    usage_history,
                    usage_saved_at_ms: 0,
                    worktree_target: None,
                    hovered_ring: None,
                    release_hover: Default::default(),
                    hovered_primary: None,
                    hovered_merge: None,
                    hovered_footprint: None,
                    hovered_worktree: None,
                    collapsed_subagents: std::collections::HashSet::new(),
                    meta_widths: std::collections::HashMap::new(),
                    confirm_clear_build: None,
                    confirm_discard: None,
                    sizes: crate::storage::Sizes::new(),
                    measuring: false,
                    toasts: Vec::new(),
                    next_toast_id: 0,
                    toast_corner: crate::ui::toast::Corner::default(),
                    hovered_strip_action: None,
                    hovered_header_action: None,
                    hovered_project_action: None,
                    sidebar_filter: Default::default(),
                    activity_view: false,
                    header_compact: false,
                    lights_placed: std::cell::Cell::new(None),
                    reading_git_status: false,
                    git_watch: Default::default(),
                    reading_records: false,
                    window_active: true,
                    reading_merged: false,
                    merged_swept_at_ms: 0,
                    drop_target: None,
                    owed_terminals: HashMap::new(),
                    owed_restores: HashMap::new(),
                    terminal_keys: HashMap::new(),
                    status_worktrees: Vec::new(),
                    tab_drop: None,
                    tab_dragging: false,
                    confirm_remove_worktree: None,
                    confirm_close_session: None,
                    editors: HashMap::new(),
                    window_bounds: None,
                    view_saved: None,
                    explorer_memory: HashMap::new(),
                    editor_marks: None,
                    audio: AudioEngine::default(),
                    audio_tick: None,
                    browsers: HashMap::new(),
                    next_browser: 0,
                    #[cfg(target_os = "macos")]
                    ios_simulator: None,
                    android: None,
                    next_untitled: 1,
                    preferences: None,
                    hooks: None,
                    hook_token: String::new(),
                    activity: Tracker::default(),
                    next_terminal: 0,
                    terminals: HashMap::new(),
                    terminal_notes: HashMap::new(),
                    note: Some(format!("could not open the workspace: {e}").into()),
                    project_menu: None,
                    settings: None,
                    adding_project: None,
                    new_worktree: None,
                    confirm_remove: None,
                    registry,
                    keymap,
                    palette: Palette::default(),
                    snippet_picker: None,
                    search: crate::search::Search::default(),
                    workspace_search: WorkspaceSearch::default(),
                    searches,
                    focus,
                    font_family,
                    chrome_family,
                    outbox,
                    caret: Caret::default(),
                    rate_limits,
                    _quota_refresh: refresh,
                };
            }
        };

        let projects = Self::load_projects(&workspace);

        // What lets the tracker tell `claude` in a terminal from `cargo` in
        // one. Names only, from the same list the catalogue is built from, so
        // the two cannot drift — and no probe, which is the expensive half of
        // `Catalogue::detect` and answers nothing this needs.
        let known_agents = ket_core::agents::known_agent_names(workspace.config());

        let mut shell = Self {
            theme,
            theme_config,
            audio_enabled,
            editor_type,
            sidebar_open: true,
            status_bar_open: true,
            sidebar_region: None,
            panel_open: true,
            panel_width: px(280.0),
            panel_view: Default::default(),
            git_panel: Default::default(),
            diffs: Default::default(),
            explorer: Explorer::default(),
            finder: Finder::default(),
            sidebar_width: px(300.0),
            projects,
            selection: None,
            project_focus: HashMap::new(),
            spaces: HashMap::new(),
            menu: None,
            tab_context_menu: None,
            link_menu: None,
            tab_open_in: None,
            tab_snippets: None,
            confirm_merge_worktree: None,
            merging: None,
            tab_rename: None,
            worktree_rename: None,
            quick_prompt: None,
            capturing_device: false,
            device_strip_hovered: None,
            backlog: None,
            quick_capture: None,
            worktree_menu: None,
            path_menu: None,
            token_reduction_menu: None,
            worktree_snippet_menu: None,
            popup: None,
            feedback: Default::default(),
            savings_period: Default::default(),
            savings: Default::default(),
            session_usage: crate::usage_card::restored_sessions(&usage_history),
            codex_rollouts: HashMap::new(),
            context_weights: HashMap::new(),
            active_pack: None,
            pack_refresh_status: PackRefreshStatus::NotConfigured,
            telemetry_env: std::collections::BTreeMap::new(),
            usage_history,
            usage_saved_at_ms: 0,
            worktree_target: None,
            hovered_ring: None,
            release_hover: Default::default(),
            hovered_primary: None,
            hovered_merge: None,
            hovered_footprint: None,
            hovered_worktree: None,
            collapsed_subagents: std::collections::HashSet::new(),
            meta_widths: std::collections::HashMap::new(),
            confirm_clear_build: None,
            confirm_discard: None,
            sizes: crate::storage::Sizes::new(),
            measuring: false,
            toasts: Vec::new(),
            next_toast_id: 0,
            toast_corner: crate::ui::toast::Corner::default(),
            hovered_strip_action: None,
            hovered_header_action: None,
            hovered_project_action: None,
            sidebar_filter: Default::default(),
            activity_view: false,
            header_compact: false,
            lights_placed: std::cell::Cell::new(None),
            reading_git_status: false,
            git_watch: Default::default(),
            reading_records: false,
            window_active: true,
            reading_merged: false,
            merged_swept_at_ms: 0,
            drop_target: None,
            owed_terminals: HashMap::new(),
            owed_restores: HashMap::new(),
            terminal_keys: HashMap::new(),
            status_worktrees: Vec::new(),
            tab_drop: None,
            tab_dragging: false,
            confirm_remove_worktree: None,
            confirm_close_session: None,
            editors: HashMap::new(),
            window_bounds: None,
            view_saved: None,
            explorer_memory: HashMap::new(),
            editor_marks: None,
            audio: AudioEngine::default(),
            audio_tick: None,
            browsers: HashMap::new(),
            next_browser: 0,
            #[cfg(target_os = "macos")]
            ios_simulator: None,
            android: None,
            next_untitled: 1,
            preferences: None,
            hooks: None,
            hook_token: String::new(),
            activity: Tracker::new(known_agents),
            next_terminal: 0,
            terminals: HashMap::new(),
            terminal_notes: HashMap::new(),
            note: None,
            project_menu: None,
            settings: None,
            adding_project: None,
            new_worktree: None,
            confirm_remove: None,
            registry,
            keymap,
            palette: Palette::default(),
            snippet_picker: None,
            search: crate::search::Search::default(),
            workspace_search: WorkspaceSearch::default(),
            searches,
            focus,
            font_family,
            chrome_family,
            outbox,
            caret: Caret::default(),
            rate_limits,
            _quota_refresh: refresh,
        };

        // Before anything can open a pane. A terminal started while the
        // listener is down gets no `KET_HOOK_PORT` and no `KET_WORKTREE_ID`
        // (see `hook_env`), so its agent reports to nowhere and the row it is
        // working in never lights up — for the whole life of that pane, since
        // the environment is fixed when the process starts. This used to sit
        // below the first `select`, which is why it was always the first
        // worktree of the first project, and only that one, whose agent went
        // unreported.
        // Before the hooks, because a pack decides what the reduction note
        // says and the hook script is handed that note at launch.
        shell.activate_pack(cx);
        shell.configure_telemetry(&workspace);
        shell.activate_rates(&workspace);
        shell.install_agent_hooks();
        shell.publish_agents(cx);

        // Before the layouts, because restoring the panels' widths and what
        // is expanded is what the window is, and after them is a frame of the
        // window being something else.
        let was_selected = shell.restore_view();
        shell.refresh_savings();
        // The refresh was started with the shell; a bar restored hidden stops
        // it before its first fetch has had a chance to matter.
        if !shell.status_bar_open {
            shell.set_status_bar(false, cx);
        }

        shell.restore_layouts(&workspace);

        // Off the window's thread and only ever here: measuring a worktree is
        // a walk of every file in it, so it is done once at launch rather than
        // on a tick. Until it lands the rows simply carry no size — see
        // `crate::storage`.
        shell.measure_all(cx);

        let layout_warning = shell.note.clone();

        // Open on the worktree the last run was left on, and failing that on
        // something rather than an empty pane: the first worktree of the first
        // project that has one. Failing, here, includes a worktree that has
        // been removed since — which is why this is a lookup rather than a
        // stored pair of indices.
        let selection = was_selected.and_then(|id| shell.locate(&id)).or_else(|| {
            shell
                .projects
                .iter()
                .position(|p| !p.worktrees.is_empty())
                .map(|project| Selection {
                    project,
                    worktree: 0,
                })
        });
        if let Some(selection) = selection {
            shell.select(selection, cx);
        }
        if layout_warning.is_some() {
            shell.note = layout_warning;
        }
        shell
    }

    /// Re-resolves the theme after the OS appearance changed.
    ///
    /// A no-op unless the preference is `System`: someone who chose dark meant
    /// dark, including at the moment their Mac decided otherwise.
    fn follow_system_appearance(&mut self, appearance: WindowAppearance, cx: &mut Context<Self>) {
        let system = crate::appearance::system_appearance(appearance);
        let next = crate::appearance::resolve(&self.theme_config, system);

        if next.appearance == self.theme.appearance {
            return;
        }

        self.theme = next;
        cx.notify();
    }
}

impl Shell {
    /// Starts or stops the blink to match.
    fn caret_blink(&mut self, on: bool, cx: &mut Context<Self>) {
        match (on, self.caret.task.is_some()) {
            (true, false) => {
                self.caret.visible = true;
                self.caret.task = Some(cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
                    loop {
                        cx.background_executor().timer(CARET_BLINK).await;
                        let alive = this.update(cx, |shell, cx| {
                            shell.caret.visible = !shell.caret.visible;
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                }));
            }
            (false, true) => {
                self.caret.task = None;
                self.caret.visible = true;
            }
            _ => {}
        }
    }

    /// Shows the caret and starts its phase over — what typing does to a caret
    /// everywhere else, so a character never lands during the half of the
    /// blink where the caret is not drawn.
    pub(crate) fn caret_wake(&mut self, cx: &mut Context<Self>) {
        self.caret.visible = true;
        if self.caret.task.is_some() {
            self.caret.task = None;
            self.caret_blink(true, cx);
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A native child view sits above GPUI's compositor. Hide it whenever
        // another tab or an overlay owns the same rectangle; CSS-style z-order
        // cannot put a GPUI menu in front of an NSView.
        let now = std::time::Instant::now();
        crate::frametrace::frame(now);
        crate::motion::frame(now, window, cx);
        // Where the window is, for `crate::view` to store on its next tick.
        // A field rather than a save: this runs every frame, including every
        // frame of a resize drag.
        self.window_bounds = Some(window.bounds());
        self.sync_browser_visibility(cx);
        #[cfg(target_os = "macos")]
        self.sync_ios_simulator_visibility(cx);
        self.sync_terminal_views(cx);
        // Before anything is built: whether the caret is in its lit phase is a
        // fact the fields read as they render.
        // The editor answers separately from `typing`: it takes keys through
        // the shell's own focus handle rather than a field of its own, so it
        // wants a blinking caret without wanting the focus moved.
        let wants_caret = self.typing(window, cx) || self.editor_active();
        self.caret_blink(wants_caret, cx);
        let modal_open = self.modal_open();
        // Built in sequence rather than inline: each borrows `cx` mutably.
        // Cached, so a terminal writing beside it does not lay it out again.
        // Its content is still `Shell::sidebar`; see `crate::region`.
        let sidebar = self.sidebar_open.then(|| {
            let region = self
                .sidebar_region
                .get_or_insert_with(|| {
                    let shell = cx.entity();
                    cx.new(|cx| {
                        crate::region::Region::new(
                            &shell,
                            |shell, window, cx| {
                                let _span = crate::frametrace::span("build:sidebar");
                                shell.sidebar(window, cx)
                            },
                            cx,
                        )
                    })
                })
                .clone();
            // The box `Shell::sidebar`'s root asks for, inside its card.
            let mut outer = div().size_full();
            crate::header::card(&self.theme)
                .flex_none()
                .w(self.sidebar_width)
                .h_full()
                .child(crate::region::Region::element(
                    &region,
                    outer.style().clone(),
                ))
        });
        let sidebar_divider = self.sidebar_open.then(|| self.sidebar_divider());
        let content = {
            let _span = crate::frametrace::span("build:panes");
            self.pane_layout(window, cx)
        };
        let panel_divider = self.panel_open.then(|| self.panel_divider());
        let panel = {
            let _span = crate::frametrace::span("build:panel");
            self.panel_open.then(|| self.right_panel(window, cx))
        };
        let palette = (!modal_open)
            .then(|| self.palette_view(window, cx))
            .flatten();
        let finder = (!modal_open)
            .then(|| self.finder_view(window, cx))
            .flatten();
        let snippet_picker = (!modal_open)
            .then(|| self.snippet_picker_view(window, cx))
            .flatten();
        let search = (!modal_open)
            .then(|| self.search_view(window, cx))
            .flatten();
        let project_menu = (!modal_open).then(|| self.project_menu_view(cx)).flatten();
        let tab_context_menu = (!modal_open)
            .then(|| self.tab_context_menu_view(cx))
            .flatten();
        let tab_open_in = (!modal_open).then(|| self.tab_open_in_view(cx)).flatten();
        let link_menu = (!modal_open).then(|| self.link_menu_view(cx)).flatten();
        let tab_snippets = (!modal_open).then(|| self.tab_snippets_view(cx)).flatten();
        let settings = self.settings_view(window, cx);
        let add_project = self.add_project_view(window, cx);
        let preferences = self.preferences_view(window, cx);
        // A surface with a text field in it takes the keyboard while it is
        // up, because `handle_input` is registered against *that field's*
        // handle and a field which is not focused is not a text field as far
        // as macOS is concerned. When it goes, the window has to take the
        // keyboard back, or the shell's own shortcuts stay dead. The terminal
        // and the editor share this one handle, so taking it back steals
        // nothing from them.
        let native_child_owns_focus = self.browser_owns_focus();
        #[cfg(target_os = "macos")]
        let native_child_owns_focus = native_child_owns_focus || self.ios_simulator_owns_focus();
        if !self.typing(window, cx) && !native_child_owns_focus && !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let new_worktree = self.new_worktree_view(window, cx);
        let confirm_remove = self.confirm_remove_view(cx);
        let worktree_menu = (!modal_open).then(|| self.worktree_menu_view(cx)).flatten();
        let path_menu = (!modal_open).then(|| self.path_menu_view(cx)).flatten();
        let token_reduction_menu = (!modal_open)
            .then(|| self.token_reduction_menu_view(cx))
            .flatten();
        let worktree_snippet_menu = (!modal_open)
            .then(|| self.worktree_snippet_menu_view(cx))
            .flatten();
        let remove_worktree = self.remove_worktree_view(cx);
        let clear_build = self.clear_build_view(cx);
        let discard = self.discard_view(cx);
        let merge_worktree = self.merge_worktree_view(cx);
        let rename_worktree = self.worktree_rename_view(window, cx);
        let quick_prompt = self.quick_prompt_view(window, cx);
        let backlog = self.backlog_view(window, cx);
        let quick_capture = self.quick_capture_view(window, cx);
        let close_session = self.close_session_view(cx);
        // One at a time: two modals stacked is a state with no way back.
        let modal = preferences
            .or(new_worktree)
            .or(settings)
            .or(add_project)
            .or(confirm_remove)
            .or(remove_worktree)
            .or(clear_build)
            .or(discard)
            .or(merge_worktree)
            .or(rename_worktree)
            .or(quick_prompt)
            .or(backlog)
            .or(quick_capture)
            .or(close_session);
        let header = {
            let _span = crate::frametrace::span("build:header");
            self.window_header(window, cx)
        };
        let status = self.status_bar_open.then(|| {
            let _span = crate::frametrace::span("build:status");
            let feedback = self.feedback_trigger(window, cx);
            self.window_status_bar(feedback)
        });
        let toasts = self.toasts_view(window, cx);
        let backdrop = paint(self.theme.backdrop);

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            // The desk the cards sit on, showing as a gutter around and
            // between them — see `crate::header`.
            .gap(crate::header::GUTTER)
            .p(crate::header::GUTTER)
            .when(self.status_bar_open, |el| el.pb(px(4.0)))
            .bg(backdrop)
            .text_color(paint(self.theme.text.primary))
            // The chrome's own mono, inherited by everything: only the panes
            // that show code or a grid ask for `self.font_family`, and only
            // the lines that are words ask for `fonts::prose`.
            .font_family(self.chrome_family.clone())
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.open_preferences(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| {
                if this.close_tab_from_shortcut(cx) {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &NewTerminalTab, _, cx| {
                if this.new_terminal_tab_from_shortcut(cx) {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &NewBlankDocument, _, cx| {
                if this.new_blank_document_from_shortcut() {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleStatusBar, _, cx| {
                this.set_status_bar(!this.status_bar_open, cx);
            }))
            .on_action(cx.listener(|this, _: &AddToBacklog, _, cx| {
                if !this.modal_open() && this.open_quick_capture(cx) {
                    cx.notify();
                }
            }))
            // Before anything under it can stop the click: a click that
            // reaches GPUI at all missed the phone, whose native view takes
            // its own, so the keyboard comes back to ket.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                #[cfg(target_os = "macos")]
                if this.release_ios_simulator_keyboard() {
                    cx.notify();
                }
                #[cfg(not(target_os = "macos"))]
                let _ = (this, cx);
            }))
            // A click anywhere but the menu closes it. The panel occludes, so
            // an event arriving here is one that missed it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // Both context menus, not just one: the project menu used
                    // to close on any keystroke instead, because it had no
                    // dismissal of its own.
                    let closed = this.worktree_menu.take().is_some()
                        | this.path_menu.take().is_some()
                        | this.token_reduction_menu.take().is_some()
                        | this.worktree_snippet_menu.take().is_some()
                        | this.project_menu.take().is_some()
                        | this.tab_open_in.take().is_some()
                        | this.tab_snippets.take().is_some()
                        | this.tab_context_menu.take().is_some()
                        | this.link_menu.take().is_some()
                        | this.popup.take().is_some()
                        | this.dismiss_feedback();
                    // Not folded into the run of `take`s above: closing this
                    // one clears a text field, which needs the context the
                    // boolean chain has no room for.
                    if this.search.open {
                        this.close_search(cx);
                        cx.notify();
                    }
                    if closed {
                        cx.notify();
                    }
                }),
            )
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, window: &mut Window, cx| {
                    // An iPhone Simulator with the keyboard takes
                    // Simulator.app's chords before anything else: nothing
                    // ket draws can be over it while it shows. Stopped here,
                    // or AppKit hands the chord on to the phone as a key.
                    #[cfg(target_os = "macos")]
                    if this.ios_simulator_key(&event.keystroke, cx) {
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    // An open menu is the topmost thing on screen, so it gets the
                    // first refusal — Escape closes it, arrows move, an item's
                    // own chord runs it. See `menu.rs`.
                    if this.menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    // Above the menu it hangs off: while the branch is open
                    // it is the topmost thing on screen, and its Escape has to
                    // close it rather than the menu behind it.
                    if this.tab_open_in_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.tab_snippets_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.tab_context_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.link_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    // The field a tab's menu just opened over its label. Above
                    // everything below it for the reason every field is:
                    // typing a name must not also drive the shell.
                    if this.tab_rename_key(event, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    if this.worktree_rename_key(event, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    // A project dialog takes every key while it is up — typing a
                    // display name must not also drive the shell — and comes before
                    // the overlays because neither can open over it.
                    if this.preferences_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    if this.project_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    // The delete confirmation is a modal, and the context menu is
                    // the topmost thing on screen — both before anything that
                    // could be reached from behind them.
                    if this.clear_build_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.discard_key(event) {
                        cx.notify();
                        return;
                    }
                    if this.remove_worktree_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.merge_worktree_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.close_session_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.quick_prompt_key(event, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    if this.backlog_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    if this.quick_capture_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    if this.feedback_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }
                    if this.project_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    // Above the menu it opened from: while the picker is up it is
                    // the topmost thing on screen, and Escape has to close it
                    // rather than the menu behind it.
                    if this.token_reduction_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.worktree_snippet_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.worktree_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    if this.path_menu_key(event, cx) {
                        cx.notify();
                        return;
                    }
                    // A popup takes only Escape. Every other key continues to
                    // the shell because this surface is non-modal and has no
                    // keyboard selection of its own.
                    if this.popup.is_some() && event.keystroke.key == "escape" {
                        this.popup = None;
                        cx.notify();
                        return;
                    }
                    // The two overlays get first refusal on every key while one is
                    // open, so typing a name cannot also drive the shell behind it.
                    if this.new_worktree_key(event, window, cx).is_some() {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }

                    if this.finder_key(event, cx) {
                        cx.notify();
                        return;
                    }

                    if this.palette_key(event, cx) {
                        cx.notify();
                        return;
                    }

                    if this.snippet_picker_key(event, cx) {
                        cx.notify();
                        return;
                    }

                    // The sidebar's search, which holds the keyboard the same
                    // way the two pickers above it do while its panel is up.
                    if this.search_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }

                    if this.workspace_search_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }

                    // Then the sidebar's own search box, which only takes keys
                    // once it has been clicked into.
                    if this.explorer_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }

                    // The browser's address field is ordinary GPUI text, so it
                    // gets the same editing and IME path as the rest of the
                    // shell before shortcuts or the pane underneath it.
                    if this.browser_key(event, window, cx) {
                        this.caret_wake(cx);
                        cx.notify();
                        return;
                    }

                    // Cmd-Shift-[ / Cmd-Shift-] cycle the focused pane's active
                    // tab, wrapping at either end — the VS Code default, and
                    // ahead of the editor so a buffer with focus does not
                    // swallow it as an unrecognised chord. See `shortcuts.rs`
                    // for what each chord in this match is and why.
                    let shortcut = global_shortcut(&event.keystroke);
                    if let Some(GlobalShortcut::CycleTab(direction)) = shortcut {
                        this.cycle_tab(direction);
                        cx.notify();
                        return;
                    }
                    // Same reasoning as `CycleTab` above: a buffer with focus
                    // must not swallow this as an unrecognised chord either.
                    if let Some(GlobalShortcut::ToggleSidebar) = shortcut {
                        this.dispatch("shell.toggle_sidebar", cx);
                        cx.notify();
                        return;
                    }
                    if let Some(GlobalShortcut::TogglePanel) = shortcut {
                        this.dispatch("shell.toggle_panel", cx);
                        cx.notify();
                        return;
                    }
                    if let Some(GlobalShortcut::OpenWorkspaceSearch) = shortcut {
                        this.open_workspace_search(cx);
                        cx.notify();
                        return;
                    }
                    if let Some(GlobalShortcut::OpenQuickPrompt) = shortcut {
                        this.open_quick_prompt(cx);
                        cx.notify();
                        return;
                    }
                    if let Some(GlobalShortcut::SendSnippet) = shortcut {
                        this.open_snippet_picker(cx);
                        cx.notify();
                        return;
                    }
                    if let Some(GlobalShortcut::OpenGitPanel) = shortcut {
                        this.show_panel_view(crate::panel::PanelView::Git, cx);
                        return;
                    }
                    // The `+` menu's rows, from anywhere: ahead of the editor
                    // so a buffer with focus does not eat them either.
                    if let Some(GlobalShortcut::NewBrowserTab) = shortcut {
                        if this.new_browser_tab_from_shortcut(cx) {
                            cx.notify();
                        }
                        return;
                    }
                    if let Some(GlobalShortcut::NewAgentSession(index)) = shortcut {
                        if this.new_agent_session_from_shortcut(index, cx) {
                            cx.notify();
                        }
                        return;
                    }
                    // The Create worktree dialog, from anywhere and for the
                    // project in context — the same family as the rows above,
                    // and ahead of the editor pane for the same reason.
                    // A note for the backlog, from anywhere: ahead of the
                    // panes, so the terminal whose selection it takes does
                    // not eat the chord first.
                    if let Some(GlobalShortcut::CaptureToBacklog) = shortcut {
                        if this.open_quick_capture(cx) {
                            cx.notify();
                        }
                        // Taken: the File menu's item has the same chord,
                        // and would open the panel a second time.
                        cx.stop_propagation();
                        return;
                    }
                    if let Some(GlobalShortcut::NewWorktree) = shortcut {
                        if this.new_worktree_from_shortcut(cx) {
                            cx.notify();
                        }
                        return;
                    }
                    // Also ahead of the panes, and for the same reason: the
                    // chord that leaves a pane cannot be one that pane is
                    // allowed to eat.
                    if let Some(GlobalShortcut::FocusPane(toward)) = shortcut {
                        this.dispatch(
                            match toward {
                                Toward::Left => "shell.focus_pane_left",
                                Toward::Right => "shell.focus_pane_right",
                                Toward::Up => "shell.focus_pane_up",
                                Toward::Down => "shell.focus_pane_down",
                            },
                            cx,
                        );
                        cx.notify();
                        return;
                    }

                    // The editor pane gets the next refusal, once the palette has
                    // had its turn — typing into a buffer must not also drive the
                    // shell's own shortcuts underneath it. See `editor/keys.rs`'s
                    // `Shell::editor_key` for what it consumes and what it lets
                    // fall through (Cmd-K for the palette, notably).
                    if this.audio_key(event, cx)
                        || this.android_key(&event.keystroke, cx)
                        || this.diff_key(event, cx)
                        || this.editor_key(event, window, cx)
                    {
                        cx.notify();
                        return;
                    }

                    match shortcut {
                        Some(GlobalShortcut::OpenPalette) => {
                            // A plain shell takes ⌘K as "clear", which is what
                            // the chord does in every terminal on the platform
                            // and what somebody in a pane full of output means
                            // by it. The palette keeps the chord everywhere
                            // else, an agent's pane included — see
                            // `Shell::clear_active_terminal`, which is what
                            // decides and says whether it acted.
                            if !this.clear_active_terminal(cx) {
                                this.open_palette(cx);
                            }
                            cx.notify();
                            return;
                        }
                        // Not routed through the registry: it opens a shell
                        // overlay rather than running a command, the same way
                        // Cmd-K does.
                        Some(GlobalShortcut::OpenSearch) => {
                            this.open_search(cx);
                            cx.notify();
                            return;
                        }
                        Some(GlobalShortcut::OpenFinder) => {
                            this.open_finder(cx);
                            cx.notify();
                            return;
                        }
                        // There is no palette entry for this one yet: that
                        // would mean registering a command in `ket-core`'s
                        // registry, which this pane does not own.
                        Some(GlobalShortcut::OpenTerminalTab) => {
                            this.open_terminal_tab(cx);
                            cx.notify();
                            return;
                        }
                        // The sidebar's merge button, from the keyboard. Acts
                        // on the selected worktree, which is the one the row
                        // button would have been clicked on.
                        Some(GlobalShortcut::MergeWorktree) => {
                            if let Some(selection) = this.selection {
                                this.request_merge_worktree(
                                    selection.project,
                                    selection.worktree,
                                    cx,
                                );
                            }
                            cx.notify();
                            return;
                        }
                        // Handled above, ahead of the editor pane.
                        Some(GlobalShortcut::ToggleSidebar)
                        | Some(GlobalShortcut::TogglePanel)
                        | Some(GlobalShortcut::CycleTab(_))
                        | Some(GlobalShortcut::OpenWorkspaceSearch)
                        | Some(GlobalShortcut::OpenQuickPrompt)
                        | Some(GlobalShortcut::SendSnippet)
                        | Some(GlobalShortcut::OpenGitPanel)
                        | Some(GlobalShortcut::NewBrowserTab)
                        | Some(GlobalShortcut::NewAgentSession(_))
                        | Some(GlobalShortcut::NewWorktree)
                        | Some(GlobalShortcut::CaptureToBacklog)
                        | Some(GlobalShortcut::FocusPane(_))
                        | None => {}
                    }

                    // Whatever it changed on screen it has drawn itself, or
                    // the program's echo will — see `terminal_input`.
                    this.terminal_key(event, window, cx);
                }),
            )
            .child(header)
            .child(
                div()
                    .id("workspace-row")
                    .flex()
                    .flex_grow()
                    .min_h_0()
                    .overflow_hidden()
                    .children(sidebar)
                    // The dividers are the gutters between the cards: a strip
                    // of desk that grabs for a resize.
                    .children(sidebar_divider)
                    .child(
                        crate::header::card(&self.theme)
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(MIN_CONTENT_SIZE)
                            .p(crate::header::CARD_INSET)
                            .child(content),
                    )
                    .children(panel_divider)
                    .children(panel)
                    .on_drag_move::<SidebarResize>(cx.listener(|this, event, _, cx| {
                        if this.resize_sidebar(event) {
                            cx.notify();
                        }
                    }))
                    .on_drag_move::<PanelResize>(cx.listener(|this, event, _, cx| {
                        if this.resize_panel(event) {
                            cx.notify();
                        }
                    })),
            )
            // Outside the workspace row, so it spans the sidebar as well: the
            // bar describes the app, not the pane you happen to be looking at.
            .children(status)
            // Non-modal overlays defer above the workspace. They are omitted
            // while a modal owns the window, so no stale menu crosses it.
            .children(palette.map(|el| deferred(el).with_priority(8)))
            .children(finder.map(|el| deferred(el).with_priority(8)))
            .children(snippet_picker.map(|el| deferred(el).with_priority(8)))
            .children(search.map(|el| deferred(el).with_priority(8)))
            .children(project_menu.map(|el| deferred(el).with_priority(8)))
            .children(tab_context_menu.map(|el| deferred(el).with_priority(8)))
            .children(link_menu.map(|el| deferred(el).with_priority(8)))
            // A priority above its parent's: a branch that drew under the menu
            // it hangs off would be hidden by it.
            .children(tab_open_in.map(|el| deferred(el).with_priority(9)))
            .children(tab_snippets.map(|el| deferred(el).with_priority(9)))
            .children(worktree_menu.map(|el| deferred(el).with_priority(8)))
            .children(path_menu.map(|el| deferred(el).with_priority(8)))
            .children(token_reduction_menu.map(|el| deferred(el).with_priority(8)))
            .children(worktree_snippet_menu.map(|el| deferred(el).with_priority(8)))
            // The modal is the last normal layer. Controls inside it can defer
            // dropdown panels above the whole modal without nesting deferred
            // draws, which GPUI rejects.
            .children(modal.map(|panel| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(crate::paint::scrim())
                    .child(panel)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            }))
            // Above everything, the modal included: what a toast is usually
            // reporting is the result of what that modal just did, and a
            // report drawn underneath the thing it is about is no report.
            .children(toasts.map(|el| deferred(el).with_priority(10)))
    }
}

fn main() {
    // This binary is also the ket host when started as one — see
    // `ket_core::host`. Before anything opens a window.
    ket_core::host::serve_if_asked();

    // The icons are compiled in; without an asset source `gpui` resolves every
    // `svg()` to nothing and draws blank space with no error. See `ui::icon`.
    // Without this every `tracing::warn!` in the shell goes nowhere, which is
    // how a hook install that failed, a terminal that would not open and a
    // session lookup that found nothing all looked identical: silence.
    ket_core::logging::init("warn");

    Application::new()
        .with_assets(crate::ui::icon::Assets)
        .run(|cx: &mut App| {
            // Quit before anything else can fail: binding first, because
            // `set_menus` reads the keymap to give the Quit item its key
            // equivalent; handling the action globally, because nothing in the
            // element tree owns quitting.
            cx.bind_keys([
                KeyBinding::new(QUIT_CHORD, Quit, None),
                KeyBinding::new(SETTINGS_CHORD, OpenSettings, None),
                KeyBinding::new(CLOSE_TAB_CHORD, CloseTab, None),
                KeyBinding::new(NEW_TERMINAL_TAB_CHORD, NewTerminalTab, None),
                KeyBinding::new(NEW_BLANK_DOCUMENT_CHORD, NewBlankDocument, None),
            ]);
            #[cfg(target_os = "macos")]
            cx.bind_keys([
                KeyBinding::new("cmd-x", EditCut, Some(EDIT_CONTEXT)),
                KeyBinding::new("cmd-c", EditCopy, Some(EDIT_CONTEXT)),
                KeyBinding::new("cmd-v", EditPaste, Some(EDIT_CONTEXT)),
                KeyBinding::new("cmd-a", EditSelectAll, Some(EDIT_CONTEXT)),
                // The menu item's key equivalent, under the same context
                // nothing sets: the shell takes the chord itself first.
                KeyBinding::new(
                    crate::shortcuts::CAPTURE_BINDING,
                    AddToBacklog,
                    Some(EDIT_CONTEXT),
                ),
            ]);
            cx.on_action::<Quit>(|_, cx| cx.quit());
            cx.set_menus(app_menus(true));

            // Before anything asks which fonts exist — see `fonts`.
            crate::fonts::install_bundled(cx.text_system());
            // Where it was left, if that is still somewhere a window can be —
            // see `crate::view::saved_window_bounds` — and centred at a size
            // that fits a laptop screen the first time ket is run.
            let bounds = crate::view::saved_window_bounds(cx)
                .unwrap_or_else(|| Bounds::centered(None, size(px(1200.0), px(780.0)), cx));

            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        // No system title bar: the header card is the title bar,
                        // and the traffic lights sit inside its left end, centred
                        // on it. See `crate::header`.
                        titlebar: Some(TitlebarOptions {
                            title: Some("ket".into()),
                            appears_transparent: true,
                            traffic_light_position: Some(point(
                                px(crate::header::TRAFFIC_LIGHTS.0),
                                px(crate::header::TRAFFIC_LIGHTS.1),
                            )),
                        }),
                        // ket moves its own window — see `crate::window_drag`.
                        //
                        // A window with a full-size content view is dragged by
                        // AppKit from the top of that view, and the tab strip runs
                        // to the top edge. gpui has the concept that would exempt
                        // it — `WindowControlArea` — but its macOS backend never
                        // implemented the hook, so the band could not be given up
                        // and a tab in it could not be picked up: the window moved
                        // before gpui's drag threshold was reached. Refusing AppKit
                        // the whole window and handing the drag back only where
                        // ket wants it is what makes a tab at the top edge
                        // draggable, which is where the tab strip belongs.
                        is_movable: false,
                        ..Default::default()
                    },
                    |window, cx| {
                        // Every text size and most spacing in the shell is in rems,
                        // and gpui's default rem is 16px — a web page's. macOS draws
                        // its own UI at 13px with 11px secondary text; 15px here puts
                        // `text_sm` and `text_xs` on exactly those, and shrinks the
                        // padding that goes with them in step.
                        let shell = cx.new(Shell::load);
                        // Installed before the shell existed, so it names the
                        // status bar's default; this names what was restored.
                        cx.set_menus(app_menus(shell.read(cx).status_bar_open));
                        window
                            .set_rem_size(px(f32::from(shell.read(cx).theme_config.ui_font_size)));
                        // Without focus the window receives no key events at
                        // all, so the palette would be unopenable.
                        window.focus(&shell.focus_handle(cx));

                        // Follow the OS live. A tool that stays dark when the system
                        // goes light is one that ignores its host, and the subscription
                        // is held for the window's lifetime rather than dropped — a
                        // dropped `Subscription` stops observing.
                        let watched = shell.clone();
                        let subscription = window.observe_window_appearance(move |window, cx| {
                            watched.update(cx, |shell, cx| {
                                shell.follow_system_appearance(window.appearance(), cx);
                            });
                        });
                        std::mem::forget(subscription);

                        // Which window has focus decides how often the tick
                        // runs, and regaining it refreshes at once. Held for
                        // the window's lifetime, like the observer above.
                        let subscription = shell.update(cx, |_, cx| {
                            cx.observe_window_activation(window, |shell, window, cx| {
                                let active = window.is_window_active();
                                let regained = active && !shell.window_active;
                                shell.window_active = active;
                                if regained {
                                    shell.poll_tick(cx);
                                }
                            })
                        });
                        std::mem::forget(subscription);

                        shell
                    },
                )
                .expect("open window");

            // The last save on the way out. The tick writes the window's shape
            // at most a tick after it moves, which is fine for a crash
            // and not fine for a quit: closing the sidebar and pressing ⌘Q is
            // exactly the sequence where the change somebody just made would be
            // the one that did not survive. Forgotten rather than held for the
            // same reason the appearance observer above is: a dropped
            // `Subscription` stops observing, and there is nothing here to hold
            // it for the life of the application.
            std::mem::forget(cx.on_app_quit(move |cx: &mut App| {
                let _ = window.update(cx, |shell, _, _| shell.persist_view());
                async {}
            }));

            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Action as _;

    #[test]
    fn the_app_menu_offers_quit() {
        // A menu bar without a Quit item is the bug: macOS resolves Cmd-Q
        // against the menu's key equivalents, so no Quit item means no way to
        // quit from the keyboard. The item's equivalent comes from the
        // QUIT_CHORD binding, which is asserted separately below.
        let menus = app_menus(true);
        let app_menu = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "ket")
            .expect("the app menu exists");

        assert!(
            app_menu.items.iter().any(|item| match item {
                MenuItem::Action { action, .. } => action.name() == Quit.name(),
                _ => false,
            }),
            "the app menu has a Quit item"
        );
    }

    #[test]
    fn the_quit_chord_is_the_platforms_own() {
        if cfg!(target_os = "macos") {
            assert_eq!(QUIT_CHORD, "cmd-q");
        } else {
            assert_eq!(QUIT_CHORD, "alt-f4");
        }
    }

    #[test]
    fn no_overlay_defers_itself() {
        // `render` above defers every overlay exactly once. A view that also
        // defers itself nests a deferred draw inside a deferred draw, and gpui
        // does not merely misdraw that — it aborts the process the moment the
        // overlay opens ("cannot call defer_draw during deferred drawing").
        // That shipped once, in the project menu, so it is worth a tripwire:
        // these files supply overlays, and none of them may call `deferred`.
        // If you need a deferred element in one of them for something that is
        // *not* an overlay, this test is the place to say so.
        let sources = [
            ("projects.rs", include_str!("projects.rs")),
            ("palette.rs", include_str!("palette.rs")),
            ("explorer.rs", include_str!("explorer.rs")),
            ("worktree_menu.rs", include_str!("worktree_menu.rs")),
            ("worktree_dialog.rs", include_str!("worktree_dialog.rs")),
        ];

        for (name, source) in sources {
            // Skip the prose: the doc comments here explain the rule.
            let code = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("deferred("),
                "{name} defers an element itself; the shell defers overlays"
            );
        }
    }
}
