//! Chromium in ket's browser tabs, for a ket built with the `chromium`
//! feature. The tab, its toolbar and its address field are the ones the
//! WebKit tab has — see `crate::browser` — and the page in it is Chromium's,
//! through `ket_chromium`.
//!
//! - **Started by the first browser tab**, not with ket: a window that never
//!   opens one never loads Chromium. When it cannot start — ket was built with
//!   the feature but is not running from an app with Chromium inside — the tab
//!   says why and how to run it instead, and nothing else changes.
//! - **One profile per worktree**, each a directory beneath one root for all
//!   of Chromium, so what a page keeps — a sign-in, say — stays with the
//!   branch it was made on. A development build has a root of its own:
//!   Chromium locks its root, and a dev ket beside a release one could not
//!   otherwise start it.
//! - **A page is created as its tab opens**, into a slot parked off the
//!   window until its pane shows it, rather than when the pane first draws:
//!   an agent can drive a page in a worktree nobody is looking at.
//! - **What a page reports** — its address, title, history, console, a crash
//!   — lands on its tab: see [`Shell::chromium_event`].
//! - **A popup is a tab** — `window.open`, or a link to a new tab — beside
//!   its opener's, in the same worktree and profile, with `window.opener`
//!   kept so a sign-in can answer the page that asked for it. When the popup
//!   closes itself, its tab goes.
//! - **A worktree's browser goes with it.** Deleting a worktree — here, from
//!   the CLI, from a phone, by a prune — closes its tabs, and once its pages
//!   have closed, deletes its profile under both roots: see
//!   [`Shell::chromium_registry_changed`]. ket-core deletes it itself when no
//!   Chromium holds the root.
//! - **Screenshot to agent**: the camera in the toolbar takes the page as
//!   Chromium draws it and opens the Quick prompt with it, pointed at the
//!   tab's worktree — what ⌘S does in a device tab. See
//!   [`Shell::capture_chromium`].
//! - **Downloads go to the Downloads folder** without asking, under a name
//!   nothing there has. The tab shows the latest in a banner under its
//!   toolbar, with Show in Finder once it is done, and a toast says when it
//!   finishes or fails, wherever the tab is.
//! - **A JavaScript dialog shows in its tab**, in place of the page, as
//!   Chrome shows one: `alert`, `confirm`, `prompt` and the question to
//!   leave a page. An agent can answer it instead — `ket browser dialog` —
//!   and whichever answers first, it goes. See [`Shell::chromium_dialog`].
//!
//! The window's root view, which every page's slot goes in, is kept from the
//! moment the window exists ([`Chromium::adopt_root`]), so a tab opened outside
//! a render — by an agent — has somewhere to put its page at once.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, Context, Entity, SharedString, WeakEntity, Window, div, prelude::*, px, relative,
};
use ket_chromium::{Dialog, DialogKind};
use ket_core::id::WorktreeId;
use ket_core::workspace::Workspace;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::MainThreadMarker;

use crate::Shell;
use crate::browser::{BrowserHandle, BrowserId, normalize_url};
use crate::browser_view::NativeBrowser;
use crate::device_capture::DeviceKind;
use crate::input::{Style, TextInput, text_field};
use crate::paint::paint;
use crate::tabs::{Tab, TabDrop, TabKind};
use crate::ui::banner::banner;
use crate::ui::button::{button, icon_button};
use crate::ui::dialog;
use crate::ui::icon::Icon;
use crate::ui::toast::Tone;
use crate::ui::tooltip::{Side, tooltip};

/// How many console messages a tab keeps; the oldest goes first.
const CONSOLE_KEPT: usize = 1_000;

/// How much of one console message, or of the script it came from, a tab
/// keeps, in bytes.
const CONSOLE_TEXT_MAX: usize = 1_024;

/// How much of a page's title a tab keeps, in bytes.
const TITLE_MAX: usize = 256;

/// How much of its address a tab named for an untitled page shows, in bytes.
const ADDRESS_TITLE_MAX: usize = 60;

/// How much of a JavaScript dialog's message its card shows, in characters:
/// a page can hand `alert` megabytes, and the card lays out all it shows.
const DIALOG_MESSAGE_MAX: usize = 10_000;

/// How far the dialog card keeps from the edges of its pane.
const DIALOG_MARGIN: gpui::Pixels = px(16.0);

/// How long quitting waits for Chromium's pages to close.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(1);

/// How long a deleted worktree's pages are given to close before its
/// profile is deleted anyway.
const PROFILE_CLOSE_WAIT: Duration = Duration::from_secs(10);

/// How often that wait looks.
const PROFILE_CLOSE_POLL: Duration = Duration::from_millis(100);

/// How long after its last page closes a profile is left, for Chromium to
/// let go of its files.
const PROFILE_SETTLE: Duration = Duration::from_secs(1);

thread_local! {
    /// Profiles being deleted, from when a deletion is decided until it has
    /// finished. No page opens on one meanwhile, so Chromium never writes
    /// into files going away, nor finds them gone beneath it — a worktree
    /// made again under the same id, or an agent opening a tab while
    /// Settings deletes. Main thread, where pages open.
    static DELETING: RefCell<HashSet<PathBuf>> = RefCell::new(HashSet::new());
}

/// Reserves `profile` for deleting, when no page has it open and no other
/// deletion has it: no page opens on it until [`release_profile`]. `false`
/// leaves it be. Main thread.
pub(crate) fn reserve_profile(profile: &Path) -> bool {
    !ket_chromium::profile_in_use(profile)
        && DELETING.with_borrow_mut(|deleting| deleting.insert(profile.to_owned()))
}

/// Lets pages open on `profile` again, its deletion done. Main thread.
pub(crate) fn release_profile(profile: &Path) {
    DELETING.with_borrow_mut(|deleting| {
        deleting.remove(profile);
    });
}

/// How long a page may take to draw a screenshot for an agent.
const SHOT_WAIT: Duration = Duration::from_secs(10);

/// A browser tab, as an agent sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code, reason = "for `ket browser`, whose side of ket-ui calls it")]
pub(crate) struct ChromiumTab {
    /// The worktree whose space the tab is in.
    pub worktree: WorktreeId,
    /// The tab.
    pub id: BrowserId,
    /// The page's address as it last reported one, or the one it was opened
    /// on.
    pub url: String,
    /// The page's title as it last reported one; empty until then.
    pub title: String,
    /// Whether the tab is the one its pane is showing.
    pub active: bool,
    /// The JavaScript dialog its page has open, waiting for an answer.
    pub dialog: Option<Dialog>,
}

/// A message a page logged to its console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleEntry {
    /// When ket heard it, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// `debug`, `info`, `warning` or `error`.
    pub level: String,
    /// What it logged, cut to [`CONSOLE_TEXT_MAX`] bytes.
    pub text: String,
    /// The script it came from, cut the same way.
    pub source: String,
    /// The line in that script.
    pub line: u32,
}

/// The window's side of Chromium.
#[derive(Default)]
pub(crate) struct Chromium {
    /// gpui's view: where every page's slot goes.
    root: Option<Retained<NSView>>,
    /// The worktrees ket's registry had at the last look, from the first
    /// page on: one gone from it since was deleted, and its browser goes.
    registered: Option<HashSet<WorktreeId>>,
}

impl Chromium {
    /// Keeps the window's root view, the first time there is one. Called as
    /// the window opens and again each frame, which costs a check.
    pub(crate) fn adopt_root(&mut self, window: &Window) {
        if self.root.is_none() {
            self.root = crate::window_drag::root_view(window);
        }
    }
}

/// What a Chromium page has said about itself, kept on its tab.
#[derive(Default)]
pub(crate) struct PageState {
    /// The worktree the page was opened for, whose profile it uses.
    worktree: Option<WorktreeId>,
    /// Its address as it last reported one, or the one it was opened on.
    pub(crate) url: String,
    /// Its title as it last reported one.
    title: String,
    /// Whether going back would go anywhere.
    pub(crate) back: bool,
    /// Whether going forward would go anywhere.
    pub(crate) forward: bool,
    /// Why its renderer ended, until it is reloaded: the pane says so in
    /// place of the page, which is blank until then.
    pub(crate) crashed: Option<String>,
    /// Its last [`CONSOLE_KEPT`] console messages, oldest first.
    console: VecDeque<ConsoleEntry>,
    /// The latest download it started, until the banner saying so is put
    /// away.
    download: Option<Download>,
    /// Another page opened it: when it closes itself, its tab goes.
    popup: bool,
    /// The JavaScript dialog it has open, which the pane shows in its place
    /// until it is answered.
    pub(crate) dialog: Option<OpenDialog>,
    /// Whether the pointer is over the toolbar's camera, whose tooltip shows.
    capture_hovered: bool,
}

/// A JavaScript dialog a page has open, and a prompt's field.
pub(crate) struct OpenDialog {
    dialog: Dialog,
    field: Option<Entity<TextInput>>,
}

/// A download a page started, as its tab last heard of it.
struct Download {
    id: u32,
    name: String,
    path: PathBuf,
    received: i64,
    total: i64,
    done: bool,
    failed: bool,
}

impl Download {
    /// What the banner says about it.
    fn line(&self) -> String {
        let name = &self.name;
        if self.done {
            format!("Downloaded {name}")
        } else if self.failed {
            format!("Couldn't download {name}")
        } else if self.total > 0 {
            let percent = (self.received.max(0) * 100 / self.total).min(100);
            format!("Downloading {name}\u{2026} {percent}%")
        } else {
            format!("Downloading {name}\u{2026} {}", size(self.received))
        }
    }

    fn tone(&self) -> Tone {
        if self.done {
            Tone::Success
        } else if self.failed {
            Tone::Error
        } else {
            Tone::Info
        }
    }
}

/// `bytes` the way Finder says them: decimal units, one place past a unit.
fn size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    let units = ["KB", "MB", "GB", "TB"];
    if bytes < 1_000.0 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes / 1_000.0;
    let mut unit = 0;
    while value >= 1_000.0 && unit + 1 < units.len() {
        value /= 1_000.0;
        unit += 1;
    }
    format!("{value:.1} {}", units[unit])
}

impl PageState {
    fn log(&mut self, entry: ConsoleEntry) {
        if self.console.len() == CONSOLE_KEPT {
            self.console.pop_front();
        }
        self.console.push_back(entry);
    }
}

/// Whether Chromium started, or why not, once something has asked: Chromium
/// starts once per process at most, so a failure stands.
static STARTED: OnceLock<Result<(), String>> = OnceLock::new();

/// Starts Chromium unless something already tried, and says whether it is
/// running — or, in words for the tab to show, why not. On the main thread.
fn start() -> Result<(), String> {
    STARTED
        .get_or_init(|| {
            let root_cache = root_cache()?;
            ket_chromium::start(ket_chromium::Config {
                root_cache,
                dev: cfg!(debug_assertions),
            })
            .map_err(|error| match error {
                ket_chromium::Error::NotBundled(why) => format!(
                    "Browser tabs need Chromium, and this ket is not running from an app with \
                     Chromium inside. Run it with ./ket-dev.sh. ({why})"
                ),
                ket_chromium::Error::Failed(why) => format!("Chromium did not start: {why}."),
            })
        })
        .clone()
}

/// Where Chromium keeps everything: `chromium` in ket's data directory, or
/// `chromium-dev` for a development build, since Chromium locks its root and
/// the two run side by side.
pub(crate) fn root_cache() -> Result<PathBuf, String> {
    ket_core::paths::chromium_root(cfg!(debug_assertions))
        .map_err(|error| format!("Chromium has nowhere to keep its data: {error}."))
}

/// The directory a worktree's profile has beneath the root — see
/// `ket_core::paths::chromium_profile_name`.
pub(crate) fn profile_name(worktree: &WorktreeId) -> String {
    ket_core::paths::chromium_profile_name(worktree)
}

/// `text` cut to at most `max` bytes on a character boundary, ending in an
/// ellipsis where it was cut.
fn capped(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    text
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Closes Chromium's pages and stops it, for ket's quit: synchronously,
/// because gpui gives what runs on quit 100 ms and Chromium may want longer.
/// Nothing if it never started.
pub(crate) fn shutdown() {
    if ket_chromium::started() {
        ket_chromium::shutdown(SHUTDOWN_WAIT);
    }
}

/// What `ket browser` — an agent's way to a page — asks of the window.
#[allow(
    dead_code,
    reason = "for `ket browser`, whose side of ket-ui calls these"
)]
impl Shell {
    /// Every browser tab in the window, in the order they were opened.
    pub(crate) fn chromium_tabs(&self) -> Vec<ChromiumTab> {
        let mut tabs: Vec<ChromiumTab> = self
            .spaces
            .iter()
            .flat_map(|(worktree, space)| {
                let active = space.active_browser_ids();
                space.browser_ids().into_iter().filter_map(move |id| {
                    let page = &self.browsers.get(&id)?.chromium;
                    Some(ChromiumTab {
                        worktree: worktree.clone(),
                        id,
                        url: page.url.clone(),
                        title: page.title.clone(),
                        active: active.contains(&id),
                        dialog: page.dialog.as_ref().map(|open| open.dialog.clone()),
                    })
                })
            })
            .collect();
        tabs.sort_by_key(|tab| tab.id.0);
        tabs
    }

    /// Browser `id`'s page, while there is one: from its tab opening until
    /// the page closes.
    pub(crate) fn chromium_browser(&self, id: BrowserId) -> Option<&ket_chromium::Browser> {
        Some(self.browsers.get(&id)?.view.as_deref()?.page())
    }

    /// What browser `id`'s page has logged to its console lately, oldest
    /// first.
    pub(crate) fn chromium_console(&self, id: BrowserId) -> Vec<ConsoleEntry> {
        self.browsers
            .get(&id)
            .map(|browser| browser.chromium.console.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Opens a browser tab on `url` in `worktree`'s space, in its focused
    /// pane, without changing which worktree is selected — for an agent. The
    /// page is created at once, whether or not the pane is on screen. When
    /// it cannot be, the tab is not left behind, and the reason comes back.
    pub(crate) fn open_chromium_tab(
        &mut self,
        worktree: &WorktreeId,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Result<BrowserId, String> {
        let known = self
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .any(|node| &node.id == worktree);
        if !known {
            return Err(format!("ket has no worktree {worktree}"));
        }
        let pane = self.spaces.entry(worktree.clone()).or_default().focused;
        let id = self
            .open_browser_tab_at(worktree, pane, Some(normalize_url(url)), cx)
            .ok_or_else(|| format!("worktree {worktree} has no pane to open a tab in"))?;
        if let Some(error) = self.browsers.get(&id).and_then(|b| b.error.clone()) {
            if let Some(space) = self.spaces.get_mut(worktree)
                && let Some((pane, index)) = space.root.find_tab(&TabKind::Browser(id))
            {
                space.close_tab(pane, index);
            }
            self.browsers.remove(&id);
            return Err(error.to_string());
        }
        self.persist_layout();
        cx.notify();
        Ok(id)
    }
}

impl Shell {
    /// Creates browser `id`'s page on `url`, in `worktree`'s profile,
    /// starting Chromium first if this is the first page. The page waits
    /// parked off the window until its pane shows it. When it cannot be
    /// created the tab says why instead; before the window has a view to put
    /// it in, the pane's first render tries again.
    pub(crate) fn open_chromium(
        &mut self,
        id: BrowserId,
        worktree: &WorktreeId,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        let root = self.chromium.root.clone();
        let Some(browser) = self.browsers.get_mut(&id) else {
            return;
        };
        browser.chromium.worktree = Some(worktree.clone());
        url.clone_into(&mut browser.chromium.url);
        let Some(root) = root else {
            return;
        };
        let opened = start().and_then(|()| {
            let main =
                MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_owned())?;
            let profile = root_cache()?.join(profile_name(worktree));
            if DELETING.with_borrow(|deleting| deleting.contains(&profile)) {
                return Err(
                    "This worktree's browser data is being deleted. Open the tab \
                            again in a moment."
                        .to_owned(),
                );
            }
            NativeBrowser::open(root, &profile, url, main)
                .map_err(|error| format!("Chromium could not open the page: {error}."))
        });
        match opened {
            Ok((view, events)) => {
                browser.view = Some(Rc::new(view));
                browser.shown = false;
                browser.loads = Some(watch(id, events, cx));
                // What a worktree deleted from now on is told by — see
                // `chromium_registry_changed`.
                if self.chromium.registered.is_none() {
                    self.chromium.registered = Workspace::open()
                        .and_then(|workspace| workspace.worktrees(None))
                        .ok()
                        .map(|worktrees| worktrees.into_iter().map(|w| w.id).collect());
                }
            }
            Err(error) => browser.error = Some(error.into()),
        }
        cx.notify();
    }

    /// Gives a page browser `opener`'s page opened a tab of its own, beside
    /// the opener's in the same pane and worktree — in front, unless it was
    /// opened behind — without changing which worktree is selected. With
    /// nowhere to put it, it is dropped, which closes it.
    fn adopt_chromium_popup(
        &mut self,
        opener: BrowserId,
        popup: ket_chromium::PopupPage,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self
            .browsers
            .get(&opener)
            .and_then(|browser| browser.chromium.worktree.clone())
        else {
            return;
        };
        let opener_tab = TabKind::Browser(opener);
        let Some((pane, index)) = self
            .spaces
            .get(&worktree)
            .and_then(|space| space.root.find_tab(&opener_tab))
        else {
            return;
        };
        let (Some(root), Some(main)) = (self.chromium.root.clone(), MainThreadMarker::new()) else {
            return;
        };
        let url = match popup.url() {
            "" => "about:blank".to_owned(),
            url => url.to_owned(),
        };
        let foreground = popup.foreground();
        let id = BrowserId(self.next_browser);
        self.next_browser += 1;
        let (view, events) = NativeBrowser::adopt(root, popup, main);
        let mut browser = BrowserHandle::new(Some(url.clone()), cx);
        browser.chromium.worktree = Some(worktree.clone());
        browser.chromium.url = url;
        browser.chromium.popup = true;
        browser.view = Some(Rc::new(view));
        browser.loads = Some(watch(id, events, cx));
        self.browsers.insert(id, browser);

        let tab = |kind| Tab {
            title: "Browser".into(),
            renamed: false,
            pinned: false,
            preview: false,
            kind,
        };
        if let Some(space) = self.spaces.get_mut(&worktree) {
            space.open_tab_in(pane, tab(TabKind::Browser(id)));
            if let Some((_, at)) = space.root.find_tab(&TabKind::Browser(id)) {
                space.move_tab(
                    pane,
                    at,
                    TabDrop::Strip {
                        pane,
                        before: index + 1,
                    },
                );
            }
            if !foreground {
                space.open_tab_in(pane, tab(opener_tab));
            }
        }
        self.persist_layout();
        self.sync_browser_visibility(cx);
        cx.notify();
    }

    /// The sidebar has been reloaded from `workspace`: a worktree its
    /// registry had at the last look and has no longer was deleted — here,
    /// from the CLI or a phone, or by a prune — so its browser tabs close,
    /// and once their pages have, its profile is deleted. Nothing until a
    /// page has opened, and never for a worktree only hidden from the
    /// sidebar.
    pub(crate) fn chromium_registry_changed(
        &mut self,
        workspace: &Workspace,
        cx: &mut Context<Self>,
    ) {
        let Some(before) = self.chromium.registered.as_ref() else {
            return;
        };
        let Ok(worktrees) = workspace.worktrees(None) else {
            return;
        };
        let now: HashSet<WorktreeId> = worktrees.into_iter().map(|w| w.id).collect();
        let gone: Vec<WorktreeId> = before.difference(&now).cloned().collect();
        self.chromium.registered = Some(now);
        if !gone.is_empty() {
            self.forget_chromium_worktrees(gone, cx);
        }
    }

    /// Closes the browser tabs of deleted worktrees `gone`, and deletes their
    /// profiles once every page in them has closed — or, a page that will
    /// not close, after [`PROFILE_CLOSE_WAIT`].
    fn forget_chromium_worktrees(&mut self, gone: Vec<WorktreeId>, cx: &mut Context<Self>) {
        let closing: Vec<BrowserId> = self
            .browsers
            .iter()
            .filter(|(_, browser)| {
                browser
                    .chromium
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| gone.contains(worktree))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &closing {
            let kind = TabKind::Browser(*id);
            for space in self.spaces.values_mut() {
                if let Some((pane, index)) = space.root.find_tab(&kind) {
                    space.close_tab(pane, index);
                }
            }
            // Dropped, its page closes.
            self.browsers.remove(id);
        }
        if !closing.is_empty() {
            self.persist_layout();
            self.sync_browser_visibility(cx);
            cx.notify();
        }
        let Ok(root) = root_cache() else {
            return;
        };
        let profiles: Vec<(WorktreeId, PathBuf)> = gone
            .into_iter()
            .map(|worktree| {
                let profile = root.join(profile_name(&worktree));
                (worktree, profile)
            })
            .collect();
        cx.spawn(async move |_, cx| {
            let given_up = Instant::now() + PROFILE_CLOSE_WAIT;
            while profiles
                .iter()
                .any(|(_, profile)| ket_chromium::profile_in_use(profile))
                && Instant::now() < given_up
            {
                cx.background_executor().timer(PROFILE_CLOSE_POLL).await;
            }
            cx.background_executor().timer(PROFILE_SETTLE).await;
            // Decided again now, on the main thread where pages open: a
            // worktree made again under the same id in the meantime keeps
            // its profile, and so does one a page still has open — a page
            // that never closed, or one opened since. Those wait for
            // Settings → Browser.
            let Ok(registered) = Workspace::open().and_then(|workspace| workspace.worktrees(None))
            else {
                return;
            };
            let reserved: Vec<(WorktreeId, PathBuf)> = profiles
                .into_iter()
                .filter(|(worktree, profile)| {
                    !registered.iter().any(|w| &w.id == worktree) && reserve_profile(profile)
                })
                .collect();
            if reserved.is_empty() {
                return;
            }
            let worktrees: Vec<WorktreeId> = reserved
                .iter()
                .map(|(worktree, _)| worktree.clone())
                .collect();
            cx.background_executor()
                .spawn(async move {
                    let Ok(workspace) = Workspace::open() else {
                        return;
                    };
                    for worktree in &worktrees {
                        workspace.remove_browser_profiles(worktree, Some(&root));
                    }
                })
                .await;
            for (_, profile) in &reserved {
                release_profile(profile);
            }
        })
        .detach();
    }

    /// Browser `id`'s page closed itself — a popup's `window.close()` — so
    /// its tab goes too.
    fn close_chromium_tab(&mut self, id: BrowserId, cx: &mut Context<Self>) {
        let kind = TabKind::Browser(id);
        for space in self.spaces.values_mut() {
            if let Some((pane, index)) = space.root.find_tab(&kind) {
                space.close_tab(pane, index);
            }
        }
        self.browsers.remove(&id);
        self.persist_layout();
        self.sync_browser_visibility(cx);
        cx.notify();
    }

    /// The render's way in for a page not created yet — see
    /// [`Shell::open_chromium`].
    pub(crate) fn ensure_chromium(
        &mut self,
        id: BrowserId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let waiting = self
            .browsers
            .get(&id)
            .filter(|browser| browser.view.is_none() && browser.error.is_none())
            .and_then(|browser| {
                Some((
                    browser.chromium.worktree.clone()?,
                    browser.chromium.url.clone(),
                ))
            });
        let Some((worktree, url)) = waiting else {
            return;
        };
        self.chromium.adopt_root(window);
        self.open_chromium(id, &worktree, &url, cx);
    }

    /// Something browser `id`'s page said, in the order it said it.
    fn chromium_event(
        &mut self,
        id: BrowserId,
        event: ket_chromium::Event,
        cx: &mut Context<Self>,
    ) {
        use ket_chromium::Event;
        let Some(browser) = self.browsers.get_mut(&id) else {
            return;
        };
        match event {
            Event::Created => {
                // Chromium may give its new page the keyboard; whatever of
                // ket's had it keeps it until the page is clicked.
                if let Some(view) = &browser.view {
                    view.release_keyboard();
                }
            }
            Event::Address(url) => {
                browser.chromium.url.clone_from(&url);
                self.browser_loaded(id, Some(url), cx);
                return;
            }
            Event::Title(title) => {
                let title = capped(title, TITLE_MAX);
                let tab = SharedString::from(tab_title(&title, &browser.chromium.url));
                browser.chromium.title = title;
                for space in self.spaces.values_mut() {
                    space.retitle(&TabKind::Browser(id), tab.clone());
                }
            }
            Event::Loading {
                loading,
                back,
                forward,
            } => {
                browser.chromium.back = back;
                browser.chromium.forward = forward;
                // Loading again — an agent's reload, say — brings a crashed
                // page's renderer back.
                if loading && browser.chromium.crashed.take().is_some() {
                    self.sync_browser_visibility(cx);
                }
            }
            Event::Console {
                level,
                text,
                source,
                line,
            } => {
                browser.chromium.log(ConsoleEntry {
                    at_ms: now_ms(),
                    level,
                    text: capped(text, CONSOLE_TEXT_MAX),
                    source: capped(source, CONSOLE_TEXT_MAX),
                    line,
                });
                // Nothing on screen shows it.
                return;
            }
            Event::Crashed(why) => {
                browser.chromium.crashed = Some(why);
                browser.chromium.dialog = None;
                self.sync_browser_visibility(cx);
            }
            Event::Dialog(dialog) => {
                let field = dialog
                    .as_ref()
                    .filter(|dialog| dialog.kind == DialogKind::Prompt)
                    .map(|dialog| {
                        let field = TextInput::new("", cx);
                        field.update(cx, |input, _| input.set_text(&dialog.default_text));
                        cx.observe(&field, |_, _, cx| cx.notify()).detach();
                        field
                    });
                browser.chromium.dialog = dialog.map(|dialog| OpenDialog { dialog, field });
                self.sync_browser_visibility(cx);
            }
            Event::Download {
                id: download,
                name,
                path,
                received,
                total,
                done,
                failed,
            } => {
                let download = Download {
                    id: download,
                    name,
                    path,
                    received,
                    total,
                    done,
                    failed,
                };
                return self.chromium_download(id, download, cx);
            }
            Event::Popup(popup) => return self.adopt_chromium_popup(id, popup, cx),
            Event::Closed if browser.chromium.popup => return self.close_chromium_tab(id, cx),
            Event::Closed => {
                let why = browser.chromium.crashed.take();
                browser.chromium.dialog = None;
                browser.view = None;
                browser.shown = false;
                browser.error.get_or_insert_with(|| {
                    let why = why.map(|why| format!(": {why}")).unwrap_or_default();
                    format!(
                        "This page has closed{why}. Close the tab and open another to keep \
                         browsing."
                    )
                    .into()
                });
            }
        }
        cx.notify();
    }

    /// Browser `id`'s page has said how a download is going: its banner
    /// shows it, and a toast says when it ends, once.
    fn chromium_download(&mut self, id: BrowserId, download: Download, cx: &mut Context<Self>) {
        let Some(browser) = self.browsers.get_mut(&id) else {
            return;
        };
        let ended = |seen: &Download| seen.id == download.id && (seen.done || seen.failed);
        let news = !browser.chromium.download.as_ref().is_some_and(ended);
        let name = download.name.clone();
        let (done, failed) = (download.done, download.failed);
        browser.chromium.download = Some(download);
        if news && done {
            self.toast_detail(
                Tone::Success,
                format!("Downloaded {name}"),
                "In Downloads",
                cx,
            );
        } else if news && failed {
            self.toast(Tone::Error, format!("Couldn't download {name}"), cx);
        }
        cx.notify();
    }

    /// Browser `id`'s JavaScript dialog, in place of its page — what Chrome
    /// shows in a tab, worded as Chrome words it: what the page says, a
    /// prompt's field, and its buttons. The page stays hidden behind it, since
    /// it is stopped until the dialog is answered.
    pub(crate) fn chromium_dialog(
        &self,
        id: BrowserId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.browsers.get(&id)?.chromium.dialog.as_ref()?;
        let theme = &self.theme;
        let (title, message, cancel, ok) = dialog_words(&open.dialog);
        let field = open.field.as_ref().map(|field| {
            text_field(
                field,
                ("browser-dialog-field", id.0),
                false,
                Style::new(theme, self.caret.visible),
                window,
                cx,
            )
        });
        let answer = |accept| {
            cx.listener(move |this: &mut Shell, _, _, cx| {
                this.answer_chromium_dialog(id, accept, cx);
                cx.notify();
            })
        };
        let footer = dialog::footer()
            .children(cancel.map(|cancel| {
                button(("browser-dialog-cancel", id.0), cancel)
                    .render(theme)
                    .on_click(answer(false))
            }))
            .child(
                button(("browser-dialog-ok", id.0), ok)
                    .primary()
                    .render(theme)
                    .on_click(answer(true)),
            );
        // Never taller than the pane, whose edge would hide the buttons: a
        // long message scrolls, and the title, field and buttons stay put.
        let card = dialog::card(("browser-dialog", id.0), px(420.0), theme)
            .max_w_full()
            .max_h(relative(1.0))
            .child(dialog::header(title, theme).flex_none())
            .children(message.map(|message| {
                div()
                    .id(("browser-dialog-message", id.0))
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(dialog::body(message, theme))
            }))
            .children(field.map(|field| field.flex_none()))
            .child(footer.flex_none());
        Some(
            dialog::centered(card)
                .p(DIALOG_MARGIN)
                .bg(paint(theme.surface))
                .into_any_element(),
        )
    }

    /// Answers browser `id`'s JavaScript dialog as the person did in its
    /// tab: OK, with what they typed in a prompt, or Cancel. The page's
    /// [`Event::Dialog`](ket_chromium::Event::Dialog) saying it closed takes
    /// it off the pane.
    /// Whether a page's prompt field has the keyboard: the shell's focus
    /// repair would take it back otherwise, and the answer could not be
    /// typed.
    pub(crate) fn chromium_dialog_typing(&self, window: &Window, cx: &gpui::App) -> bool {
        self.browsers.values().any(|browser| {
            browser
                .chromium
                .dialog
                .as_ref()
                .and_then(|open| open.field.as_ref())
                .is_some_and(|field| field.read(cx).is_focused(window))
        })
    }

    /// Keys in a page's prompt field: typing and editing go to the field,
    /// Return answers OK, Escape answers Cancel.
    pub(crate) fn chromium_dialog_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((id, field)) = self.browsers.iter().find_map(|(id, browser)| {
            let field = browser.chromium.dialog.as_ref()?.field.clone()?;
            field.read(cx).is_focused(window).then_some((*id, field))
        }) else {
            return false;
        };
        if crate::input::is_text(&event.keystroke) {
            return true;
        }
        if field.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.answer_chromium_dialog(id, true, cx),
            "escape" => self.answer_chromium_dialog(id, false, cx),
            _ => return false,
        }
        window.focus(&self.focus);
        true
    }

    fn answer_chromium_dialog(&mut self, id: BrowserId, accept: bool, cx: &mut Context<Self>) {
        let Some(browser) = self.browsers.get(&id) else {
            return;
        };
        let text = browser
            .chromium
            .dialog
            .as_ref()
            .and_then(|open| open.field.as_ref())
            .map(|field| field.read(cx).text());
        if let Some(view) = &browser.view {
            view.page().answer_dialog(accept, text.as_deref());
        }
    }

    /// The banner under browser `id`'s toolbar about its latest download,
    /// with Show in Finder once it is done, and a way to put it away.
    pub(crate) fn chromium_download_banner(
        &self,
        id: BrowserId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let download = self.browsers.get(&id)?.chromium.download.as_ref()?;
        let theme = &self.theme;
        let reveal = download.done.then(|| {
            let path = download.path.clone();
            button(("browser-download-reveal", id.0), "Show in Finder")
                .small()
                .leading(Icon::FolderOpen)
                .render(theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Err(error) = ket_core::surface::reveal_in_file_manager(&path) {
                        this.toast(Tone::Error, error.to_string(), cx);
                    }
                    cx.notify();
                }))
        });
        let dismiss = icon_button(("browser-download-dismiss", id.0), Icon::Close)
            .bare()
            .small()
            .render(theme)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(browser) = this.browsers.get_mut(&id) {
                    browser.chromium.download = None;
                }
                cx.notify();
            }));
        Some(
            div()
                .flex_none()
                .px(px(8.0))
                .pt(px(8.0))
                .child(
                    banner(
                        ("browser-download", id.0),
                        download.tone(),
                        download.line(),
                        theme,
                    )
                    .items_center()
                    .children(reveal)
                    .child(dismiss),
                )
                .into_any_element(),
        )
    }

    /// The toolbar's camera: a screenshot of the page to an agent.
    pub(crate) fn chromium_capture_button(
        &self,
        id: BrowserId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let hovered = self
            .browsers
            .get(&id)
            .is_some_and(|browser| browser.chromium.capture_hovered);
        // To the left, inside the toolbar: under it would be over the page,
        // which draws over everything of ket's.
        tooltip(
            ("browser-capture-tip", id.0),
            icon_button(("browser-capture", id.0), Icon::Camera)
                .bare()
                .small()
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.capture_device(DeviceKind::Browser(id), cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .into_any_element(),
            "Screenshot to agent",
            Side::Left,
            hovered,
            t,
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if let Some(browser) = this.browsers.get_mut(&id) {
                browser.chromium.capture_hovered = *hovered;
            }
            cx.notify();
        }))
        .into_any_element()
    }

    /// Takes a screenshot of browser `id`'s page, as Chromium draws it, and
    /// opens the Quick prompt with it, sending to the tab's worktree — or adds
    /// it to the prompt already open. What ⌘S does in a device tab.
    pub(crate) fn capture_chromium(&mut self, id: BrowserId, cx: &mut Context<Self>) {
        use gpui::FutureExt as _;
        use ket_chromium::protocol::browser_protocol::page::{
            CaptureScreenshotFormat, CaptureScreenshotParams,
        };
        let Some((cdp, worktree)) = self.browsers.get(&id).and_then(|browser| {
            let cdp = browser.view.as_ref()?.page().cdp();
            Some((cdp, browser.chromium.worktree.clone()))
        }) else {
            self.toast(Tone::Info, "Open a page first, then take a screenshot", cx);
            return;
        };
        self.capturing_device = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |shell: WeakEntity<Shell>, cx| {
            let shot = cdp
                .call(CaptureScreenshotParams {
                    format: Some(CaptureScreenshotFormat::Png),
                    ..Default::default()
                })
                .with_timeout(SHOT_WAIT, &executor)
                .await;
            let saved = match shot {
                Ok(Ok(shot)) => {
                    // Decoded and written off the window's thread.
                    executor
                        .spawn(async move {
                            use base64::Engine as _;
                            let png = base64::engine::general_purpose::STANDARD
                                .decode(String::from(shot.data))
                                .map_err(|_| "Chromium's picture was not an image".to_owned())?;
                            ket_core::capture::save_screenshot(&png).map_err(|e| e.to_string())
                        })
                        .await
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("the page didn't draw one in time".to_owned()),
            };
            let _ = shell.update(cx, |shell, cx| {
                shell.capturing_device = false;
                match saved {
                    Ok(path) => {
                        shell.open_screenshot_prompt(path, DeviceKind::Browser(id), worktree, cx);
                    }
                    Err(why) => shell.toast(
                        Tone::Error,
                        format!("Couldn't take a screenshot: {why}"),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Copies, pastes and the rest in the page in front, when the page
    /// itself has the keyboard — for the Edit menu, whose items come to ket
    /// whenever Chromium's view does not take them itself. Says whether it
    /// did.
    pub(crate) fn chromium_edit(&self, edit: ket_chromium::Edit) -> bool {
        let Some(view) = self
            .active_browser_ids()
            .into_iter()
            .filter_map(|id| self.browsers.get(&id)?.view.clone())
            .find(|view| view.page_has_keyboard())
        else {
            return false;
        };
        view.page().edit(edit);
        true
    }

    /// Opens browser `id`'s DevTools in a window of their own, or closes it.
    pub(crate) fn toggle_chromium_devtools(&self, id: BrowserId) {
        if let Some(view) = self.browsers.get(&id).and_then(|b| b.view.as_ref()) {
            view.toggle_devtools();
        }
    }

    /// Whether browser `id`'s DevTools window is open.
    pub(crate) fn chromium_devtools_open(&self, id: BrowserId) -> bool {
        self.browsers
            .get(&id)
            .and_then(|b| b.view.as_ref())
            .is_some_and(|view| view.devtools_open())
    }
}

/// Hears what browser `id`'s page says, in order, until it closes: `Closed`
/// is the last thing it says.
fn watch(
    id: BrowserId,
    mut events: tokio::sync::mpsc::UnboundedReceiver<ket_chromium::Event>,
    cx: &mut Context<Shell>,
) -> gpui::Task<()> {
    cx.spawn(async move |this: WeakEntity<Shell>, cx| {
        while let Some(event) = events.recv().await {
            if this
                .update(cx, |shell, cx| shell.chromium_event(id, event, cx))
                .is_err()
            {
                return;
            }
        }
    })
}

/// A JavaScript dialog's title, what it says, its Cancel — none for an
/// alert — and its OK, as Chrome words them.
fn dialog_words(dialog: &Dialog) -> (String, Option<String>, Option<&'static str>, &'static str) {
    let says = || match host(&dialog.url) {
        "" => "This page says".to_owned(),
        host => format!("{host} says"),
    };
    let message = (!dialog.message.is_empty()).then(|| shown(&dialog.message));
    match dialog.kind {
        DialogKind::Alert => (says(), message, None, "OK"),
        DialogKind::Confirm | DialogKind::Prompt => (says(), message, Some("Cancel"), "OK"),
        DialogKind::Leave { reload } => (
            if reload {
                "Reload site?"
            } else {
                "Leave site?"
            }
            .to_owned(),
            Some("Changes you made may not be saved.".to_owned()),
            Some("Cancel"),
            if reload { "Reload" } else { "Leave" },
        ),
    }
}

/// What a dialog's card shows of `message`: all of it, or its first
/// [`DIALOG_MESSAGE_MAX`] characters and an ellipsis.
fn shown(message: &str) -> String {
    match message.char_indices().nth(DIALOG_MESSAGE_MAX) {
        Some((cut, _)) => format!("{}\u{2026}", &message[..cut]),
        None => message.to_owned(),
    }
}

/// What a browser tab is called: its page's title, or, for a page with none,
/// what Chrome calls an untitled page: its address without the scheme, query
/// or fragment (a file's, its name), cut to [`ADDRESS_TITLE_MAX`] bytes;
/// "Browser" for a blank page. A page has none when its title is empty,
/// `about:blank`, or only its address, which is what Chromium reports for an
/// untitled page once it has loaded.
fn tab_title(title: &str, url: &str) -> String {
    // An address without `http://` or `https://` and a trailing slash, as
    // Chromium's stand-in title may leave them off. Chromium reports its
    // addresses canonical, the scheme in lower case.
    let bare = |address: &str| -> String {
        let address = address.trim();
        let rest = ["http://", "https://"]
            .iter()
            .find_map(|scheme| address.strip_prefix(scheme))
            .unwrap_or(address);
        rest.trim_end_matches('/').to_owned()
    };
    if !title.trim().is_empty() && title != "about:blank" && bare(title) != bare(url) {
        return title.to_owned();
    }
    let url = url.trim();
    // What follows the scheme, up to a query or fragment.
    let path = |rest: &str| -> String {
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        rest[..end].trim_end_matches('/').to_owned()
    };
    let named = match url.split_once("://") {
        _ if url.is_empty() || url == "about:blank" => String::new(),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("file") => {
            let file = path(rest);
            file.rsplit('/').next().unwrap_or_default().to_owned()
        }
        // A web page: its host, without credentials, then its path.
        Some((_, rest)) if !host(url).is_empty() => {
            let at = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
            format!("{}{}", host(url), path(&rest[at..]))
        }
        _ => url.to_owned(),
    };
    if named.is_empty() {
        "Browser".to_owned()
    } else {
        capped(named, ADDRESS_TITLE_MAX)
    }
}

/// The host and port of `url`, as Chrome names a page that opened a dialog:
/// `localhost:3000`. Empty for a page with none, such as a file.
///
/// Only a web page's own address names a site. A `data:`, `blob:` or
/// `about:` document has an opaque origin, and its text can hold anything
/// that looks like an address — `data:text/html,https://bank.example/…` —
/// which must never be shown as where a dialog came from.
pub(crate) fn host(url: &str) -> &str {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "";
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return "";
    }
    // A backslash ends the host as a slash does, as Chrome reads one.
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let host = &rest[..end];
    host.rsplit_once('@').map_or(host, |(_, host)| host)
}

/// `why`, from Chromium, as a sentence: capitalised, with a full stop.
pub(crate) fn sentence(why: &str) -> String {
    let mut chars = why.trim().chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        assert_eq!(capped("short".into(), 8), "short");
        let cut = capped("é".repeat(600), CONSOLE_TEXT_MAX);
        assert!(cut.len() <= CONSOLE_TEXT_MAX);
        assert!(cut.ends_with('…'));
        assert!(cut.starts_with("éé"));
    }

    #[test]
    fn the_console_keeps_the_latest_messages() {
        let mut page = PageState::default();
        for line in 0..(CONSOLE_KEPT as u32 + 5) {
            page.log(ConsoleEntry {
                at_ms: 0,
                level: "info".into(),
                text: String::new(),
                source: String::new(),
                line,
            });
        }
        assert_eq!(page.console.len(), CONSOLE_KEPT);
        assert_eq!(page.console.front().map(|entry| entry.line), Some(5));
    }

    #[test]
    fn a_download_says_how_it_is_going() {
        let mut download = Download {
            id: 1,
            name: "report.pdf".into(),
            path: PathBuf::from("/Users/me/Downloads/report.pdf"),
            received: 450,
            total: 1_000,
            done: false,
            failed: false,
        };
        assert_eq!(download.line(), "Downloading report.pdf\u{2026} 45%");
        download.total = 0;
        assert_eq!(download.line(), "Downloading report.pdf\u{2026} 450 bytes");
        download.done = true;
        assert_eq!(download.line(), "Downloaded report.pdf");
        assert_eq!(download.tone(), Tone::Success);
        download.done = false;
        download.failed = true;
        assert_eq!(download.line(), "Couldn't download report.pdf");
    }

    #[test]
    fn sizes_read_as_finder_says_them() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(1_500), "1.5 KB");
        assert_eq!(size(3_200_000), "3.2 MB");
        assert_eq!(size(-1), "0 bytes");
    }

    #[test]
    fn chromium_reasons_read_as_sentences() {
        assert_eq!(
            sentence("the page's renderer crashed (code 11)"),
            "The page's renderer crashed (code 11)."
        );
        assert_eq!(sentence("done."), "Done.");
        assert_eq!(sentence("  "), "");
    }

    #[test]
    fn a_dialog_is_worded_as_chrome_words_it() {
        let dialog = |kind, message: &str, url: &str| Dialog {
            kind,
            message: message.to_owned(),
            default_text: String::new(),
            url: url.to_owned(),
        };
        assert_eq!(
            dialog_words(&dialog(
                DialogKind::Confirm,
                "Delete 3 files?",
                "http://localhost:3000/files?x=1"
            )),
            (
                "localhost:3000 says".to_owned(),
                Some("Delete 3 files?".to_owned()),
                Some("Cancel"),
                "OK"
            )
        );
        assert_eq!(
            dialog_words(&dialog(DialogKind::Alert, "", "file:///tmp/a.html")),
            ("This page says".to_owned(), None, None, "OK")
        );
        let leave = dialog_words(&dialog(DialogKind::Leave { reload: true }, "", ""));
        assert_eq!((leave.0.as_str(), leave.3), ("Reload site?", "Reload"));
        assert_eq!(host("https://me:pw@example.com#top"), "example.com");
        assert_eq!(host("about:blank"), "");
        // An opaque origin names no site, whatever its text looks like.
        assert_eq!(host("data:text/html,https://bank.example/<script>"), "");
        assert_eq!(host("blob:https://bank.example/0b1c"), "");
        assert_eq!(host("javascript://bank.example/%0Aalert(1)"), "");
        assert_eq!(host("https://evil.example\\@bank.example/"), "evil.example");
    }

    #[test]
    fn a_long_dialog_message_is_cut_short() {
        let alert = |message: String| Dialog {
            kind: DialogKind::Alert,
            message,
            default_text: String::new(),
            url: String::new(),
        };
        let long = dialog_words(&alert("line\n".repeat(10_000)))
            .1
            .unwrap_or_default();
        assert_eq!(long.chars().count(), DIALOG_MESSAGE_MAX + 1);
        assert!(long.starts_with("line\nline\n") && long.ends_with('\u{2026}'));
        // Cut by characters, never inside one.
        let wide = shown(&"é".repeat(DIALOG_MESSAGE_MAX + 1));
        assert_eq!(wide, format!("{}\u{2026}", "é".repeat(DIALOG_MESSAGE_MAX)));
        let exact = "x".repeat(DIALOG_MESSAGE_MAX);
        assert_eq!(shown(&exact), exact);
    }

    #[test]
    fn an_untitled_page_is_named_for_its_address() {
        assert_eq!(tab_title("Docs", "https://example.com/docs"), "Docs");
        assert_eq!(
            tab_title("", "https://me:pw@example.com/docs/a/?q=1#top"),
            "example.com/docs/a"
        );
        assert_eq!(tab_title(" ", "http://localhost:3000/"), "localhost:3000");
        assert_eq!(
            tab_title("about:blank", "https://example.com"),
            "example.com"
        );
        assert_eq!(
            tab_title("", "file:///Users/me/site/index.html"),
            "index.html"
        );
        assert_eq!(
            tab_title("", "data:text/html,<p>hi"),
            "data:text/html,<p>hi"
        );
        assert_eq!(tab_title("", "about:blank"), "Browser");
        assert_eq!(tab_title("", ""), "Browser");
        // What Chromium reports once an untitled page has loaded: its address.
        assert_eq!(
            tab_title(
                "https://example.com/docs?q=1",
                "https://example.com/docs?q=1"
            ),
            "example.com/docs"
        );
        assert_eq!(
            tab_title("localhost:3000", "http://localhost:3000/"),
            "localhost:3000"
        );
        assert_eq!(
            tab_title("example.com", "https://docs.example/"),
            "example.com"
        );
        let long = tab_title("", &format!("https://example.com/{}", "a".repeat(200)));
        assert!(long.len() <= ADDRESS_TITLE_MAX && long.starts_with("example.com/a"));
        assert!(long.ends_with('\u{2026}'));
    }
}
