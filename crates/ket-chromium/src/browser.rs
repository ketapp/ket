//! A browser: one page, drawn by Chromium into a native view in a slot ket
//! places. Each page gets a container view inside its parent with CEF's view
//! inside that (its DevTools open in a window of their own), and
//! [`Browser::view`] hands out the container: CEF finishes closing a page only once its view
//! is freed, which a caller holding it would prevent. State lives in
//! main-thread registries by id, not in the handlers, which CEF may release
//! from any thread.
//!
//! A page's popups are pages of their own ([`PopupPage`]): CEF creates each
//! in a hidden container beside its opener's, and ket moves the container
//! into a slot of its own when it takes the popup.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cef::{
    BrowserHost, BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame, Rect,
    Registration, RequestContext, RequestContextSettings, RuntimeStyle, WindowInfo,
};
use objc2::rc::{Retained, autoreleasepool};
use objc2::{MainThreadMarker, MainThreadOnly, Message as _};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use tokio::sync::mpsc;

use crate::cdp::Cdp;
use crate::engine::{self, Closer};
use crate::handlers::{self, CdpObserver, PageContext};
use crate::{Error, failed};

/// What a page reports, in order, on the channel [`Browser::open`] returns.
#[derive(Debug)]
pub enum Event {
    /// The page exists: [`Browser::view`] has its view from now on.
    Created,
    /// The main frame's address changed.
    Address(String),
    /// The page's title changed.
    Title(String),
    /// Loading started or stopped.
    Loading {
        /// Whether it is loading now.
        loading: bool,
        /// Whether [`Browser::back`] would go anywhere.
        back: bool,
        /// Whether [`Browser::forward`] would go anywhere.
        forward: bool,
    },
    /// The page logged to its console.
    Console {
        /// `debug`, `info`, `warning` or `error`.
        level: String,
        /// What it logged.
        text: String,
        /// The script it came from.
        source: String,
        /// The line in that script.
        line: u32,
    },
    /// The page's renderer ended unexpectedly, and how; the page is blank
    /// until it is reloaded.
    Crashed(String),
    /// A download the page started, saved without asking to the Downloads
    /// folder under a name no other file there has: once as it starts, again
    /// as it goes, and last when it is `done` or has `failed`.
    Download {
        /// Chromium's id for it, the same in every report about it.
        id: u32,
        /// The file's name, as saved.
        name: String,
        /// Where it is being saved.
        path: PathBuf,
        /// How many bytes have come so far.
        received: i64,
        /// How many there are in all, or 0 when the server did not say.
        total: i64,
        /// It is finished, and the file is at `path`.
        done: bool,
        /// It stopped short — the connection, the disk, or a cancel.
        failed: bool,
    },
    /// The page opened another — `window.open`, or a link to a new tab —
    /// which waits for ket to give it a place. Dropped, it closes.
    Popup(PopupPage),
    /// The page opened a JavaScript dialog, and is stopped until it is
    /// answered — see [`Browser::answer_dialog`]; `None` once it has closed,
    /// however it was answered.
    Dialog(Option<Dialog>),
    /// The page is gone. Last on the channel.
    Closed,
}

/// A JavaScript dialog a page has open: `alert`, `confirm`, `prompt`, or
/// the question `beforeunload` asks before the page is left. Chromium's
/// DevTools protocol can answer it too (`Page.handleJavaScriptDialog`);
/// [`Event::Dialog`] says when it has closed either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    /// Which.
    pub kind: DialogKind,
    /// What it says. Empty for [`DialogKind::Leave`], whose words a page no
    /// longer chooses.
    pub message: String,
    /// What a prompt's field starts out holding.
    pub default_text: String,
    /// The address of the page that opened it; empty for a leave prompt.
    pub url: String,
}

/// Which JavaScript dialog a [`Dialog`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// `alert`: OK only.
    Alert,
    /// `confirm`: OK or Cancel.
    Confirm,
    /// `prompt`: a line of text, then OK or Cancel.
    Prompt,
    /// `beforeunload`: whether to leave the page — or reload it, when that
    /// is what was asked for.
    Leave {
        /// The page is being reloaded, not left.
        reload: bool,
    },
}

/// An editing command, for whichever frame has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Copy the selection.
    Copy,
    /// Cut the selection.
    Cut,
    /// Paste over the selection.
    Paste,
    /// Select everything.
    SelectAll,
    /// Undo the last edit.
    Undo,
    /// Redo what was undone.
    Redo,
}

/// One page. Main thread only; dropping it closes the page.
pub struct Browser {
    id: u64,
    cdp: Cdp,
    _main_thread: PhantomData<*const ()>,
}

/// A page another page opened — by `window.open`, or a link to a new tab —
/// in the opener's profile and with `window.opener` set, so a sign-in can
/// answer the page that asked for it. It comes on the opener's channel as
/// [`Event::Popup`], drawn in a hidden view beside the opener's until
/// [`adopt`](Self::adopt) gives it a place of its own. Dropped unadopted,
/// the page closes.
pub struct PopupPage {
    id: u64,
    url: String,
    foreground: bool,
    cdp: Cdp,
    /// Taken by [`adopt`](Self::adopt).
    events: Option<mpsc::UnboundedReceiver<Event>>,
}

impl PopupPage {
    /// Where the opener sent it, as it asked; empty when it did not say.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether it was meant to be in front: false for a link opened in a
    /// background tab — ⌘-clicked, say.
    pub fn foreground(&self) -> bool {
        self.foreground
    }

    /// Moves the page into `parent`, filling it and following its size, and
    /// hands it over as a [`Browser`] with its channel, which has everything
    /// it has said so far. On the main thread.
    pub fn adopt(mut self, parent: &NSView) -> (Browser, mpsc::UnboundedReceiver<Event>) {
        let events = self
            .events
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1);
        if let Some(container) = container(self.id) {
            container.removeFromSuperview();
            container.setFrame(parent.bounds());
            parent.addSubview(&container);
            container.setHidden(false);
        }
        PAGES.with_borrow_mut(|pages| {
            if let Some(page) = pages.get_mut(&self.id) {
                page.parent = parent.retain();
            }
        });
        let browser = Browser {
            id: self.id,
            cdp: self.cdp.clone(),
            _main_thread: PhantomData,
        };
        (browser, events)
    }
}

impl Drop for PopupPage {
    fn drop(&mut self) {
        if self.events.is_none() {
            return;
        }
        let id = self.id;
        if MainThreadMarker::new().is_some() {
            close(id);
        } else {
            engine::on_ui(None, move || close(id));
        }
    }
}

impl std::fmt::Debug for PopupPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopupPage")
            .field("url", &self.url)
            .field("foreground", &self.foreground)
            .finish_non_exhaustive()
    }
}

struct Page {
    /// Where it is going until its browser exists.
    start: Start,
    /// Until the profile is ready to create the browser with.
    client: Option<cef::Client>,
    /// Kept until the browser holds its own.
    context: Option<RequestContext>,
    /// CEF has been asked to create the browser and has not yet: it holds
    /// the container's address meanwhile.
    requested: bool,
    browser: Option<cef::Browser>,
    closing: bool,
    observer: Option<Registration>,
    /// DevTools messages sent before the browser existed, by call id.
    queued: Vec<(usize, Vec<u8>)>,
    cdp: Cdp,
    events: mpsc::UnboundedSender<Event>,
    /// For a popup, its opener and the id CEF gave the popup there.
    popup: Option<(u64, c_int)>,
    /// The view its container is in, where a popup it opens waits.
    parent: Retained<NSView>,
    /// Where it keeps its cookies and storage; a popup, its opener's.
    profile: PathBuf,
}

/// Where a page goes before its browser exists. [`create`] hands CEF the
/// address to create it on; a navigation asked for after that and before CEF
/// reports the browser made is kept, the latest only, and loaded once it is —
/// unless it is where the page was going anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Start {
    /// The address the browser is created on.
    url: String,
    /// CEF has `url`, which can no longer change.
    handed: bool,
    /// The latest navigation asked for since.
    then: Option<String>,
}

impl Start {
    /// For a page whose browser [`create`] has yet to ask for.
    fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            handed: false,
            then: None,
        }
    }

    /// For a popup, whose address CEF already has.
    fn handed(url: &str) -> Self {
        Self {
            handed: true,
            ..Self::new(url)
        }
    }

    /// `url` was asked for before the browser exists.
    fn navigate(&mut self, url: &str) {
        if self.handed {
            self.then = Some(url.to_owned());
        } else {
            url.clone_into(&mut self.url);
        }
    }

    /// The address to create the browser on, which from now on stays.
    fn hand_over(&mut self) -> String {
        self.handed = true;
        self.url.clone()
    }

    /// The browser exists: what to load in it now, if anything.
    fn created(&mut self) -> Option<String> {
        self.then.take().filter(|then| *then != self.url)
    }
}

thread_local! {
    static PAGES: RefCell<HashMap<u64, Page>> = RefCell::new(HashMap::new());
    /// Every container, a page's or its DevTools', by the same ids.
    static VIEWS: RefCell<HashMap<u64, Retained<NSView>>> = RefCell::new(HashMap::new());
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl Browser {
    /// Opens `url` in a new page filling `parent` and following its size,
    /// keeping its cookies and storage in `profile`, a directory beneath
    /// [`Config::root_cache`](crate::Config::root_cache). Pages with the
    /// same profile share them. The page itself follows a moment later,
    /// once the profile is ready: [`Event::Created`].
    pub fn open(
        parent: &NSView,
        profile: &Path,
        url: &str,
    ) -> Result<(Browser, mpsc::UnboundedReceiver<Event>), Error> {
        if !engine::started() {
            return Err(failed("Chromium is not running"));
        }
        let profile = engine::profile_path(profile)?;
        let settings = RequestContextSettings {
            cache_path: engine::cef_path(&profile)?,
            ..Default::default()
        };
        engine::on_shutdown(Closer {
            close_all,
            forget_all,
        });
        let id = place(parent);
        let (events, received) = mpsc::unbounded_channel();
        let cdp = Cdp::new(id);
        let page = Page {
            start: Start::new(url),
            client: Some(handlers::page_client(id, events.clone())),
            context: None,
            requested: false,
            browser: None,
            closing: false,
            observer: None,
            queued: Vec::new(),
            cdp: cdp.clone(),
            events,
            popup: None,
            parent: parent.retain(),
            profile: profile.clone(),
        };
        PAGES.with_borrow_mut(|pages| pages.insert(id, page));
        // A profile other than the root initializes asynchronously, and
        // `PageContext` creates the browser once it has.
        let mut handler = PageContext::new(id);
        let Some(context) =
            cef::request_context_create_context(Some(&settings), Some(&mut handler))
        else {
            PAGES.with_borrow_mut(|pages| pages.remove(&id));
            unplace(id);
            return Err(failed(format!(
                "Chromium could not open the profile at {}",
                profile.display()
            )));
        };
        PAGES.with_borrow_mut(|pages| {
            if let Some(page) = pages.get_mut(&id) {
                page.context = Some(context);
            }
        });
        let browser = Browser {
            id,
            cdp,
            _main_thread: PhantomData,
        };
        Ok((browser, received))
    }

    /// The page's view, from [`Event::Created`] until [`Event::Closed`]: a
    /// container filling the parent, with Chromium's view inside. Focus the
    /// page with [`focus`](Self::focus), not by making this first responder.
    pub fn view(&self) -> Option<Retained<NSView>> {
        self.browser()?;
        VIEWS.with_borrow(|views| views.get(&self.id).cloned())
    }

    /// Loads `url` in the main frame. Before [`Event::Created`] it is where
    /// the page starts, or, once Chromium has been given that, what it loads
    /// as soon as it exists; the latest asked for wins.
    pub fn navigate(&self, url: &str) {
        if self.browser().is_some() {
            navigate(self.id, url);
        } else {
            PAGES.with_borrow_mut(|pages| {
                if let Some(page) = pages.get_mut(&self.id) {
                    page.start.navigate(url);
                }
            });
        }
    }

    /// Goes back a page, if there is one. This and [`forward`](Self::forward),
    /// [`reload`](Self::reload) and [`stop`](Self::stop) do nothing before
    /// [`Event::Created`]: the page has no history yet, and its first load
    /// is on its way.
    pub fn back(&self) {
        self.browser().inspect(ImplBrowser::go_back);
    }

    /// Goes forward a page, if there is one.
    pub fn forward(&self) {
        self.browser().inspect(ImplBrowser::go_forward);
    }

    /// Loads the page again.
    pub fn reload(&self) {
        self.browser().inspect(ImplBrowser::reload);
    }

    /// Stops loading.
    pub fn stop(&self) {
        self.browser().inspect(ImplBrowser::stop_load);
    }

    /// Gives the page keyboard focus, or takes it away.
    pub fn focus(&self, on: bool) {
        self.host().inspect(|host| host.set_focus(on.into()));
    }

    /// Copies, pastes and the rest, in the focused frame.
    pub fn edit(&self, edit: Edit) {
        let Some(browser) = self.browser() else {
            return;
        };
        let Some(frame) = browser.focused_frame().or_else(|| browser.main_frame()) else {
            return;
        };
        match edit {
            Edit::Copy => frame.copy(),
            Edit::Cut => frame.cut(),
            Edit::Paste => frame.paste(),
            Edit::SelectAll => frame.select_all(),
            Edit::Undo => frame.undo(),
            Edit::Redo => frame.redo(),
        }
    }

    /// Opens Chromium's DevTools for the page, in a window of their own.
    ///
    /// Not docked in ket's pane: on macOS CEF makes DevTools Chrome style,
    /// and Chrome style cannot have a native parent view (CEF issue 3294), so
    /// a parent handed in fails a check inside `ShowDevTools` and takes the
    /// process down. Does nothing while they are already showing.
    pub fn show_devtools(&self) {
        let Some(host) = self.host().filter(|host| host.has_dev_tools() != 1) else {
            return;
        };
        let window = WindowInfo {
            runtime_style: RuntimeStyle::CHROME,
            ..Default::default()
        };
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        host.show_dev_tools(
            Some(&window),
            Some(&mut handlers::devtools_client(id)),
            Some(&BrowserSettings::default()),
            None,
        );
    }

    /// Whether the page's DevTools are open: ket's toggle asks rather than
    /// remembering, since their window can be closed from its own title bar.
    pub fn devtools_open(&self) -> bool {
        self.host().is_some_and(|host| host.has_dev_tools() == 1)
    }

    /// Closes the page's DevTools, if they are showing.
    pub fn close_devtools(&self) {
        self.host().inspect(ImplBrowserHost::close_dev_tools);
    }

    /// Answers the JavaScript dialog the page has open: OK, with `text` in
    /// a prompt's field, or Cancel. False when none is waiting.
    pub fn answer_dialog(&self, accept: bool, text: Option<&str>) -> bool {
        handlers::answer_dialog(self.id, accept, text)
    }

    /// The page's DevTools protocol, for any thread.
    pub fn cdp(&self) -> Cdp {
        self.cdp.clone()
    }

    /// Closes the page without asking it: `beforeunload` handlers do not
    /// get to keep it open. [`Event::Closed`] follows.
    pub fn close(&self) {
        close(self.id);
    }

    fn browser(&self) -> Option<cef::Browser> {
        PAGES.with_borrow(|pages| pages.get(&self.id)?.browser.clone())
    }

    fn host(&self) -> Option<BrowserHost> {
        self.browser()?.host()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.close();
    }
}

/// Makes a container filling `parent` and following its size, under a new
/// id.
fn place(parent: &NSView) -> u64 {
    let mtm = MainThreadMarker::from(parent);
    let container = NSView::initWithFrame(NSView::alloc(mtm), parent.bounds());
    container.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    parent.addSubview(&container);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    VIEWS.with_borrow_mut(|views| views.insert(id, container));
    id
}

/// Takes container `id` away.
fn unplace(id: u64) {
    if let Some(container) = VIEWS.with_borrow_mut(|views| views.remove(&id)) {
        container.removeFromSuperview();
    }
}

/// Container `id`, while it is there.
fn container(id: u64) -> Option<Retained<NSView>> {
    VIEWS.with_borrow(|views| views.get(&id).cloned())
}

/// Settings for a windowed, Alloy-style browser filling container `id`.
fn window_in(id: u64) -> Option<WindowInfo> {
    let container = container(id)?;
    let size = container.bounds().size;
    let bounds = Rect {
        x: 0,
        y: 0,
        width: size.width.round() as i32,
        height: size.height.round() as i32,
    };
    let handle: *mut c_void = Retained::as_ptr(&container).cast_mut().cast();
    let window = WindowInfo {
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    Some(window.set_as_child(handle, &bounds))
}

/// The page's profile is ready: create its browser, unless it was closed in
/// the meantime.
pub(crate) fn create(id: u64, context: Option<&mut RequestContext>) {
    let start = PAGES.with_borrow_mut(|pages| {
        let page = pages.get_mut(&id)?;
        Some((page.client.take()?, page.closing, page.start.hand_over()))
    });
    let Some((mut client, closing, url)) = start else {
        return;
    };
    let Some(window) = window_in(id).filter(|_| !closing) else {
        return forget(id);
    };
    engine::opening();
    let asked = cef::browser_host_create_browser(
        Some(&window),
        Some(&mut client),
        Some(&CefString::from(url.as_str())),
        Some(&BrowserSettings::default()),
        None,
        context,
    );
    PAGES.with_borrow_mut(|pages| {
        if let Some(page) = pages.get_mut(&id) {
            page.requested = asked == 1 && page.browser.is_none();
        }
    });
    if asked != 1 {
        engine::closed();
        if let Some(events) = PAGES.with_borrow(|pages| Some(pages.get(&id)?.events.clone())) {
            let _ = events.send(Event::Crashed("Chromium would not create the page".into()));
        }
        forget(id);
    }
}

/// CEF has created the page's browser.
pub(crate) fn created(id: u64, browser: &cef::Browser) {
    let Some(host) = browser.host() else {
        return;
    };
    let Some(cdp) = PAGES.with_borrow(|pages| Some(pages.get(&id)?.cdp.clone())) else {
        host.close_browser(1);
        return;
    };
    let observer = host.add_dev_tools_message_observer(Some(&mut CdpObserver::new(cdp.clone())));
    let state = PAGES.with_borrow_mut(|pages| {
        let page = pages.get_mut(&id)?;
        page.browser = Some(browser.clone());
        page.observer = observer;
        page.requested = false;
        let queued = std::mem::take(&mut page.queued);
        Some((
            queued,
            page.closing,
            page.events.clone(),
            page.context.take(),
            page.start.created(),
        ))
    });
    // The browser holds its own reference to the context now.
    let Some((queued, closing, events, _context, then)) = state else {
        return;
    };
    for (call, message) in queued {
        if host.send_dev_tools_message(Some(&message)) != 1 {
            cdp.answer(call, Err("Chromium refused the message".into()));
        }
    }
    // A navigation asked for while CEF made the page, in place of the
    // address it was made on.
    if let (false, Some(url)) = (closing, then) {
        navigate(id, &url);
    }
    let _ = events.send(Event::Created);
    if closing {
        host.close_browser(1);
    }
}

/// CEF is closing a page or DevTools and leaves the view to ket: free CEF's
/// view on the next turn, not inside CEF's call, in a pool of its own so it
/// goes then and not at the end of a run-loop turn shutdown never reaches.
pub(crate) fn release_view(id: u64) {
    engine::on_ui(None, move || {
        let Some(container) = VIEWS.with_borrow(|views| views.get(&id).cloned()) else {
            return;
        };
        autoreleasepool(|_| {
            for child in container.subviews().to_vec() {
                child.removeFromSuperview();
            }
        });
    });
}

/// A page or DevTools has closed.
pub(crate) fn closed(id: u64, devtools: bool) {
    engine::closed();
    if !devtools {
        forget(id);
    }
}

/// Lets go of a page and says it has closed.
fn forget(id: u64) {
    let Some(page) = PAGES.with_borrow_mut(|pages| pages.remove(&id)) else {
        return;
    };
    if page.requested {
        // Only when shutdown gives up waiting: CEF may yet put the browser
        // in the container, so it must not be freed. The process is exiting.
        if let Some(container) = VIEWS.with_borrow(|views| views.get(&id).cloned()) {
            std::mem::forget(container);
        }
    }
    unplace(id);
    crate::handlers::forget_downloads(id);
    crate::handlers::forget_dialog(id);
    page.cdp.close();
    let _ = page.events.send(Event::Closed);
}

/// Closes page `id` without asking it — see [`Browser::close`].
fn close(id: u64) {
    if !engine::alive() {
        return;
    }
    let browser = PAGES.with_borrow_mut(|pages| {
        let page = pages.get_mut(&id).filter(|page| !page.closing)?;
        page.closing = true;
        // Not created yet: `create` or `created` finish the close.
        page.browser.clone()
    });
    if let Some(host) = browser.and_then(|browser| browser.host()) {
        host.close_browser(1);
    }
}

/// Page `opener` wants a popup, which CEF creates once this returns the
/// settings and client for it: a hidden container beside the opener's, and
/// a page of its own registered under a new id, handed to ket as
/// [`Event::Popup`]. `None` refuses it.
pub(crate) fn popup(
    opener: u64,
    popup_id: c_int,
    url: &str,
    foreground: bool,
) -> Option<(WindowInfo, cef::Client)> {
    if !engine::started() {
        return None;
    }
    let (told, parent, profile) = PAGES.with_borrow(|pages| {
        let page = pages.get(&opener).filter(|page| !page.closing)?;
        Some((
            page.events.clone(),
            page.parent.clone(),
            page.profile.clone(),
        ))
    })?;
    let id = place(&parent);
    let Some(window) = window_in(id) else {
        unplace(id);
        return None;
    };
    container(id)?.setHidden(true);
    let (events, received) = mpsc::unbounded_channel();
    let cdp = Cdp::new(id);
    let client = handlers::page_client(id, events.clone());
    let page = Page {
        start: Start::handed(url),
        client: None,
        context: None,
        // CEF holds the container's address from here on.
        requested: true,
        browser: None,
        closing: false,
        observer: None,
        queued: Vec::new(),
        cdp: cdp.clone(),
        events,
        popup: Some((opener, popup_id)),
        parent,
        profile,
    };
    PAGES.with_borrow_mut(|pages| pages.insert(id, page));
    let popup = PopupPage {
        id,
        url: url.to_owned(),
        foreground,
        cdp,
        events: Some(received),
    };
    if let Err(unheard) = told.send(Event::Popup(popup)) {
        // Nobody is listening to the opener: refused, and nothing was asked
        // of CEF to undo.
        if let Event::Popup(mut popup) = unheard.0 {
            popup.events = None;
        }
        PAGES.with_borrow_mut(|pages| pages.remove(&id));
        unplace(id);
        return None;
    }
    engine::opening();
    Some((window, client))
}

/// CEF could not create the popup `opener` asked for as `popup_id`: let go
/// of it, which tells whoever adopted it that it has closed.
pub(crate) fn popup_aborted(opener: u64, popup_id: c_int) {
    let id = PAGES.with_borrow_mut(|pages| {
        let (id, page) = pages
            .iter_mut()
            .find(|(_, page)| page.popup == Some((opener, popup_id)) && page.browser.is_none())?;
        page.requested = false;
        Some(*id)
    });
    if let Some(id) = id {
        engine::closed();
        forget(id);
    }
}

/// Whether any page — closing or not, popups included — still keeps its
/// cookies and storage in `profile`: until none does, Chromium may have its
/// files open. On the main thread.
pub fn profile_in_use(profile: &Path) -> bool {
    PAGES.with_borrow(|pages| pages.values().any(|page| page.profile == profile))
}

/// Loads `url` in the page's main frame.
pub(crate) fn navigate(id: u64, url: &str) {
    let browser = PAGES.with_borrow(|pages| pages.get(&id)?.browser.clone());
    if let Some(frame) = browser.and_then(|browser| browser.main_frame()) {
        frame.load_url(Some(&CefString::from(url)));
    }
}

/// Sends a DevTools message, or keeps it until the browser exists. From
/// [`Cdp::call`], on the UI thread.
pub(crate) fn send_devtools(id: u64, call: usize, message: Vec<u8>, cdp: &Cdp) {
    let browser = PAGES.with_borrow_mut(|pages| {
        let page = pages.get_mut(&id)?;
        if page.browser.is_none() {
            page.queued.push((call, message.clone()));
        }
        Some(page.browser.clone())
    });
    let sent = match browser {
        None => return cdp.answer(call, Err("the page has closed".into())),
        Some(None) => return,
        Some(Some(browser)) => browser
            .host()
            .is_some_and(|host| host.send_dev_tools_message(Some(&message)) == 1),
    };
    if !sent {
        cdp.answer(call, Err("Chromium refused the message".into()));
    }
}

/// Asks every page and its DevTools to close, for [`engine::shutdown`].
fn close_all() {
    let browsers: Vec<cef::Browser> = PAGES.with_borrow_mut(|pages| {
        pages
            .values_mut()
            .filter_map(|page| {
                page.closing = true;
                page.browser.clone()
            })
            .collect()
    });
    for host in browsers.iter().filter_map(ImplBrowser::host) {
        host.close_dev_tools();
        host.close_browser(1);
    }
}

/// Lets go of everything still open, once [`engine::shutdown`] has waited
/// as long as it will.
fn forget_all() {
    let ids: Vec<u64> = PAGES.with_borrow(|pages| pages.keys().copied().collect());
    ids.into_iter().for_each(forget);
    let left: Vec<u64> = VIEWS.with_borrow(|views| views.keys().copied().collect());
    left.into_iter().for_each(unplace);
}

#[cfg(test)]
mod tests {
    use super::Start;

    #[test]
    fn a_navigation_before_the_hand_over_changes_where_the_page_starts() {
        let mut start = Start::new("https://first.example/");
        start.navigate("https://second.example/");
        assert_eq!(start.hand_over(), "https://second.example/");
        assert_eq!(start.created(), None);
    }

    #[test]
    fn a_navigation_while_chromium_makes_the_page_is_loaded_once_it_exists() {
        let mut start = Start::new("https://first.example/");
        assert_eq!(start.hand_over(), "https://first.example/");
        start.navigate("https://second.example/");
        start.navigate("https://third.example/");
        assert_eq!(start.created().as_deref(), Some("https://third.example/"));
        // Once only.
        assert_eq!(start.created(), None);
    }

    #[test]
    fn a_navigation_back_to_the_first_address_loads_nothing_more() {
        let mut start = Start::new("https://first.example/");
        start.hand_over();
        start.navigate("https://second.example/");
        start.navigate("https://first.example/");
        assert_eq!(start.created(), None);
    }

    #[test]
    fn a_popup_already_has_its_address() {
        let mut start = Start::handed("https://popup.example/");
        start.navigate("https://elsewhere.example/");
        assert_eq!(
            start.created().as_deref(),
            Some("https://elsewhere.example/")
        );
    }
}
