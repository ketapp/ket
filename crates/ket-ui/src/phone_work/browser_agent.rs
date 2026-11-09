//! `ket browser`, done in the window: what a terminal asked of a worktree's
//! Chromium tab, handed to this window by the host — see
//! `ket_core::host::take_browser_asks` — and answered through
//! `ket_core::host::finish_browser`.
//!
//! The tab is the worktree's Chromium tab in front, else its latest. The page
//! is reached in process, through its DevTools protocol
//! ([`ket_chromium::Cdp`]): no remote-debugging port is ever opened. Going
//! back, forward and reloading go through the page's own handle; a new
//! address goes through `Page.navigate`, which says when a load failed. Each
//! waits for the page to load, up to [`LOAD_WAIT`], and answers with where
//! it ended up. A screenshot is written where the device tabs' are — see
//! `ket_core::capture` — and only its path is sent; the console is the tab's
//! own record of it.
//!
//! An agent drives the page the way a person would: `snapshot` reads its
//! accessibility tree and hands each node a ref (see `snapshot` and `refs`),
//! and `click`, `fill`, `type`, `press`, `scroll` and `wait` act on a ref or
//! a CSS selector with the protocol's trusted mouse and keyboard events (see
//! `act`, `typing`, `scroll` and `wait`). `record` makes a video of the page
//! between two runs of the CLI, kept in this window with the page. A
//! JavaScript dialog the page opens is said at once, and answered with
//! `dialog` (see `dialog`).

mod act;
mod dialog;
mod held;
mod keys;
mod record;
mod refs;
mod scroll;
mod snapshot;
mod typing;
mod wait;

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::{AsyncApp, BackgroundExecutor, Context, EntityId, FutureExt as _, WeakEntity};
use ket_chromium::protocol::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, FrameTree, GetFrameTreeParams,
    GetLayoutMetricsParams, GetNavigationHistoryParams, NavigateParams, Viewport,
};
use ket_chromium::protocol::js_protocol::runtime::{
    EvaluateParams, ExceptionDetails, ReleaseObjectGroupParams, RemoteObject, RemoteObjectType,
    TimeDelta,
};
use ket_chromium::{Cdp, CdpEvent};
use ket_core::host::{
    BrowserAction, BrowserAnswer, BrowserAsk, BrowserTab, ConsoleLine, finish_browser,
};
use ket_core::id::WorktreeId;
use tokio::sync::broadcast;

use self::held::{Held, Release};
use crate::Shell;
use crate::browser::{BrowserId, normalize_url};
use crate::chromium::{ChromiumTab, ConsoleEntry};

/// How long a load is waited for. Past it the page is reported as it stands:
/// one that never finishes — a stream held open — has still loaded enough to
/// look at.
const LOAD_WAIT: Duration = Duration::from_secs(10);

/// How often a load is looked at while it is waited for.
const LOAD_LOOK: Duration = Duration::from_millis(100);

/// How long one look at the page may take. A page busy loading can answer
/// late, and the next look is soon.
const LOOK_WAIT: Duration = Duration::from_secs(2);

/// How long the window gives any one request in all: short of the host's
/// twenty seconds, so the terminal hears why from here.
const ANSWER_WAIT: Duration = Duration::from_secs(18);

/// How long a page may take to draw a screenshot.
const SHOT_WAIT: Duration = Duration::from_secs(12);

/// How long JavaScript may run before the page stops it, in milliseconds:
/// a loop that never ends would otherwise hang the page.
const EVAL_MS: f64 = 10_000.0;

/// The tallest full-page screenshot, in device pixels: about as much as
/// Chromium draws in one go.
const SHOT_MAX: f64 = 16_384.0;

/// Device pixels to a CSS pixel on the densest display a Mac has: what
/// [`SHOT_MAX`] is shared by when a full page's size is asked in CSS pixels.
const MAC_MOST_SCALE: f64 = 2.0;

/// The most of a value sent back, in bytes; past it the terminal is told how
/// much there was.
const VALUE_MAX: usize = 4 * 1024;

type Answer = Result<BrowserAnswer, String>;

/// What is done to a tab's page, once the tab is found.
enum Step {
    /// A tab just opened on this address: wait for it.
    Opened(String),
    /// Load this address.
    Navigate(String),
    /// Go back a page.
    Back,
    /// Go forward a page.
    Forward,
    /// Load the page again.
    Reload,
    /// Save a PNG of the page — all of it, or what the tab shows.
    Screenshot {
        /// All of it.
        full_page: bool,
    },
    /// Run this JavaScript.
    Eval(String),
    /// Read the page or act on it as a person would: `snapshot`, `click`,
    /// `fill`, `type`, `press`, `scroll`, `wait`.
    Act(BrowserAction),
    /// Answer the dialog the page has open: OK, with this in a prompt's
    /// field, or Cancel.
    Dialog {
        /// OK.
        accept: bool,
        /// What a prompt holds.
        text: Option<String>,
    },
}

impl Shell {
    /// Does what `ket browser` asked of this window's tabs, answering each
    /// once it is done.
    pub(crate) fn answer_browser_asks(&mut self, asks: Vec<BrowserAsk>, cx: &mut Context<Self>) {
        for ask in asks {
            self.answer_browser_ask(ask, cx);
        }
    }

    fn answer_browser_ask(&mut self, ask: BrowserAsk, cx: &mut Context<Self>) {
        let BrowserAsk {
            req,
            worktree,
            action,
        } = ask;
        let worktree = WorktreeId::from(worktree.as_str());
        let tab = self.agent_browser_tab(&worktree);
        // The page is stopped until its dialog is answered, and anything
        // asked of it would wait as long: said at once instead.
        if let Some(open) = tab.as_ref().and_then(|tab| tab.dialog.as_ref())
            && dialog::waits(&action)
        {
            return finish_browser(req, Err(dialog::waiting(open)));
        }
        let step = match (action, tab.as_ref()) {
            (BrowserAction::Tabs, _) => {
                let acted_on = tab.as_ref().map(|tab| tab.id);
                let tabs = self
                    .chromium_tabs()
                    .into_iter()
                    .filter(|tab| tab.worktree == worktree)
                    .map(|tab| BrowserTab {
                        active: Some(tab.id) == acted_on,
                        dialog: tab.dialog.as_ref().map(dialog::what),
                        url: tab.url,
                        title: tab.title,
                    })
                    .collect();
                return finish_browser(req, Ok(BrowserAnswer::Tabs { tabs }));
            }
            // Found whichever tab is in front now, or none.
            (BrowserAction::RecordStop, _) => return self.stop_recording(req, worktree, cx),
            (BrowserAction::Open { url: Some(url) }, None) => {
                return match self.open_chromium_tab(&worktree, &url, cx) {
                    Ok(id) => self.drive_browser(req, id, Step::Opened(normalize_url(&url)), cx),
                    Err(why) => finish_browser(req, Err(why)),
                };
            }
            (_, None) => return finish_browser(req, Err(no_tab())),
            (BrowserAction::Open { url: None }, Some(tab)) => {
                return finish_browser(
                    req,
                    Ok(BrowserAnswer::Done {
                        url: tab.url.clone(),
                        title: tab.title.clone(),
                    }),
                );
            }
            (BrowserAction::Open { url: Some(url) }, Some(_)) => {
                Step::Navigate(normalize_url(&url))
            }
            (BrowserAction::Back, Some(_)) => Step::Back,
            (BrowserAction::Forward, Some(_)) => Step::Forward,
            (BrowserAction::Reload, Some(_)) => Step::Reload,
            (
                BrowserAction::Console {
                    errors_only,
                    since_ms,
                    lines,
                },
                Some(tab),
            ) => {
                let since = since_ms.map(|ms| ket_core::now_ms().saturating_sub(ms));
                let logged = self.chromium_console(tab.id);
                let lines = console_lines(logged, errors_only, since, lines);
                return finish_browser(req, Ok(BrowserAnswer::Console { lines }));
            }
            (BrowserAction::Screenshot { full_page }, Some(_)) => Step::Screenshot { full_page },
            (BrowserAction::RecordStart { max_seconds }, Some(tab)) => {
                return self.start_recording(req, worktree, tab.id, max_seconds, cx);
            }
            (BrowserAction::Eval { expression }, Some(_)) => Step::Eval(expression),
            (BrowserAction::Dialog { accept, text }, Some(tab)) => {
                match dialog::answer(tab.dialog.as_ref(), accept, text) {
                    Ok(text) => Step::Dialog { accept, text },
                    Err(why) => return finish_browser(req, Err(why)),
                }
            }
            (
                action @ (BrowserAction::Snapshot { .. }
                | BrowserAction::Click { .. }
                | BrowserAction::Fill { .. }
                | BrowserAction::Type { .. }
                | BrowserAction::Press { .. }
                | BrowserAction::Scroll { .. }
                | BrowserAction::Wait { .. }),
                Some(_),
            ) => Step::Act(action),
        };
        if let Some(tab) = tab {
            self.drive_browser(req, tab.id, step, cx);
        }
    }

    /// The tab `ket browser` acts on in `worktree`: the Chromium tab in its
    /// focused pane — in a split, the one the person is looking at — else
    /// one in front in another pane, else its latest.
    fn agent_browser_tab(&self, worktree: &WorktreeId) -> Option<ChromiumTab> {
        let tabs: Vec<ChromiumTab> = self
            .chromium_tabs()
            .into_iter()
            .filter(|tab| &tab.worktree == worktree)
            .collect();
        let focused = self
            .spaces
            .get(worktree)
            .and_then(|space| match space.active_tab()?.kind {
                crate::tabs::TabKind::Browser(id) => Some(id),
                _ => None,
            });
        focused
            .and_then(|id| tabs.iter().find(|tab| tab.id == id))
            .or_else(|| tabs.iter().rev().find(|tab| tab.active))
            .or(tabs.last())
            .cloned()
    }

    /// Does `step` to browser `id`'s page without holding the window up, and
    /// answers request `req` when it is done.
    fn drive_browser(&mut self, req: u64, id: BrowserId, step: Step, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Shell>, cx| {
            let executor = cx.background_executor().clone();
            let answer = drive(&this, id, step, cx)
                .with_timeout(ANSWER_WAIT, &executor)
                .await
                .unwrap_or_else(|_| Err("the page didn't answer in time".to_owned()));
            finish_browser(req, answer);
        })
        .detach();
    }
}

fn no_tab() -> String {
    "this worktree has no browser tab; `ket browser open <url>` opens one".to_owned()
}

fn page_gone() -> String {
    "the worktree's browser tab has no page now; close it and open another".to_owned()
}

/// Does `step` to browser `id`'s page and says where it ended up — or, if
/// the page opens a JavaScript dialog meanwhile, which stops it, says that.
async fn drive(this: &WeakEntity<Shell>, id: BrowserId, step: Step, cx: &mut AsyncApp) -> Answer {
    let executor = cx.background_executor().clone();
    let cdp = page_of(this, id, &executor, cx)
        .await
        .ok_or_else(page_gone)?;
    let mut page = Page::new(&cdp, &executor, id, this.entity_id());
    let mut opening = match step {
        // Its answer may open another; enabling the page's events would wait
        // on the page, which this one stops.
        Step::Dialog { .. } => cdp.events(),
        _ => page.watch().await,
    };
    tokio::select! {
        biased;
        opened = dialog::opening(&mut opening) => {
            let (releases, objects) = page.hand_over();
            let_go_after_dialog(cdp.clone(), executor.clone(), opening, releases, objects);
            Err(dialog::waiting(&opened))
        }
        // What it still holds goes with `page`.
        answer = take(this, id, &page, step, cx) => answer,
    }
}

/// The longest what an action held waits for its dialog to be answered
/// before it goes anyway.
const DIALOG_LET_GO: Duration = Duration::from_secs(600);

/// An action a dialog stopped never let go of what it held: the handles it
/// took on the page's nodes, in its group, and any key or mouse button it
/// pressed and had yet to let go of (see `held`). They go once the dialog
/// has been answered, by the person or by `ket browser dialog` — `events`
/// having heard it open, so it hears it close — rather than piling up click
/// after click, or leaving the page mid-press. Not before: the page answers
/// nothing while its dialog is up.
fn let_go_after_dialog(
    cdp: Cdp,
    executor: BackgroundExecutor,
    mut events: broadcast::Receiver<CdpEvent>,
    releases: Vec<Release>,
    objects: ReleaseObjectGroupParams,
) {
    let waiting = executor.clone();
    executor
        .spawn(async move {
            let closed = dialog::closing(&mut events)
                .with_timeout(DIALOG_LET_GO, &waiting)
                .await;
            // A page that has closed has nothing left to let go of.
            if matches!(closed, Ok(false)) {
                return;
            }
            for release in releases {
                release.send(&cdp);
            }
            cdp.send(objects);
        })
        .detach();
}

/// Does `step` to browser `id`'s page, `page`, and says where it ended up.
async fn take(
    this: &WeakEntity<Shell>,
    id: BrowserId,
    page: &Page<'_>,
    step: Step,
    cx: &mut AsyncApp,
) -> Answer {
    let cdp = page.cdp;
    let (back, forward) = match step {
        Step::Opened(url) => {
            // A new page starts out blank, before its first load lands.
            let blank = url == "about:blank";
            return page
                .settle(|now| blank || !(now.url.is_empty() || now.url == "about:blank"))
                .await;
        }
        Step::Navigate(url) => {
            let from = page.spot().await;
            let went = cdp
                .call(NavigateParams::new(url.clone()))
                .await
                .map_err(|error| error.to_string())?;
            if let Some(why) = went.error_text.filter(|why| !why.is_empty()) {
                return Err(format!("couldn't load {url}: {why}"));
            }
            return page.settle(left(from)).await;
        }
        Step::Screenshot { full_page } => return page.screenshot(full_page).await,
        Step::Eval(expression) => return page.eval(expression).await,
        Step::Act(action) => return page.act(action).await,
        Step::Dialog { accept, text } => return page.answer_dialog(this, accept, text, cx).await,
        Step::Reload => (false, false),
        Step::Back => (true, false),
        Step::Forward => (false, true),
    };
    if back || forward {
        let history = cdp
            .call(GetNavigationHistoryParams::default())
            .await
            .map_err(|error| error.to_string())?;
        let last = i64::try_from(history.entries.len()).unwrap_or(i64::MAX) - 1;
        if back && history.current_index <= 0 {
            return Err("there's no page to go back to".to_owned());
        }
        if forward && history.current_index >= last {
            return Err("there's no page to go forward to".to_owned());
        }
    }
    let from = page.spot().await;
    let moved = this
        .update(cx, |shell, _| {
            let page = shell.chromium_browser(id)?;
            if back {
                page.back();
            } else if forward {
                page.forward();
            } else {
                page.reload();
            }
            Some(())
        })
        .ok()
        .flatten();
    moved.ok_or_else(page_gone)?;
    page.settle(left(from)).await
}

/// Browser `id`'s DevTools protocol, once it has a page: at once, unless the
/// tab was opened before the window could make one, which its first draw
/// does.
async fn page_of(
    this: &WeakEntity<Shell>,
    id: BrowserId,
    executor: &BackgroundExecutor,
    cx: &mut AsyncApp,
) -> Option<Cdp> {
    let deadline = Instant::now() + LOAD_WAIT;
    loop {
        let cdp = this
            .read_with(cx, |shell, _| {
                shell.chromium_browser(id).map(|page| page.cdp())
            })
            .ok()?;
        if cdp.is_some() || Instant::now() >= deadline {
            return cdp;
        }
        executor.timer(LOAD_LOOK).await;
    }
}

/// Whether the page has gone on from `from`: another document, or another
/// place in the same one. Not knowing where it was, any page counts.
fn left(from: Option<Spot>) -> impl Fn(&Spot) -> bool {
    move |now| {
        from.as_ref()
            .is_none_or(|from| from.loader != now.loader || from.url != now.url)
    }
}

/// Where a page is.
struct Spot {
    /// Its main frame.
    frame: String,
    /// The document's loader: a new one for every load, reloads included.
    loader: String,
    /// Its address, with any fragment.
    url: String,
    /// The address it could not load, when it is showing an error instead.
    unreachable: Option<String>,
}

/// One page's DevTools protocol, and a clock to wait on, for one action.
struct Page<'a> {
    cdp: &'a Cdp,
    executor: &'a BackgroundExecutor,
    /// The tab it is in.
    tab: BrowserId,
    /// Whose refs its nodes' are: the tab's, in this window — see
    /// `snapshot` and `refs`.
    refs: refs::Tab,
    /// The group the action's handles on the page's nodes are in.
    objects: Objects,
    /// What the action has pressed and not let go of yet.
    held: RefCell<Held>,
    /// Whether what it holds has been handed over to go after a dialog.
    handed_over: bool,
}

impl<'a> Page<'a> {
    /// Browser `tab`'s page, in the window whose shell is `window`, for a
    /// new action.
    fn new(
        cdp: &'a Cdp,
        executor: &'a BackgroundExecutor,
        tab: BrowserId,
        window: EntityId,
    ) -> Self {
        Self {
            cdp,
            executor,
            tab,
            refs: refs::Tab::new(window, tab),
            objects: Objects::new(),
            held: RefCell::default(),
            handed_over: false,
        }
    }

    /// What the action holds, for a dialog that stopped it to let go of
    /// once it closes, rather than now: the releases of what it pressed,
    /// and of its handles.
    fn hand_over(&mut self) -> (Vec<Release>, ReleaseObjectGroupParams) {
        self.handed_over = true;
        (self.held.get_mut().releases(), self.objects.release())
    }
}

impl Drop for Page<'_> {
    /// However the action ends — done, failed on the way, or dropped at the
    /// window's deadline — what it still holds goes: a key or mouse button
    /// it pressed and never let go of (see `held`), and its handles on the
    /// page's nodes. Not if a dialog stopped it, which has them to let go of
    /// once it closes.
    fn drop(&mut self) {
        if self.handed_over {
            return;
        }
        for release in self.held.get_mut().releases() {
            release.send(self.cdp);
        }
        self.let_go();
    }
}

impl Page<'_> {
    /// The page's frames, if it says in time.
    async fn frame_tree(&self) -> Option<FrameTree> {
        let tree = self
            .cdp
            .call(GetFrameTreeParams::default())
            .with_timeout(LOOK_WAIT, self.executor)
            .await
            .ok()?
            .ok()?;
        Some(tree.frame_tree)
    }

    /// Where the page is now, if it says in time.
    async fn spot(&self) -> Option<Spot> {
        let frame = self.frame_tree().await?.frame;
        Some(Spot {
            frame: frame.id.inner().clone(),
            loader: frame.loader_id.inner().clone(),
            url: frame.url + frame.url_fragment.as_deref().unwrap_or(""),
            unreachable: frame.unreachable_url.filter(|url| !url.is_empty()),
        })
    }

    /// Evaluates `expression` in the page for its value, if it answers in
    /// time.
    async fn value(&self, expression: &str) -> Option<serde_json::Value> {
        let mut params = EvaluateParams::new(expression);
        params.return_by_value = Some(true);
        params.object_group = self.objects.group();
        let answer = self
            .cdp
            .call(params)
            .with_timeout(LOOK_WAIT, self.executor)
            .await
            .ok()?
            .ok()?;
        answer
            .exception_details
            .is_none()
            .then_some(answer.result.value)
            .flatten()
    }

    /// Waits up to [`LOAD_WAIT`] for the page to be somewhere `moved` says is
    /// new and to finish loading there, then says where it is.
    async fn settle(&self, moved: impl Fn(&Spot) -> bool) -> Answer {
        let deadline = Instant::now() + LOAD_WAIT;
        let mut last = None;
        loop {
            if let Some(now) = self.spot().await {
                let arrived = moved(&now);
                last = Some(now);
                if arrived
                    && self.value("document.readyState").await.as_ref()
                        == Some(&serde_json::Value::from("complete"))
                {
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            self.executor.timer(LOAD_LOOK).await;
        }
        if let Some(url) = last.as_ref().and_then(|spot| spot.unreachable.clone()) {
            return Err(format!("couldn't load {url}"));
        }
        self.here(last).await
    }

    /// Says where the page is now — `last` being where it was last seen,
    /// should it not say.
    async fn here(&self, last: Option<Spot>) -> Answer {
        let (url, title) = self.address(last).await?;
        Ok(BrowserAnswer::Done { url, title })
    }

    /// The page's address and title — `last` being where it was last seen,
    /// should it not say.
    async fn address(&self, last: Option<Spot>) -> Result<(String, String), String> {
        let page = self.value("[location.href, document.title]").await;
        let text = |at: usize| {
            page.as_ref()
                .and_then(|page| page.get(at)?.as_str().map(str::to_owned))
        };
        let url = text(0)
            .or_else(|| last.map(|spot| spot.url))
            .ok_or_else(|| "the page didn't say where it is".to_owned())?;
        Ok((url, text(1).unwrap_or_default()))
    }

    /// Does one of the actions that read the page or act on it as a person
    /// would.
    async fn act(&self, action: BrowserAction) -> Answer {
        match action {
            BrowserAction::Snapshot { interactive, max } => self.snapshot(interactive, max).await,
            BrowserAction::Click {
                target,
                double,
                right,
            } => self.click(&target, double, right).await,
            BrowserAction::Fill { target, text } => self.fill(&target, &text).await,
            BrowserAction::Type { text, keys } => self.type_text(&text, keys).await,
            BrowserAction::Press { key } => self.press(&key).await,
            BrowserAction::Scroll { target, by } => self.scroll(target.as_ref(), by).await,
            BrowserAction::Wait { until, timeout_ms } => self.wait(until, timeout_ms).await,
            other => Err(format!("ket can't do {other:?} here")),
        }
    }

    /// Sends `command` and waits for its answer, or for why not in words.
    async fn ask<C: ket_chromium::Command>(&self, command: C) -> Result<C::Response, String> {
        self.cdp
            .call(command)
            .await
            .map_err(|error| error.to_string())
    }

    /// Lets go of the handles the action has taken on the page's nodes, so
    /// the page can collect them — and of no other action's. Not waited
    /// for: the page takes it before anything sent after it.
    fn let_go(&self) {
        self.cdp.send(self.objects.release());
    }
}

/// Counts the actions done to pages, for each one's [`Objects`].
static ACTIONS: AtomicU64 = AtomicU64::new(0);

/// The DevTools object group the handles one action takes on a page's nodes
/// are in, let go of together once it is done. Each action has its own: two
/// at once on one page — a `wait` looking while a `click` aims — never let go
/// of a handle the other still holds.
struct Objects(String);

impl Objects {
    /// A group no action has had before.
    fn new() -> Self {
        let n = ACTIONS.fetch_add(1, Ordering::Relaxed) + 1;
        Self(format!("ket-browser-{n}"))
    }

    /// The group, as a command that may take a handle names it.
    fn group(&self) -> Option<String> {
        Some(self.0.clone())
    }

    /// What lets go of every handle in the group, and of none in another.
    fn release(&self) -> ReleaseObjectGroupParams {
        ReleaseObjectGroupParams::new(self.0.clone())
    }
}

impl Page<'_> {
    /// Saves a PNG of the page, or of all of it, and says where.
    async fn screenshot(&self, full_page: bool) -> Answer {
        let mut params = CaptureScreenshotParams {
            format: Some(CaptureScreenshotFormat::Png),
            ..Default::default()
        };
        if full_page {
            let size = self
                .cdp
                .call(GetLayoutMetricsParams::default())
                .await
                .map_err(|error| error.to_string())?
                .css_content_size;
            // In CSS pixels, at the densest a Mac display draws: not the
            // page's `devicePixelRatio`, which the page can redefine to
            // anything, zero and infinity among them.
            let most = SHOT_MAX / MAC_MOST_SCALE;
            params.capture_beyond_viewport = Some(true);
            params.clip = Some(Viewport {
                x: 0.0,
                y: 0.0,
                width: size.width.clamp(1.0, most),
                height: size.height.clamp(1.0, most),
                scale: 1.0,
            });
        }
        let shot = self
            .cdp
            .call(params)
            .with_timeout(SHOT_WAIT, self.executor)
            .await
            .map_err(|_| "the page didn't draw a screenshot in time".to_owned())?
            .map_err(|error| error.to_string())?;
        // Megabytes, for a long page: decoded and written off the window's
        // thread.
        let saved = self.executor.spawn(async move {
            use base64::Engine as _;
            let png = base64::engine::general_purpose::STANDARD
                .decode(String::from(shot.data))
                .map_err(|error| format!("Chromium's screenshot was not an image: {error}"))?;
            ket_core::capture::save_screenshot(&png).map_err(|error| error.to_string())
        });
        let path = saved.await?;
        Ok(BrowserAnswer::Screenshot {
            path: path.display().to_string(),
        })
    }

    /// Runs `expression` in the page as its console would — top-level
    /// `await`, a promise awaited — and says what it came to.
    async fn eval(&self, expression: String) -> Answer {
        let mut params = EvaluateParams::new(expression);
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        params.repl_mode = Some(true);
        params.user_gesture = Some(true);
        params.timeout = Some(TimeDelta::new(EVAL_MS));
        // What it throws is a handle on the page, kept in the group.
        params.object_group = self.objects.group();
        let answer = self.cdp.call(params).await.map_err(|error| {
            let why = error.to_string();
            if why.contains("returned by value") || why.contains("reference chain") {
                "the value can't be sent back as JSON (a DOM node or a window, say); return \
                 a string or plain data instead"
                    .to_owned()
            } else {
                why
            }
        })?;
        if let Some(thrown) = answer.exception_details {
            return Err(capped(threw(thrown)).0);
        }
        let (value, cut_from) = capped(shown(answer.result));
        Ok(BrowserAnswer::Value { value, cut_from })
    }
}

/// What a value is as text: a string as it is, `undefined`, a function or a
/// symbol as the console writes it, anything else as JSON.
fn shown(value: RemoteObject) -> String {
    if let Some(raw) = value.unserializable_value {
        // NaN, Infinity, -0, a BigInt.
        return raw.as_ref().to_owned();
    }
    match (value.r#type, value.value) {
        (RemoteObjectType::Undefined, _) => "undefined".to_owned(),
        (RemoteObjectType::Function | RemoteObjectType::Symbol, _) | (_, None) => {
            value.description.unwrap_or_default()
        }
        (_, Some(serde_json::Value::String(text))) => text,
        (_, Some(other)) => other.to_string(),
    }
}

/// What a thrown exception says: the error's own description, stack and
/// all, or what was thrown.
fn threw(details: ExceptionDetails) -> String {
    match details.exception {
        Some(RemoteObject {
            description: Some(description),
            ..
        }) => description,
        Some(thrown) => format!("{}: {}", details.text, shown(thrown)),
        None => details.text,
    }
}

/// `text` cut to [`VALUE_MAX`] bytes on a character boundary, and how long it
/// was when it was cut.
fn capped(mut text: String) -> (String, Option<u64>) {
    let full = text.len();
    if full <= VALUE_MAX {
        return (text, None);
    }
    let mut end = VALUE_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, u64::try_from(full).ok())
}

/// The latest `lines` of what a page logged since `since`, errors only if
/// asked, each with its time on this Mac's clock.
fn console_lines(
    logged: Vec<ConsoleEntry>,
    errors_only: bool,
    since: Option<u64>,
    lines: u32,
) -> Vec<ConsoleLine> {
    let mut kept: Vec<ConsoleLine> = logged
        .into_iter()
        .filter(|entry| !errors_only || entry.level == "error")
        .filter(|entry| since.is_none_or(|since| entry.at_ms >= since))
        .map(|entry| ConsoleLine {
            time: clock(entry.at_ms),
            at_ms: entry.at_ms,
            level: entry.level,
            text: entry.text,
            source: entry.source,
            line: entry.line,
        })
        .collect();
    let excess = kept.len().saturating_sub(lines as usize);
    kept.drain(..excess);
    kept
}

/// `at_ms` as `HH:MM:SS` in this Mac's time zone.
fn clock(at_ms: u64) -> String {
    i64::try_from(at_ms)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at_ms: u64, level: &str, line: u32) -> ConsoleEntry {
        ConsoleEntry {
            at_ms,
            level: level.to_owned(),
            text: format!("message {line}"),
            source: "app.js".to_owned(),
            line,
        }
    }

    #[test]
    fn the_console_keeps_the_latest_of_what_was_asked_for() {
        let logged = vec![
            entry(1_000, "error", 1),
            entry(2_000, "info", 2),
            entry(3_000, "error", 3),
            entry(4_000, "error", 4),
            entry(5_000, "warning", 5),
        ];
        let lines = |errors_only, since, most| {
            console_lines(logged.clone(), errors_only, since, most)
                .into_iter()
                .map(|line| line.line)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(false, None, 200), [1, 2, 3, 4, 5]);
        assert_eq!(lines(true, None, 200), [1, 3, 4]);
        assert_eq!(lines(true, Some(2_500), 200), [3, 4]);
        assert_eq!(lines(false, None, 2), [4, 5]);
        assert_eq!(clock(0).len(), "00:00:00".len());
    }

    #[test]
    fn each_action_takes_handles_in_a_group_of_its_own_and_lets_go_of_only_that() {
        let (click, wait) = (Objects::new(), Objects::new());
        assert_ne!(click.group(), wait.group());
        assert!(click.0.starts_with("ket-browser-") && wait.0.starts_with("ket-browser-"));
        // Letting go of the wait's handles names the wait's group, and only
        // it: the click's stay.
        assert_eq!(Some(wait.release().object_group), wait.group());
        assert_ne!(Some(wait.release().object_group), click.group());
        // However many actions run, none shares a group.
        let groups: std::collections::HashSet<String> =
            (0..1_000).map(|_| Objects::new().0).collect();
        assert_eq!(groups.len(), 1_000);
        assert!(!groups.contains(&click.0) && !groups.contains(&wait.0));
    }

    #[test]
    fn a_long_value_is_cut_on_a_character_boundary_and_says_how_long_it_was() {
        assert_eq!(capped("short".to_owned()), ("short".to_owned(), None));
        let (cut, from) = capped("é".repeat(VALUE_MAX));
        assert!(cut.len() <= VALUE_MAX && cut.chars().all(|c| c == 'é'));
        assert_eq!(from, Some(2 * VALUE_MAX as u64));
    }
}
