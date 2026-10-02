//! Ephemeral in-app browser tabs.
//!
//! macOS supplies the engine: Wry creates a `WKWebView`, while `gpui-wry`
//! keeps the native child view aligned with the GPUI pane that owns it. Native
//! child views always sit above GPUI's compositor, so this module explicitly
//! hides them when a different tab or an overlay is visible.

use gpui::{
    AnyElement, Context, Entity, KeyDownEvent, Pixels, Point, SharedString, Window, div,
    prelude::*, px,
};

use crate::Shell;
use crate::input::{Style, TextInput, is_text, text_field};
use crate::paint::paint;
use crate::tabs::{PaneId, Tab, TabKind};
use crate::ui::button::{button, icon_button};
use crate::ui::icon::Icon;
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu};

/// Process-local identity for one browser tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BrowserId(pub(crate) u64);

/// Browser UI and, on macOS, its lazily-created native child view.
pub(crate) struct BrowserHandle {
    address: Entity<TextInput>,
    #[cfg(target_os = "macos")]
    view: Option<Entity<gpui_wry::WebView>>,
    #[cfg(target_os = "macos")]
    shown: bool,
    /// The page the native view loads when it is created, for a tab opened
    /// on a link rather than blank.
    #[cfg(target_os = "macos")]
    initial: Option<String>,
    /// The page's address as it last reported loading one — a link clicked,
    /// a redirect, back or forward — and whether the address field has
    /// shown it yet. It waits while the field is being typed in.
    #[cfg(target_os = "macos")]
    page: Option<(String, bool)>,
    /// Hears the page's loads; dropped with the tab.
    #[cfg(target_os = "macos")]
    loads: Option<gpui::Task<()>>,
    /// While ket's own menus or popovers are over the pane: the still of the
    /// page the pane shows in place of the live one, which is hidden — see
    /// [`Shell::sync_browser_visibility`].
    #[cfg(target_os = "macos")]
    still: Still,
    /// No address yet: the tab was opened blank and nothing has loaded. The
    /// page stays hidden so the pane shows in the theme's colour, rather than
    /// WebKit's white `about:blank` — Wry cannot colour a page's background
    /// on macOS.
    #[cfg(target_os = "macos")]
    blank: bool,
    error: Option<SharedString>,
}

/// Where a browser pane is with its still, while something is over it.
#[cfg(target_os = "macos")]
#[derive(Default)]
enum Still {
    /// Nothing is over the pane: the live page shows.
    #[default]
    None,
    /// Asked for. The live page stays up until it comes, a frame or two, so
    /// the pane never goes blank in between.
    Taking,
    /// Drawn under the live page for this many more frames before the live
    /// page hides. Hiding a native view takes effect at once and presenting
    /// a gpui frame does not, so hiding it the frame the still first drew
    /// left a frame with neither: a blink of empty pane.
    Settling(std::sync::Arc<gpui::RenderImage>, u8),
    /// Shown, with the live page hidden behind it.
    Shown(std::sync::Arc<gpui::RenderImage>),
    /// WebKit gave none: the pane is blank behind the overlay, as it always
    /// was before stills.
    Failed,
}

impl BrowserHandle {
    fn new(url: Option<String>, cx: &mut Context<Shell>) -> Self {
        let address = TextInput::new("Enter URL", cx);
        address.update(cx, |input, _| match &url {
            Some(url) => input.set_text(url),
            // Blank: there is nothing to look at until an address is typed.
            None => input.request_focus(),
        });
        cx.observe(&address, |_, _, cx| cx.notify()).detach();
        #[cfg(not(target_os = "macos"))]
        let _ = url;
        #[cfg(target_os = "macos")]
        let blank = url.is_none();
        Self {
            address,
            #[cfg(target_os = "macos")]
            view: None,
            #[cfg(target_os = "macos")]
            shown: false,
            #[cfg(target_os = "macos")]
            initial: url,
            #[cfg(target_os = "macos")]
            page: None,
            #[cfg(target_os = "macos")]
            loads: None,
            #[cfg(target_os = "macos")]
            still: Still::None,
            #[cfg(target_os = "macos")]
            blank,
            error: None,
        }
    }
}

/// Where a link clicked in a terminal opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkAction {
    /// A new browser tab beside the terminal.
    Ket,
    /// Whatever the system opens URLs with.
    System,
}

/// The menu a clicked terminal link opens, and what it was opened for.
pub(crate) struct LinkMenu {
    menu: OpenMenu<LinkAction>,
    uri: String,
    /// The terminal's pane, which a ket browser tab opens in.
    pane: PaneId,
}

/// The link menu's width.
const LINK_MENU_W: Pixels = px(240.0);

/// How much of a link the menu's caption spells out before eliding the rest.
const LINK_CAPTION_CHARS: usize = 36;

/// Where a hidden page waits, in logical points from the window's corner:
/// far enough up and left that no window is big enough to reach it — see
/// `Shell::sync_browser_visibility`.
#[cfg(target_os = "macos")]
const PARKED: f64 = -100_000.0;

impl Shell {
    /// Opens a fresh, private browser tab in the pane whose `+` menu was used.
    pub(crate) fn open_browser_tab_in(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        self.open_browser_tab_with(pane, None, cx);
    }

    /// Opens a browser tab in `pane`, blank or already on `url`.
    fn open_browser_tab_with(&mut self, pane: PaneId, url: Option<String>, cx: &mut Context<Self>) {
        let Some(worktree) = self.selected_id() else {
            return;
        };
        let id = BrowserId(self.next_browser);
        self.next_browser += 1;
        self.browsers.insert(id, BrowserHandle::new(url, cx));

        let opened = self.spaces.get_mut(&worktree).is_some_and(|space| {
            space.open_tab_in(
                pane,
                Tab {
                    title: "Browser".into(),
                    renamed: false,
                    pinned: false,
                    kind: TabKind::Browser(id),
                },
            )
        });
        if !opened {
            self.browsers.remove(&id);
        }
    }

    /// Asks where a link clicked in a terminal should open, from a menu at
    /// the pointer.
    pub(crate) fn open_link_menu(&mut self, uri: String, pane: PaneId, at: Point<Pixels>) {
        let caption: String = if uri.chars().count() > LINK_CAPTION_CHARS {
            let head: String = uri.chars().take(LINK_CAPTION_CHARS - 1).collect();
            format!("{head}…")
        } else {
            uri.clone()
        };
        let entries = vec![
            MenuEntry::Item(
                MenuItem::new(LinkAction::Ket, "Open in ket Browser").icon(Icon::BrowserWindow),
            ),
            MenuEntry::Item(
                MenuItem::new(LinkAction::System, "Open in Default Browser")
                    .icon(Icon::ExternalLink),
            ),
        ];
        self.menu = None;
        self.link_menu = Some(LinkMenu {
            menu: OpenMenu::new("terminal-link".into(), None, LINK_MENU_W, entries)
                .with_header(caption)
                .at(at),
            uri,
            pane,
        });
    }

    fn run_link_action(&mut self, action: LinkAction, cx: &mut Context<Self>) {
        let Some(open) = self.link_menu.take() else {
            return;
        };
        match action {
            LinkAction::Ket => {
                self.open_browser_tab_with(open.pane, Some(open.uri), cx);
                self.persist_layout();
            }
            LinkAction::System => cx.open_url(&open.uri),
        }
        cx.notify();
    }

    /// Routes keyboard navigation to the link menu while it is open.
    pub(crate) fn link_menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.link_menu.as_mut() else {
            return false;
        };
        match open.menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => {}
            MenuKey::Close => self.link_menu = None,
            MenuKey::Run(action) => self.run_link_action(action, cx),
        }
        cx.notify();
        true
    }

    /// Renders the link menu.
    pub(crate) fn link_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.link_menu.as_ref()?;
        Some(open.menu.view(
            &self.theme,
            cx,
            |shell, action, cx| shell.run_link_action(*action, cx),
            |shell, cx| {
                shell.link_menu = None;
                cx.notify();
            },
        ))
    }

    /// Removes native views after their owning tabs or panes have closed.
    pub(crate) fn prune_browsers(&mut self) {
        let live: Vec<_> = self
            .spaces
            .values()
            .flat_map(|space| space.browser_ids())
            .collect();
        self.browsers.retain(|id, _| live.contains(id));
    }

    /// Whether an overlay must cover the pane area occupied by native views.
    pub(crate) fn browser_occluded(&self) -> bool {
        // A tab in the air counts. A WKWebView sits above everything GPUI
        // draws, so while one is showing this pane can neither report the
        // pointer crossing it nor be covered by a drop wash — a browser would
        // be the one pane in the window a tab could not be dropped into.
        self.tab_dragging
            || self.modal_open()
            || self.menu.is_some()
            || self.tab_context_menu.is_some()
            || self.link_menu.is_some()
            || self.tab_open_in.is_some()
            || self.tab_snippets.is_some()
            || self.worktree_menu.is_some()
            || self.token_reduction_menu.is_some()
            || self.worktree_snippet_menu.is_some()
            || self.project_menu.is_some()
            || self.popup.is_some()
            || self.palette.open
            || self.finder.open
    }

    /// Browser tabs selected in all visible leaves of the current worktree.
    fn active_browser_ids(&self) -> Vec<BrowserId> {
        self.space()
            .map_or_else(Vec::new, |space| space.active_browser_ids())
    }

    /// Prevents the shell's render-time focus repair from stealing the
    /// keyboard back from a visible WKWebView.
    pub(crate) fn browser_owns_focus(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            !self.browser_occluded()
                && self.active_browser_ids().into_iter().any(|id| {
                    self.browsers
                        .get(&id)
                        .is_some_and(|browser| browser.shown && browser.view.is_some())
                })
        }

        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Keeps every native child view in step with tab and overlay state.
    ///
    /// A page under one of ket's menus, popups or dialogs is not just hidden
    /// — that left the pane blank behind the menu. It is swapped for a still
    /// of itself, taken as the overlay opens: the live page stays up until the
    /// still arrives, a frame or two, then hides behind it, and comes back
    /// when the overlay closes. The page does not move while it is a still,
    /// and does not take clicks, which go to whatever is over it.
    pub(crate) fn sync_browser_visibility(&mut self, cx: &mut Context<Self>) {
        // A drag that ended where no listener could hear it — released outside
        // the window, or dropped when the application was deactivated — would
        // otherwise leave `tab_dragging` set and every browser pane blank for
        // the rest of the session. gpui's own view of it is authoritative, and
        // this runs at the top of every frame. See `Shell::render`.
        if self.tab_dragging && !cx.has_active_drag() {
            self.tab_dragging = false;
        }

        #[cfg(target_os = "macos")]
        {
            let active = self.active_browser_ids();
            let occluded = self.browser_occluded();
            let mut stills = Vec::new();
            let mut settling = false;
            for (id, browser) in &mut self.browsers {
                let visible = active.contains(id);
                if !visible || !occluded {
                    browser.still = Still::None;
                } else if matches!(browser.still, Still::None)
                    && browser.view.is_some()
                    && !browser.blank
                {
                    browser.still = Still::Taking;
                    stills.push(*id);
                } else if let Still::Settling(image, frames) = &browser.still {
                    browser.still = match frames {
                        0 => Still::Shown(image.clone()),
                        n => Still::Settling(image.clone(), n - 1),
                    };
                    settling = true;
                }
                // Live while nothing is over it, while its still is on the
                // way, and while the still settles in underneath it — and
                // never while it has nothing to show.
                let should_show = visible
                    && !browser.blank
                    && (!occluded || matches!(browser.still, Still::Taking | Still::Settling(..)));
                if browser.shown == should_show {
                    continue;
                }
                if let Some(view) = browser.view.as_ref() {
                    view.update(cx, |view, _| {
                        if should_show {
                            // Its frame comes back with this frame's layout:
                            // gpui-wry sets it for a visible view every frame.
                            view.show();
                        } else {
                            view.hide();
                            // A hidden view still takes drops — AppKit finds
                            // drag destinations by frame, hidden or not — so
                            // a browser tab behind a terminal swallowed the
                            // files dropped on it. Moved off the window it
                            // has nothing to land on; at its own size, so
                            // coming back is a move rather than a resize —
                            // shrunk to nothing, WebKit redrew it from white
                            // as a menu over it closed.
                            let raw = view.raw();
                            let parked = raw.bounds().map(|bounds| wry::Rect {
                                position: wry::dpi::LogicalPosition::new(PARKED, PARKED).into(),
                                size: bounds.size,
                            });
                            let _ = raw.set_bounds(parked.unwrap_or_default());
                        }
                    });
                }
                browser.shown = should_show;
            }
            // A still settling in needs the next frames to come, though
            // nothing else may change in them.
            if settling {
                cx.notify();
            }
            for id in stills {
                self.take_browser_still(id, cx);
            }
        }

        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    /// Asks WebKit for a still of the page in browser `id`, and puts it in
    /// the pane when it comes — unless the overlay that wanted it has gone.
    #[cfg(target_os = "macos")]
    fn take_browser_still(&mut self, id: BrowserId, cx: &mut Context<Self>) {
        let Some(view) = self
            .browsers
            .get(&id)
            .and_then(|browser| browser.view.clone())
        else {
            return;
        };
        let (tell, still) = tokio::sync::oneshot::channel();
        crate::browser_snapshot::take(view.read(cx).raw(), move |tiff| {
            let _ = tell.send(tiff);
        });
        let svg = cx.svg_renderer();
        let decoder = cx.background_executor().clone();
        cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
            let tiff = still.await.ok().flatten();
            // Decoded by gpui, off the main thread, in its own optimised code
            // — see `browser_snapshot` for why not by hand.
            let image = match tiff {
                Some(tiff) => {
                    decoder
                        .spawn(async move {
                            gpui::Image::from_bytes(gpui::ImageFormat::Tiff, tiff)
                                .to_image_data(svg)
                                .ok()
                        })
                        .await
                }
                None => None,
            };
            let _ = this.update(cx, |shell, cx| {
                let Some(browser) = shell.browsers.get_mut(&id) else {
                    return;
                };
                // Closed before it came: the live page is already back.
                if !matches!(browser.still, Still::Taking) {
                    return;
                }
                browser.still = match image {
                    Some(image) => Still::Settling(image, 2),
                    None => Still::Failed,
                };
                shell.sync_browser_visibility(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether any browser address field currently owns GPUI keyboard focus.
    pub(crate) fn browser_typing(&self, window: &Window, cx: &gpui::App) -> bool {
        self.browsers
            .values()
            .any(|browser| browser.address.read(cx).is_focused(window))
    }

    /// Routes editing commands and submit/cancel for the active URL field.
    pub(crate) fn browser_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = self.active_browser_ids().into_iter().find(|id| {
            self.browsers
                .get(id)
                .is_some_and(|browser| browser.address.read(cx).is_focused(window))
        }) else {
            return false;
        };
        let address = self.browsers[&id].address.clone();

        // Printable text must continue to GPUI's input context, while still
        // preventing the shell's shortcuts from seeing it.
        if is_text(&event.keystroke) {
            return true;
        }
        if address.update(cx, |input, cx| input.key(&event.keystroke, cx)) {
            return true;
        }

        match event.keystroke.key.as_str() {
            "enter" => self.navigate_browser(id, cx),
            "escape" => {
                window.blur();
                self.focus_browser_page(id, cx);
            }
            // Let application shortcuts such as Cmd-W continue to the action
            // system after the field has declined them.
            _ => return false,
        }
        true
    }

    fn navigate_browser(&mut self, id: BrowserId, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(browser) = self.browsers.get_mut(&id) {
            browser.blank = false;
        }
        let Some(browser) = self.browsers.get(&id) else {
            return;
        };
        let url = normalize_url(&browser.address.read(cx).text());
        browser.address.update(cx, |input, _| input.set_text(&url));

        #[cfg(target_os = "macos")]
        if let Some(view) = browser.view.as_ref() {
            if let Err(error) = view.update(cx, |view, _| view.raw().load_url(&url)) {
                self.note = Some(format!("could not open {url}: {error}").into());
                return;
            }
            self.focus_browser_page(id, cx);
        }
    }

    /// Back or forward in the page's own history. Through the page, since
    /// Wry offers no such call; at either end of the history it does nothing.
    fn browser_history(&mut self, id: BrowserId, back: bool, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self
            .browsers
            .get(&id)
            .and_then(|browser| browser.view.as_ref())
        {
            let script = if back {
                "history.back()"
            } else {
                "history.forward()"
            };
            let _ = view.update(cx, |view, _| view.raw().evaluate_script(script));
        }

        #[cfg(not(target_os = "macos"))]
        let _ = (id, back, cx);
    }

    /// The page is at a new address: `Some` as a load reported it, `None`
    /// when the page said its address changed without loading — then WebKit
    /// is asked, rather than taking an address from the page. Kept for the
    /// address field, which takes it once nobody is typing in it — see
    /// [`Shell::browser_pane`].
    #[cfg(target_os = "macos")]
    fn browser_loaded(&mut self, id: BrowserId, url: Option<String>, cx: &mut Context<Self>) {
        let Some(browser) = self.browsers.get_mut(&id) else {
            return;
        };
        if url.as_deref().is_some_and(|url| url != "about:blank") {
            browser.blank = false;
        }
        let url = match url {
            Some(url) => url,
            None => match browser
                .view
                .as_ref()
                .and_then(|view| view.read(cx).raw().url().ok())
            {
                Some(url) => url,
                None => return,
            },
        };
        // The same address again — a page that rewrites its history as it
        // scrolls — changes nothing.
        if url.is_empty() || browser.page.as_ref().is_some_and(|(page, _)| *page == url) {
            return;
        }
        browser.page = Some((url, false));
        cx.notify();
    }

    fn reload_browser(&mut self, id: BrowserId, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self
            .browsers
            .get(&id)
            .and_then(|browser| browser.view.as_ref())
            && let Err(error) = view.update(cx, |view, _| view.raw().reload())
        {
            self.note = Some(format!("could not reload the browser: {error}").into());
        }

        #[cfg(not(target_os = "macos"))]
        let _ = (id, cx);
    }

    fn focus_browser_page(&self, id: BrowserId, cx: &gpui::App) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self
            .browsers
            .get(&id)
            .and_then(|browser| browser.view.as_ref())
        {
            let _ = view.read(cx).raw().focus();
        }

        #[cfg(not(target_os = "macos"))]
        let _ = (id, cx);
    }

    /// The browser tab in front, if the focused pane's active tab is one.
    pub(crate) fn active_browser_tab(&self) -> Option<BrowserId> {
        match self.space()?.active_tab()?.kind {
            TabKind::Browser(id) => Some(id),
            _ => None,
        }
    }

    /// Asks the page in browser `id` what is selected on it, and hands the
    /// text and the page's address to `done` once it answers. Nothing comes
    /// back when nothing is selected, or the page never answers.
    pub(crate) fn quote_browser_selection(
        &self,
        id: BrowserId,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Shell, String, String, &mut Context<Shell>) + 'static,
    ) {
        #[cfg(target_os = "macos")]
        {
            let Some(view) = self
                .browsers
                .get(&id)
                .and_then(|browser| browser.view.clone())
            else {
                return;
            };
            let raw = view.read(cx).raw();
            let address = raw.url().unwrap_or_default();
            let (tell, told) = tokio::sync::oneshot::channel::<String>();
            // Wry's callback may be called more than once in principle; the
            // first answer is the one.
            let tell = std::sync::Mutex::new(Some(tell));
            let asked = raw.evaluate_script_with_callback(
                "window.getSelection ? String(window.getSelection()) : ''",
                move |answer| {
                    if let Some(tell) = tell.lock().ok().and_then(|mut tell| tell.take()) {
                        let _ = tell.send(answer);
                    }
                },
            );
            if asked.is_err() {
                return;
            }
            cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
                let Ok(answer) = told.await else {
                    return;
                };
                // The value comes back as JSON: a quoted string.
                let text = serde_json::from_str::<String>(&answer).unwrap_or(answer);
                if text.trim().is_empty() {
                    return;
                }
                let _ = this.update(cx, |shell, cx| {
                    done(shell, text, address, cx);
                    cx.notify();
                });
            })
            .detach();
        }

        #[cfg(not(target_os = "macos"))]
        let _ = (id, cx, done);
    }

    pub(crate) fn focus_browser_parent(&self, id: BrowserId, cx: &gpui::App) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self
            .browsers
            .get(&id)
            .and_then(|browser| browser.view.as_ref())
        {
            let _ = view.read(cx).raw().focus_parent();
        }

        #[cfg(not(target_os = "macos"))]
        let _ = (id, cx);
    }

    #[cfg(target_os = "macos")]
    fn ensure_browser_view(&mut self, id: BrowserId, window: &mut Window, cx: &mut Context<Self>) {
        use raw_window_handle::HasWindowHandle as _;

        let needs_view = self
            .browsers
            .get(&id)
            .is_some_and(|browser| browser.view.is_none() && browser.error.is_none());
        if !needs_view {
            return;
        }

        // `Some` address from a load, `None` from the page's own history
        // calls — see `ADDRESS_WATCH`.
        let (loaded, mut loads) = tokio::sync::mpsc::unbounded_channel::<Option<String>>();
        let changed = loaded.clone();
        let result = window
            .window_handle()
            .map_err(|error| error.to_string())
            .and_then(|handle| {
                let url = self
                    .browsers
                    .get(&id)
                    .and_then(|browser| browser.initial.clone())
                    .unwrap_or_else(|| "about:blank".to_owned());
                wry::WebViewBuilder::new()
                    .with_url(url)
                    // Every load, the address it lands on: what the address
                    // field shows after a link, a redirect, back or forward.
                    .with_on_page_load_handler(move |_, url| {
                        let _ = loaded.send(Some(url));
                    })
                    // And the moves a load never reports: a web app changing
                    // its page in place. What the page sends is ignored — it
                    // only says to look; the address comes from WebKit.
                    .with_initialization_script(ADDRESS_WATCH)
                    .with_ipc_handler(move |_| {
                        let _ = changed.send(None);
                    })
                    .with_visible(false)
                    .with_incognito(true)
                    .with_accept_first_mouse(true)
                    .with_back_forward_navigation_gestures(true)
                    .build_as_child(&handle)
                    .map_err(|error| error.to_string())
            });

        match result {
            Ok(raw) => {
                // Wry may make a newly-created child first responder. Put the
                // keyboard back before the address field asks for it below.
                let _ = raw.focus_parent();
                let view = cx.new(|cx| gpui_wry::WebView::new(raw, window, cx));
                let show = !self.browser_occluded()
                    && self
                        .active_browser_ids()
                        .into_iter()
                        .any(|active| active == id);
                view.update(cx, |view, _| {
                    if show {
                        view.show();
                    } else {
                        view.hide();
                    }
                });
                // Ends when the page goes, which drops the handler's sender.
                let watch = cx.spawn(async move |this: gpui::WeakEntity<Shell>, cx| {
                    while let Some(url) = loads.recv().await {
                        if this
                            .update(cx, |shell, cx| shell.browser_loaded(id, url, cx))
                            .is_err()
                        {
                            return;
                        }
                    }
                });
                if let Some(browser) = self.browsers.get_mut(&id) {
                    browser.view = Some(view);
                    browser.shown = show;
                    browser.loads = Some(watch);
                }
            }
            Err(error) => {
                if let Some(browser) = self.browsers.get_mut(&id) {
                    browser.error = Some(format!("could not create the browser: {error}").into());
                }
            }
        }
    }

    /// Renders browser chrome and the native page surface.
    pub(crate) fn browser_pane(
        &mut self,
        id: BrowserId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[cfg(target_os = "macos")]
        self.ensure_browser_view(id, window, cx);

        // The address field takes the page's latest address once nobody is
        // typing in it: a link clicked in the page changes what it shows.
        #[cfg(target_os = "macos")]
        if let Some(browser) = self.browsers.get_mut(&id)
            && let Some((url, shown)) = browser.page.as_mut()
            && !*shown
            && !browser.address.read(cx).is_focused(window)
        {
            *shown = true;
            let url = url.clone();
            browser.address.update(cx, |input, _| input.set_text(&url));
        }

        let Some(browser) = self.browsers.get(&id) else {
            return browser_message(&self.theme, "This browser tab is no longer available.");
        };
        let address = browser.address.clone();
        let error = browser.error.clone();
        #[cfg(target_os = "macos")]
        let view = browser.view.clone();
        #[cfg(target_os = "macos")]
        let still = match &browser.still {
            Still::Settling(image, _) | Still::Shown(image) => Some(image.clone()),
            _ => None,
        };
        let theme = self.theme;

        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .h(px(42.0))
            .px(px(8.0))
            .bg(paint(theme.panel))
            .border_b_1()
            .border_color(paint(theme.border))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.focus_browser_parent(id, cx)),
            )
            .child(
                icon_button(("browser-back", id.0), Icon::ArrowLeft)
                    .bare()
                    .small()
                    .render(&theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser_history(id, true, cx);
                        cx.notify();
                    })),
            )
            .child(
                icon_button(("browser-forward", id.0), Icon::ArrowRight)
                    .bare()
                    .small()
                    .render(&theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browser_history(id, false, cx);
                        cx.notify();
                    })),
            )
            .child(
                icon_button(("browser-reload", id.0), Icon::Refresh)
                    .bare()
                    .small()
                    .render(&theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reload_browser(id, cx);
                        cx.notify();
                    })),
            )
            .child(
                text_field(
                    &address,
                    ("browser-address", id.0),
                    false,
                    Style::new(&theme, self.caret.visible).leading(Icon::Search),
                    window,
                    cx,
                )
                .flex_1()
                .min_w_0()
                .h(px(30.0)),
            )
            .child(
                button(("browser-go", id.0), "Go")
                    .primary()
                    .render(&theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate_browser(id, cx);
                        cx.notify();
                    })),
            );

        #[cfg(target_os = "macos")]
        let body = match (view, error) {
            // The view stays in the tree while it is hidden behind its still,
            // so its frame keeps following the pane.
            (Some(view), _) => div()
                .relative()
                .flex()
                .flex_1()
                .overflow_hidden()
                .child(view)
                .children(still.map(|still| {
                    gpui::img(still)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(gpui::ObjectFit::Fill)
                }))
                .into_any_element(),
            (None, Some(error)) => browser_message(&theme, error),
            (None, None) => browser_message(&theme, "Opening browser…"),
        };

        #[cfg(not(target_os = "macos"))]
        let body = {
            let _ = error;
            browser_message(
                &theme,
                "In-app browsing currently requires macOS. A Windows backend has not been chosen yet.",
            )
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .bg(paint(theme.surface))
            .child(toolbar)
            .child(body)
            .into_any_element()
    }
}

/// Run in every page's main frame before its own scripts: tells ket the
/// address may have changed whenever the page moves through its history
/// without loading — `pushState`, `replaceState`, back and forward within
/// it, a new `#fragment`. It sends no address; ket asks WebKit for that.
#[cfg(target_os = "macos")]
const ADDRESS_WATCH: &str = r#"(() => {
  const moved = () => {
    try { window.ipc.postMessage('address'); } catch (_) {}
  };
  for (const name of ['pushState', 'replaceState']) {
    const original = history[name];
    history[name] = function (...args) {
      const result = original.apply(this, args);
      moved();
      return result;
    };
  }
  addEventListener('popstate', moved);
  addEventListener('hashchange', moved);
})();"#;

fn normalize_url(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "about:blank".to_owned();
    }
    if value.contains("://") || value.starts_with("about:") {
        value.to_owned()
    } else {
        format!("http://{value}")
    }
}

fn browser_message(theme: &ket_core::theme::Theme, message: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .px_6()
        .text_sm()
        .text_color(paint(theme.text.dim))
        .child(message.into())
        .into_any_element()
}
