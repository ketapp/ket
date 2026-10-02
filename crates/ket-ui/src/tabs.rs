//! Tabs, and the pane tree each worktree owns.
//!
//! Each worktree is a *space*: clicking one in the sidebar swaps the content
//! area to that worktree's pane tree rather than accumulating every tab in one strip.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{
    AnyElement, App, Axis, Bounds, BoxShadow, ClipboardItem, Context, CursorStyle, DragMoveEvent,
    Entity, FontWeight, Image, ImageFormat, KeyDownEvent, MouseButton, MouseDownEvent, Pixels,
    Point, Render, Rgba, SharedString, Window, WindowControlArea, deferred, div, point, prelude::*,
    px, relative, size,
};
use ket_core::activity::{Activity, Signal};
use ket_core::config::AgentSpec;
use ket_core::id::WorktreeId;
use ket_core::keybinding::Chord;
use ket_core::surface::{
    EditorSurface, ExternalSurface, InstalledEditor, Position, ProcessSpawner,
};
use ket_core::theme::Theme;

use crate::Shell;
use crate::browser::BrowserId;
use crate::editor::EditorState;
use crate::fonts::Prose;
use crate::input::{Style, TextInput, is_text, text_line};
use crate::paint::{alpha, paint};
use crate::snippets::{SnippetMenu, SnippetPick};
use crate::terminal::TerminalId;
use crate::ui::agent::{agent_label, provider};
use crate::ui::button::{button, icon_button};
use crate::ui::dialog::{body, card, centered, footer, header};
use crate::ui::filetype::{file_mark, icon_mark};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu};

/// What picking an item in a pane's `+` menu does.
///
/// The menu component hands this back without interpreting it; this enum is
/// the tab strip's own vocabulary for what its menu can open.
///
/// Not `Copy`: `Agent` names the one of possibly several runnable agents a
/// person picked, and a name is not free to duplicate implicitly the way a
/// bare `PaneId` is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    /// A new untitled buffer, in the pane whose menu was opened.
    File(PaneId),
    /// A fresh session with the named agent — never resumed — in the pane
    /// whose menu was opened. The name is one of `Shell::runnable_agents`,
    /// not necessarily the worktree's own configured one.
    Agent(PaneId, SharedString),
    /// A terminal tab, in the pane whose menu was opened.
    Terminal(PaneId),
    /// An ephemeral system-webview tab, in the pane whose menu was opened.
    NewBrowser(PaneId),
    /// The process-local iPhone Simulator proof of concept.
    #[cfg(target_os = "macos")]
    IosSimulator(PaneId),
    /// The Android emulator, where the Android SDK is installed.
    Android(PaneId),
}

/// A tab addressed by its pane and position when its context menu opened.
///
/// This deliberately does not borrow the active tab: right-clicking an
/// inactive tab must leave the reader's active tab alone, while later command
/// wiring still needs to know exactly which tab the menu describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabContextTarget {
    pane: PaneId,
    index: usize,
}

/// The actions displayed by the first tab context-menu mock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabContextAction {
    OpenIn,
    PinTab,
    /// Send a saved prompt snippet to the tab's agent. Only offered on an
    /// agent session. Opens a submenu of snippets.
    SendSnippet,
    Close,
    CloseOthers,
    CloseToRight,
    CloseToLeft,
    CopyPath,
    CopyRelativePath,
    ChangeTitle,
}

/// One place a document can be opened, in the **Open in** submenu.
///
/// The editor arm carries the entry itself rather than an index into
/// [`ket_core::surface::installed_editors`], so what the row runs cannot drift
/// from what the row says. Borrowed rather than owned because that list is
/// resolved once and cached for the life of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenInApp {
    /// An editor ket found on this machine.
    Editor(&'static InstalledEditor),
    /// The platform's file manager, showing the file in its folder.
    FileManager,
}

/// The **Open in** submenu, and the document it will hand over.
///
/// The path is resolved when the submenu opens rather than when a row is
/// picked: the tab it came from is addressed by position, and the whole point
/// of a submenu is that other things can happen to the strip while it is up.
pub(crate) struct TabOpenInMenu {
    menu: OpenMenu<OpenInApp>,
    /// The file to open, absolute.
    path: SharedString,
    /// Where in it, when the tab is an editor with a cursor. An audio file has
    /// no position worth sending — see [`Shell::open_in_target`].
    at: Option<(u32, u32)>,
    /// Whether the pointer is somewhere that keeps the branch up: the row it
    /// hangs off, or one of its own. Cleared when the pointer rests on a
    /// sibling row instead, and read again a beat later to decide whether
    /// that was a departure or a crossing — see
    /// [`Shell::hover_tab_context_row`].
    held: bool,
}

/// The **Send snippet** submenu, and the agent session a pick goes to.
///
/// The terminal is resolved when the submenu opens, for the reason
/// [`TabOpenInMenu`] resolves its path then.
pub(crate) struct TabSnippetMenu {
    open: SnippetMenu,
    /// The agent session the snippet is typed into.
    terminal: TerminalId,
    /// Whether the pointer is somewhere that keeps the branch up — see
    /// [`TabOpenInMenu::held`].
    held: bool,
}

/// How far the submenu overlaps the panel it hangs off.
///
/// A submenu that starts exactly at its parent's edge reads as a second window
/// that happens to be adjacent; a few pixels of overlap reads as one menu
/// growing a branch, which is what it is.
const SUBMENU_OVERLAP: Pixels = px(6.0);

/// The submenu's width. Narrower than its parent: these are app names, and a
/// panel as wide as the menu it hangs off would look like a second menu rather
/// than a branch of one.
const SUBMENU_W: Pixels = px(200.0);

/// How long a sibling row has to hold the pointer before the branch closes.
///
/// The branch hangs level with its row and off to the right, so a hand moving
/// diagonally towards one of its lower rows crosses the sibling underneath on
/// the way. Closing on the crossing snaps the target shut under the pointer;
/// waiting this long first lets the hand arrive. Long enough to cross a row at
/// an ordinary speed, short enough that a real move away does not leave a
/// stale panel hanging.
const SUBMENU_GRACE: Duration = Duration::from_millis(300);

/// The mark for each of [`ket_core::surface::installed_editors`]'s rows, by
/// position.
///
/// Built once. gpui keys a decoded image by a hash of its bytes and decodes
/// each key once, so handing the same `Arc` back every time the menu opens is
/// what keeps the icons from being decoded on every right-click. `None` where
/// the editor has no bundle to read a mark from — see `ket_core::app_icon`.
fn editor_icons() -> &'static [Option<Arc<Image>>] {
    static ICONS: OnceLock<Vec<Option<Arc<Image>>>> = OnceLock::new();
    ICONS.get_or_init(|| {
        ket_core::surface::installed_editors()
            .iter()
            .map(|found| {
                found
                    .icon
                    .clone()
                    .map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)))
            })
            .collect()
    })
}

/// A shared-menu instance and the tab it is describing.
pub(crate) struct TabContextMenu {
    menu: OpenMenu<TabContextAction>,
    target: TabContextTarget,
    /// Where the panel was put, so a submenu can be hung off one of its rows.
    /// The tab it belongs to is behind the open menu by then, and there is
    /// nothing else left on screen to anchor to — the same reason
    /// `WorktreeTarget` keeps one.
    at: Point<Pixels>,
}

/// A tab title being edited in the strip.
///
/// The tab is addressed the way [`TabContextTarget`] addresses one, plus the
/// worktree whose space holds it: selecting another worktree swaps the entire
/// pane tree, so a pane and an index on their own would point at whatever tab
/// happens to sit there in the space that replaced it.
pub(crate) struct TabRename {
    /// The space the tab belongs to.
    worktree: WorktreeId,
    /// The leaf whose strip the field is drawn in.
    pane: PaneId,
    /// The tab's position in that leaf.
    index: usize,
    /// The field holding the keyboard.
    ///
    /// An entity built when the rename starts and dropped when it ends, the
    /// way the editor's find bar owns its query line — a field is what macOS
    /// registers an input handler against, so it cannot be rebuilt with the
    /// strip around it. See [`crate::input`].
    input: Entity<TextInput>,
}

/// The sunken track the tabs sit in, matching a segmented switch's.
const TAB_TRACK_H: Pixels = px(34.0);

/// How wide a tab's rename field is.
///
/// Fixed rather than grown to fit what is typed: a field that widens shoves
/// every tab to its right along the strip, and a strip that reflows while you
/// are reading what you typed is worse than a long name scrolling inside its
/// own box — which is what [`text_line`] already does.
const RENAME_W: Pixels = px(150.0);

/// A tab's label.
///
/// One and a half pixels under `ui::LABEL`, which is what every other
/// control in the chrome takes. The strip is the one place a run of labels
/// sits shoulder to shoulder across the whole window, and at the shared size
/// it read as the loudest thing on screen rather than as the quiet index it is.
const TAB_LABEL: Pixels = px(11.0);

/// The tab menu's own width, which is also how far right its submenu starts.
const TAB_MENU_W: Pixels = px(280.0);

fn tab_context_origin(target: TabContextTarget) -> SharedString {
    format!("tab-context-{}-{}", target.pane.0, target.index).into()
}

fn tab_open_in_origin(target: TabContextTarget) -> SharedString {
    format!("tab-open-in-{}-{}", target.pane.0, target.index).into()
}

fn tab_snippets_origin(target: TabContextTarget) -> SharedString {
    format!("tab-snippets-{}-{}", target.pane.0, target.index).into()
}

/// The menu for one tab. `agent` adds **Send snippet**, which only an agent
/// session can take: typed into a plain shell, a prompt is a command line.
///
/// `pins` is whether each tab of the strip is pinned. It flips the pin row,
/// and a bulk close with only pinned tabs in its reach is disabled, since it
/// would pass over every one of them.
fn tab_context_menu(
    target: TabContextTarget,
    at: Point<Pixels>,
    pins: &[bool],
    document: bool,
    agent: bool,
) -> TabContextMenu {
    let pinned = pins.get(target.index).copied().unwrap_or(false);
    let unpinned = |pins: &[bool]| pins.iter().any(|pinned| !pinned);
    let right = unpinned(pins.get(target.index + 1..).unwrap_or_default());
    let left = unpinned(pins.get(..target.index).unwrap_or_default());
    let close_others = MenuItem::new(TabContextAction::CloseOthers, "Close Others");
    let close_to_right = MenuItem::new(TabContextAction::CloseToRight, "Close Tabs To The Right");
    let close_to_left = MenuItem::new(TabContextAction::CloseToLeft, "Close Tabs To The Left");
    // Disabled rather than dropped on a terminal or a browser, so the menu has
    // one shape whichever tab it was opened over — the rule most of the menu
    // follows, and the reason Close Tabs To The Right stays visible on the last
    // tab. There is nothing on disk behind those tabs to open.
    let open_in = MenuItem::new(TabContextAction::OpenIn, "Open in")
        .icon(Icon::ExternalLink)
        .submenu();
    let mut entries = vec![
        MenuEntry::Item(if document {
            open_in
        } else {
            open_in.disabled()
        }),
        MenuEntry::Item(if pinned {
            MenuItem::new(TabContextAction::PinTab, "Unpin Tab").icon(Icon::PinOff)
        } else {
            MenuItem::new(TabContextAction::PinTab, "Pin Tab").icon(Icon::Pin)
        }),
    ];
    // Dropped rather than disabled on anything but an agent session, like
    // Copy Path below: a shell or a document has no agent to send to at all.
    if agent {
        entries.push(MenuEntry::Item(
            MenuItem::new(TabContextAction::SendSnippet, "Send snippet")
                .icon(Icon::Layers)
                .submenu(),
        ));
    }
    entries.extend([
        MenuEntry::Separator,
        MenuEntry::Item(
            MenuItem::new(TabContextAction::Close, "Close")
                .icon(Icon::Close)
                .danger()
                .chord(Chord::parse("cmd+w").expect("the shipped chord parses")),
        ),
        MenuEntry::Item(if left || right {
            close_others
        } else {
            close_others.disabled()
        }),
        MenuEntry::Item(if right {
            close_to_right
        } else {
            close_to_right.disabled()
        }),
        MenuEntry::Item(if left {
            close_to_left
        } else {
            close_to_left.disabled()
        }),
    ]);
    // Dropped rather than disabled on a tab with no file behind it, unlike the
    // rest of the menu: a terminal or a browser has no path at all, so a
    // greyed-out Copy Path reads as a path that copying is merely unavailable
    // for right now, which is the wrong thing to promise.
    if document {
        entries.extend([
            MenuEntry::Separator,
            MenuEntry::Item(
                MenuItem::new(TabContextAction::CopyPath, "Copy Path").icon(Icon::Copy),
            ),
            MenuEntry::Item(
                MenuItem::new(TabContextAction::CopyRelativePath, "Copy Relative Path")
                    .icon(Icon::Copy),
            ),
        ]);
    }
    entries.extend([
        MenuEntry::Separator,
        MenuEntry::Item(
            MenuItem::new(TabContextAction::ChangeTitle, "Change Title")
                .icon(Icon::Pencil)
                .chord(Chord::parse("cmd+r").expect("the shipped chord parses")),
        ),
    ]);
    TabContextMenu {
        menu: OpenMenu::new(tab_context_origin(target), None, TAB_MENU_W, entries).at(at),
        target,
        at,
    }
}

/// The `+` menu's origin string for one pane — how a pane recognises the
/// open menu as its own.
fn new_tab_origin(pane: PaneId) -> SharedString {
    format!("new-tab-{}", pane.0).into()
}

/// The `+` button's menu for one pane.
///
/// `agents` is every agent Settings has enabled and this machine can run —
/// see `Shell::runnable_agents` — each drawn as its own row so that having
/// more than one enabled agent means seeing more than one here, rather than
/// only whichever one the worktree happens to be configured for. Omitted
/// entirely, heading included, when there are none: a menu with nothing to
/// start would rather say nothing than hold a disabled placeholder open
/// forever.
///
/// Two named sections, so each row says only what differs — "Terminal"
/// under "New tab", "Codex" under "Agent session". New Terminal never looks
/// at `agents`: it is deliberately a blank shell regardless of what is
/// enabled.
///
/// Every row carries a chord the shell dispatches app-wide, so the hints
/// are promises the keyboard keeps with the menu shut, not only while it
/// is open. Agent rows get theirs by position — see
/// `shortcuts::agent_session_chord`. `simulator` is whether this Mac has an
/// Xcode for the iPhone Simulator entry — see
/// `crate::ios_simulator::xcode_installed`. `android` is whether the Android
/// SDK and its emulator are installed.
/// The bugdroid green: Android's mark has no monochrome form, so the simulator
/// entry carries it the same way an agent's provider mark carries its brand
/// colour.
const ANDROID_GREEN: Rgba = Rgba {
    r: 0.239,
    g: 0.863,
    b: 0.518,
    a: 1.0,
};

fn new_tab_menu(
    pane: PaneId,
    agents: &[AgentSpec],
    simulator: bool,
    android: bool,
    t: &Theme,
) -> OpenMenu<MenuAction> {
    let chord = |text: &str| Chord::parse(text).expect("the shipped chord parses");
    let mut entries = vec![
        MenuEntry::Heading("New tab".into()),
        MenuEntry::Item(
            MenuItem::new(MenuAction::File(pane), "File")
                .icon(Icon::FileLines)
                .chord(chord("cmd+n")),
        ),
        MenuEntry::Item(
            MenuItem::new(MenuAction::Terminal(pane), "Terminal")
                .icon(Icon::TerminalWindow)
                .chord(chord("cmd+t")),
        ),
        MenuEntry::Item(
            MenuItem::new(MenuAction::NewBrowser(pane), "Browser")
                .icon(Icon::BrowserWindow)
                .chord(chord(crate::shortcuts::NEW_BROWSER_CHORD)),
        ),
    ];
    #[cfg(target_os = "macos")]
    if simulator {
        entries.push(MenuEntry::Item(
            MenuItem::new(MenuAction::IosSimulator(pane), "iPhone Simulator")
                .tinted_icon(Icon::Apple, paint(t.text.primary)),
        ));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = simulator;
    if android {
        entries.push(MenuEntry::Item(
            MenuItem::new(MenuAction::Android(pane), "Android Emulator")
                .tinted_icon(Icon::Android, ANDROID_GREEN),
        ));
    }

    if !agents.is_empty() {
        entries.push(MenuEntry::Heading("Agent session".into()));
        entries.extend(agents.iter().enumerate().map(|(index, spec)| {
            let (which, tint) = provider(&spec.name, t);
            let item = MenuItem::new(
                MenuAction::Agent(pane, spec.name.clone().into()),
                agent_label(&spec.name),
            )
            .tinted_icon(which, tint);
            MenuEntry::Item(match crate::shortcuts::agent_session_chord(index) {
                Some(text) => item.chord(chord(&text)),
                None => item,
            })
        }));
    }

    OpenMenu::new(
        new_tab_origin(pane),
        // No search row: a curated list, not a searchable one. Type-ahead
        // still narrows it.
        None,
        px(220.0),
        entries,
    )
    // Never scrolled: with the simulators and four agents it outgrew the
    // cap, and the last agent vanished below it.
    .uncapped()
}

impl Shell {
    /// Runs one `+`-menu pick.
    ///
    /// Also where the menu closes, so every path that hands an action back
    /// — click, Enter, the item's own chord — ends the menu exactly once.
    pub(crate) fn run_menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        match action {
            MenuAction::File(pane) => self.new_plain_text_file(pane),
            MenuAction::Agent(pane, name) => {
                self.focus_pane_before_opening(pane);
                self.open_agent_tab_in(pane, &name, cx);
            }
            MenuAction::Terminal(pane) => {
                self.focus_pane_before_opening(pane);
                self.open_terminal_tab_in(pane, cx);
            }
            MenuAction::NewBrowser(pane) => self.open_browser_tab_in(pane, cx),
            #[cfg(target_os = "macos")]
            MenuAction::IosSimulator(pane) => self.open_ios_simulator_tab_in(pane, cx),
            MenuAction::Android(pane) => self.open_android_tab_in(pane, cx),
        }
        self.menu = None;
        self.persist_layout();
    }

    /// Focuses `pane` before a `+`-menu pick opens a session tab in it.
    ///
    /// Into the pane the menu was opened from, which is the pane a person
    /// just split and is looking at. Focusing it first is not enough on its
    /// own: `open_terminal_tab_in`/`open_agent_tab_in` put the tab in the
    /// focused pane, and a session tab already open elsewhere in the space
    /// would simply be focused there instead — which looks exactly like the
    /// menu item doing nothing.
    fn focus_pane_before_opening(&mut self, pane: PaneId) {
        if let Some(id) = self.selected_id()
            && let Some(space) = self.spaces.get_mut(&id)
            && space.root.leaf(pane).is_some()
        {
            space.focused = pane;
        }
    }

    /// Whether the open menu, if any, belongs to this pane's `+` button.
    fn new_tab_menu_open(&self, pane: PaneId) -> bool {
        !self.modal_open()
            && self
                .menu
                .as_ref()
                .is_some_and(|menu| menu.opened_by(&new_tab_origin(pane)))
    }

    /// Opens the visual tab menu for one tab without changing focus or active
    /// selection. The stored target is the seam future commands will use.
    fn open_tab_context_menu(&mut self, pane: PaneId, index: usize, at: Point<Pixels>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(space) = self.spaces.get(&id) else {
            return;
        };
        let Some((tabs, _)) = space.root.leaf(pane) else {
            return;
        };
        if index >= tabs.len() {
            return;
        }
        let pins: Vec<bool> = tabs.iter().map(|tab| tab.pinned).collect();

        self.menu = None;
        self.worktree_menu = None;
        self.project_menu = None;
        self.path_menu = None;
        self.popup = None;
        self.tab_open_in = None;
        self.tab_snippets = None;
        let target = TabContextTarget { pane, index };
        let document = self.open_in_target(target).is_some();
        let agent = self.agent_terminal_at(target).is_some();
        self.tab_context_menu = Some(tab_context_menu(target, at, &pins, document, agent));
    }

    /// The agent session behind one tab, if it is one.
    ///
    /// A terminal tab with no agent is a plain shell, and gets nothing: see
    /// [`tab_context_menu`].
    fn agent_terminal_at(&self, target: TabContextTarget) -> Option<TerminalId> {
        let worktree = self.selected_id()?;
        let tab = self
            .spaces
            .get(&worktree)
            .and_then(|space| space.root.leaf(target.pane))
            .and_then(|(tabs, _)| tabs.get(target.index))?;
        match tab.kind {
            TabKind::Terminal(terminal)
                if self
                    .terminals
                    .get(&terminal)
                    .is_some_and(|handle| handle.agent.is_some()) =>
            {
                Some(terminal)
            }
            _ => None,
        }
    }

    /// The file behind one tab, and where in it — the two things **Open in**
    /// has to know before it can offer anywhere to send them.
    ///
    /// `None` for a terminal, a browser, and an untitled buffer that has never
    /// been saved: no one file stands behind any of them,
    /// and an editor handed a path that is not there opens an empty window
    /// named after a file that does not exist.
    ///
    /// An editor contributes its cursor, because opening at the line you were
    /// reading is the whole point of a precise handoff (see
    /// [`ket_core::surface`]); an audio file has no cursor to send.
    fn open_in_target(
        &self,
        target: TabContextTarget,
    ) -> Option<(SharedString, Option<(u32, u32)>)> {
        let worktree = self.selected_id()?;
        let tab = self
            .spaces
            .get(&worktree)
            .and_then(|space| space.root.leaf(target.pane))
            .and_then(|(tabs, _)| tabs.get(target.index))?;

        let (path, at) = match &tab.kind {
            TabKind::Editor(key) => {
                let state = self.editors.get(key.as_ref());
                // The buffer's own path first: a Save As moves the file out
                // from under the key the tab was opened with. Falling back to
                // the key covers a tab restored from a layout, whose editor
                // state is not built until the pane renders.
                let path = match state.and_then(EditorState::saved_path) {
                    Some(path) => path.display().to_string(),
                    None if !key.starts_with("untitled:") => key.to_string(),
                    None => return None,
                };
                let at = state.map(EditorState::cursor_line_col);
                (path, at)
            }
            TabKind::Audio(path) => (path.to_string(), None),
            // The file as it is now, which a deleted one no longer has.
            TabKind::Diff(key) if key.path().exists() => (key.path().display().to_string(), None),
            TabKind::Terminal(_) | TabKind::Browser(_) | TabKind::Diff(_) => return None,
            #[cfg(target_os = "macos")]
            TabKind::IosSimulator => return None,
            TabKind::Android => return None,
        };

        // Absolute or nothing: an editor is spawned with no working directory
        // of ket's choosing, so a relative path resolves against whatever the
        // process happened to start in.
        let path = match std::path::Path::new(&path).is_absolute() {
            true => path,
            false => Self::worktree_root(&worktree)?
                .join(&path)
                .display()
                .to_string(),
        };
        Some((path.into(), at))
    }

    /// Runs one tab-menu row.
    ///
    /// Taking the menu carries its original target forward: a row acts on the
    /// tab the menu was opened for, not on whichever one became active in the
    /// meantime — right-clicking an inactive tab deliberately leaves the
    /// active one alone.
    fn run_tab_context_action(&mut self, action: TabContextAction, cx: &mut Context<Self>) {
        let Some(open) = self.tab_context_menu.as_ref() else {
            return;
        };
        let target = open.target;

        // The one row that leaves its menu standing. A submenu is a branch of
        // the menu it hangs off, and dismissing the parent to show it would
        // take away the only thing saying what is being opened.
        if action == TabContextAction::OpenIn {
            // Already up from the pointer resting on the row: a click then
            // keeps it rather than rebuilding it under the reader.
            if self.tab_open_in.is_none() {
                self.open_tab_open_in_menu(target);
            }
            return;
        }
        if action == TabContextAction::SendSnippet {
            if self.tab_snippets.is_none() {
                self.open_tab_snippet_menu(target, cx);
            }
            return;
        }

        self.close_tab_menus();
        match action {
            TabContextAction::Close => {
                self.request_close_tab(target.pane, target.index, cx);
            }
            TabContextAction::PinTab => {
                let pinned = self
                    .space()
                    .and_then(|space| space.root.leaf(target.pane))
                    .and_then(|(tabs, _)| tabs.get(target.index))
                    .is_some_and(|tab| tab.pinned);
                self.set_tab_pinned(target.pane, target.index, !pinned, cx);
            }
            TabContextAction::CloseOthers => {
                self.close_tabs_around(target, |index| index != target.index, cx);
            }
            TabContextAction::CloseToRight => {
                self.close_tabs_around(target, |index| index > target.index, cx);
            }
            TabContextAction::CloseToLeft => {
                self.close_tabs_around(target, |index| index < target.index, cx);
            }
            TabContextAction::CopyPath => self.copy_tab_path(target, false, cx),
            TabContextAction::CopyRelativePath => self.copy_tab_path(target, true, cx),
            TabContextAction::ChangeTitle => self.begin_tab_rename(target, cx),
            // Both branch off the menu and returned above.
            TabContextAction::OpenIn | TabContextAction::SendSnippet => {}
        }
    }

    /// Follows the pointer down the tab menu.
    ///
    /// Resting on **Open in** opens its branch, the way a native submenu opens
    /// without a click. Resting on a sibling closes it again — after
    /// [`SUBMENU_GRACE`], and only if the pointer has not reached the branch
    /// or come back to its row by then, which is what `held` records. The
    /// timer is fire-and-forget: a branch that was closed some other way in
    /// the meantime, or reopened, is simply found in whatever state it is in.
    fn hover_tab_context_row(&mut self, action: TabContextAction, cx: &mut Context<Self>) {
        // The row's own branch opens, or is held if it is already up. Opening
        // one closes the other outright: two branches off one menu at once is
        // two answers to where the pointer is.
        let target = self.tab_context_menu.as_ref().map(|open| open.target);
        match action {
            TabContextAction::OpenIn => match self.tab_open_in.as_mut() {
                Some(open) => open.held = true,
                None => {
                    if let Some(target) = target {
                        self.tab_snippets = None;
                        self.open_tab_open_in_menu(target);
                        cx.notify();
                    }
                }
            },
            TabContextAction::SendSnippet => match self.tab_snippets.as_mut() {
                Some(open) => open.held = true,
                None => {
                    if let Some(target) = target {
                        self.tab_open_in = None;
                        self.open_tab_snippet_menu(target, cx);
                        cx.notify();
                    }
                }
            },
            _ => {}
        }

        // Any branch this row does not own belongs to a sibling, and lets go.
        let mut releasing = false;
        if action != TabContextAction::OpenIn
            && let Some(open) = self.tab_open_in.as_mut()
        {
            open.held = false;
            releasing = true;
        }
        if action != TabContextAction::SendSnippet
            && let Some(open) = self.tab_snippets.as_mut()
        {
            open.held = false;
            releasing = true;
        }
        if !releasing {
            return;
        }
        cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
            cx.background_executor().timer(SUBMENU_GRACE).await;
            let _ = this.update(cx, |shell, cx| {
                let mut closed = false;
                if shell.tab_open_in.as_ref().is_some_and(|open| !open.held) {
                    shell.tab_open_in = None;
                    closed = true;
                }
                if shell.tab_snippets.as_ref().is_some_and(|open| !open.held) {
                    shell.tab_snippets = None;
                    closed = true;
                }
                if closed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Where a branch hung off `action`'s row starts: beside the row, level
    /// with it.
    ///
    /// Measured from where the parent was *asked* to go, which is where it is
    /// unless it was opened close enough to an edge to be snapped back inside
    /// the window; the branch snaps too, so the worst that costs is a panel
    /// sitting a little off its row rather than one off-screen.
    fn submenu_anchor(parent: &TabContextMenu, action: TabContextAction) -> Point<Pixels> {
        let row = parent
            .menu
            .sections()
            .iter()
            .flatten()
            .position(|item| item.action == action)
            .unwrap_or(0);
        Point {
            x: parent.at.x + TAB_MENU_W - SUBMENU_OVERLAP,
            y: parent.at.y + parent.menu.row_top(row),
        }
    }

    /// Hangs the **Send snippet** submenu off its own row.
    fn open_tab_snippet_menu(&mut self, target: TabContextTarget, cx: &mut Context<Self>) {
        let Some(terminal) = self.agent_terminal_at(target) else {
            return;
        };
        let snippets = self.load_snippets(cx);
        let Some(parent) = self.tab_context_menu.as_ref() else {
            return;
        };
        let anchor = Self::submenu_anchor(parent, TabContextAction::SendSnippet);
        let mut open = SnippetMenu::new(tab_snippets_origin(target), snippets, false, None);
        open.menu = open.menu.at(anchor);
        self.tab_snippets = Some(TabSnippetMenu {
            open,
            terminal,
            held: true,
        });
    }

    /// Sends the picked snippet to the tab's agent: pasted, then submitted,
    /// as the quick prompt sends a prompt. Both menus close.
    fn run_tab_snippet(&mut self, pick: SnippetPick, cx: &mut Context<Self>) {
        let Some(open) = self.tab_snippets.take() else {
            return;
        };
        self.tab_context_menu = None;
        if pick == SnippetPick::Manage {
            self.manage_snippets(cx);
            return;
        }
        if let Some(snippet) = open.open.picked(pick) {
            let body = snippet.body.clone();
            self.submit_to_terminal(open.terminal, &body, cx);
        }
    }

    /// Routes keys to the **Send snippet** submenu, the way
    /// [`Shell::tab_open_in_key`] does for its sibling.
    pub(crate) fn tab_snippets_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.tab_snippets.as_mut() else {
            return false;
        };
        match open.open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.tab_snippets = None,
            MenuKey::Run(pick) => self.run_tab_snippet(pick, cx),
        }
        cx.notify();
        true
    }

    /// Renders the **Send snippet** submenu.
    pub(crate) fn tab_snippets_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.tab_snippets.as_ref()?;
        Some(open.open.menu.view_with_hover(
            &self.theme,
            cx,
            |shell, pick, cx| shell.run_tab_snippet(*pick, cx),
            |shell, cx| {
                shell.tab_snippets = None;
                cx.notify();
            },
            |shell, _, _| {
                if let Some(open) = shell.tab_snippets.as_mut() {
                    open.held = true;
                }
            },
        ))
    }

    /// Hangs the **Open in** submenu off its own row.
    ///
    /// Beside the row rather than under the pointer: the pointer is wherever
    /// the reader happened to click along a 280px row, and a panel that appears
    /// in a different place each time reads as a new menu rather than a branch
    /// of the one already open.
    fn open_tab_open_in_menu(&mut self, target: TabContextTarget) {
        let Some(parent) = self.tab_context_menu.as_ref() else {
            return;
        };
        let Some((path, at)) = self.open_in_target(target) else {
            return;
        };

        // Each editor wears its own mark, read from its bundle on this machine
        // rather than drawn into ket — see `ket_core::app_icon` — so the row
        // shows what a person knows from their Dock without this shell
        // shipping anyone's trademark, the rule that made file marks
        // lettermarks rather than language logos (see `ui::filetype`). One
        // found on `PATH` outside any bundle gets a generic glyph instead: an
        // empty column in a list where the others have marks reads as an
        // image that failed to load. Finder keeps its folder because it is the
        // one row here that does something different, and sits behind a
        // hairline for the same reason: revealing a file is not opening it.
        let mut entries: Vec<MenuEntry<OpenInApp>> = ket_core::surface::installed_editors()
            .iter()
            .zip(editor_icons())
            .map(|(found, icon)| {
                let item = MenuItem::new(OpenInApp::Editor(found), found.profile.label);
                MenuEntry::Item(match icon {
                    Some(icon) => item.image(Arc::clone(icon)),
                    None => item.icon(Icon::Code),
                })
            })
            .collect();
        if let Some(name) = ket_core::surface::FILE_MANAGER {
            if !entries.is_empty() {
                entries.push(MenuEntry::Separator);
            }
            entries.push(MenuEntry::Item(
                MenuItem::new(OpenInApp::FileManager, name).icon(Icon::Folder),
            ));
        }

        let anchor = Self::submenu_anchor(parent, TabContextAction::OpenIn);

        self.tab_open_in = Some(TabOpenInMenu {
            menu: OpenMenu::new(tab_open_in_origin(target), None, SUBMENU_W, entries).at(anchor),
            path,
            at,
            // Opened from its own row, by pointer or by key: nothing has moved
            // away from it yet.
            held: true,
        });
    }

    /// Hands the tab's file to the app that was picked.
    ///
    /// Both menus close: the branch has been taken, and leaving the parent open
    /// behind a window that just came up in front of ket would be a menu nobody
    /// asked to keep.
    fn run_open_in(&mut self, app: OpenInApp) {
        let Some(open) = self.tab_open_in.take() else {
            return;
        };
        self.tab_context_menu = None;
        let path = std::path::PathBuf::from(open.path.as_ref());

        // Each arm words its own success. Finder was *shown* the file and an
        // editor was asked to *open* it, and a single sentence covering both
        // would have to be vague about which happened.
        let outcome = match app {
            OpenInApp::FileManager => ket_core::surface::reveal_in_file_manager(&path).map(|()| {
                format!(
                    "showed in {}",
                    ket_core::surface::FILE_MANAGER.unwrap_or("the file manager")
                )
            }),
            OpenInApp::Editor(found) => {
                let mut position = Position::file(path);
                if let Some((line, column)) = open.at {
                    position = position.with_line(line).with_column(column);
                }
                ExternalSurface::for_program(&found.program, Arc::new(ProcessSpawner))
                    .reveal(&position)
                    .map(|()| format!("opened in {}", found.profile.label))
            }
        };

        self.note = Some(match outcome {
            Ok(message) => message.into(),
            Err(e) => format!("{e}").into(),
        });
    }

    /// Routes keys to the **Open in** submenu, above the menu it hangs off.
    ///
    /// Escape closes the branch and leaves the parent standing, which is what
    /// a submenu's Escape does everywhere: one level back, not all the way out.
    pub(crate) fn tab_open_in_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.tab_open_in.as_mut() else {
            return false;
        };
        match open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.tab_open_in = None,
            MenuKey::Run(app) => self.run_open_in(app),
        }
        cx.notify();
        true
    }

    /// Renders the **Open in** submenu.
    pub(crate) fn tab_open_in_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.tab_open_in.as_ref()?;
        Some(open.menu.view_with_hover(
            &self.theme,
            cx,
            |shell, app, _| shell.run_open_in(*app),
            |shell, cx| {
                shell.tab_open_in = None;
                cx.notify();
            },
            // The pointer arrived: whatever sibling it crossed on the way was
            // a crossing. No redraw — nothing visible changed.
            |shell, _, _| {
                if let Some(open) = shell.tab_open_in.as_mut() {
                    open.held = true;
                }
            },
        ))
    }

    /// Copies the tab's file path to the clipboard.
    ///
    /// `relative` gives the path from the worktree root — what a person pastes
    /// into a commit message, a review, or a note to somebody with their own
    /// checkout, where an absolute path naming this machine's home directory is
    /// noise at best and wrong at worst.
    ///
    /// A path that is somehow not under the worktree falls back to the absolute
    /// one rather than to a chain of `../`: a relative path that climbs out of
    /// the tree is not what anybody asked for, and the absolute one is at least
    /// true.
    fn copy_tab_path(&mut self, target: TabContextTarget, relative: bool, cx: &mut Context<Self>) {
        let Some((path, _)) = self.open_in_target(target) else {
            return;
        };

        let copied = match relative {
            false => path.to_string(),
            true => self
                .selected_id()
                .and_then(|worktree| Self::worktree_root(&worktree))
                .and_then(|root| {
                    std::path::Path::new(path.as_ref())
                        .strip_prefix(&root)
                        .ok()
                        .map(|rela| rela.display().to_string())
                })
                .unwrap_or_else(|| path.to_string()),
        };

        cx.write_to_clipboard(ClipboardItem::new_string(copied));
    }

    /// Puts a field over one tab's label, with its current title selected.
    ///
    /// Selected rather than appended to, because a rename replaces the name
    /// that is there — the rule every field opened on a value follows. See
    /// [`TextInput::with_text`].
    fn begin_tab_rename(&mut self, target: TabContextTarget, cx: &mut Context<Self>) {
        let Some(worktree) = self.selected_id() else {
            return;
        };
        let Some(title) = self
            .spaces
            .get(&worktree)
            .and_then(|space| space.root.leaf(target.pane))
            .and_then(|(tabs, _)| tabs.get(target.index))
            .map(|tab| tab.title.clone())
        else {
            return;
        };

        let input = TextInput::with_text("Tab name", title.as_ref(), cx);
        input.update(cx, |input, _| input.request_focus());
        // A typed character does not arrive through the key handler — it comes
        // back later from macOS's input context — so without this the strip
        // keeps painting the text it had when the field opened.
        self.watch_field(&input, cx);
        self.tab_rename = Some(TabRename {
            worktree,
            pane: target.pane,
            index: target.index,
            input,
        });
    }

    /// Gives the tab the name that was typed.
    ///
    /// A blank field abandons the rename instead of leaving a nameless tab:
    /// [`Tab::title`] is the only name a tab has — nothing keeps the filename
    /// or the running agent's name to fall back to — so an empty one would
    /// leave a tab there is no way to point at.
    fn commit_tab_rename(&mut self, cx: &mut Context<Self>) {
        let Some(rename) = self.tab_rename.take() else {
            return;
        };
        let typed = rename.input.read(cx).text().trim().to_owned();
        if typed.is_empty() {
            return;
        }
        let Some((tabs, _)) = self
            .spaces
            .get_mut(&rename.worktree)
            .and_then(|space| space.root.leaf_mut(rename.pane))
        else {
            return;
        };
        let Some(tab) = tabs.get_mut(rename.index) else {
            return;
        };
        if tab.title.as_ref() == typed {
            return;
        }
        tab.title = typed.into();
        tab.renamed = true;
        self.persist_layout();
    }

    /// Abandons a rename, leaving the title as it was.
    ///
    /// The field is dropped rather than emptied: it is the only thing holding
    /// the keyboard, and the shell takes the window's handle back once nothing
    /// is — see [`Self::tab_rename_open`].
    fn cancel_tab_rename(&mut self) {
        self.tab_rename = None;
    }

    /// Whether a tab title is being edited, and so holding the keyboard.
    ///
    /// Asked by `Shell::typing`, which is what keeps the window from taking
    /// the focus back out from under the field on the next frame.
    pub(crate) fn tab_rename_open(&self) -> bool {
        self.tab_rename.is_some()
    }

    /// Keys while a tab is being renamed: the field takes its own editing
    /// chords, Enter commits, Escape abandons.
    pub(crate) fn tab_rename_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(input) = self.tab_rename.as_ref().map(|rename| rename.input.clone()) else {
            return false;
        };

        // Reported as handled so the shell's own shortcuts do not also fire,
        // but never consumed: the character has to keep travelling until
        // macOS's input context sees it. See [`crate::input`].
        if is_text(&event.keystroke) {
            return true;
        }
        if input.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }

        match event.keystroke.key.as_str() {
            "escape" => self.cancel_tab_rename(),
            "enter" => self.commit_tab_rename(cx),
            // Anything else reaches the shell: a rename is one field in a
            // strip, not a modal — the same rule the panel's search box
            // follows.
            _ => return false,
        }
        true
    }

    /// Closes the tab menu and any branch hanging off it.
    ///
    /// One call rather than two fields cleared at each of the seven places
    /// that dismiss a tab menu: a submenu left standing after its parent has
    /// gone is a panel describing a row nobody can see any more.
    pub(crate) fn close_tab_menus(&mut self) {
        self.tab_context_menu = None;
        self.tab_open_in = None;
        self.tab_snippets = None;
    }

    /// Routes keyboard navigation to the tab context menu while it is open.
    pub(crate) fn tab_context_menu_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.tab_context_menu.as_mut() else {
            return false;
        };
        match open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => return true,
            MenuKey::Close => self.close_tab_menus(),
            MenuKey::Run(action) => self.run_tab_context_action(action, cx),
        }
        cx.notify();
        true
    }

    /// Renders the tab context menu.
    pub(crate) fn tab_context_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.tab_context_menu.as_ref()?;
        Some(open.menu.view_with_hover(
            &self.theme,
            cx,
            |shell, action, cx| shell.run_tab_context_action(*action, cx),
            |shell, cx| {
                shell.close_tab_menus();
                cx.notify();
            },
            |shell, action, cx| shell.hover_tab_context_row(*action, cx),
        ))
    }

    /// Closes one tab — asking first when it is a terminal running a CLI
    /// agent session, since that closes over a live conversation rather than
    /// an empty pty. Every other tab kind, and a plain shell, close at once.
    pub(crate) fn request_close_tab(&mut self, pane: PaneId, index: usize, cx: &mut Context<Self>) {
        let session = self
            .space()
            .and_then(|space| space.root.leaf(pane))
            .and_then(|(tabs, _)| tabs.get(index))
            .and_then(|tab| match tab.kind {
                TabKind::Terminal(terminal) => Some(terminal),
                _ => None,
            })
            .and_then(|terminal| {
                let agent = self.terminals.get(&terminal)?.agent.as_deref()?;
                Some((terminal, agent_label(agent).into()))
            });

        match session {
            Some((terminal, agent)) => {
                self.menu = None;
                self.confirm_close_session = Some(CloseSessionConfirm { terminal, agent });
            }
            None => self.close_tab_now(pane, index),
        }
        cx.notify();
    }

    /// Pins or unpins one tab — see [`Space::set_pinned`].
    pub(crate) fn set_tab_pinned(
        &mut self,
        pane: PaneId,
        index: usize,
        pinned: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if !self
            .spaces
            .get_mut(&id)
            .is_some_and(|space| space.set_pinned(pane, index, pinned))
        {
            return;
        }
        self.close_tab_menus();
        // The tab moves along the strip, and a rename addresses its tab by
        // position. Same reasoning as `close_tab_now`.
        self.cancel_tab_rename();
        self.persist_layout();
        cx.notify();
    }

    /// Closes the tabs of the menu's strip whose index `sweep` picks — the
    /// bulk closes: Close Others, and Close Tabs To The Right or Left.
    ///
    /// Pinned tabs are passed over without a word: staying put through a
    /// sweep is what pinning one asked for. Tabs running an agent session are
    /// left open too. Closing one asks first, because it ends a live
    /// conversation, and a menu row that raised a confirmation per session —
    /// or ended them all on a single click — would be the wrong side of that.
    /// Those are said, so a strip that is not as short as expected is
    /// explained rather than mysterious.
    ///
    /// Closed from the rightmost down, so the indices still to be closed do
    /// not move under the ones already gone.
    fn close_tabs_around(
        &mut self,
        target: TabContextTarget,
        sweep: impl Fn(usize) -> bool,
        cx: &mut Context<Self>,
    ) {
        let Some((tabs, _)) = self.space().and_then(|space| space.root.leaf(target.pane)) else {
            return;
        };

        let mut closing = Vec::new();
        let mut kept = 0usize;
        for (index, tab) in tabs.iter().enumerate() {
            if !sweep(index) || tab.pinned {
                continue;
            }
            let is_session = match tab.kind {
                TabKind::Terminal(terminal) => self
                    .terminals
                    .get(&terminal)
                    .is_some_and(|terminal| terminal.agent.is_some()),
                _ => false,
            };
            if is_session {
                kept += 1;
            } else {
                closing.push(index);
            }
        }

        for index in closing.into_iter().rev() {
            self.close_tab_now(target.pane, index);
        }

        if kept > 0 {
            let noun = if kept == 1 { "session" } else { "sessions" };
            self.toast(
                crate::ui::toast::Tone::Info,
                format!("Kept {kept} agent {noun} open"),
                cx,
            );
        }
        cx.notify();
    }

    /// Closes the active tab for the app-level Cmd-W action.
    pub(crate) fn close_tab_from_shortcut(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tab_shortcut_blocked() {
            return false;
        }
        if let Some(target) = self.tab_context_menu.as_ref().map(|open| open.target) {
            self.request_close_tab(target.pane, target.index, cx);
            return true;
        }
        let Some(space) = self.space() else {
            return false;
        };
        let pane = space.focused;
        let Some((_, active)) = space.root.leaf(pane) else {
            return false;
        };
        self.request_close_tab(pane, active, cx);
        true
    }

    /// Opens a fresh blank terminal in the focused pane for Cmd-T.
    pub(crate) fn new_terminal_tab_from_shortcut(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.tab_creation_target() else {
            return false;
        };
        self.open_terminal_tab_in(pane, cx);
        true
    }

    /// Opens a new in-memory editor in the focused pane for Cmd-N.
    pub(crate) fn new_blank_document_from_shortcut(&mut self) -> bool {
        let Some(pane) = self.tab_creation_target() else {
            return false;
        };
        self.new_plain_text_file(pane);
        true
    }

    /// Opens a browser tab in the focused pane for the `+` menu's Browser
    /// chord.
    pub(crate) fn new_browser_tab_from_shortcut(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self.tab_creation_target() else {
            return false;
        };
        self.open_browser_tab_in(pane, cx);
        self.persist_layout();
        true
    }

    /// Starts a fresh session with the runnable agent at `index` — the
    /// `+` menu's order — in the focused pane.
    pub(crate) fn new_agent_session_from_shortcut(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.tab_creation_target() else {
            return false;
        };
        let Some(spec) = self.runnable_agents().into_iter().nth(index) else {
            return false;
        };
        self.open_agent_tab_in(pane, &spec.name, cx);
        self.persist_layout();
        true
    }

    /// The pane new-tab shortcuts should address while chrome owns the keys.
    fn tab_creation_target(&self) -> Option<PaneId> {
        if self.tab_shortcut_blocked() || self.tab_context_menu.is_some() {
            return None;
        }
        self.space().map(|space| space.focused)
    }

    /// Whether another surface currently owns app-level tab commands.
    fn tab_shortcut_blocked(&self) -> bool {
        self.modal_open()
            || self.tab_rename.is_some()
            || self.menu.is_some()
            || self.worktree_menu.is_some()
            || self.token_reduction_menu.is_some()
            || self.worktree_snippet_menu.is_some()
            || self.project_menu.is_some()
            || self.popup.is_some()
            || self.palette.open
            || self.finder.open
    }

    /// The actual removal — reached directly for anything that needs no
    /// confirmation, and from [`Self::close_session_confirmed`] for anything
    /// that just got one.
    fn close_tab_now(&mut self, pane: PaneId, index: usize) {
        let terminal = self
            .space()
            .and_then(|space| space.root.leaf(pane))
            .and_then(|(tabs, _)| tabs.get(index))
            .and_then(|tab| match tab.kind {
                TabKind::Terminal(terminal) => Some(terminal),
                _ => None,
            });
        if let Some(terminal) = terminal {
            self.close_terminal(terminal);
            self.menu = None;
            self.close_tab_menus();
            self.cancel_tab_rename();
            return;
        }
        if let Some(id) = self.selected_id() {
            let closing_audio = self
                .spaces
                .get(&id)
                .and_then(|space| space.root.leaf(pane))
                .and_then(|(tabs, _)| tabs.get(index))
                .and_then(|tab| match &tab.kind {
                    TabKind::Audio(path) => Some(path.clone()),
                    _ => None,
                });
            if let Some(path) = closing_audio {
                self.audio.stop_if_active(path.as_ref());
            }
            if let Some(space) = self.spaces.get_mut(&id) {
                space.close_tab(pane, index);
            }
        }
        self.prune_browsers();
        #[cfg(target_os = "macos")]
        self.prune_ios_simulator();
        self.prune_android();
        self.prune_diffs();
        self.menu = None;
        self.close_tab_menus();
        // A rename addresses its tab by position, and closing one moves every
        // tab after it along. Rather than working out whether this particular
        // removal moved the one being renamed, the rename goes — the reader
        // has just done something else with the strip.
        self.cancel_tab_rename();
        self.persist_layout();
    }

    /// Kills the session and closes its tab, once the reader has agreed to.
    pub(crate) fn close_session_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_close_session.take() else {
            return;
        };
        // Closes the tab as a side effect — see `Self::close_terminal`.
        self.close_terminal(confirm.terminal);
        cx.notify();
    }

    /// Handles a key while the confirmation is up.
    pub(crate) fn close_session_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.confirm_close_session.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.confirm_close_session = None,
            "enter" => self.close_session_confirmed(cx),
            _ => {}
        }
        true
    }

    /// The confirmation dialog, or nothing when it is closed.
    pub(crate) fn close_session_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm_close_session.as_ref()?;
        let t = &self.theme;

        let cancel = button("cancel-close-session", "Cancel")
            .ghost()
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| {
                this.confirm_close_session = None;
                cx.notify();
            }));

        let end = button("confirm-close-session", "End Session")
            .danger()
            .leading(Icon::Close)
            .render(t)
            .on_click(cx.listener(|this, _, _, cx| this.close_session_confirmed(cx)));

        Some(
            centered(
                card("close-session", px(440.0), t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(header(format!("End {} session?", confirm.agent), t))
                    .child(body(
                        format!(
                            "Closing this tab ends the {} session running in it. \
                             Anything it has not saved elsewhere is lost.",
                            confirm.agent
                        ),
                        t,
                    ))
                    .child(footer().child(cancel).child(end)),
            )
            .into_any_element(),
        )
    }
}

/// What a tab is showing.
#[derive(Clone, PartialEq, Eq)]
pub enum TabKind {
    /// A tweak editor open on one file, named by its absolute path.
    Editor(SharedString),
    /// An audio file viewer, named by its absolute path.
    Audio(SharedString),
    /// An interactive shell in the worktree's directory.
    ///
    /// Carries its own id: two terminal tabs are two terminals, and without
    /// something to tell them apart they are the same tab twice.
    Terminal(TerminalId),
    /// An ephemeral system-webview tab.
    Browser(BrowserId),
    /// Xcode's already-booted iPhone Simulator, via the POC.
    #[cfg(target_os = "macos")]
    IosSimulator,
    /// A running Android emulator's screen, drawn from its frames.
    Android,
    /// One file's diff, read-only. See `crate::diff_view`.
    Diff(crate::diff_view::DiffKey),
}

/// One tab in a worktree's space.
#[derive(Clone, PartialEq, Eq)]
pub struct Tab {
    /// What the tab shows in its label.
    pub(crate) title: SharedString,
    /// What it renders.
    pub(crate) kind: TabKind,
    /// Whether [`Self::title`] was typed by a person rather than derived from
    /// what the tab shows.
    ///
    /// The strip does not always draw the stored title: a lone terminal takes
    /// the label of the agent running in it, which is the right name for a tab
    /// nobody has named and the wrong one for a tab somebody just did. This is
    /// what tells those two apart, and what decides whether the title is worth
    /// saving in the layout — see [`crate::layout`].
    pub(crate) renamed: bool,
    /// Whether the tab is pinned.
    ///
    /// Pinned tabs lead their strip as one unbroken run — every placement goes
    /// through [`pinned_run`] to keep it so — and the bulk closes in the tab
    /// menu pass them over. Closing one by name still closes it: a pin keeps a
    /// tab out of the way of a sweep, not out of reach of the reader.
    pub(crate) pinned: bool,
}

/// How many tabs at the head of a strip are pinned.
///
/// The pinned tabs are always exactly this prefix, so this is also where the
/// unpinned run starts, and the one place a tab can move to when it changes
/// sides.
pub(crate) fn pinned_run(tabs: &[Tab]) -> usize {
    tabs.iter().take_while(|tab| tab.pinned).count()
}

/// Where a tab can actually land when it is aimed at gap `at` of a strip
/// whose pinned run is `run` long: inside that run for a pinned tab, past it
/// for any other. Aiming across the boundary lands on it rather than being
/// refused, so a drag still goes as far as it is allowed to.
fn gap_for(at: usize, pinned: bool, run: usize) -> usize {
    if pinned { at.min(run) } else { at.max(run) }
}

/// The "are you sure" step before closing a tab that is running a CLI agent
/// session.
///
/// Closing a blank terminal or any other tab kind needs no such thing —
/// there is nothing running in it that a click would lose. Closing this one
/// kills the pty, and with it whatever the agent has not written anywhere
/// else, so it asks first the same way removing a worktree does — see
/// `worktree_menu::RemoveWorktreeConfirm`.
pub(crate) struct CloseSessionConfirm {
    /// The terminal that would be killed.
    terminal: TerminalId,
    /// Its agent, in the words a person reads rather than the catalogue's
    /// raw name — see `crate::ui::agent::agent_label`.
    agent: SharedString,
}

/// Stable identity for a leaf while its ancestors are reshaped.
///
/// An index path would change whenever a sibling disappears, which would make
/// focus jump just when closing a pane is already changing the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PaneId(pub(crate) u64);

/// Smallest reachable extent of a pane along the axis that splits it.
///
/// A pane smaller than this no longer has a reliably clickable tab strip, so
/// dragging stops here rather than allowing content to disappear.
const MIN_PANE_SIZE: Pixels = px(160.0);

/// Hit target between panes. Kept narrow enough not to consume useful content.
const DIVIDER_SIZE: Pixels = crate::tree::DIVIDER_LINE;

#[derive(Clone, PartialEq, Eq)]
struct PaneResize {
    path: Vec<usize>,
    divider: usize,
}

impl Render for PaneResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// How much of a pane's width or height each of its four edge bands takes.
const EDGE_FRACTION: f32 = 0.22;

/// The narrowest an edge band gets, so a small pane can still be aimed at.
const EDGE_MIN: Pixels = px(48.0);

/// The widest, so a large pane keeps a middle worth aiming at.
const EDGE_MAX: Pixels = px(220.0);

/// A tab picked up to be dropped somewhere else in the pane tree.
///
/// Carries where the tab was rather than the tab itself. A strip can change
/// under a drag that is still in the air — an agent opening a session, another
/// pane collapsing — so what actually moves is resolved again at the drop; see
/// `Shell::dragged_tab`.
#[derive(Clone)]
pub(crate) struct TabDrag {
    /// The space the drag began in.
    ///
    /// A tab cannot cross worktrees, and pane ids are per-space, so a drag
    /// that outlived a change of selection names panes in a tree that is no
    /// longer on screen.
    pub(crate) worktree: WorktreeId,
    /// The leaf the tab is being taken from.
    pub(crate) pane: PaneId,
    /// Where it sat in that leaf's strip when the drag began.
    pub(crate) index: usize,
    /// What it shows, which is how it is found again if the strip has shifted.
    pub(crate) kind: TabKind,
    /// Whether it is pinned, which decides the gaps it can land in — see
    /// [`gap_for`].
    pub(crate) pinned: bool,
    /// The label, drawn under the cursor.
    pub(crate) title: SharedString,
    /// The theme. The ghost renders in its own view, with no access to the
    /// shell's.
    pub(crate) theme: Theme,
    /// Where inside the tab the pointer was when the drag began.
    ///
    /// Filled in by the listener, which is where gpui says how far into the
    /// tab the pointer was. The ghost needs it to undo it — see `Render`.
    pub(crate) grab: Point<Pixels>,
}

/// The ghost's height, and the part of it the cursor holds.
///
/// The cursor sits [`GHOST_GRAB_X`] in from the leading edge and halfway down,
/// whatever the tab it came from was like. A constant, because the whole point
/// is that it stops depending on where the tab was grabbed.
const GHOST_H: Pixels = px(24.0);

/// How far into the ghost the cursor holds it.
const GHOST_GRAB_X: Pixels = px(14.0);

impl Render for TabDrag {
    /// The label, and nothing else — the tab stays in its strip until the drop
    /// lands, so what follows the cursor only has to say which one is moving.
    ///
    /// **It has to undo gpui's placement to do that.** gpui draws a drag view
    /// with its top-left at `pointer - grab`, where `grab` is how far into the
    /// *dragged element* the pointer was. That is right when the thing under
    /// the cursor is the thing that was picked up, and this one is not: a tab
    /// is as wide as its icon, its label and its close button make it, and the
    /// ghost is a bare chip narrower than most of them and 24px tall against
    /// the strip's 36. Grabbing a wide tab anywhere near its close button
    /// therefore placed the whole ghost to the *left* of the cursor with clear
    /// air between the two, and how far off it sat changed with every tab and
    /// every grab. That is the drag that felt broken.
    ///
    /// So the offset is taken back out — the wrapper's origin is
    /// `pointer - grab`, so a child at `+grab` is exactly on the pointer — and
    /// the chip is hung off one fixed anchor instead. Same trick as the
    /// sidebar's ghost, which re-anchors to stay in its column; this one only
    /// has to stay under the cursor, and unlike a row a tab may legitimately
    /// be carried anywhere in the window, so it needs no clamping.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div().relative().child(
            div()
                .absolute()
                .left(self.grab.x - GHOST_GRAB_X)
                .top(self.grab.y - GHOST_H / 2.0)
                .flex()
                .items_center()
                .h(GHOST_H)
                .px(px(10.0))
                .rounded(crate::ui::RADIUS_LG)
                .bg(paint(t.elevated))
                .border_1()
                .border_color(paint(t.border))
                // Black, like every other lifted thing in the shell. This was
                // the window's own ink at 0.18 — bone white — which on a
                // near-black ground is not a shadow at all but a halo, and the
                // one element in the chrome that glowed.
                .shadow(vec![BoxShadow {
                    color: crate::paint::shadow(0.5, &t),
                    offset: point(px(0.0), px(2.0)),
                    blur_radius: px(12.0),
                    spread_radius: px(0.0),
                }])
                .prose()
                .text_size(TAB_LABEL)
                .text_color(paint(t.text.primary))
                .child(self.title.clone()),
        )
    }
}

/// A pane's content area: everything it occupies but its tab strip.
///
/// The strip is a drop target in its own right — it names a gap between two
/// tabs — so the body's edge bands are measured against what is left once it
/// is taken off the top, and the wash is drawn there too. Otherwise the top
/// band would sit under the strip, and the pointer would be offered a division
/// while it hovered a gap.
fn body_area(pane: Bounds<Pixels>) -> Bounds<Pixels> {
    let strip = crate::tree::TOP_STRIP.min(pane.size.height);
    Bounds {
        origin: point(pane.origin.x, pane.origin.y + strip),
        size: size(pane.size.width, pane.size.height - strip),
    }
}

/// Which drop a pointer inside a pane's body is asking for.
///
/// Every edge offers its distance as a fraction of its own band, and the
/// smallest fraction under one wins. Comparing raw distances instead would
/// make a wide, short pane resolve both of its corners to a horizontal
/// divider, because in such a pane the pointer is nearer the top or bottom
/// edge almost everywhere.
fn body_drop(pane: PaneId, bounds: Bounds<Pixels>, at: Point<Pixels>) -> TabDrop {
    let band = |extent: Pixels| {
        let wanted = extent * EDGE_FRACTION;
        if wanted < EDGE_MIN {
            EDGE_MIN
        } else if wanted > EDGE_MAX {
            EDGE_MAX
        } else {
            wanted
        }
    };
    let across = band(bounds.size.width);
    let down = band(bounds.size.height);

    let edges = [
        (at.x - bounds.left(), across, Axis::Horizontal, true),
        (bounds.right() - at.x, across, Axis::Horizontal, false),
        (at.y - bounds.top(), down, Axis::Vertical, true),
        (bounds.bottom() - at.y, down, Axis::Vertical, false),
    ];

    // What this pane has left to give along each axis. A pane divided below
    // `MIN_PANE_SIZE` has no reliably clickable tab strip left, and a pane
    // that cannot be clicked is one that cannot be closed — so an edge with
    // no room behind it stops offering a division and offers the pane itself
    // instead. Height counts the strip the body does not include, because the
    // minimum is about a whole pane rather than about its content area.
    let divisible = |extent: Pixels| extent >= MIN_PANE_SIZE * 2.0;
    let across_fits = divisible(bounds.size.width);
    // `bounds` is the body, so the strip both panes would get goes back on.
    let down_fits = divisible(bounds.size.height + crate::tree::TOP_STRIP);

    let mut nearest: Option<(f32, Axis, bool)> = None;
    for (distance, band, axis, before) in edges {
        let fits = match axis {
            Axis::Horizontal => across_fits,
            Axis::Vertical => down_fits,
        };
        if !fits || band <= px(0.0) || distance >= band {
            continue;
        }
        let share = distance / band;
        if nearest.is_none_or(|(smallest, _, _)| share < smallest) {
            nearest = Some((share, axis, before));
        }
    }

    match nearest {
        Some((_, axis, before)) => TabDrop::Split { pane, axis, before },
        None => TabDrop::Into { pane },
    }
}

/// The wash showing what a dragged tab would take if it landed here.
///
/// `accent` rather than `selection`, so the preview does not read as the
/// focused-pane border it is drawn inside. A split shows the half it would
/// take rather than the share it would really get: in the flattening case that
/// share depends on how many children the *parent* has, which a pane cannot
/// see from inside itself, and half is what every editor with this gesture
/// draws.
fn drop_wash(drop: TabDrop, t: &Theme) -> AnyElement {
    // Inset from the top by the strip's height, because the wash is a child of
    // the whole pane and belongs only over its body — see [`body_area`].
    let wash = div()
        .absolute()
        .top(crate::tree::TOP_STRIP)
        .left_0()
        .right_0()
        .bottom_0()
        .bg(alpha(paint(t.accent), 0.16))
        .border_1()
        .border_color(paint(t.accent));

    match drop {
        TabDrop::Split {
            axis: Axis::Horizontal,
            before,
            ..
        } => wash
            .when(before, |el| el.right(relative(0.5)))
            .when(!before, |el| el.left(relative(0.5))),
        TabDrop::Split {
            axis: Axis::Vertical,
            before,
            ..
        } => wash
            // Half of the *body*, so the inset above is doubled back out of the
            // half being taken — otherwise the top half would be short by a
            // strip and the bottom half long by one.
            .when(before, |el| {
                el.bottom(relative(0.5)).mb(crate::tree::TOP_STRIP / 2.0)
            })
            .when(!before, |el| {
                el.top(relative(0.5)).mt(crate::tree::TOP_STRIP / 2.0)
            }),
        TabDrop::Into { .. } | TabDrop::Strip { .. } => wash,
    }
    .into_any_element()
}

/// The rule drawn in the gap along a strip that a dragged tab would drop into.
///
/// Absolutely positioned, and that is the point: a rule taking part in the
/// layout would push every label along the strip aside as the pointer crossed
/// it, so the gap would move away from the pointer that was aiming at it.
fn tab_rule(t: &Theme, trailing: bool) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(2.0))
        .bg(paint(t.accent))
        .when(trailing, |el| el.right(px(-1.0)))
        .when(!trailing, |el| el.left(px(-1.0)))
}

/// One node in a worktree's pane layout.
#[derive(Clone)]
pub(crate) enum Pane {
    /// A tab strip and the content selected within it.
    Leaf {
        /// Identity used by focus and event handlers.
        id: PaneId,
        /// Open tabs, left to right. Empty only transiently while it collapses.
        tabs: Vec<Tab>,
        /// Index into `tabs`.
        active: usize,
    },
    /// Children laid out along one axis.
    Split {
        /// Horizontal puts the new pane to the right; vertical puts it below.
        axis: Axis,
        /// Never fewer than two after a mutation finishes.
        children: Vec<Pane>,
        /// Normalized relative shares, changed without rebuilding pane content.
        sizes: Vec<f32>,
    },
}

impl Pane {
    /// Builds an ordinary document tab for pane-management tests.
    #[cfg(test)]
    fn test_tab() -> Tab {
        Tab {
            title: "file.rs".into(),
            kind: TabKind::Editor("/tmp/file.rs".into()),
            renamed: false,
            pinned: false,
        }
    }

    /// An empty leaf.
    ///
    /// A worktree opens with no tabs; selecting it opens a terminal in this
    /// leaf (see `Shell::select`).
    fn empty(id: PaneId) -> Self {
        Self::Leaf {
            id,
            tabs: Vec::new(),
            active: 0,
        }
    }

    fn leaf(&self, wanted: PaneId) -> Option<(&[Tab], usize)> {
        match self {
            Self::Leaf { id, tabs, active } if *id == wanted => Some((tabs, *active)),
            Self::Leaf { .. } => None,
            Self::Split { children, .. } => children.iter().find_map(|child| child.leaf(wanted)),
        }
    }

    fn leaf_mut(&mut self, wanted: PaneId) -> Option<(&mut Vec<Tab>, &mut usize)> {
        match self {
            Self::Leaf { id, tabs, active } if *id == wanted => Some((tabs, active)),
            Self::Leaf { .. } => None,
            Self::Split { children, .. } => {
                children.iter_mut().find_map(|child| child.leaf_mut(wanted))
            }
        }
    }

    pub(crate) fn find_tab(&self, kind: &TabKind) -> Option<(PaneId, usize)> {
        match self {
            Self::Leaf { id, tabs, .. } => tabs
                .iter()
                .position(|tab| &tab.kind == kind)
                .map(|index| (*id, index)),
            Self::Split { children, .. } => children.iter().find_map(|child| child.find_tab(kind)),
        }
    }

    /// The first terminal anywhere in this tree, in layout order.
    ///
    /// Layout order rather than most-recently-opened: the shell a worktree
    /// opens with is the leftmost tab of the first pane, and that is the one
    /// people mean by the worktree's terminal however many others they have
    /// since split off.
    fn first_terminal(&self) -> Option<(PaneId, usize)> {
        match self {
            Self::Leaf { id, tabs, .. } => tabs
                .iter()
                .position(|tab| matches!(tab.kind, TabKind::Terminal(_)))
                .map(|index| (*id, index)),
            Self::Split { children, .. } => children.iter().find_map(Self::first_terminal),
        }
    }

    /// The first tab of the first pane that has one.
    fn first_tab(&self) -> Option<(PaneId, usize)> {
        match self {
            Self::Leaf { id, tabs, .. } => (!tabs.is_empty()).then_some((*id, 0)),
            Self::Split { children, .. } => children.iter().find_map(Self::first_tab),
        }
    }

    fn rename_editor(&mut self, key: &str, title: &SharedString) {
        match self {
            Self::Leaf { tabs, .. } => {
                for tab in tabs {
                    if matches!(&tab.kind, TabKind::Editor(open) if open.as_ref() == key) {
                        tab.title = title.clone();
                    }
                }
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.rename_editor(key, title);
                }
            }
        }
    }

    fn split(&mut self, wanted: PaneId, axis: Axis, new_id: PaneId) -> bool {
        match self {
            Self::Leaf { id, .. } if *id == wanted => {
                let first = self.clone();
                *self = Self::Split {
                    axis,
                    children: vec![first, Self::empty(new_id)],
                    sizes: vec![0.5, 0.5],
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { children, .. } => children
                .iter_mut()
                .any(|child| child.split(wanted, axis, new_id)),
        }
    }

    /// Removes one tab from `pane` and hands it over, leaving `active` on a
    /// neighbour.
    ///
    /// Deliberately does not collapse a leaf it empties, unlike
    /// [`Space::close_tab`]: a move needs the tab in hand before the tree is
    /// reshaped, and a leaf that vanished here would take with it the pane a
    /// drop onto that same pane is aiming at.
    fn take_tab(&mut self, pane: PaneId, index: usize) -> Option<Tab> {
        let (tabs, active) = self.leaf_mut(pane)?;
        if index >= tabs.len() {
            return None;
        }
        let tab = tabs.remove(index);
        if index < *active {
            *active -= 1;
        } else {
            *active = (*active).min(tabs.len().saturating_sub(1));
        }
        Some(tab)
    }

    /// Inserts `tab` into `pane` at `at`, clamped to the strip's length, and
    /// makes it the one that pane is showing.
    ///
    /// Showing it rather than leaving the selection alone: a tab dropped into
    /// a pane and then hidden behind whatever that pane was already on looks
    /// exactly like a drop that did not land.
    ///
    /// A gap on the wrong side of the pinned run lands on its edge instead —
    /// see [`gap_for`].
    fn insert_tab(&mut self, pane: PaneId, at: usize, tab: Tab) -> bool {
        let Some((tabs, active)) = self.leaf_mut(pane) else {
            return false;
        };
        let at = gap_for(at.min(tabs.len()), tab.pinned, pinned_run(tabs));
        tabs.insert(at, tab);
        *active = at;
        true
    }

    /// Places `tab` alone in a new leaf `axis`-adjacent to `target`.
    ///
    /// `before` puts the new leaf on the near side — left of a horizontal
    /// neighbour, above a vertical one.
    ///
    /// A drop whose axis matches the split the target already sits in becomes
    /// a **sibling** in that split rather than a nested split inside the
    /// target. Nesting is the obvious reading of "divide this pane" and it is
    /// the wrong one: dropping twice on the right-hand edge of a row would
    /// leave the third pane at a quarter of the window and the tree a chain,
    /// when what was asked for both times was one more column. Only a parent
    /// can tell, and `Pane` has no parent pointers, so the test lives in the
    /// `Split` arm looking down at its own children.
    fn split_with(
        &mut self,
        target: PaneId,
        axis: Axis,
        before: bool,
        new_id: PaneId,
        tab: &Tab,
    ) -> bool {
        match self {
            // Reached with no same-axis parent having claimed it — a
            // cross-axis split, or the root itself — so the leaf is wrapped
            // where it stands.
            Self::Leaf { id, .. } if *id == target => {
                let existing = self.clone();
                let fresh = Self::Leaf {
                    id: new_id,
                    tabs: vec![tab.clone()],
                    active: 0,
                };
                *self = Self::Split {
                    axis,
                    children: if before {
                        vec![fresh, existing]
                    } else {
                        vec![existing, fresh]
                    },
                    sizes: vec![0.5, 0.5],
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split {
                axis: mine,
                children,
                sizes,
            } => {
                if *mine == axis
                    && let Some(index) = children
                        .iter()
                        .position(|child| matches!(child, Self::Leaf { id, .. } if *id == target))
                {
                    let at = if before { index } else { index + 1 };
                    // The newcomer takes an equal share and the rest are
                    // scaled to make room for it, so two even halves become
                    // three even thirds — while a row somebody had dragged to
                    // 70/30 keeps that ratio between the two it already had.
                    // Renormalising all of them to equal would quietly undo
                    // every resize in the split.
                    let share = 1.0 / (children.len() + 1) as f32;
                    for size in sizes.iter_mut() {
                        *size *= 1.0 - share;
                    }
                    children.insert(
                        at,
                        Self::Leaf {
                            id: new_id,
                            tabs: vec![tab.clone()],
                            active: 0,
                        },
                    );
                    sizes.insert(at, share);
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.split_with(target, axis, before, new_id, tab))
            }
        }
    }

    /// Every terminal this pane tree shows.
    /// Every tab in this pane tree, in the order the panes hold them.
    fn all_tabs<'a>(&'a self, out: &mut Vec<&'a Tab>) {
        match self {
            Self::Leaf { tabs, .. } => out.extend(tabs.iter()),
            Self::Split { children, .. } => {
                for child in children {
                    child.all_tabs(out);
                }
            }
        }
    }

    fn terminal_ids(&self, out: &mut Vec<TerminalId>) {
        match self {
            Self::Leaf { tabs, .. } => out.extend(tabs.iter().filter_map(|tab| match tab.kind {
                TabKind::Terminal(id) => Some(id),
                _ => None,
            })),
            Self::Split { children, .. } => {
                for child in children {
                    child.terminal_ids(out);
                }
            }
        }
    }

    /// Every browser tab in this pane tree, including inactive ones.
    fn browser_ids(&self, out: &mut Vec<BrowserId>) {
        match self {
            Self::Leaf { tabs, .. } => out.extend(tabs.iter().filter_map(|tab| match tab.kind {
                TabKind::Browser(id) => Some(id),
                _ => None,
            })),
            Self::Split { children, .. } => {
                for child in children {
                    child.browser_ids(out);
                }
            }
        }
    }

    /// Terminal tabs currently selected in the leaves of this pane tree.
    ///
    /// The ones a person can actually see, as against [`Self::terminal_ids`],
    /// which is every terminal the tree holds open behind its tabs.
    fn active_terminal_ids(&self, out: &mut Vec<TerminalId>) {
        match self {
            Self::Leaf { tabs, active, .. } => {
                if let Some(Tab {
                    kind: TabKind::Terminal(id),
                    ..
                }) = tabs.get(*active)
                {
                    out.push(*id);
                }
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.active_terminal_ids(out);
                }
            }
        }
    }

    /// Browser tabs currently selected in the leaves of this pane tree.
    fn active_browser_ids(&self, out: &mut Vec<BrowserId>) {
        match self {
            Self::Leaf { tabs, active, .. } => {
                if let Some(Tab {
                    kind: TabKind::Browser(id),
                    ..
                }) = tabs.get(*active)
                {
                    out.push(*id);
                }
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.active_browser_ids(out);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn has_ios_simulator(&self) -> bool {
        self.find_tab(&TabKind::IosSimulator).is_some()
    }

    #[cfg(target_os = "macos")]
    fn ios_simulator_active(&self) -> bool {
        matches!(
            self,
            Self::Leaf { tabs, active, .. }
                if matches!(tabs.get(*active).map(|tab| &tab.kind), Some(TabKind::IosSimulator))
        ) || matches!(self, Self::Split { children, .. } if children.iter().any(Self::ios_simulator_active))
    }

    fn leaf_ids(&self, ids: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { id, .. } => ids.push(*id),
            Self::Split { children, .. } => {
                for child in children {
                    child.leaf_ids(ids);
                }
            }
        }
    }

    fn remove_leaf(&mut self, wanted: PaneId) -> bool {
        let Self::Split {
            children, sizes, ..
        } = self
        else {
            return false;
        };

        let removed_here = if let Some(index) = children
            .iter()
            .position(|child| matches!(child, Self::Leaf { id, .. } if *id == wanted))
        {
            children.remove(index);
            if index < sizes.len() {
                sizes.remove(index);
            }
            true
        } else if !children.iter_mut().any(|child| child.remove_leaf(wanted)) {
            return false;
        } else {
            false
        };

        if children.len() == 1 {
            *self = children.remove(0);
        } else if removed_here {
            let total: f32 = sizes.iter().sum();
            if total > f32::EPSILON {
                for size in sizes {
                    *size /= total;
                }
            }
        }
        true
    }

    fn split_mut(&mut self, path: &[usize]) -> Option<(&[Pane], &mut Vec<f32>, Axis)> {
        if path.is_empty() {
            return match self {
                Self::Split {
                    axis,
                    children,
                    sizes,
                } => Some((children, sizes, *axis)),
                Self::Leaf { .. } => None,
            };
        }

        match self {
            Self::Split { children, .. } => children.get_mut(path[0])?.split_mut(&path[1..]),
            Self::Leaf { .. } => None,
        }
    }

    /// The index path from here down to `wanted`, when it is in this subtree.
    ///
    /// Focus moves sideways by walking back up this path looking for a split
    /// that runs the right way, which is the only thing in the shell that
    /// needs to know where a leaf sits rather than merely that it exists.
    fn path_to(&self, wanted: PaneId, path: &mut Vec<usize>) -> bool {
        match self {
            Self::Leaf { id, .. } => *id == wanted,
            Self::Split { children, .. } => {
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    if child.path_to(wanted, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
        }
    }

    /// The node at `path`, when there is one.
    fn at(&self, path: &[usize]) -> Option<&Pane> {
        let Some((index, rest)) = path.split_first() else {
            return Some(self);
        };
        match self {
            Self::Split { children, .. } => children.get(*index)?.at(rest),
            Self::Leaf { .. } => None,
        }
    }

    /// The leaf of this subtree nearest the `axis` edge named by `last`.
    ///
    /// Which pane a sideways move actually lands on. Crossing leftwards into a
    /// column of panes should arrive at the one against the boundary just
    /// crossed rather than at whichever comes first in the tree, so a split on
    /// the same axis is entered from its far end. A cross-axis split has no
    /// near or far along this direction, so it is entered from the top, which
    /// is at least the same answer every time.
    fn edge_leaf(&self, axis: Axis, last: bool) -> Option<PaneId> {
        match self {
            Self::Leaf { id, .. } => Some(*id),
            Self::Split {
                axis: mine,
                children,
                ..
            } => {
                let child = if *mine == axis && last {
                    children.last()
                } else {
                    children.first()
                }?;
                child.edge_leaf(axis, last)
            }
        }
    }

    fn minimum_extent(&self, axis: Axis) -> Pixels {
        match self {
            Self::Leaf { .. } => MIN_PANE_SIZE,
            Self::Split {
                axis: split_axis,
                children,
                ..
            } if *split_axis == axis => {
                children
                    .iter()
                    .map(|child| child.minimum_extent(axis))
                    .fold(px(0.0), |total, extent| total + extent)
                    + DIVIDER_SIZE * children.len().saturating_sub(1)
            }
            Self::Split { children, .. } => children
                .iter()
                .map(|child| child.minimum_extent(axis))
                .fold(
                    px(0.0),
                    |largest, extent| {
                        if extent > largest { extent } else { largest }
                    },
                ),
        }
    }
}

/// Where a dragged tab would land if it were let go now.
///
/// Three answers rather than one, because a strip and a body mean different
/// things: a strip names the *gap* between two tabs, the middle of a body means
/// "in this pane, after what is already there", and an edge names a pane that
/// does not exist yet. A single "onto this pane" target could say none of them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabDrop {
    /// Into `pane`'s strip, in the gap before tab `before`.
    Strip {
        /// The pane whose strip is being aimed at.
        pane: PaneId,
        /// The slot the dragged tab would take.
        before: usize,
    },
    /// Into `pane`, after everything already open in it.
    Into {
        /// The pane whose body is under the pointer.
        pane: PaneId,
    },
    /// Dividing `pane`, with the dragged tab alone in the new leaf.
    Split {
        /// The pane being divided.
        pane: PaneId,
        /// Horizontal puts the new leaf beside it, vertical above or below.
        axis: Axis,
        /// Whether the new leaf takes the near side — the left, or the top.
        before: bool,
    },
}

impl TabDrop {
    /// The pane this target is about, whichever kind it is.
    pub(crate) fn pane(self) -> PaneId {
        match self {
            Self::Strip { pane, .. } | Self::Into { pane } | Self::Split { pane, .. } => pane,
        }
    }
}

/// Which way focus moves when it moves between panes.
///
/// A direction rather than an axis and a sign at every call site: the two
/// together are the only thing the tree walk needs, but "left" is what the
/// command is called and what a reader is thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Toward {
    /// The pane to the left of the focused one.
    Left,
    /// The pane to its right.
    Right,
    /// The pane above it.
    Up,
    /// The pane below it.
    Down,
}

impl Toward {
    /// The split axis this direction runs along, and whether it moves towards
    /// the later child of such a split.
    pub(crate) fn along(self) -> (Axis, bool) {
        match self {
            Self::Left => (Axis::Horizontal, false),
            Self::Right => (Axis::Horizontal, true),
            Self::Up => (Axis::Vertical, false),
            Self::Down => (Axis::Vertical, true),
        }
    }
}

/// A worktree's open panes and the leaf that receives new tabs and commands.
///
/// Stable leaf identities keep focus meaningful while the tree collapses. The
/// root is always present, so closing can never strand the content area empty.
#[derive(Clone)]
pub struct Space {
    /// The pane tree rendered for this worktree.
    pub(crate) root: Pane,
    /// The leaf that receives keyboard actions and newly opened tabs.
    pub(crate) focused: PaneId,
    /// Monotonic within this space; identities are never reused after closing.
    pub(crate) next_id: u64,
}

impl Default for Space {
    fn default() -> Self {
        let focused = PaneId(0);
        Self {
            root: Pane::empty(focused),
            focused,
            next_id: 1,
        }
    }
}

impl Space {
    /// Every terminal open anywhere in this space.
    pub(crate) fn terminal_ids(&self) -> Vec<TerminalId> {
        let mut out = Vec::new();
        self.root.terminal_ids(&mut out);
        out
    }

    /// Every tab open anywhere in this space, split panes included.
    ///
    /// For the sidebar's search, which indexes what is open rather than what
    /// is visible: a tab in the pane you are not looking at is exactly the
    /// thing you would go looking for by name.
    pub(crate) fn all_tabs(&self) -> Vec<&Tab> {
        let mut out = Vec::new();
        self.root.all_tabs(&mut out);
        out
    }

    /// Every diff tab open in this space, split panes included.
    pub(crate) fn diff_keys(&self) -> Vec<crate::diff_view::DiffKey> {
        self.all_tabs()
            .into_iter()
            .filter_map(|tab| match &tab.kind {
                TabKind::Diff(key) => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every browser tab open in this space.
    pub(crate) fn browser_ids(&self) -> Vec<BrowserId> {
        let mut out = Vec::new();
        self.root.browser_ids(&mut out);
        out
    }

    /// Browser tabs visible in this space's pane leaves.
    pub(crate) fn active_browser_ids(&self) -> Vec<BrowserId> {
        let mut out = Vec::new();
        self.root.active_browser_ids(&mut out);
        out
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn has_ios_simulator(&self) -> bool {
        self.root.has_ios_simulator()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn ios_simulator_active(&self) -> bool {
        self.root.ios_simulator_active()
    }

    /// Whether the focused pane is showing the simulator.
    #[cfg(target_os = "macos")]
    pub(crate) fn ios_simulator_focused(&self) -> bool {
        self.active_tab()
            .is_some_and(|tab| matches!(tab.kind, TabKind::IosSimulator))
    }

    pub(crate) fn has_android(&self) -> bool {
        self.root.find_tab(&TabKind::Android).is_some()
    }

    /// Whether the focused pane is showing the Android emulator — where
    /// typing goes to it.
    pub(crate) fn android_focused(&self) -> bool {
        self.active_tab()
            .is_some_and(|tab| matches!(tab.kind, TabKind::Android))
    }

    pub(crate) fn focus_android(&mut self) -> bool {
        let Some((pane, index)) = self.root.find_tab(&TabKind::Android) else {
            return false;
        };
        self.focused = pane;
        if let Some((_, active)) = self.root.leaf_mut(pane) {
            *active = index;
        }
        true
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn focus_ios_simulator(&mut self) -> bool {
        let Some((pane, index)) = self.root.find_tab(&TabKind::IosSimulator) else {
            return false;
        };
        self.focused = pane;
        if let Some((_, active)) = self.root.leaf_mut(pane) {
            *active = index;
        }
        true
    }

    /// Terminal tabs selected in all visible leaves of this space.
    pub(crate) fn active_terminal_ids(&self) -> Vec<TerminalId> {
        let mut out = Vec::new();
        self.root.active_terminal_ids(&mut out);
        out
    }

    /// What the tab showing `kind` is labelled, when one is open.
    ///
    /// The name a person recognises for a pane. A terminal reporting its
    /// spending is worth naming by its tab rather than by the id ket gave it,
    /// which means nothing to anybody reading the status bar.
    pub(crate) fn tab_title(&self, kind: &TabKind) -> Option<SharedString> {
        let (pane, index) = self.root.find_tab(kind)?;
        let (tabs, _) = self.root.leaf(pane)?;
        tabs.get(index).map(|tab| tab.title.clone())
    }

    /// The selected tab in the focused leaf.
    pub(crate) fn active_tab(&self) -> Option<&Tab> {
        let (tabs, active) = self.root.leaf(self.focused)?;
        tabs.get(active)
    }

    /// A space-wide lookup prevents one terminal or editor buffer from gaining
    /// competing tab identities merely because another pane had focus.
    pub(crate) fn open_tab(&mut self, tab: Tab) {
        if let Some((id, index)) = self.root.find_tab(&tab.kind) {
            self.focused = id;
            if let Some((_, active)) = self.root.leaf_mut(id) {
                *active = index;
            }
            return;
        }

        if let Some((tabs, active)) = self.root.leaf_mut(self.focused) {
            tabs.push(tab);
            *active = tabs.len() - 1;
        }
    }

    pub(crate) fn open_tab_in(&mut self, pane: PaneId, tab: Tab) -> bool {
        let Some((tabs, active)) = self.root.leaf_mut(pane) else {
            return false;
        };
        match tabs.iter().position(|open| open.kind == tab.kind) {
            Some(index) => *active = index,
            // The end of whichever run it belongs to. Only a terminal a saved
            // layout brought back arrives here already pinned.
            None => {
                let at = if tab.pinned {
                    pinned_run(tabs)
                } else {
                    tabs.len()
                };
                tabs.insert(at, tab);
                *active = at;
            }
        }
        self.focused = pane;
        true
    }

    /// Pins or unpins the tab at `(pane, index)`, moving it to the boundary
    /// between the two runs: the end of the pinned one, or the head of the
    /// rest. Reports whether anything changed.
    ///
    /// The pane keeps showing the tab it was showing, wherever that moved to.
    pub(crate) fn set_pinned(&mut self, pane: PaneId, index: usize, pinned: bool) -> bool {
        let Some((tabs, active)) = self.root.leaf_mut(pane) else {
            return false;
        };
        if tabs.get(index).is_none_or(|tab| tab.pinned == pinned) {
            return false;
        }
        let mut tab = tabs.remove(index);
        tab.pinned = pinned;
        // Measured with the tab out, which is what makes one number serve
        // both ways: the end of the run it joins, or the slot it just left.
        let to = pinned_run(tabs);
        tabs.insert(to, tab);
        *active = if *active == index {
            to
        } else {
            let shifted = if *active > index {
                *active - 1
            } else {
                *active
            };
            if shifted >= to { shifted + 1 } else { shifted }
        };
        true
    }

    /// Puts the space back on the tab the worktree opened with.
    ///
    /// The terminal, when there is one: a worktree is a shell you work in, and
    /// coming back to it means coming back to that. A space whose terminal has
    /// been closed falls back to its first tab rather than doing nothing —
    /// somewhere is the point, and the first pane is where the eye goes.
    ///
    /// Reports whether it found anywhere to go, so callers can skip the
    /// re-tiling and persistence that only a move needs.
    pub(crate) fn focus_main_tab(&mut self) -> bool {
        let Some((pane, index)) = self.root.first_terminal().or_else(|| self.root.first_tab())
        else {
            return false;
        };
        self.focused = pane;
        if let Some((_, active)) = self.root.leaf_mut(pane) {
            *active = index;
        }
        true
    }

    /// Brings `terminal`'s tab forward in whichever pane holds it. Reports
    /// whether it is still open.
    pub(crate) fn focus_terminal(&mut self, terminal: TerminalId) -> bool {
        let Some((pane, index)) = self.root.find_tab(&TabKind::Terminal(terminal)) else {
            return false;
        };
        self.focused = pane;
        if let Some((_, active)) = self.root.leaf_mut(pane) {
            *active = index;
        }
        true
    }

    /// Whether `pane` is still a leaf in this space.
    pub(crate) fn contains_pane(&self, pane: PaneId) -> bool {
        self.root.leaf(pane).is_some()
    }

    /// Whether `pane` already has a shell in it.
    ///
    /// Asked before one is started in a pane a saved layout said had one, so
    /// that arriving at a worktree twice does not stack a second shell on the
    /// first.
    pub(crate) fn pane_has_terminal(&self, pane: PaneId) -> bool {
        self.root.leaf(pane).is_some_and(|(tabs, _)| {
            tabs.iter()
                .any(|tab| matches!(tab.kind, TabKind::Terminal(_)))
        })
    }

    /// Drops panes that came back from a saved layout with nothing in them.
    ///
    /// Neither a terminal nor a browser tab survives the process that ran it,
    /// so a pane that held only those restores with an empty strip — and an
    /// empty pane beside a full one is a `+` button and the words "select a
    /// worktree", which nothing in the running app can produce and nobody
    /// asked for. Splitting a worktree's terminal in two and coming back the
    /// next morning is enough to reach it.
    ///
    /// The last leaf is always kept, empty or not: a space needs somewhere to
    /// put the tab that gets opened next. So is anything named in `keep` — a
    /// pane that held a shell is empty only until its worktree is opened and a
    /// fresh one is started in it, and dropping it before then is what stopped
    /// a split arrangement of shells ever surviving a restart.
    pub(crate) fn prune_empty_panes(&mut self, keep: &[PaneId]) {
        let mut ids = Vec::new();
        self.root.leaf_ids(&mut ids);
        let empty: Vec<PaneId> = ids
            .into_iter()
            .filter(|id| {
                !keep.contains(id) && self.root.leaf(*id).is_some_and(|(tabs, _)| tabs.is_empty())
            })
            .collect();

        for id in empty {
            if self.leaf_count() <= 1 {
                break;
            }
            self.root.remove_leaf(id);
        }

        // Focus follows, since the pane it named may be one of the ones that
        // just went. A space whose focus points at nothing takes no new tabs
        // at all — `open_tab` looks the focused leaf up and gives up.
        if !self.contains_pane(self.focused) {
            let mut ids = Vec::new();
            self.root.leaf_ids(&mut ids);
            if let Some(first) = ids.first() {
                self.focused = *first;
            }
        }
    }

    /// Updates the label of an Untitled editor after its first successful save.
    pub(crate) fn rename_editor(&mut self, key: &str, title: SharedString) {
        self.root.rename_editor(key, &title);
    }

    /// Moves the tab at `(from, index)` to wherever `drop` names.
    ///
    /// Reports whether anything actually moved, so a caller can skip the
    /// re-tiling and the `state.json` write that only a real move needs.
    ///
    /// The tab comes out before it goes back in, which is what makes a reorder
    /// within one strip land where the insertion line was drawn: every gap to
    /// the right of a tab shifts one place left the moment it leaves.
    pub(crate) fn move_tab(&mut self, from: PaneId, index: usize, drop: TabDrop) -> bool {
        let Some((tabs, _)) = self.root.leaf(from) else {
            return false;
        };
        if index >= tabs.len() {
            return false;
        }
        let last = tabs.len() - 1;
        let only = tabs.len() == 1;
        let pinned = tabs[index].pinned;
        // The run as it will be once this tab is out of it, which is what
        // `insert_tab` measures against.
        let run = pinned_run(tabs) - usize::from(pinned);

        // The drops that would end where they began. Refused here rather than
        // performed and undone, because a split that collapses on the frame it
        // was made still burns a pane id and still writes a layout out to disk.
        match drop {
            TabDrop::Strip { pane, before } if pane == from => {
                let landing = if before > index { before - 1 } else { before };
                if gap_for(landing, pinned, run) == index {
                    return false;
                }
            }
            // A pinned tab dropped on its own pane only goes to the end of
            // the pinned run, so the last pinned tab is already there.
            TabDrop::Into { pane } if pane == from && index == if pinned { run } else { last } => {
                return false;
            }
            TabDrop::Split { pane, .. } if pane == from && only => return false,
            _ => {}
        }

        let Some(tab) = self.root.take_tab(from, index) else {
            return false;
        };

        let landed = match drop {
            TabDrop::Strip { pane, before } => {
                // The same shift again, now that the tab really has left.
                let at = if pane == from && before > index {
                    before - 1
                } else {
                    before
                };
                self.root.insert_tab(pane, at, tab.clone()).then_some(pane)
            }
            TabDrop::Into { pane } => self
                .root
                .insert_tab(pane, usize::MAX, tab.clone())
                .then_some(pane),
            TabDrop::Split { pane, axis, before } => {
                let new_id = PaneId(self.next_id);
                self.root
                    .split_with(pane, axis, before, new_id, &tab)
                    .then(|| {
                        self.next_id += 1;
                        new_id
                    })
            }
        };

        // A destination that has gone leaves the tab with nowhere to be.
        // Putting it back where it came from is the only outcome that does not
        // lose it outright.
        let Some(destination) = landed else {
            self.root.insert_tab(from, index, tab);
            return false;
        };
        self.focused = destination;

        // The take may have emptied the source. Collapsing it here rather than
        // through `close_pane` is what keeps the focus this move just set:
        // `close_pane` moves focus to a neighbour of the pane it removes, which
        // would take it straight back off the pane the tab was dropped into.
        if self
            .root
            .leaf(from)
            .is_some_and(|(tabs, _)| tabs.is_empty())
            && self.leaf_count() > 1
        {
            self.root.remove_leaf(from);
        }
        true
    }

    /// The leaf that lies `axis`-wards of the focused one, if any does.
    ///
    /// Worked out from the tree rather than from rectangles, because the shell
    /// keeps ratios and never learns what a pane measured: walk up from the
    /// focused leaf to the nearest split running the right way that has a
    /// sibling on the side being moved towards, then back down that sibling to
    /// whichever leaf sits against the boundary just crossed.
    pub(crate) fn neighbour(&self, axis: Axis, forward: bool) -> Option<PaneId> {
        let mut path = Vec::new();
        if !self.root.path_to(self.focused, &mut path) {
            return None;
        }

        while let Some(index) = path.pop() {
            let Some(Pane::Split {
                axis: mine,
                children,
                ..
            }) = self.root.at(&path)
            else {
                continue;
            };
            if *mine != axis {
                continue;
            }
            let sibling = if forward {
                index.checked_add(1).filter(|next| *next < children.len())
            } else {
                index.checked_sub(1)
            };
            if let Some(sibling) = sibling {
                // Entered from the end nearest the divider just crossed:
                // moving right arrives at the sibling's leftmost leaf.
                return children[sibling].edge_leaf(axis, !forward);
            }
        }
        None
    }

    /// Divides the focused leaf, carrying its active tab into the new pane.
    ///
    /// Carrying rather than opening empty: a "split right" that leaves a blank
    /// pane beside the one being read is a gesture somebody then has to finish
    /// by hand, and it is not what dragging that same tab to that same edge
    /// does. The two have to agree, so they share one primitive.
    ///
    /// A pane holding one tab, or none, still divides and still gets an empty
    /// neighbour — moving an only tab would empty its leaf and collapse the
    /// split on the spot, which is no division at all.
    pub(crate) fn split(&mut self, axis: Axis) {
        let focused = self.focused;
        let carried = self
            .root
            .leaf(focused)
            .and_then(|(tabs, active)| (tabs.len() > 1).then_some(active));

        if let Some(index) = carried
            && self.move_tab(
                focused,
                index,
                TabDrop::Split {
                    pane: focused,
                    axis,
                    before: false,
                },
            )
        {
            return;
        }

        let new_id = PaneId(self.next_id);
        if self.root.split(focused, axis, new_id) {
            self.next_id += 1;
            self.focused = new_id;
        }
    }

    /// Removes the focused leaf unless it is the only route back to the worktree.
    pub(crate) fn close_focused_pane(&mut self) -> bool {
        self.close_pane(self.focused)
    }

    fn close_pane(&mut self, id: PaneId) -> bool {
        let mut ids = Vec::new();
        self.root.leaf_ids(&mut ids);
        if ids.len() == 1 {
            return false;
        }
        let Some(index) = ids.iter().position(|candidate| *candidate == id) else {
            return false;
        };
        let next_focus = ids
            .get(index + 1)
            .or_else(|| index.checked_sub(1).and_then(|i| ids.get(i)))
            .copied();
        let Some(next_focus) = next_focus else {
            return false;
        };

        if self.root.remove_leaf(id) {
            self.focused = next_focus;
            true
        } else {
            false
        }
    }

    pub(crate) fn close_tab(&mut self, pane: PaneId, index: usize) {
        if self.root.leaf(pane).is_none() {
            return;
        }
        self.focused = pane;
        let should_close_pane = match self.root.leaf_mut(pane) {
            Some((tabs, active)) if index < tabs.len() && tabs.len() > 1 => {
                tabs.remove(index);
                if index < *active {
                    *active -= 1;
                } else {
                    *active = (*active).min(tabs.len() - 1);
                }
                false
            }
            Some((tabs, _)) if index < tabs.len() => true,
            _ => false,
        };
        if should_close_pane {
            self.close_pane(pane);
        }
    }

    pub(crate) fn leaf_count(&self) -> usize {
        let mut ids = Vec::new();
        self.root.leaf_ids(&mut ids);
        ids.len()
    }

    fn resize_split(
        &mut self,
        path: &[usize],
        divider: usize,
        cursor: Pixels,
        extent: Pixels,
    ) -> bool {
        if extent <= px(0.0) {
            return false;
        }
        let Some((children, sizes, axis)) = self.root.split_mut(path) else {
            return false;
        };
        if divider + 1 >= sizes.len() {
            return false;
        }
        let usable_extent = extent - DIVIDER_SIZE * sizes.len().saturating_sub(1);
        if usable_extent <= px(0.0) {
            return false;
        }

        let total: f32 = sizes.iter().sum();
        if total <= f32::EPSILON {
            return false;
        }
        for size in sizes.iter_mut() {
            *size /= total;
        }

        let prefix: f32 = sizes[..divider].iter().sum();
        let pair = sizes[divider] + sizes[divider + 1];
        let before_minimum = children[divider].minimum_extent(axis) / usable_extent;
        let after_minimum = children[divider + 1].minimum_extent(axis) / usable_extent;
        if pair < before_minimum + after_minimum {
            return false;
        }
        let usable_cursor = cursor - DIVIDER_SIZE * divider;
        let boundary = (usable_cursor / usable_extent)
            .clamp(prefix + before_minimum, prefix + pair - after_minimum);
        sizes[divider] = boundary - prefix;
        sizes[divider + 1] = pair - sizes[divider];
        true
    }
}

impl Shell {
    /// Makes `pane` the selected worktree's focused pane, closing any open
    /// menu, as a press anywhere in it does. Answers whether `pane` was one
    /// of that worktree's.
    ///
    /// Also called by a press that stops before reaching the pane — an
    /// editor row, which has to keep the pane's empty-space handler from
    /// seeing it — so a click into a split still moves the keyboard there.
    pub(crate) fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some(worktree) = self.selected_id() else {
            return false;
        };
        let Some(space) = self.spaces.get_mut(&worktree) else {
            return false;
        };
        if space.root.leaf(pane).is_none() {
            return false;
        }
        space.focused = pane;
        self.menu = None;
        true
    }

    /// The space belonging to the selected worktree.
    pub(crate) fn space(&self) -> Option<&Space> {
        let selection = self.selection?;
        let node = self
            .projects
            .get(selection.project)?
            .worktrees
            .get(selection.worktree)?;
        self.spaces.get(&node.id)
    }

    /// The selected worktree's id.
    pub(crate) fn selected_id(&self) -> Option<WorktreeId> {
        let selection = self.selection?;
        Some(
            self.projects
                .get(selection.project)?
                .worktrees
                .get(selection.worktree)?
                .id
                .clone(),
        )
    }

    /// Takes the selected worktree's space back to its shell.
    ///
    /// What clicking a worktree you are already in does. Persists because
    /// which tab was active is part of the arrangement a restart restores.
    pub(crate) fn focus_main_tab(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if !self
            .spaces
            .get_mut(&id)
            .is_some_and(|space| space.focus_main_tab())
        {
            return;
        }
        self.persist_layout();
    }

    /// Takes the selected worktree's space to the terminal `pane` names — the
    /// key an agent's hooks report it under — or to its shell when that
    /// terminal has since been closed.
    pub(crate) fn focus_agent_pane(&mut self, pane: Option<&str>) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let terminal = pane.and_then(|pane| self.pane_for(pane));
        let found = terminal.is_some_and(|terminal| {
            self.spaces
                .get_mut(&id)
                .is_some_and(|space| space.focus_terminal(terminal))
        });
        if found {
            self.persist_layout();
        } else {
            self.focus_main_tab();
        }
    }

    /// Moves the focused pane's active tab left or right, wrapping at either
    /// end — the VS Code convention (⌘⇧[ / ⌘⇧]) for Cmd-Shift bracket.
    pub(crate) fn cycle_tab(&mut self, delta: isize) {
        let Some(id) = self.selected_id() else {
            return;
        };
        let Some(space) = self.spaces.get_mut(&id) else {
            return;
        };
        let focused = space.focused;
        let Some((tabs, active)) = space.root.leaf_mut(focused) else {
            return;
        };
        let len = tabs.len() as isize;
        if len < 2 {
            return;
        }
        *active = (*active as isize + delta).rem_euclid(len) as usize;
        self.persist_layout();
    }

    /// Notes where a dragged tab would land, answering whether that changed.
    ///
    /// A listener that answers `false` asks for no frame, which matters here:
    /// every listener for a drag type runs on every mouse move — gpui filters
    /// these by type, not by hitbox — so without the comparison the whole pane
    /// tree would be rebuilt several times per move to draw the same rule in
    /// the same gap. Callers bounds-check before calling, for the same reason.
    fn note_tab_drop(&mut self, drop: TabDrop) -> bool {
        if self.tab_drop == Some(drop) {
            return false;
        }
        self.tab_drop = Some(drop);
        true
    }

    /// Where the tab this drag picked up is *now*.
    ///
    /// The address the drag recorded is checked before it is trusted, because
    /// a strip can be rearranged while a tab is in the air. When it no longer
    /// holds, the tab is found again by what it shows. That is the fallback
    /// rather than the primary because a kind is not unique across a space —
    /// [`Space::open_tab_in`] dedupes only within one leaf, so the same file
    /// can legitimately be open in two panes, and only the recorded address
    /// can say which of them was picked up.
    fn dragged_tab(&self, drag: &TabDrag) -> Option<(PaneId, usize)> {
        let space = self.space()?;
        let still_there = space
            .root
            .leaf(drag.pane)
            .and_then(|(tabs, _)| tabs.get(drag.index))
            .is_some_and(|tab| tab.kind == drag.kind);
        match still_there {
            true => Some((drag.pane, drag.index)),
            false => space.root.find_tab(&drag.kind),
        }
    }

    /// Carries out a tab drag that has just been let go.
    ///
    /// Nothing is destroyed by a move — the tab it could not place goes back
    /// where it came from — so unlike closing a tab this neither prunes
    /// browsers nor asks before taking a running agent's terminal somewhere
    /// else.
    fn drop_tab(&mut self, drag: &TabDrag, cx: &mut Context<Self>) {
        let target = self.tab_drop.take();
        self.tab_dragging = false;
        cx.notify();

        // A drag that outlived the selection it began in names panes in a tree
        // that is no longer on screen; there is nowhere for it to land.
        let Some(target) = target.filter(|_| self.selected_id().as_ref() == Some(&drag.worktree))
        else {
            return;
        };
        let Some((pane, index)) = self.dragged_tab(drag) else {
            return;
        };
        if !self
            .spaces
            .get_mut(&drag.worktree)
            .is_some_and(|space| space.move_tab(pane, index, target))
        {
            return;
        }

        self.menu = None;
        self.close_tab_menus();
        // A rename addresses its tab by position, and a move shifts positions
        // in two strips at once — so a field left standing would be editing
        // whichever tab slid under it. Same reasoning as `close_tab_now`.
        self.cancel_tab_rename();
        self.persist_layout();
    }

    /// Forgets a tab drag, whether it landed somewhere or was simply let go.
    fn end_tab_drag(&mut self, cx: &mut Context<Self>) {
        if self.tab_drop.take().is_some() || self.tab_dragging {
            self.tab_dragging = false;
            cx.notify();
        }
    }

    /// The tab target to draw, if one is worth drawing.
    ///
    /// Asked of gpui rather than of the field alone, because a drag released
    /// over nothing at all is a drag nothing tells the shell about: the field
    /// would still hold the last gap the pointer crossed, and the rule would
    /// sit there until something else was dragged. Same reasoning as the
    /// sidebar's — see `crate::tree::Shell::sidebar`.
    fn tab_drop_shown(&self, cx: &App) -> Option<TabDrop> {
        cx.has_active_drag().then_some(self.tab_drop).flatten()
    }

    /// Renders one leaf's tab strip and routes its events by stable pane id.
    pub(crate) fn tab_bar(
        &self,
        pane: PaneId,
        tabs: &[Tab],
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let can_close_last = self.space().is_some_and(|space| space.leaf_count() > 1);
        let menu_open = self.new_tab_menu_open(pane);
        let mono = self.font_family.clone();
        let worktree = self.selected_id();
        let tab_count = tabs.len();
        let last_tab = tab_count.saturating_sub(1);
        let pinned_count = pinned_run(tabs);

        // Only a target naming *this* strip draws anything here, and only the
        // gap it names: a target on some other pane, or on a body rather than
        // a strip, belongs to whichever element is showing it.
        let gap = match self.tab_drop_shown(cx) {
            Some(TabDrop::Strip {
                pane: aimed,
                before,
            }) if aimed == pane => Some(before),
            _ => None,
        };

        // At most one tab in the window is being renamed, and laying its field
        // out needs the window mutably while every other tab needs nothing but
        // `cx.listener`. So the field is built once, here, and handed to the
        // tab it belongs to on the way past — see `rename_index` below.
        let rename = self.tab_rename.as_ref().filter(|rename| {
            rename.pane == pane && self.selected_id().as_ref() == Some(&rename.worktree)
        });
        let rename_index = rename.map(|rename| rename.index);
        let mut rename_field = rename.map(|rename| {
            // Only exists while it has the keyboard, so there is no idle
            // state for it to be quieter in: always focused.
            crate::ui::field::field(
                SharedString::from(format!("tab-rename-field-{}", pane.0)),
                "",
                "",
            )
            .compact()
            .focused(true)
            .body(
                text_line(
                    &rename.input,
                    SharedString::from(format!("tab-rename-{}", pane.0)),
                    Style::new(t, self.caret.visible),
                    window,
                    cx,
                )
                .into_any_element(),
            )
            .render(t)
            .flex_none()
            .w(RENAME_W)
            .into_any_element()
        });

        // What the selected worktree is doing by every source but the
        // agents' own reports — a build in the shell, an agent ket could only
        // watch start. Those are known per worktree, not per tab.
        let now = ket_core::now_ms();
        let inferred = self
            .selected_id()
            .map(|id| self.activity.inferred(&id, now))
            .unwrap_or(Activity::Idle);

        // So a tab only wears them when it is the worktree's one terminal,
        // where attributing them is unambiguous; with more, a blank shell
        // would otherwise borrow a neighbour's build and spinner. What an
        // agent says about itself is per pane, and always goes on its own
        // tab — see `terminal_status`.
        let single_terminal = self
            .space()
            .is_some_and(|space| space.terminal_ids().len() == 1);
        let terminal_status = |terminal: TerminalId| -> (Activity, Signal) {
            let numeric = terminal.0.to_string();
            self.terminal_keys
                .get(&terminal)
                .and_then(|key| self.activity.pane(key, now))
                .or_else(|| self.activity.pane(&numeric, now))
                .unwrap_or_else(|| {
                    if single_terminal {
                        let signal = inferred.signal();
                        (inferred.clone(), signal)
                    } else {
                        (Activity::Idle, Signal::Quiet)
                    }
                })
        };

        // Air between the last tab and `+`, where a rule used to stand. The
        // last tab's own trailing border already closes the run of tabs, so a
        // second line beside it only doubled the edge. None with an empty
        // pane: the button sits at the left edge with nothing to clear.
        let add_gap = if tabs.is_empty() { px(0.0) } else { px(6.0) };

        let add = div()
            .relative()
            .flex_none()
            .ml(add_gap)
            // The pane's own `on_mouse_down` clears `self.menu` on anything
            // that isn't the menu it opened — otherwise a second press here
            // would clear it on the way down and the click handler below,
            // finding no menu left to toggle off, would open a fresh one.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                icon_button(("add-tab", pane.0), Icon::Plus)
                    .bare()
                    .small()
                    .render(t)
                    .when(menu_open, |el| el.bg(paint(t.selection)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(id) = this.selected_id()
                            && let Some(space) = this.spaces.get_mut(&id)
                            && space.root.leaf(pane).is_some()
                        {
                            space.focused = pane;
                            // Toggling rather than only opening: a second
                            // click on the button that summoned a menu is the
                            // most common way of dismissing one.
                            this.menu = if this.new_tab_menu_open(pane) {
                                None
                            } else {
                                this.close_tab_menus();
                                this.worktree_menu = None;
                                this.project_menu = None;
                                this.popup = None;
                                let agents = this.runnable_agents();
                                #[cfg(target_os = "macos")]
                                let simulator = crate::ios_simulator::xcode_installed();
                                #[cfg(not(target_os = "macos"))]
                                let simulator = false;
                                let android = ket_core::android::installed();
                                Some(new_tab_menu(pane, &agents, simulator, android, &this.theme))
                            };
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })),
            )
            .children(menu_open.then(|| {
                let panel = self
                    .menu
                    .as_ref()
                    .map(|menu| {
                        menu.view(
                            &self.theme,
                            cx,
                            |shell, action, cx| shell.run_menu_action(action.clone(), cx),
                            |shell, cx| {
                                shell.menu = None;
                                cx.notify();
                            },
                        )
                    })
                    .unwrap_or_else(|| div().into_any_element());
                deferred(panel).with_priority(1)
            }));

        // An empty pane has no tab to hang an insertion rule off, so it draws
        // its own. Without this the one strip that most needs to say "the tab
        // lands here" is the one that says nothing at all.
        let empty_gap = (tabs.is_empty() && gap == Some(0))
            .then(|| div().flex_none().w(px(2.0)).h_full().bg(paint(t.accent)));

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .h(crate::tree::TOP_STRIP)
            .px(px(8.0))
            // Deliberately no `window_control_area`, unlike every other strip
            // along the top edge. This one's tabs are picked up and carried,
            // and a region that both drags a tab and moves the window can only
            // do one of them. The empty run past the last tab gets the window
            // drag back below, where there is nothing to pick up.
            // The coarse claim: anywhere in this strip that is not a tab means
            // the end of it, which is what makes the empty run past the last
            // tab a place a drop can land rather than a dead zone beside one.
            // Whichever tab the pointer is really over refines this afterwards.
            .on_drag_move::<TabDrag>(cx.listener(
                move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                    if event.bounds.contains(&event.event.position)
                        && this.note_tab_drop(TabDrop::Strip {
                            pane,
                            before: gap_for(tab_count, event.drag(cx).pinned, pinned_count),
                        })
                    {
                        cx.notify();
                    }
                },
            ))
            // The tabs sit in one sunken track, the way a segmented switch's
            // answers do, with the active one raised in it.
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(TAB_TRACK_H)
                    .p(px(3.0))
                    .gap(px(2.0))
                    .rounded(crate::ui::RADIUS_MD)
                    .bg(paint(t.sunken))
                    .children(tabs.iter().cloned().enumerate().map(|(index, tab)| {
                        let closable = tabs.len() > 1 || can_close_last;
                        // Whether clicking this tab could bring a different tab's
                        // content forward — the one case where hovering a tab is worth
                        // acknowledging.
                        let switchable = tabs.len() > 1;
                        let active = index == active;
                        let group = SharedString::from(format!("tab-group-{}-{index}", pane.0));
                        let close_group =
                            SharedString::from(format!("close-group-{}-{index}", pane.0));

                        // Whether the buffer behind this tab has unsaved edits. Only an
                        // editor can be dirty; the other views have nothing to save.
                        let dirty = match &tab.kind {
                            TabKind::Editor(key) => self
                                .editors
                                .get(key.as_ref())
                                .is_some_and(EditorState::is_dirty),
                            _ => false,
                        };

                        // A terminal is the one tab that reports something beyond
                        // itself: an agent runs *in* it, so its mark carries that
                        // agent's state and its label takes the agent's name. The
                        // stored title is left alone — this is a thing to draw, not a
                        // change to persist on the tick.
                        //
                        // Same grammar as the sidebar's rail, and for the same reason:
                        // `is_working` is true of an agent parked at its own prompt,
                        // so a strip keyed on it spins and glows green all day. See
                        // `Activity::signal`.
                        let (activity, signal) = match &tab.kind {
                            TabKind::Terminal(terminal) => terminal_status(*terminal),
                            _ => (Activity::Idle, Signal::Quiet),
                        };
                        // A session its project launched with a command of its
                        // own wears that project's tint — see
                        // `crate::agent_override`.
                        let tinted = match &tab.kind {
                            TabKind::Terminal(terminal) => self
                                .terminals
                                .get(terminal)
                                .is_some_and(|terminal| terminal.launched_with.is_some()),
                            _ => false,
                        };
                        let tint = crate::agent_override::hue(t);
                        let terminal_light = match signal {
                            Signal::Blocked => paint(t.status.attention),
                            Signal::Failed => paint(t.status.failed),
                            Signal::Working => paint(t.status.running),
                            // Git rewriting the tree in this terminal. It takes a
                            // colour because it is real progress on the worktree, and
                            // its own rather than the running green because the thing
                            // making it is git, not an agent.
                            Signal::Merging => paint(t.status.merging),
                            // A build in the shell moves without taking a colour: the
                            // running hue means an agent.
                            Signal::Running | Signal::Quiet => paint(t.text.dim),
                        };

                        // A terminal running an agent takes the status colour; one
                        // sitting at a prompt keeps its provider colour. The shape
                        // always belongs to the provider. In particular, the generic
                        // six-ray working asterisk looks like Claude's mark and made
                        // a working Codex tab claim to be Claude.
                        let working = matches!(tab.kind, TabKind::Terminal(_))
                            && matches!(signal, Signal::Working | Signal::Merging);

                        let mark: AnyElement = match &tab.kind {
                            // The path, not the label: a lettermark comes from the
                            // extension, and a tab someone has renamed no longer has
                            // one in its title.
                            TabKind::Editor(path) | TabKind::Audio(path) => {
                                file_mark(path.as_ref(), !active, mono.clone(), t)
                                    .into_any_element()
                            }
                            TabKind::Browser(_) => {
                                icon_mark(Icon::ExternalLink, paint(t.text.dim), !active)
                                    .into_any_element()
                            }
                            #[cfg(target_os = "macos")]
                            TabKind::IosSimulator => {
                                icon_mark(Icon::Smartphone, paint(t.text.dim), !active)
                                    .into_any_element()
                            }
                            TabKind::Android => {
                                icon_mark(Icon::Smartphone, paint(t.text.dim), !active)
                                    .into_any_element()
                            }
                            TabKind::Diff(key) => {
                                file_mark(key.rel.as_ref(), !active, mono.clone(), t)
                                    .into_any_element()
                            }
                            TabKind::Terminal(id) => match self
                                .terminals
                                .get(id)
                                .and_then(|terminal| terminal.agent.as_deref())
                                .or_else(|| activity.agent())
                            {
                                Some(agent) => {
                                    let (which, provider_light) = provider(agent, t);
                                    icon_mark(
                                        which,
                                        if matches!(signal, Signal::Quiet) {
                                            provider_light
                                        } else {
                                            terminal_light
                                        },
                                        !active,
                                    )
                                    .into_any_element()
                                }
                                None if working => icon_mark(Icon::Asterisk, terminal_light, false)
                                    .into_any_element(),
                                None => icon_mark(Icon::Terminal, terminal_light, !active)
                                    .into_any_element(),
                            },
                        };

                        // A title somebody typed wins over the agent's name. Otherwise
                        // renaming the tab a session runs in — the one people most
                        // want to name, since two Claude tabs are otherwise both just
                        // "Claude" — looks like it did nothing.
                        let label: SharedString = match (&tab.kind, activity.agent()) {
                            (TabKind::Terminal(_), Some(agent)) if !tab.renamed => {
                                SharedString::from(agent.to_owned())
                            }
                            _ => tab.title.clone(),
                        };
                        // What the ghost says, taken here rather than from `tab.title`:
                        // a lone terminal wears its agent's name, and a pill reading
                        // "Terminal" while the tab it came from reads "Claude" is a
                        // pill that looks like it picked up something else.
                        let dragged = label.clone();

                        // The field replaces this tab's label while it is up, and the
                        // tab gives up the handlers that would fight it: a click has
                        // to place the caret rather than re-activate the tab, and the
                        // close cell must not sit under a pointer aimed at the end of
                        // a name.
                        let renaming = rename_index == Some(index);
                        let label: AnyElement =
                            match renaming.then(|| rename_field.take()).flatten() {
                                Some(field) => field,
                                None => label.into_any_element(),
                            };

                        // The active tab is raised in the track rather than marked
                        // with a line: the agent's state is already in its mark, and
                        // the fill is what says "this one".
                        div()
                            .id(SharedString::from(format!("tab-{}-{index}", pane.0)))
                            .group(group.clone())
                            .relative()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(8.0))
                            .h_full()
                            .px(px(10.0))
                            .rounded(crate::ui::RADIUS_SM)
                            // A tab's name is a word, not an identifier — "Claude",
                            // "Terminal", a file's name read as a title — so it takes
                            // the prose face, as the sidebar's project names do.
                            .prose()
                            .text_size(TAB_LABEL)
                            .cursor_pointer()
                            .text_color(match (tinted, active) {
                                (true, true) => tint,
                                (true, false) => alpha(tint, 0.8),
                                (false, true) => paint(t.text.primary),
                                (false, false) => paint(t.text.dim),
                            })
                            .when(active, |el| el.font_weight(FontWeight::MEDIUM))
                            .when(active && !tinted, |el| el.bg(paint(t.selection)))
                            // Raised in the tint rather than the neutral fill,
                            // with an edge. An overlay rather than a border on
                            // the tab, which would widen it by two pixels and
                            // shove every tab after it along the strip.
                            .when(active && tinted, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .rounded(crate::ui::RADIUS_SM)
                                        .bg(alpha(tint, 0.14))
                                        .border_1()
                                        .border_color(alpha(tint, 0.45)),
                                )
                            })
                            // Hover feedback only where clicking would do something:
                            // the label comes up to full strength and a soft fill
                            // follows the pointer, so a strip of several tabs says
                            // which one is about to be switched to. A pane holding one
                            // tab has nothing to switch to, and its tab is the active
                            // one anyway, so it stays still.
                            //
                            // The fill is inset and rounded rather than run full
                            // height: a square wash edge to edge reads as a rectangle
                            // drawn round the words, and would collide with the
                            // strip's own bottom edge where the active line lives. It
                            // is a child rather than a background on the tab itself so
                            // that inset is possible at all, and it comes first so it
                            // paints behind the label.
                            .when(!active && switchable, |el| {
                                el.hover(|style| style.text_color(paint(t.text.primary)))
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left_0()
                                            .right_0()
                                            .rounded(crate::ui::RADIUS_SM)
                                            .group_hover(group.clone(), |style| {
                                                style.bg(paint(t.hover))
                                            }),
                                    )
                            })
                            .child(mark)
                            .child(label)
                            .when((closable || tab.pinned) && !renaming, |el| {
                                // The unsaved dot and the close mark share one 16px
                                // cell, and the pointer swaps them. Two cells would
                                // shift every label along the strip as the mouse
                                // crossed it; hiding the close mark behind a dot would
                                // make an unsaved tab the one you cannot close.
                                //
                                // A pinned tab wears its pin there instead, and the
                                // cell unpins it: a cross one stray click from closing
                                // it would undo the point of pinning. Closing it is
                                // still the menu's Close, or Cmd-W.
                                let pinned = tab.pinned;
                                el.child(crate::ui::button::close_cell(
                                    SharedString::from(format!("close-{}-{index}", pane.0)),
                                    if pinned { Icon::Pin } else { Icon::Close },
                                    dirty,
                                    group.clone(),
                                    close_group.clone(),
                                    cx.listener(move |this, _, _, cx| {
                                        if pinned {
                                            this.set_tab_pinned(pane, index, false, cx);
                                        } else {
                                            this.request_close_tab(pane, index, cx);
                                        }
                                    }),
                                    t,
                                ))
                            })
                            // The gap this tab's leading edge marks. The trailing one
                            // is drawn only by the last tab, so the gap between two
                            // neighbours is claimed once rather than ruled twice.
                            .children((gap == Some(index)).then(|| tab_rule(t, false)))
                            .children(
                                (index == last_tab && gap == Some(tab_count))
                                    .then(|| tab_rule(t, true)),
                            )
                            // The precise claim. It runs after the strip's coarse one
                            // because a tab is a child of the strip and gpui's capture
                            // phase asks a parent before its children — and it has to
                            // bounds-check, because these listeners are filtered by
                            // drag type and not by hitbox.
                            .on_drag_move::<TabDrag>(cx.listener(
                                move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                                    let at = event.event.position;
                                    if !event.bounds.contains(&at) {
                                        return;
                                    }
                                    // Which half of the tab the pointer is in picks the
                                    // gap, so every pixel of the strip names one.
                                    let before = if at.x > event.bounds.center().x {
                                        index + 1
                                    } else {
                                        index
                                    };
                                    // Onto the edge of the run the tab may sit
                                    // in, so the rule is drawn where it will land.
                                    let before =
                                        gap_for(before, event.drag(cx).pinned, pinned_count);
                                    if this.note_tab_drop(TabDrop::Strip { pane, before }) {
                                        cx.notify();
                                    }
                                },
                            ))
                            .when(!renaming, |el| {
                                el.when_some(worktree.clone(), |el, worktree| {
                                    // `on_drag` starts only once the pointer moves with
                                    // the button down, so a tab that is clicked is
                                    // still a tab that gets activated.
                                    el.on_drag(
                                        TabDrag {
                                            worktree,
                                            pane,
                                            index,
                                            kind: tab.kind.clone(),
                                            pinned: tab.pinned,
                                            title: dragged.clone(),
                                            theme: *t,
                                            // Filled in by the listener, which is
                                            // where gpui says how far into the tab the
                                            // pointer was.
                                            grab: point(px(0.0), px(0.0)),
                                        },
                                        |drag, grab, _, cx| {
                                            let mut drag = drag.clone();
                                            drag.grab = grab;
                                            cx.new(|_| drag)
                                        },
                                    )
                                })
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.open_tab_context_menu(pane, index, event.position);
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(id) = this.selected_id()
                                            && let Some(space) = this.spaces.get_mut(&id)
                                            && let Some((tabs, active)) = space.root.leaf_mut(pane)
                                            && index < tabs.len()
                                        {
                                            space.focused = pane;
                                            *active = index;
                                        }
                                        this.menu = None;
                                        this.close_tab_menus();
                                        this.persist_layout();
                                        cx.notify();
                                    },
                                ))
                            })
                            // A press anywhere else keeps the name, the way renaming
                            // does in a file list: the field's own handler gives the
                            // keyboard up on the same press, and a field left drawn
                            // without it would be one nothing could finish.
                            .when(renaming, |el| {
                                el.on_mouse_down_out(cx.listener(
                                    |this, _: &MouseDownEvent, _, cx| {
                                        this.commit_tab_rename(cx);
                                        cx.notify();
                                    },
                                ))
                            })
                    }))
                    .children(empty_gap),
            )
            // No spacer before the `+`: it follows the last tab, and with an
            // empty pane it sits at the left edge rather than stranded across
            // the window from everything it relates to.
            .child(add)
            // The empty run past the last tab, and what moves the window from
            // this strip. AppKit was refused the whole window so that the tabs
            // to the left of here could be picked up (see `is_movable` in
            // `main`), and this is where the drag is handed back — the one part
            // of a tab strip with nothing in it to carry.
            .child(
                div()
                    .flex_grow()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        crate::window_drag::drag_window(window);
                    }),
            )
            .into_any_element()
    }

    /// Renders a pane tree with one tab strip per leaf.
    ///
    /// The tree is wrapped so a tab drag has one place to be answered. The
    /// wrapper clears the target on every move and every pane claims it back,
    /// which is the only ordering that works: gpui filters `on_drag_move` by
    /// drag type rather than by hitbox, so all of them run on all moves, and
    /// the capture phase asks a parent before its children. The drop lands
    /// here too rather than on the pane it was released over — by then the
    /// target is already worked out, and a drop a pixel into a divider would
    /// otherwise belong to no element and read as the drag not having worked.
    pub(crate) fn pane_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(space) = self.space().cloned() else {
            return self.landing_layout(cx);
        };
        // Which pane wears the focus ring — none while there is one pane, which
        // is focused by definition, and a ring round the whole card only
        // traces the card's own edge a few pixels in.
        let focused = matches!(space.root, Pane::Split { .. }).then_some(space.focused);
        let tree = self.pane_node(&space.root, focused, &[], window, cx);

        div()
            .id("pane-tree")
            .flex()
            .flex_1()
            .overflow_hidden()
            .child(tree)
            .on_drag_move::<TabDrag>(cx.listener(|this, _, _, cx| {
                this.tab_drop.take();
                // Also where the drag is first noticed at all, which is what
                // gets the native browser views out of the way — see
                // `Shell::browser_occluded`. There is no earlier hook: the
                // constructor `on_drag` takes cannot reach the shell.
                this.tab_dragging = true;
                // Unconditionally, unlike the panes' own listeners. gpui places
                // the thing under the cursor during a draw and asks for no
                // draw of its own, so this frame is what makes the ghost follow
                // the pointer at all — and a ghost that only moves while it is
                // over a pane sticks the moment it crosses a divider or leaves
                // for the sidebar.
                cx.notify();
            }))
            .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| this.drop_tab(drag, cx)))
            // A drag let go somewhere else in the window drops nowhere and
            // tells nobody, so the target it was last over is forgotten here.
            // Without this it outlives the drag that set it and the next thing
            // dragged anywhere — a divider will do — finds a pane still
            // washed for a move that ended some time ago.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_tab_drag(cx)),
            )
            .into_any_element()
    }

    /// The landing view. The header card carries the sidebar and panel
    /// toggles, so nothing here has to bring either back.
    fn landing_layout(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .size_full()
            .child(self.landing_view(cx))
            .into_any_element()
    }

    fn pane_node(
        &mut self,
        pane: &Pane,
        focused: Option<PaneId>,
        path: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match pane {
            Pane::Leaf { id, tabs, active } => {
                let tab_bar = self.tab_bar(*id, tabs, *active, window, cx);
                let content = match tabs.get(*active).map(|tab| tab.kind.clone()) {
                    Some(TabKind::Terminal(terminal)) => self.terminal_pane(terminal, window, cx),
                    Some(TabKind::Editor(key)) => self.editor_pane(&key, Some(*id), window, cx),
                    Some(TabKind::Audio(path)) => self.audio_pane(&path, window, cx),
                    Some(TabKind::Browser(browser)) => self.browser_pane(browser, window, cx),
                    #[cfg(target_os = "macos")]
                    Some(TabKind::IosSimulator) => self.ios_simulator_pane(window, cx),
                    Some(TabKind::Android) => self.android_pane(window, cx),
                    Some(TabKind::Diff(key)) => self.diff_pane(&key, window, cx),
                    None => div()
                        .flex()
                        .size_full()
                        .justify_center()
                        .items_center()
                        .text_color(paint(self.theme.text.dim))
                        .child(
                            self.note
                                .clone()
                                .unwrap_or_else(|| "Open a tab to begin.".into()),
                        )
                        .into_any_element(),
                };
                // A pane showing a session its project launched with its own
                // command is edged in that project's tint.
                let tinted = match tabs.get(*active).map(|tab| &tab.kind) {
                    Some(TabKind::Terminal(terminal)) => self
                        .terminals
                        .get(terminal)
                        .is_some_and(|terminal| terminal.launched_with.is_some()),
                    _ => false,
                };
                let id = *id;
                // A strip target is drawn by the strip; only the two that name
                // a body belong to this pane's content area.
                let wash = match self.tab_drop_shown(cx) {
                    Some(drop @ (TabDrop::Into { .. } | TabDrop::Split { .. }))
                        if drop.pane() == id =>
                    {
                        Some(drop)
                    }
                    _ => None,
                };
                let theme = self.theme;

                div()
                    .id(("pane", id.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    // Both the zones and the wash hang off the pane itself
                    // rather than off a wrapper around `content`, so that
                    // adding them changes no layout at all: every pane body
                    // sizes itself against its parent, and one more box in
                    // between is one more chance to get that wrong.
                    // `body_area` takes the strip off the top instead, which is
                    // all such a wrapper would have been measuring.
                    .on_drag_move::<TabDrag>(cx.listener(
                        move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                            let body = body_area(event.bounds);
                            let at = event.event.position;
                            if body.contains(&at) && this.note_tab_drop(body_drop(id, body, at)) {
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if this.focus_pane(id) {
                                cx.notify();
                            }
                        }),
                    )
                    .child(tab_bar)
                    .child(content)
                    // Last, so it paints over the pane rather than under it:
                    // children are drawn in order, and this one is the preview.
                    .children(wash.map(|drop| drop_wash(drop, &theme)))
                    // The focus ring is an overlay, not a border on the pane:
                    // a border is inside the box, so it would push this pane's
                    // strip and body a pixel down and right of its neighbours'.
                    // After the strip and body so it paints over their fills.
                    // Rounded to sit concentric with the card's corner, which
                    // a pane on the card's edge shares.
                    .when(focused == Some(id) || tinted, |el| {
                        el.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(crate::ui::RADIUS_MD)
                                .border_1()
                                .border_color(if tinted {
                                    alpha(crate::agent_override::hue(&theme), 0.55)
                                } else {
                                    paint(theme.selection)
                                }),
                        )
                    })
                    .into_any_element()
            }
            Pane::Split {
                axis,
                children,
                sizes,
            } => {
                debug_assert_eq!(children.len(), sizes.len());
                let mut rendered = Vec::with_capacity(children.len() * 2 - 1);
                for (index, (child, share)) in children.iter().zip(sizes).enumerate() {
                    let mut child_path = path.to_vec();
                    child_path.push(index);
                    let minimum = child.minimum_extent(*axis);
                    let child = self.pane_node(child, focused, &child_path, window, cx);
                    rendered.push(
                        div()
                            // A flex column, not the block box `div()` gives by
                            // default. A pane fills its slot by growing —
                            // `flex_1` on a leaf, `size_full` on a nested split
                            // — and neither means anything inside a block box:
                            // block layout hands a child `height: auto`, which
                            // for a pane is its tab strip and nothing else. The
                            // slot itself is sized by the split around it, so
                            // this only has to pass that height on.
                            .flex()
                            .flex_col()
                            .flex_basis(relative(*share))
                            .flex_shrink()
                            .when(*axis == Axis::Horizontal, |el| el.min_w(minimum))
                            .when(*axis == Axis::Vertical, |el| el.min_h(minimum))
                            .overflow_hidden()
                            .child(child)
                            .into_any_element(),
                    );
                    if index + 1 < children.len() {
                        rendered.push(self.pane_divider(*axis, path, index));
                    }
                }
                let resize_path = path.to_vec();
                let resize_axis = *axis;
                div()
                    .id(SharedString::from(format!("split-{resize_path:?}")))
                    .flex()
                    .when(*axis == Axis::Vertical, |el| el.flex_col())
                    .size_full()
                    .overflow_hidden()
                    .children(rendered)
                    .on_drag_move::<PaneResize>(cx.listener(
                        move |this, event: &DragMoveEvent<PaneResize>, _, cx| {
                            let drag = event.drag(cx).clone();
                            if drag.path != resize_path {
                                return;
                            }
                            let (cursor, extent) = match resize_axis {
                                Axis::Horizontal => (
                                    event.event.position.x - event.bounds.left(),
                                    event.bounds.size.width,
                                ),
                                Axis::Vertical => (
                                    event.event.position.y - event.bounds.top(),
                                    event.bounds.size.height,
                                ),
                            };
                            if let Some(id) = this.selected_id()
                                && let Some(space) = this.spaces.get_mut(&id)
                                && space.resize_split(&drag.path, drag.divider, cursor, extent)
                            {
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.persist_layout()),
                    )
                    .into_any_element()
            }
        }
    }

    fn pane_divider(&self, axis: Axis, path: &[usize], divider: usize) -> AnyElement {
        let drag = PaneResize {
            path: path.to_vec(),
            divider,
        };
        let cursor = match axis {
            Axis::Horizontal => CursorStyle::ResizeLeftRight,
            Axis::Vertical => CursorStyle::ResizeUpDown,
        };

        div()
            .id(SharedString::from(format!("divider-{path:?}-{divider}")))
            .relative()
            .flex_none()
            .when(axis == Axis::Horizontal, |el| el.w(DIVIDER_SIZE).h_full())
            .when(axis == Axis::Vertical, |el| el.h(DIVIDER_SIZE).w_full())
            .bg(paint(self.theme.panel))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "divider-grab-{path:?}-{divider}"
                    )))
                    .absolute()
                    .when(axis == Axis::Horizontal, |el| {
                        el.top_0()
                            .bottom_0()
                            .left(-crate::tree::DIVIDER_GRAB)
                            .right(-crate::tree::DIVIDER_GRAB)
                    })
                    .when(axis == Axis::Vertical, |el| {
                        el.left_0()
                            .right_0()
                            .top(-crate::tree::DIVIDER_GRAB)
                            .bottom(-crate::tree::DIVIDER_GRAB)
                    })
                    .cursor(cursor)
                    .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf_ids(space: &Space) -> Vec<PaneId> {
        let mut ids = Vec::new();
        space.root.leaf_ids(&mut ids);
        ids
    }

    fn assert_sizes(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
        }
    }

    fn sample_agent(name: &str) -> AgentSpec {
        AgentSpec {
            name: name.to_owned(),
            transport: ket_core::config::Transport::Pty,
            command: name.to_owned(),
            args: Vec::new(),
            env: Default::default(),
            env_remove: Vec::new(),
            launch: None,
        }
    }

    #[test]
    fn the_new_tab_menu_puts_the_agent_session_after_the_only_separator() {
        let pane = PaneId(3);
        let agent = sample_agent("claude");
        let menu = new_tab_menu(pane, &[agent], false, false, &Theme::dark());

        assert!(menu.opened_by(new_tab_origin(pane).as_ref()));

        let sections = menu.sections();
        assert_eq!(
            sections.len(),
            2,
            "exactly one heading, ahead of the agent rows"
        );
        let titles: Vec<_> = sections
            .iter()
            .flatten()
            .map(|item| item.title.as_ref())
            .collect();
        assert_eq!(titles, ["File", "Terminal", "Browser", "Claude Code"]);

        let actions: Vec<_> = sections
            .iter()
            .flatten()
            .map(|item| item.action.clone())
            .collect();
        assert_eq!(
            actions,
            [
                MenuAction::File(pane),
                MenuAction::Terminal(pane),
                MenuAction::NewBrowser(pane),
                MenuAction::Agent(pane, "claude".into()),
            ],
            "every item acts on the pane whose menu was opened"
        );

        // The hint on the terminal row is the chord the shell really
        // dispatches, so the menu cannot promise a shortcut it does not own.
        let terminal = &sections[0][1];
        assert_eq!(
            terminal.chord.as_ref(),
            Some(&Chord::parse("cmd+t").unwrap())
        );
    }

    #[test]
    fn the_new_tab_menu_lists_every_runnable_agent() {
        let pane = PaneId(3);
        let agents = [sample_agent("claude"), sample_agent("codex")];
        let menu = new_tab_menu(pane, &agents, false, false, &Theme::dark());

        let sections = menu.sections();
        let titles: Vec<_> = sections
            .iter()
            .flatten()
            .map(|item| item.title.as_ref())
            .collect();
        assert_eq!(
            titles,
            ["File", "Terminal", "Browser", "Claude Code", "Codex"],
            "an enabled agent gets its own row, not just the worktree's assigned one"
        );
    }

    #[test]
    fn the_new_tab_menu_has_no_agent_section_without_a_runnable_agent() {
        let pane = PaneId(3);
        let menu = new_tab_menu(pane, &[], false, false, &Theme::dark());

        let sections = menu.sections();
        assert_eq!(
            sections.len(),
            1,
            "nothing to start means no dangling heading either"
        );
        let titles: Vec<_> = sections
            .iter()
            .flatten()
            .map(|item| item.title.as_ref())
            .collect();
        assert_eq!(titles, ["File", "Terminal", "Browser"]);
    }

    #[test]
    fn the_new_tab_menu_filters_by_what_was_typed() {
        use gpui::{KeyDownEvent, Keystroke, Modifiers};

        let mut menu = new_tab_menu(PaneId(0), &[], false, false, &Theme::dark());
        for character in "term".chars() {
            menu.key(&KeyDownEvent {
                keystroke: Keystroke {
                    modifiers: Modifiers::default(),
                    key: character.to_string(),
                    key_char: Some(character.to_string()),
                },
                is_held: false,
            });
        }

        let sections = menu.sections();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0][0].title.as_ref(), "Terminal");
    }

    #[test]
    fn splitting_a_leaf_creates_two_equal_children_and_focuses_the_new_one() {
        let mut space = Space::default();
        space.split(Axis::Horizontal);

        assert_eq!(leaf_ids(&space), vec![PaneId(0), PaneId(1)]);
        assert_eq!(space.focused, PaneId(1));
        match &space.root {
            Pane::Split { axis, sizes, .. } => {
                assert_eq!(*axis, Axis::Horizontal);
                assert_eq!(sizes, &[0.5, 0.5]);
            }
            Pane::Leaf { .. } => panic!("split left a leaf at the root"),
        }
    }

    #[test]
    fn closing_a_pane_restores_the_surviving_leaf_as_the_root() {
        let mut space = Space::default();
        space.split(Axis::Vertical);

        assert!(space.close_focused_pane());
        assert_eq!(leaf_ids(&space), vec![PaneId(0)]);
        assert_eq!(space.focused, PaneId(0));
        assert!(matches!(space.root, Pane::Leaf { .. }));
    }

    #[test]
    fn closing_the_last_tab_collapses_its_leaf() {
        let mut space = Space::default();
        space.split(Axis::Horizontal);
        // Panes open empty now, so give this one the tab the test is about
        // to close.
        space.open_tab_in(PaneId(1), Pane::test_tab());

        space.close_tab(PaneId(1), 0);

        assert_eq!(leaf_ids(&space), vec![PaneId(0)]);
        assert_eq!(space.focused, PaneId(0));
    }

    #[test]
    fn a_split_with_one_child_collapses_into_its_parent() {
        let mut space = Space::default();
        space.split(Axis::Horizontal);
        space.split(Axis::Vertical);

        assert!(space.close_focused_pane());

        assert_eq!(leaf_ids(&space), vec![PaneId(0), PaneId(1)]);
        assert!(matches!(
            space.root,
            Pane::Split {
                axis: Axis::Horizontal,
                ..
            }
        ));
    }

    #[test]
    fn focus_moves_to_the_next_leaf_when_the_focused_one_disappears() {
        let mut space = Space::default();
        space.split(Axis::Horizontal);
        space.focused = PaneId(0);

        assert!(space.close_focused_pane());
        assert_eq!(space.focused, PaneId(1));
    }

    #[test]
    fn the_only_leaf_cannot_be_closed() {
        let mut space = Space::default();
        space.open_tab_in(PaneId(0), Pane::test_tab());

        assert!(!space.close_focused_pane());
        space.close_tab(PaneId(0), 0);

        // The leaf survives losing its last tab when it is the only one:
        // collapsing it would leave the worktree with no pane at all.
        assert_eq!(leaf_ids(&space), vec![PaneId(0)]);
    }

    #[test]
    fn dragging_a_divider_changes_only_its_adjacent_shares() {
        let mut space = Space {
            root: Pane::Split {
                axis: Axis::Horizontal,
                children: vec![
                    Pane::empty(PaneId(0)),
                    Pane::empty(PaneId(1)),
                    Pane::empty(PaneId(2)),
                ],
                sizes: vec![0.2, 0.3, 0.5],
            },
            focused: PaneId(0),
            next_id: 3,
        };

        // A thousand pixels of panes between two dividers, and the second
        // divider dragged to 700 of them.
        let extent = px(1000.0) + DIVIDER_SIZE * 2;
        assert!(space.resize_split(&[], 1, px(700.0) + DIVIDER_SIZE, extent));

        let Pane::Split { sizes, .. } = &space.root else {
            panic!("root is not split");
        };
        assert_sizes(sizes, &[0.2, 0.5, 0.3]);
    }

    #[test]
    fn dragging_a_divider_stops_before_a_pane_becomes_unreachable() {
        let mut space = Space::default();
        space.split(Axis::Horizontal);

        assert!(space.resize_split(&[], 0, px(10.0), px(1000.0) + DIVIDER_SIZE));

        let Pane::Split { sizes, .. } = &space.root else {
            panic!("root is not split");
        };
        let least = MIN_PANE_SIZE / px(1000.0);
        assert_sizes(sizes, &[least, 1.0 - least]);
    }

    #[test]
    fn a_nested_split_reserves_room_for_every_leaf_on_the_same_axis() {
        let mut space = Space::default();
        space.split(Axis::Horizontal);
        space.focused = PaneId(0);
        space.split(Axis::Horizontal);

        assert!(space.resize_split(&[], 0, px(10.0), px(1000.0) + DIVIDER_SIZE));

        let Pane::Split { sizes, .. } = &space.root else {
            panic!("root is not split");
        };
        // The nested split keeps room for both its panes and the divider
        // between them.
        let least = (MIN_PANE_SIZE * 2.0 + DIVIDER_SIZE) / px(1000.0);
        assert_sizes(sizes, &[least, 1.0 - least]);
    }

    #[test]
    fn closing_a_pane_preserves_the_remaining_resize_proportions() {
        let mut space = Space {
            root: Pane::Split {
                axis: Axis::Horizontal,
                children: vec![
                    Pane::empty(PaneId(0)),
                    Pane::empty(PaneId(1)),
                    Pane::empty(PaneId(2)),
                ],
                sizes: vec![0.2, 0.3, 0.5],
            },
            focused: PaneId(1),
            next_id: 3,
        };

        assert!(space.close_focused_pane());

        let Pane::Split { sizes, .. } = &space.root else {
            panic!("root is not split");
        };
        assert_sizes(sizes, &[2.0 / 7.0, 5.0 / 7.0]);
    }
}
