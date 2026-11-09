//! `click` and `fill`: the page acted on with the DevTools protocol's own
//! mouse and text input, whose events the page cannot tell from a person's
//! — `isTrusted`, focus moving, a framework's change handlers — as `eval`'s
//! `.click()` cannot.
//!
//! An element is aimed at by ref, from a snapshot — in the page or in a frame
//! from its site — or by CSS selector, in the page itself. A click goes
//! where a person's would: the element is scrolled into view, its centre
//! found, and the page asked what is at that point; if it is not the
//! element or inside it, the click would land on whatever covers it, and
//! the terminal is told what that is instead. Chromium gives a frame's
//! element's box in the page's viewport, and looks for what is at a point
//! there through every frame from the page's site, so a frame's element is
//! aimed at as the page's own are. An `<iframe>` aimed at is clicked into:
//! what is at the point may be in its own document or a frame inside it,
//! but not in another frame laid over it. A click — or a key, see
//! `typing` — that starts loading another page waits for it, as `open`
//! does.

use std::time::{Duration, Instant};

use gpui::FutureExt as _;
use ket_chromium::CdpEvent;
use ket_chromium::protocol::browser_protocol::dom::{
    BackendNodeId, DescribeNodeParams, FocusParams, GetContentQuadsParams,
    GetNodeForLocationParams, Quad, ResolveNodeParams, ScrollIntoViewIfNeededParams,
};
use ket_chromium::protocol::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, InsertTextParams, MouseButton,
};
use ket_chromium::protocol::browser_protocol::page::{
    EnableParams, FrameTree, GetLayoutMetricsParams,
};
use ket_chromium::protocol::js_protocol::runtime::{
    CallArgument, CallFunctionOnParams, EvaluateParams, RemoteObjectId,
};
use ket_core::host::wire::BrowserTarget;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast::{self, error::RecvError};

use super::{Answer, LOOK_WAIT, Page, Spot, left, refs, threw};

/// How long after a click or a key the page is given to start loading
/// another: a link's is at once, a script's soon after.
const NAVIGATION_GRACE: Duration = Duration::from_millis(300);

/// What an element is, for what may be done to it.
const PROBE: &str = r"function () {
    const el = this.nodeType === Node.ELEMENT_NODE ? this : this.parentElement;
    if (!this.isConnected || !el) return { connected: false };
    const tag = el.localName;
    let kind = 'none';
    if (tag === 'select') kind = 'select';
    else if (tag === 'textarea') kind = 'text';
    else if (tag === 'input') {
        const type = el.type;
        if (type === 'checkbox' || type === 'radio') kind = type;
        else if (['date', 'datetime-local', 'month', 'time', 'week', 'range', 'color'].includes(type)) kind = 'value';
        else if (['button', 'submit', 'reset', 'image', 'file', 'hidden'].includes(type)) kind = 'none';
        else kind = 'text';
    } else if (el.isContentEditable) kind = 'text';
    return {
        connected: true,
        kind,
        frame: tag === 'iframe' || tag === 'frame',
        disabled: el.matches(':disabled'),
        readonly: !!el.readOnly,
    };
}";

/// Whether the node at the point a click aims at — the argument — is
/// something other than the element, something inside it, or the label of a
/// field it is.
const COVERED: &str = r"function (hit) {
    const me = this.nodeType === Node.ELEMENT_NODE ? this : this.parentElement;
    for (let n = hit; n; n = n.parentNode || n.host) if (n === me) return false;
    const at = hit.nodeType === Node.ELEMENT_NODE ? hit : hit.parentElement;
    const label = at && at.closest('label');
    return !(label && label.control === me);
}";

/// What a node — `this` — is, as the terminal names it: `div.modal-backdrop`.
const NAME: &str = r"function () {
    const at = this.nodeType === Node.ELEMENT_NODE ? this : this.parentElement;
    if (!at) return 'something else';
    let name = at.localName;
    if (at.id) name += '#' + at.id;
    else for (const c of [...at.classList].slice(0, 2)) name += '.' + c;
    return name;
}";

/// Deletes the selection, as deleting it by hand does, input events and
/// all.
const DELETE: &str = "function () { this.ownerDocument.execCommand('delete'); }";

/// Selects what a field holds, for typing to replace.
const SELECT_ALL: &str = r"function () {
    if ((this.localName === 'input' || this.localName === 'textarea') && this.select) {
        this.select();
        return;
    }
    const range = document.createRange();
    range.selectNodeContents(this);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
}";

/// Picks a `<select>`'s option by its text or value, as choosing it would.
const PICK: &str = r"function (want) {
    const plain = (text) => text.replace(/\s+/g, ' ').trim();
    const options = [...this.options];
    const option = options.find((o) => plain(o.label) === plain(want))
        || options.find((o) => plain(o.text) === plain(want))
        || options.find((o) => o.value === want);
    if (!option) return { options: options.map((o) => plain(o.label) || o.value) };
    if (option.disabled) return { disabled: true };
    for (const o of options) o.selected = o === option;
    this.dispatchEvent(new Event('input', { bubbles: true }));
    this.dispatchEvent(new Event('change', { bubbles: true }));
    return { picked: true };
}";

/// Sets an input no keyboard types into — a date, a colour, a range — as
/// its picker would; `false` if it did not take the value.
const SET_VALUE: &str = r"function (value) {
    this.focus();
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(this, value);
    if (value !== '' && this.value === '') return false;
    this.dispatchEvent(new Event('input', { bubbles: true }));
    this.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
}";

/// An element an action is aimed at.
pub(super) struct Aimed {
    /// The page's handle on it, in the action's group.
    pub object: RemoteObjectId,
    /// How the terminal names it: `@e6`, `"#save"`.
    pub named: String,
    /// The frame whose document it is in; `None` for the page's own.
    pub frame: Option<String>,
}

/// What [`PROBE`] says of an element.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Probe {
    connected: bool,
    kind: String,
    frame: bool,
    disabled: bool,
    readonly: bool,
}

/// Where on the page a point is, in CSS pixels from the top left of the
/// viewport — where the protocol's mouse events go — and how tall what it is
/// the centre of shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Point {
    pub x: f64,
    pub y: f64,
    pub height: f64,
}

/// What a ref from before the page changed says.
pub(super) fn stale(target: &BrowserTarget) -> String {
    format!("{target} is from before the page changed; run ket browser snapshot again")
}

/// Why the node a ref names could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Unresolved {
    /// The page didn't say which document it shows.
    Unplaced,
    /// The ref names nothing in that document: it is from another.
    Stale,
    /// The page didn't hand the node over, saying why — or didn't answer.
    Refused(String),
}

impl Page<'_> {
    /// The page's handle on the node ref `n` names, in the document the
    /// refs are for, and the frame whose document it is in.
    pub(super) async fn resolve(&self, n: u32) -> Result<(RemoteObjectId, String), Unresolved> {
        let loader = self.spot().await.ok_or(Unresolved::Unplaced)?.loader;
        let node = refs::node(self.refs, n, &loader).ok_or(Unresolved::Stale)?;
        // From the page's protocol, a node in one of its frames as much as
        // its own: they share a process, and its ids.
        let resolve = ResolveNodeParams {
            backend_node_id: Some(BackendNodeId::new(node.node)),
            object_group: self.objects.group(),
            ..ResolveNodeParams::default()
        };
        let resolved = self.ask(resolve).await.map_err(Unresolved::Refused)?;
        let object = resolved
            .object
            .object_id
            .ok_or_else(|| Unresolved::Refused("no handle on the node".to_owned()))?;
        Ok((object, node.frame))
    }

    /// The element `target` names: by ref, in the document the refs are
    /// for, or the first a selector matches.
    pub(super) async fn aim(&self, target: &BrowserTarget) -> Result<Aimed, String> {
        let named = target.to_string();
        let mut frame = None;
        let object = match target {
            BrowserTarget::Ref(n) => {
                let (object, of) = self.resolve(*n).await.map_err(|_| stale(target))?;
                frame = Some(of);
                object
            }
            BrowserTarget::Selector(selector) => {
                let literal = serde_json::to_string(selector).map_err(|error| error.to_string())?;
                let mut find = EvaluateParams::new(format!("document.querySelector({literal})"));
                find.object_group = self.objects.group();
                let found = self.ask(find).await?;
                if found.exception_details.is_some() {
                    return Err(format!("{named} is not a CSS selector"));
                }
                found
                    .result
                    .object_id
                    .ok_or_else(|| format!("no element matches {named}"))?
            }
        };
        Ok(Aimed {
            object,
            named,
            frame,
        })
    }

    /// Calls `function` on `object` with `args`, for what it returns, as
    /// JSON.
    pub(super) async fn call_on(
        &self,
        object: &RemoteObjectId,
        function: &str,
        args: Vec<CallArgument>,
    ) -> Result<Value, String> {
        let mut call = CallFunctionOnParams::new(function);
        call.object_id = Some(object.clone());
        call.arguments = Some(args);
        call.return_by_value = Some(true);
        call.object_group = self.objects.group();
        let answer = self.ask(call).await?;
        if let Some(thrown) = answer.exception_details {
            return Err(threw(thrown));
        }
        Ok(answer.result.value.unwrap_or(Value::Null))
    }

    /// What `aimed` is, if it is still on the page.
    async fn probe(&self, aimed: &Aimed, target: &BrowserTarget) -> Result<Probe, String> {
        let probe: Probe =
            serde_json::from_value(self.call_on(&aimed.object, PROBE, Vec::new()).await?)
                .unwrap_or_default();
        if !probe.connected {
            return Err(match target {
                BrowserTarget::Ref(_) => stale(target),
                BrowserTarget::Selector(_) => format!("{} has left the page", aimed.named),
            });
        }
        Ok(probe)
    }

    /// Scrolls `aimed` into view and finds the centre of what of it shows.
    pub(super) async fn centre(&self, aimed: &Aimed) -> Result<Point, String> {
        let unseen = || format!("{} isn't visible on the page", aimed.named);
        let into_view = ScrollIntoViewIfNeededParams {
            object_id: Some(aimed.object.clone()),
            ..ScrollIntoViewIfNeededParams::default()
        };
        self.ask(into_view).await.map_err(|_| unseen())?;
        let quads = GetContentQuadsParams {
            object_id: Some(aimed.object.clone()),
            ..GetContentQuadsParams::default()
        };
        let quads = self.ask(quads).await.map_err(|_| unseen())?.quads;
        let view = self
            .ask(GetLayoutMetricsParams::default())
            .await?
            .css_layout_viewport;
        visible_centre(&quads, view.client_width as f64, view.client_height as f64)
            .ok_or_else(unseen)
    }

    /// Fails, saying what, when something other than `aimed` is on top at
    /// `at`, where a click on it would land.
    async fn uncovered(&self, aimed: &Aimed, at: Point, probe: &Probe) -> Result<(), String> {
        let unreached = || format!("{} can't be reached where it shows", aimed.named);
        // The page is asked by where the point is in the document, not in
        // the viewport: scrolled down, those differ by how far.
        let view = self
            .ask(GetLayoutMetricsParams::default())
            .await?
            .css_layout_viewport;
        let mut look = GetNodeForLocationParams::new(
            (at.x + view.page_x as f64).floor() as i64,
            (at.y + view.page_y as f64).floor() as i64,
        );
        look.include_user_agent_shadow_dom = Some(false);
        let hit = self.ask(look).await.map_err(|_| unreached())?;
        let tree = self.frame_tree().await;
        let (at_frame, mine) = (hit.frame_id.inner(), aimed.frame.as_deref());
        let mut landed = landing(at_frame, mine, None, tree.as_ref());
        if probe.frame && landed != Landing::Here {
            // An `<iframe>`'s own frame, whose document is inside it.
            let describe = DescribeNodeParams {
                object_id: Some(aimed.object.clone()),
                ..DescribeNodeParams::default()
            };
            let described = self.ask(describe).await.map_err(|_| unreached())?;
            let inner = described.node.frame_id.map(|frame| frame.inner().clone());
            landed = landing(at_frame, mine, inner.as_deref(), tree.as_ref());
        }
        match landed {
            Landing::Inside => return Ok(()),
            Landing::Iframe => return Err(format!("{} is covered by an iframe", aimed.named)),
            Landing::Here | Landing::Page => {}
        }
        let resolve = ResolveNodeParams {
            backend_node_id: Some(hit.backend_node_id),
            object_group: self.objects.group(),
            ..ResolveNodeParams::default()
        };
        let hit = self
            .ask(resolve)
            .await?
            .object
            .object_id
            .ok_or_else(unreached)?;
        let argument = CallArgument {
            object_id: Some(hit.clone()),
            ..CallArgument::default()
        };
        if landed == Landing::Here
            && self.call_on(&aimed.object, COVERED, vec![argument]).await? != Value::Bool(true)
        {
            return Ok(());
        }
        let cover = self.call_on(&hit, NAME, Vec::new()).await?;
        let cover = cover.as_str().unwrap_or("something else");
        Err(format!("{} is covered by {cover}", aimed.named))
    }

    /// Clicks `target`, twice or with the right button if asked, and says
    /// where the page is afterwards.
    pub(super) async fn click(&self, target: &BrowserTarget, double: bool, right: bool) -> Answer {
        let aimed = self.aim(target).await?;
        let probe = self.probe(&aimed, target).await?;
        let at = self.centre(&aimed).await?;
        self.uncovered(&aimed, at, &probe).await?;
        let (button, buttons) = if right {
            (MouseButton::Right, 2)
        } else {
            (MouseButton::Left, 1)
        };
        let from = self.spot().await;
        let watching = self.watch().await;
        self.mouse(
            DispatchMouseEventType::MouseMoved,
            at,
            MouseButton::None,
            0,
            0,
        )
        .await?;
        for clicks in 1..=(if double { 2 } else { 1 }) {
            self.mouse(
                DispatchMouseEventType::MousePressed,
                at,
                button.clone(),
                buttons,
                clicks,
            )
            .await?;
            self.mouse(
                DispatchMouseEventType::MouseReleased,
                at,
                button.clone(),
                0,
                clicks,
            )
            .await?;
        }
        self.landed(from, watching).await
    }

    /// Puts `text` in field `target` in place of what it holds, or picks a
    /// `<select>`'s option, and says where the page is.
    pub(super) async fn fill(&self, target: &BrowserTarget, text: &str) -> Answer {
        let aimed = self.aim(target).await?;
        let probe = self.probe(&aimed, target).await?;
        let named = &aimed.named;
        if probe.disabled {
            return Err(format!("{named} is disabled"));
        }
        match probe.kind.as_str() {
            "checkbox" => {
                return Err(format!(
                    "{named} is a checkbox; `ket browser click {named}` ticks or unticks it"
                ));
            }
            "radio" => {
                return Err(format!(
                    "{named} is a radio button; `ket browser click {named}` picks it"
                ));
            }
            "text" | "value" if probe.readonly => return Err(format!("{named} is read-only")),
            "select" => self.pick(&aimed, text).await?,
            "value" => {
                let set = self
                    .call_on(&aimed.object, SET_VALUE, vec![value(text)])
                    .await?;
                if set != Value::Bool(true) {
                    return Err(format!("{named} doesn't take \"{text}\""));
                }
            }
            "text" => {
                let focus = FocusParams {
                    object_id: Some(aimed.object.clone()),
                    ..FocusParams::default()
                };
                self.ask(focus)
                    .await
                    .map_err(|_| format!("{named} can't take the focus"))?;
                self.call_on(&aimed.object, SELECT_ALL, Vec::new()).await?;
                if text.is_empty() {
                    self.call_on(&aimed.object, DELETE, Vec::new()).await?;
                } else {
                    self.ask(InsertTextParams::new(text)).await?;
                }
            }
            _ => return Err(format!("{named} is not something that can be filled")),
        }
        self.here(None).await
    }

    /// Picks `<select>` `aimed`'s option by its text or value.
    async fn pick(&self, aimed: &Aimed, want: &str) -> Result<(), String> {
        let picked = self.call_on(&aimed.object, PICK, vec![value(want)]).await?;
        if picked.get("disabled").is_some() {
            return Err(format!("{}'s option \"{want}\" is disabled", aimed.named));
        }
        let Some(options) = picked.get("options").and_then(Value::as_array) else {
            return Ok(());
        };
        let options: Vec<&str> = options.iter().filter_map(Value::as_str).collect();
        Err(format!(
            "{} has no option \"{want}\"; {}",
            aimed.named,
            options_text(&options)
        ))
    }

    /// Sends one mouse event at `at` — noting, in the same turn, a button
    /// it presses or lets go of: see `held`.
    pub(super) async fn mouse(
        &self,
        kind: DispatchMouseEventType,
        at: Point,
        button: MouseButton,
        buttons: i64,
        clicks: i64,
    ) -> Result<(), String> {
        self.held.borrow_mut().mouse(&kind, at, &button, clicks);
        self.ask(mouse_event(kind, at, button, buttons, clicks))
            .await
            .map(drop)
    }

    /// Starts listening for the page starting to load another, for
    /// [`Page::landed`], or opening a dialog: the page's events, with its
    /// domain on — which it stays, costing the page nothing it notices.
    pub(super) async fn watch(&self) -> broadcast::Receiver<CdpEvent> {
        let events = self.cdp.events();
        let _ = self
            .cdp
            .call(EnableParams::default())
            .with_timeout(LOOK_WAIT, self.executor)
            .await;
        events
    }

    /// Says where the page is once what was just done to it, at `from`,
    /// has had its effect: waiting for the next page as `open` does if it
    /// started loading one within [`NAVIGATION_GRACE`], at once if not.
    pub(super) async fn landed(
        &self,
        from: Option<Spot>,
        mut events: broadcast::Receiver<CdpEvent>,
    ) -> Answer {
        let frame = from.as_ref().map(|spot| spot.frame.clone());
        let deadline = Instant::now() + NAVIGATION_GRACE;
        let mut going = false;
        while !going {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match events.recv().with_timeout(remaining, self.executor).await {
                Ok(Ok(event)) => going = navigates(&event, frame.as_deref()),
                Ok(Err(RecvError::Lagged(_))) => {}
                Ok(Err(RecvError::Closed)) | Err(_) => break,
            }
        }
        let now = self.spot().await;
        // A page that moved before anything was heard has moved all the same.
        going |= matches!(
            (&from, &now),
            (Some(from), Some(now)) if from.loader != now.loader || from.url != now.url
        );
        if going {
            self.settle(left(from)).await
        } else {
            self.here(now).await
        }
    }
}

/// One mouse event at `at`, as the protocol sends it.
pub(super) fn mouse_event(
    kind: DispatchMouseEventType,
    at: Point,
    button: MouseButton,
    buttons: i64,
    clicks: i64,
) -> DispatchMouseEventParams {
    let mut event = DispatchMouseEventParams::new(kind, at.x, at.y);
    event.button = Some(button);
    event.buttons = Some(buttons);
    if clicks > 0 {
        event.click_count = Some(clicks);
    }
    event
}

/// Where a click on an element lands, by the frame whose document has what
/// is at the point the click aims at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Landing {
    /// In the element's own document: whether what is hit is the element,
    /// inside it or its label says.
    Here,
    /// In the `<iframe>` aimed at: its own document, or a frame inside it.
    Inside,
    /// In the page's own document, over a frame's element: what is hit
    /// there covers it.
    Page,
    /// In another frame's document, over the element.
    Iframe,
}

/// Where a click on an element in frame `mine` — the page's own if `None` —
/// lands when what is at its point is in frame `hit`. `inner` is the
/// element's own frame when it is an `<iframe>`, and `tree` the page's
/// frames, the main frame at the top.
pub(super) fn landing(
    hit: &str,
    mine: Option<&str>,
    inner: Option<&str>,
    tree: Option<&FrameTree>,
) -> Landing {
    let main = tree.map(|tree| tree.frame.id.inner().as_str());
    if mine.or(main).is_none_or(|mine| mine == hit) {
        return Landing::Here;
    }
    // Only by the frames' tree: an `<iframe>` laid over the one aimed at is
    // another frame, whatever site it is from.
    if let (Some(inner), Some(tree)) = (inner, tree)
        && find(tree, inner).is_some_and(|inner| find(inner, hit).is_some())
    {
        return Landing::Inside;
    }
    if main == Some(hit) {
        Landing::Page
    } else {
        Landing::Iframe
    }
}

/// Frame `id`, with the frames inside it, if it is `tree`'s or one inside
/// it.
fn find<'a>(tree: &'a FrameTree, id: &str) -> Option<&'a FrameTree> {
    let mut left = vec![tree];
    while let Some(next) = left.pop() {
        if next.frame.id.inner() == id {
            return Some(next);
        }
        left.extend(next.child_frames.iter().flatten());
    }
    None
}

/// `text` as an argument to a function called on the page.
fn value(text: &str) -> CallArgument {
    CallArgument {
        value: Some(Value::from(text)),
        ..CallArgument::default()
    }
}

/// Whether `event` says the page's main frame, `frame`, has started going
/// somewhere else — another document, or another place in this one. Not
/// knowing the frame, any frame's counts.
pub(super) fn navigates(event: &CdpEvent, frame: Option<&str>) -> bool {
    let params = &event.params;
    let id = match event.method.as_str() {
        "Page.frameRequestedNavigation" => {
            // A link to a new tab or a download leaves this page be.
            if params
                .get("disposition")
                .and_then(Value::as_str)
                .is_some_and(|disposition| disposition != "currentTab")
            {
                return false;
            }
            params.get("frameId")
        }
        "Page.frameStartedNavigating"
        | "Page.frameStartedLoading"
        | "Page.navigatedWithinDocument" => params.get("frameId"),
        "Page.frameNavigated" => params.get("frame").and_then(|frame| frame.get("id")),
        _ => return false,
    };
    match (frame, id.and_then(Value::as_str)) {
        (Some(frame), Some(id)) => frame == id,
        _ => true,
    }
}

/// The centre of the first of `quads` that shows in a viewport `width` by
/// `height`, cut to what of it is inside: where a click on it lands.
pub(super) fn visible_centre(quads: &[Quad], width: f64, height: f64) -> Option<Point> {
    quads.iter().find_map(|quad| {
        let points: Vec<(f64, f64)> = quad
            .inner()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[x, y]| (x.clamp(0.0, width), y.clamp(0.0, height)))
            .collect();
        if points.len() != 4 {
            return None;
        }
        // The shoelace formula: a quad squashed to a line has none.
        let area = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|((x1, y1), (x2, y2))| x1 * y2 - x2 * y1)
            .sum::<f64>()
            .abs()
            / 2.0;
        if area <= 1.0 {
            return None;
        }
        let (top, bottom) = points
            .iter()
            .fold((f64::MAX, f64::MIN), |(top, bottom), (_, y)| {
                (top.min(*y), bottom.max(*y))
            });
        Some(Point {
            x: points.iter().map(|(x, _)| x).sum::<f64>() / 4.0,
            y: points.iter().map(|(_, y)| y).sum::<f64>() / 4.0,
            height: bottom - top,
        })
    })
}

/// A `<select>`'s options, for saying it has none by the name asked for.
fn options_text(options: &[&str]) -> String {
    const SHOWN: usize = 10;
    if options.is_empty() {
        return "it has no options".to_owned();
    }
    let mut text = options
        .iter()
        .take(SHOWN)
        .map(|option| format!("\"{option}\""))
        .collect::<Vec<_>>()
        .join(", ");
    if options.len() > SHOWN {
        text.push_str(&format!(" and {} more", options.len() - SHOWN));
    }
    format!("it has {text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(method: &str, params: Value) -> CdpEvent {
        CdpEvent {
            method: method.to_owned(),
            params,
        }
    }

    #[test]
    fn the_main_frame_going_somewhere_is_a_navigation_and_nothing_else_is() {
        let main = Some("MAIN");
        let requested = |disposition: &str, frame: &str| {
            event(
                "Page.frameRequestedNavigation",
                serde_json::json!({"frameId": frame, "disposition": disposition, "url": "x"}),
            )
        };
        assert!(navigates(&requested("currentTab", "MAIN"), main));
        assert!(!navigates(&requested("newTab", "MAIN"), main));
        assert!(!navigates(&requested("download", "MAIN"), main));
        assert!(!navigates(&requested("currentTab", "CHILD"), main));
        assert!(navigates(&requested("currentTab", "CHILD"), None));
        let navigated = event(
            "Page.frameNavigated",
            serde_json::json!({"frame": {"id": "MAIN"}, "type": "Navigation"}),
        );
        assert!(navigates(&navigated, main));
        let within = event(
            "Page.navigatedWithinDocument",
            serde_json::json!({"frameId": "MAIN", "url": "x#y"}),
        );
        assert!(navigates(&within, main));
        assert!(!navigates(
            &event("Page.loadEventFired", serde_json::json!({"timestamp": 1})),
            main
        ));
    }

    #[test]
    fn a_click_lands_at_the_centre_of_what_shows() {
        let quad = |points: [f64; 8]| Quad::new(points.to_vec());
        // Wholly in view.
        assert_eq!(
            visible_centre(
                &[quad([10.0, 20.0, 110.0, 20.0, 110.0, 60.0, 10.0, 60.0])],
                800.0,
                600.0
            ),
            Some(Point {
                x: 60.0,
                y: 40.0,
                height: 40.0
            })
        );
        // Half below the fold: the half that shows.
        assert_eq!(
            visible_centre(
                &[quad([0.0, 500.0, 100.0, 500.0, 100.0, 700.0, 0.0, 700.0])],
                800.0,
                600.0
            ),
            Some(Point {
                x: 50.0,
                y: 550.0,
                height: 100.0
            })
        );
        // Out of view, then a second box that is in it: the second.
        let out = quad([0.0, 900.0, 10.0, 900.0, 10.0, 950.0, 0.0, 950.0]);
        let shown = quad([0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0]);
        assert_eq!(
            visible_centre(&[out.clone(), shown], 800.0, 600.0).map(|at| (at.x, at.y)),
            Some((5.0, 5.0))
        );
        assert_eq!(visible_centre(&[out], 800.0, 600.0), None);
        assert_eq!(visible_centre(&[], 800.0, 600.0), None);
    }

    /// A frame tree as the protocol gives it: `MAIN` with two frames laid
    /// over each other, `A` (with `A1` inside it) and `B`.
    fn frames() -> FrameTree {
        fn frame(id: &str, parent: Option<&str>, children: Vec<Value>) -> Value {
            serde_json::json!({
                "frame": {"id": id, "parentId": parent, "loaderId": "L", "url": "about:blank",
                          "domainAndRegistry": "", "securityOrigin": "://",
                          "mimeType": "text/html", "secureContextType": "InsecureScheme",
                          "crossOriginIsolatedContextType": "NotIsolated",
                          "gatedAPIFeatures": []},
                "childFrames": children
            })
        }
        let a = frame("A", Some("MAIN"), vec![frame("A1", Some("A"), Vec::new())]);
        let b = frame("B", Some("MAIN"), Vec::new());
        serde_json::from_value(frame("MAIN", None, vec![a, b]))
            .expect("a frame tree as the protocol gives it")
    }

    #[test]
    fn a_click_on_an_iframe_lands_in_it_and_not_in_one_laid_over_it() {
        let tree = frames();
        let tree = Some(&tree);
        // `<iframe>` A, in the page: its own document and the frame inside
        // it are inside it.
        let on_a = |hit| landing(hit, None, Some("A"), tree);
        assert_eq!(on_a("A"), Landing::Inside);
        assert_eq!(on_a("A1"), Landing::Inside);
        // B laid over it is another frame, same site or not.
        assert_eq!(on_a("B"), Landing::Iframe);
        // The page's own over it: what is there says whether it covers.
        assert_eq!(on_a("MAIN"), Landing::Here);
        assert_eq!(on_a("GONE"), Landing::Iframe);
        // `<iframe>` A1, in A's document: A's own over it is A's document,
        // the page's over it the page's, and B another frame.
        let on_a1 = |hit| landing(hit, Some("A"), Some("A1"), tree);
        assert_eq!(on_a1("A1"), Landing::Inside);
        assert_eq!(on_a1("A"), Landing::Here);
        assert_eq!(on_a1("MAIN"), Landing::Page);
        assert_eq!(on_a1("B"), Landing::Iframe);
        // B's own frame holds nothing of A's.
        assert_eq!(landing("A1", None, Some("B"), tree), Landing::Iframe);
        // An iframe whose own frame isn't known, or a page whose frames
        // aren't: nothing in another frame is inside it.
        assert_eq!(landing("A", None, None, tree), Landing::Iframe);
        assert_eq!(landing("A", Some("MAIN"), Some("A"), None), Landing::Iframe);
    }

    #[test]
    fn a_click_on_anything_else_lands_in_its_own_document_or_on_what_covers_it() {
        let tree = frames();
        let tree = Some(&tree);
        // An element of the page's.
        assert_eq!(landing("MAIN", None, None, tree), Landing::Here);
        assert_eq!(landing("A", None, None, tree), Landing::Iframe);
        // An element in frame A: the page's own over it covers it, A1's is
        // another frame — a frame inside the element is not the element.
        assert_eq!(landing("A", Some("A"), None, tree), Landing::Here);
        assert_eq!(landing("MAIN", Some("A"), None, tree), Landing::Page);
        assert_eq!(landing("A1", Some("A"), None, tree), Landing::Iframe);
        // Not knowing where anything is, what is hit says.
        assert_eq!(landing("X", None, None, None), Landing::Here);
    }

    #[test]
    fn a_missing_option_says_which_there_are() {
        assert_eq!(options_text(&[]), "it has no options");
        assert_eq!(options_text(&["Red", "Green"]), "it has \"Red\", \"Green\"");
        let many: Vec<String> = (1..=12).map(|n| n.to_string()).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(options_text(&many).ends_with("\"10\" and 2 more"));
        assert_eq!(
            stale(&BrowserTarget::Ref(6)),
            "@e6 is from before the page changed; run ket browser snapshot again"
        );
    }
}
