//! The right panel's Network view, and the tab a request opens as.
//!
//! What the network inspector logged for the worktree the panel is showing
//! (see `ket_core::network`): one row per request, newest first, with what
//! was sent and how much of it was new. A row opens the request as a
//! read-only tab — the size broken down into system prompt, tools, resent
//! and new, then the body as the log kept it.
//!
//! **Read once, then told.** A worktree's log is read from disk when the
//! panel first shows it, on a background thread. From then on the host
//! tells every window of each request as it is written down — see
//! `Reply::NetworkRequest` — and the row is added without reading the file
//! again. A body is read only when its tab opens.
//!
//! **Flags in the list.** A request whose new part carried something that
//! looks like an identifier or a secret (see `ket_core::network::flags`) is
//! tinted, with a tag naming what, so it is seen without being opened; the
//! context row counts them and filters to them. The tab marks each match.
//!
//! **One menu for the rest.** The context row's ⋯ holds what is done to the
//! whole log — read it again, show its folder, its settings, clear it — so
//! the row keeps only the count and the flag filter.
//!
//! **Virtualised.** A busy worktree logs a request per tool call, thousands
//! in the five days a log is kept, so the list is a `uniform_list`: only the
//! rows on screen are built, as the file tree's are.
//!
//! **Picked with the pointer.** The body is text, and reads as text: a drag
//! picks characters, a double-click the word under the pointer, a triple
//! the line, ⇧-click runs the pick on, and ⌘C takes what is picked. The
//! pick is kept as lines and bytes of the body, not rows, so it survives
//! Wrap Lines; the rows draw their part of it.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, ClickEvent, Context, DispatchPhase, FontWeight, HighlightStyle, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    StyledText, UniformListScrollHandle, Window, canvas, div, point, prelude::*, px, uniform_list,
};
use ket_core::id::WorktreeId;
use ket_core::network::flags::{self, Flag, Kind, Source};
use ket_core::network::store::{self, RequestSummary};
use ket_core::permissions::{self, Patterns, Rule, Verdict};
use ket_core::storage::format_bytes;
use ket_core::theme::Theme;

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::panel::PanelView;
use crate::tabs::{Tab, TabKind};
use crate::toasts::{Alert, AlertAction};
use crate::ui::WELL_XS;
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{caption, tinted_tag};
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::menu::{MenuEntry, MenuItem, MenuKey, OpenMenu, dropdown};
use crate::ui::row::sized_row;
use crate::ui::toast::Tone;

/// Height of one request row in the panel.
const ROW_HEIGHT: gpui::Pixels = px(26.0);

/// A request row's inset on the right: the context row's 6px plus the 4px
/// between the ⋯ button's edge and its glyph.
const ROW_INSET_RIGHT: gpui::Pixels = px(10.0);

/// Width of the ⋯ menu.
const MENU_W: gpui::Pixels = px(220.0);

/// How near the body's top or bottom a dragged pick starts it scrolling.
const AUTOSCROLL_EDGE: Pixels = px(28.0);

/// One frame of that scrolling.
const AUTOSCROLL_TICK: std::time::Duration = std::time::Duration::from_millis(16);

/// What the context row's ⋯ menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkAction {
    /// Reads the log from disk again.
    Reload,
    /// Shows the log's folder in the file manager.
    Reveal,
    /// Opens Settings › Network.
    Settings,
    /// Deletes the worktree's whole log.
    Clear,
}

/// Height of one line of a body in a request tab.
const LINE_HEIGHT: gpui::Pixels = px(18.0);

/// Size of a body's text in a request tab.
const BODY_TEXT: gpui::Pixels = px(11.0);

/// With wrap off, a body's line longer than this is cut on screen, with a
/// mark saying so: a file an agent read arrives as one line with its breaks
/// escaped, and shaping all of it to show its first screenful is waste.
const LINE_CAP: usize = 2_000;

/// The wrap width, in characters, until the body has been measured once,
/// and the least it is ever given.
const FALLBACK_COLUMNS: usize = 100;
const MIN_COLUMNS: usize = 16;

/// The Network view's state.
pub(crate) struct NetworkPanel {
    /// The worktree whose log `rows` are.
    worktree: Option<WorktreeId>,
    /// The log, newest first.
    rows: Vec<RequestSummary>,
    /// Whether the log has been read since `worktree` was set.
    read: bool,
    /// Whether a read is in flight.
    reading: bool,
    /// Requests told of while a read is in flight: newer than anything the
    /// file had when it was opened, so kept in front of what it returns.
    arrived: Vec<RequestSummary>,
    /// Stepped when the shown log is deleted, so a read in flight does not
    /// bring it back.
    generation: u64,
    /// Whether the list shows flagged requests only.
    flagged_only: bool,
    /// While it does, which of `rows` those are, as the list last drew them.
    shown: Option<Arc<Vec<usize>>>,
    /// Flagged requests each worktree's alert is counting, while it is up.
    alerted: HashMap<String, Tally>,
    /// The ⋯ menu, while it is open.
    pub(crate) menu: Option<OpenMenu<NetworkAction>>,
    /// Whether request tabs wrap their bodies' long lines. One setting for
    /// every tab, so turning it off sticks from one request to the next.
    pub(crate) wrap: bool,
    scroll: UniformListScrollHandle,
}

impl Default for NetworkPanel {
    fn default() -> Self {
        Self {
            worktree: None,
            rows: Vec::new(),
            read: false,
            reading: false,
            arrived: Vec::new(),
            generation: 0,
            flagged_only: false,
            shown: None,
            alerted: HashMap::new(),
            menu: None,
            wrap: true,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

/// What one worktree's alert has counted since it went up.
#[derive(Default)]
struct Tally {
    requests: usize,
    kinds: std::collections::BTreeSet<Kind>,
}

/// Which request a tab shows: the worktree its log is in, and its id there.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RequestKey {
    pub worktree: WorktreeId,
    pub id: String,
}

/// One request tab's state.
pub(crate) struct RequestState {
    /// The request's line of the log, once read.
    summary: Option<RequestSummary>,
    /// The body as kept, whole, for Copy.
    body: SharedString,
    /// The body as kept, by line, for the list to draw.
    lines: Arc<Vec<SharedString>>,
    /// What is flagged on each of `lines`; empty for a request with no flags.
    marks: Arc<Marks>,
    /// Where what was flagged came from.
    sources: Vec<Source>,
    /// Who issued each flagged key, and where it is revoked.
    issuers: Vec<(&'static str, &'static str)>,
    /// The checkout the request's worktree is, for the paths its tools read.
    root: Option<PathBuf>,
    /// Files this tab has already stopped agents reading.
    refused: Vec<String>,
    error: Option<SharedString>,
    reading: bool,
    /// Stepped when its log is deleted — see [`NetworkPanel::generation`].
    generation: u64,
    /// How many characters fit across the body, once it has been measured.
    columns: Option<usize>,
    /// `lines` cut to fit, for the column count they were cut at.
    wrapped: Option<(usize, Arc<Vec<Segment>>)>,
    /// A line, and the byte in it, to scroll to once the rows it is in are
    /// known.
    reveal: Option<(usize, usize)>,
    /// Text picked with the pointer, for ⌘C.
    pick: Option<Pick>,
    /// Where the pointer is while the button is held on the body: the pick
    /// follows it, and the body scrolls when it leaves the top or bottom.
    drag: Option<Point<Pixels>>,
    /// Whether that scrolling is running — see
    /// [`Shell::start_network_autoscroll`].
    autoscrolling: bool,
    /// The advance of one character of the body, as last drawn.
    cell_width: Pixels,
    scroll: UniformListScrollHandle,
}

impl Default for RequestState {
    fn default() -> Self {
        Self {
            summary: None,
            body: SharedString::default(),
            lines: Arc::default(),
            marks: Arc::default(),
            sources: Vec::new(),
            issuers: Vec::new(),
            root: None,
            refused: Vec::new(),
            error: None,
            reading: false,
            generation: 0,
            columns: None,
            wrapped: None,
            reveal: None,
            pick: None,
            drag: None,
            autoscrolling: false,
            cell_width: px(7.0),
            scroll: UniformListScrollHandle::new(),
        }
    }
}

impl RequestState {
    /// The wrapped rows for the body's width, cut again only when it changes.
    fn wrapped(&mut self) -> Arc<Vec<Segment>> {
        let columns = self.columns.unwrap_or(FALLBACK_COLUMNS);
        if let Some((cut_at, rows)) = &self.wrapped
            && *cut_at == columns
        {
            return rows.clone();
        }
        let mut rows = Vec::with_capacity(self.lines.len());
        for (index, line) in self.lines.iter().enumerate() {
            push_segments(index, line, columns, &mut rows);
        }
        let rows = Arc::new(rows);
        self.wrapped = Some((columns, rows.clone()));
        rows
    }

    /// The row showing byte `at` of `line`, as wrapped or not.
    fn row_of(&mut self, line: usize, at: usize, wrap: bool) -> usize {
        if !wrap {
            return line;
        }
        let rows = self.wrapped();
        let row = rows.partition_point(|(index, range)| {
            let index = *index as usize;
            index < line || (index == line && range.end <= at)
        });
        // Past the end only for a byte at the very end of its line.
        row.min(rows.len().saturating_sub(1))
    }

    /// Scrolls byte `at` of `line` into the middle of the body.
    fn scroll_to(&mut self, line: usize, at: usize, wrap: bool) {
        let row = self.row_of(line, at, wrap);
        self.scroll
            .scroll_to_item(row, gpui::ScrollStrategy::Center);
    }

    /// The line numbers' column: wide enough for the last line's.
    fn gutter(&self) -> Pixels {
        px(18.0 + 7.0 * self.lines.len().to_string().len() as f32)
    }

    /// The byte of the body under `at` in the window, as a line and a byte
    /// in it — the nearest one, with a pointer past the body's edges taken
    /// as at them. None before the body is drawn, or for an empty one.
    fn pos_at(&mut self, at: Point<Pixels>, wrap: bool) -> Option<Pos> {
        let (view, offset) = {
            let handle = &self.scroll.0.borrow().base_handle;
            (handle.bounds(), handle.offset())
        };
        if view.size.height <= px(0.0) {
            return None;
        }
        let y = at.y.max(view.top()).min(view.bottom() - px(1.0));
        let row = (f32::from(y - view.top() - offset.y) / f32::from(LINE_HEIGHT)).floor();
        let row = row.max(0.0) as usize;
        let x = f32::from(at.x - view.left() - self.gutter()) / f32::from(self.cell_width).max(1.0);
        let column = x.round().max(0.0) as usize;
        let (line, shown) = if wrap {
            let rows = self.wrapped();
            let (line, range) = rows.get(row.min(rows.len().checked_sub(1)?))?.clone();
            (line as usize, range)
        } else {
            let line = row.min(self.lines.len().checked_sub(1)?);
            (line, 0..cut_at(&self.lines[line]))
        };
        let text = &self.lines[line][shown.clone()];
        let at = text
            .char_indices()
            .nth(column)
            .map_or(text.len(), |(at, _)| at);
        Some((line, shown.start + at))
    }

    /// The text picked, with the body's own line breaks.
    fn picked_text(&self) -> Option<String> {
        let (start, end) = self.pick?.range();
        if start == end {
            return None;
        }
        let mut out = String::new();
        for line in start.0..=end.0 {
            let text = self.lines.get(line)?;
            let from = if line == start.0 { start.1 } else { 0 };
            let to = if line == end.0 { end.1 } else { text.len() };
            if line > start.0 {
                out.push('\n');
            }
            out.push_str(text.get(from..to).unwrap_or_default());
        }
        Some(out)
    }
}

/// A place in a body: a line, and a byte in it.
type Pos = (usize, usize);

/// Text picked in a body with the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pick {
    /// What the press took — a point, the word under it on a double-click,
    /// the line on a triple — kept whole so a drag back past it keeps it.
    anchor: (Pos, Pos),
    /// Where the pointer has run to since.
    head: Pos,
}

impl Pick {
    /// The bytes picked, first to last.
    fn range(&self) -> (Pos, Pos) {
        let (from, to) = self.anchor;
        (from.min(self.head), to.max(self.head))
    }

    /// Whether nothing is picked: a click that did not move.
    fn is_empty(&self) -> bool {
        let (from, to) = self.range();
        from == to
    }
}

/// The run of like characters around byte `at` of `line`, for a
/// double-click: an identifier, a run of punctuation or a run of spaces,
/// the classes every editor splits on. A click past the last character
/// takes that character's run.
fn word_at(line: &str, at: usize) -> Range<usize> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            2
        } else if c.is_whitespace() {
            0
        } else {
            1
        }
    };
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let Some(last) = chars.len().checked_sub(1) else {
        return 0..0;
    };
    let pivot = chars.partition_point(|(start, _)| *start < at).min(last);
    let kind = class(chars[pivot].1);
    let mut start = pivot;
    while start > 0 && class(chars[start - 1].1) == kind {
        start -= 1;
    }
    let mut end = pivot + 1;
    while end < chars.len() && class(chars[end].1) == kind {
        end += 1;
    }
    chars[start].0..chars.get(end).map_or(line.len(), |(start, _)| *start)
}

/// A row's part of `pick` — the bytes `shown` of line `index` — as bytes
/// of the row: from the pick's start on the line it starts on, to its end
/// on the line it ends on, and whole between. None for a row the pick does
/// not reach, including a later row of the line it ends on.
fn row_pick(index: usize, shown: &Range<usize>, pick: Option<(Pos, Pos)>) -> Option<Range<usize>> {
    let (from, to) = pick?;
    if index < from.0 || index > to.0 {
        return None;
    }
    let start = if index == from.0 { from.1 } else { shown.start };
    let end = if index == to.0 { to.1 } else { shown.end };
    let start = start.clamp(shown.start, shown.end) - shown.start;
    let end = end.clamp(shown.start, shown.end) - shown.start;
    (start < end).then_some(start..end)
}

/// `marked` and `picked` as the runs `StyledText` takes — in order, none
/// overlapping — with text that is both drawn as marked on the pick's
/// ground.
fn runs(
    marked: &[Range<usize>],
    picked: Option<&Range<usize>>,
    mark: HighlightStyle,
    pick: HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = marked
        .iter()
        .chain(picked)
        .flat_map(|range| [range.start, range.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|pair| {
            let (from, to) = (pair[0], pair[1]);
            let within = |range: &Range<usize>| range.start <= from && to <= range.end;
            let style = match (marked.iter().any(within), picked.is_some_and(within)) {
                (true, true) => HighlightStyle {
                    background_color: pick.background_color,
                    ..mark
                },
                (true, false) => mark,
                (false, true) => pick,
                (false, false) => return None,
            };
            Some((from..to, style))
        })
        .collect()
}

/// How far to scroll this frame for a drag whose pointer is at `y`: up when
/// it is near or above the body's top, down near or below its bottom,
/// faster the further out it is, and nothing between.
fn edge_step(state: &RequestState, y: Pixels) -> Pixels {
    let view = state.scroll.0.borrow().base_handle.bounds();
    let past = if y < view.top() + AUTOSCROLL_EDGE {
        y - (view.top() + AUTOSCROLL_EDGE)
    } else if y > view.bottom() - AUTOSCROLL_EDGE {
        y - (view.bottom() - AUTOSCROLL_EDGE)
    } else {
        return px(0.0);
    };
    let step = past / 3.0;
    if step < px(0.0) {
        step.max(px(-40.0)).min(px(-2.0))
    } else {
        step.min(px(40.0)).max(px(2.0))
    }
}

/// What is flagged on each line of a body, and as what.
type Marks = Vec<Vec<(Range<usize>, Kind)>>;

/// One row of a wrapped body: its line, and the bytes of it the row shows.
type Segment = (u32, Range<usize>);

/// Appends `line`'s rows, `columns` characters wide at most, to `out`. A row
/// breaks after the last space that fits, and mid-word only when a word is
/// wider than the body — as the editor wraps.
fn push_segments(index: usize, line: &str, columns: usize, out: &mut Vec<Segment>) {
    let columns = columns.max(1);
    let index = index as u32;
    let mut start = 0;
    loop {
        let mut limit = line.len();
        let mut after_space = None;
        for (count, (at, c)) in line[start..].char_indices().enumerate() {
            if count == columns {
                limit = start + at;
                break;
            }
            if c.is_whitespace() {
                after_space = Some(start + at + c.len_utf8());
            }
        }
        if limit == line.len() {
            out.push((index, start..limit));
            return;
        }
        let end = after_space.unwrap_or(limit);
        out.push((index, start..end));
        start = end;
    }
}

/// Where `line` is cut with wrap off: its end, or [`LINE_CAP`] brought back
/// to a character's start.
fn cut_at(line: &str) -> usize {
    if line.len() <= LINE_CAP {
        return line.len();
    }
    let mut cut = LINE_CAP;
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

/// `line` as a row with wrap off: whole, or cut at [`LINE_CAP`] with a mark
/// saying how much more there is, and the byte length shown of it.
fn cut_line(line: &SharedString) -> (SharedString, usize) {
    let cut = cut_at(line);
    if cut == line.len() {
        return (line.clone(), cut);
    }
    let text = format!(
        "{} … {} more characters on this line",
        &line[..cut],
        line[cut..].chars().count()
    );
    (text.into(), cut)
}

/// What a request tab reads in the background.
struct Read {
    body: SharedString,
    lines: Vec<SharedString>,
    marks: Marks,
    sources: Vec<Source>,
    issuers: Vec<(&'static str, &'static str)>,
    root: Option<PathBuf>,
}

/// All open request tabs' states, by the request each shows.
pub(crate) type Requests = HashMap<RequestKey, RequestState>;

/// The time of day a request was sent, to the second, on a 12-hour clock:
/// `3:04:12 PM`.
fn clock(at_ms: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at_ms as i64).map_or_else(
        String::new,
        |time| {
            time.with_timezone(&chrono::Local)
                .format("%-I:%M:%S %p")
                .to_string()
        },
    )
}

/// The model without its maker's prefix — `opus-5-5` — or, for a request
/// that names none, its path.
fn what(summary: &RequestSummary) -> String {
    if summary.model.is_empty() {
        return summary.path.clone();
    }
    summary
        .model
        .strip_prefix("claude-")
        .unwrap_or(&summary.model)
        .to_owned()
}

/// A row's flag tag: the two most serious kinds, and how many more.
fn flag_tag(flags: &[Flag]) -> String {
    let mut tag = flags
        .iter()
        .take(2)
        .map(|flag| flag.kind.label())
        .collect::<Vec<_>>()
        .join(" · ");
    if flags.len() > 2 {
        tag.push_str(&format!(" +{}", flags.len() - 2));
    }
    tag
}

/// What the tab for `summary` is called.
pub(crate) fn tab_title(summary: &RequestSummary) -> String {
    format!("{} {}", what(summary), clock(summary.at))
}

/// Every request tab open in `spaces`.
fn open_requests(spaces: &HashMap<WorktreeId, crate::tabs::Space>) -> Vec<RequestKey> {
    spaces
        .values()
        .flat_map(|space| space.all_tabs())
        .filter_map(|tab| match &tab.kind {
            TabKind::NetworkRequest(key) => Some(key.clone()),
            _ => None,
        })
        .collect()
}

impl Shell {
    /// Shows the Network view and reads it.
    pub(crate) fn open_network_panel(&mut self, cx: &mut Context<Self>) {
        self.sync_explorer(cx);
        self.panel_open = true;
        self.panel_view = PanelView::Network;
        self.persist_view();
        self.refresh_network_panel(cx);
    }

    /// Reads the shown worktree's log, if the view is on screen and has not.
    pub(crate) fn refresh_network_panel(&mut self, cx: &mut Context<Self>) {
        if !self.panel_open || self.panel_view != PanelView::Network || self.network_panel.reading {
            return;
        }
        let Some(worktree) = self.network_panel.worktree.clone() else {
            return;
        };
        self.network_panel.reading = true;
        self.network_panel.arrived.clear();
        let generation = self.network_panel.generation;
        let read_id = worktree.clone();
        let read = cx.background_executor().spawn(async move {
            let mut rows = store::summaries(read_id.as_str());
            rows.reverse();
            rows
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let rows = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.network_panel.reading = false;
                // The panel moved on to another worktree while this one was
                // being read, or its log was deleted: what came back is not
                // what it is showing.
                if shell.network_panel.worktree != Some(worktree)
                    || shell.network_panel.generation != generation
                {
                    return;
                }
                // What the file says, under what arrived while it was being
                // read. Nothing else is kept: a row the sweep has since
                // dropped goes with it, which is what Reload is for.
                // Pushed as they came, oldest first; the list is newest first.
                let mut arrived = std::mem::take(&mut shell.network_panel.arrived);
                arrived.reverse();
                shell.network_panel.rows = arrived
                    .into_iter()
                    .filter(|row| !rows.iter().any(|read| read.id == row.id))
                    .collect();
                shell.network_panel.rows.extend(rows);
                shell.network_panel.read = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds the requests the host has told of since the last call to the
    /// panel, when they are the worktree it shows, and raises an alert for
    /// any that was flagged, whichever worktree it is. Whether the panel
    /// changed.
    pub(crate) fn drain_network_requests(&mut self, cx: &mut Context<Self>) -> bool {
        let told = ket_core::host::take_network_requests();
        if told.is_empty() {
            return false;
        }
        for (worktree, summary) in &told {
            if !summary.flags.is_empty() {
                self.alert_flagged(worktree, summary, cx);
            }
        }
        let Some(shown) = self.network_panel.worktree.as_ref() else {
            return false;
        };
        let mut landed = false;
        for (worktree, summary) in told {
            if worktree != shown.as_str() {
                continue;
            }
            if self
                .network_panel
                .rows
                .iter()
                .any(|row| row.id == summary.id)
            {
                continue;
            }
            if self.network_panel.reading {
                self.network_panel.arrived.push(summary.clone());
            }
            self.network_panel.rows.insert(0, summary);
            landed = true;
        }
        landed
    }

    /// Warns that `summary`, a request from `worktree`, sent something
    /// flagged: an alert that stays until dismissed, one per worktree,
    /// counting the requests since it went up.
    fn alert_flagged(&mut self, worktree: &str, summary: &RequestSummary, cx: &mut Context<Self>) {
        let subject = SharedString::from(format!("network-flags-{worktree}"));
        // Dismissed since the last one: start counting again.
        if !self.alert_up(&subject) {
            self.network_panel.alerted.remove(worktree);
        }
        let tally = self
            .network_panel
            .alerted
            .entry(worktree.to_owned())
            .or_default();
        tally.requests += 1;
        tally
            .kinds
            .extend(summary.flags.iter().map(|flag| flag.kind));
        let requests = tally.requests;
        let kinds: Vec<Kind> = tally.kinds.iter().copied().collect();
        let to = provider(&summary.upstream);
        let message = match requests {
            1 => format!("Personal or secret data went to {to}"),
            n => format!("{n} requests sent personal or secret data to {to}"),
        };
        let detail = format!("{}: {}", self.worktree_name(worktree), kind_list(&kinds));
        let key = RequestKey {
            worktree: WorktreeId::new(worktree),
            id: summary.id.clone(),
        };
        let open: AlertAction =
            Rc::new(move |shell, cx| shell.show_network_request(key.clone(), cx));
        self.alert(
            Tone::Warning,
            message,
            detail,
            Alert {
                subject,
                action: Some(("Open request".into(), open)),
            },
            cx,
        );
    }

    /// Goes to `key`'s worktree and opens the request as a tab, with the
    /// Network view beside it.
    fn show_network_request(&mut self, key: RequestKey, cx: &mut Context<Self>) {
        let selection = self
            .projects
            .iter()
            .enumerate()
            .find_map(|(project, node)| {
                node.worktrees
                    .iter()
                    .position(|w| w.id == key.worktree)
                    .map(|worktree| crate::tree::Selection { project, worktree })
            });
        if let Some(selection) = selection {
            self.select(selection, cx);
        }
        self.open_network_panel(cx);
        self.open_network_request_tab(key, false, cx);
        cx.notify();
    }

    /// A worktree as the sidebar names it.
    fn worktree_name(&self, worktree: &str) -> String {
        self.projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|node| node.id.as_str() == worktree)
            .map_or_else(
                || worktree.to_owned(),
                |node| {
                    node.name
                        .clone()
                        .unwrap_or_else(|| node.branch.clone())
                        .to_string()
                },
            )
    }

    /// The Network view.
    pub(crate) fn network_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_explorer(cx);
        if self.network_panel.worktree != self.explorer.worktree {
            self.network_panel.worktree = self.explorer.worktree.clone();
            self.network_panel.rows.clear();
            self.network_panel.read = false;
        }
        if !self.network_panel.read {
            self.refresh_network_panel(cx);
        }

        let t = self.theme;
        let rows = &self.network_panel.rows;
        let flagged = rows.iter().filter(|row| !row.flags.is_empty()).count();
        let flagged_only = self.network_panel.flagged_only;
        // The count alone: what was sent is each row's, and summed it says
        // nothing anyone acts on.
        let totals: Option<SharedString> = match rows.len() {
            0 => None,
            1 => Some("1 request".into()),
            n => Some(format!("{n} requests").into()),
        };

        let context = self
            .panel_context_row(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .child(sized_icon(Icon::Network, px(12.0), paint(t.text.dim)))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(paint(t.text.primary))
                            .child(self.panel_worktree_name()),
                    ),
            )
            .children(totals.map(|totals| {
                div()
                    .flex_none()
                    .font_weight(FontWeight::NORMAL)
                    .whitespace_nowrap()
                    .child(totals)
            }))
            // How many were flagged, in red, and the switch that lists only
            // those: there while any are, or while the list is filtered.
            .children((flagged > 0 || flagged_only).then(|| {
                icon_button("network-flagged", Icon::TriangleAlert)
                    .bare()
                    .dense()
                    .count(flagged.to_string())
                    .pressed(flagged_only)
                    .tint(paint(t.status.failed))
                    .render(&t)
                    .text_color(paint(t.status.failed))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.network_panel.flagged_only = !this.network_panel.flagged_only;
                        this.network_panel
                            .scroll
                            .scroll_to_item(0, gpui::ScrollStrategy::Top);
                        cx.notify();
                    }))
            }))
            .child(self.network_more_button(cx));

        let body: AnyElement = if self.explorer.root.is_none() {
            notice("Select a worktree to see what its agents sent", None, &t).into_any_element()
        } else if !self.network_panel.read {
            notice("Reading…", None, &t).into_any_element()
        } else if flagged_only && flagged == 0 {
            notice(
                "Nothing flagged in this worktree",
                Some("Press the warning button above to list every request again."),
                &t,
            )
            .into_any_element()
        } else if self.network_panel.rows.is_empty() {
            notice(
                "Nothing logged for this worktree",
                Some("Claude Code and Codex, started while logging is on, are logged here."),
                &t,
            )
            .child(
                button("network-open-settings", "Open Network settings")
                    .small()
                    .leading(Icon::Sliders)
                    .render(&t)
                    .mt(px(6.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_preferences_at(crate::preferences::Section::Network, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
        } else {
            self.network_list(cx)
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(self.panel_strip(cx))
            .child(context)
            .child(body)
            .into_any_element()
    }

    /// The list of requests, virtualised.
    fn network_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        self.network_panel.shown = self.network_panel.flagged_only.then(|| {
            Arc::new(
                self.network_panel
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| !row.flags.is_empty())
                    .map(|(at, _)| at)
                    .collect(),
            )
        });
        let count = self
            .network_panel
            .shown
            .as_ref()
            .map_or(self.network_panel.rows.len(), |shown| shown.len());
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(paint(t.rule))
            .child(
                uniform_list(
                    "network-rows",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.network_rows(range, cx)),
                )
                .track_scroll(self.network_panel.scroll.clone())
                .flex_grow()
                .size_full(),
            )
            .into_any_element()
    }

    /// Wraps request tabs' long lines, or stops — from the tab's button,
    /// and from View › Wrap Lines while a request tab is in front.
    pub(crate) fn toggle_network_wrap(&mut self, cx: &mut Context<Self>) {
        self.network_panel.wrap = !self.network_panel.wrap;
        // Keep the line at the top of every open request tab in view across
        // the change, not only the one in front: each one's offset counts
        // rows, and the rows are about to mean something else.
        let wrap = self.network_panel.wrap;
        for state in self.network_requests.values_mut() {
            // Every row is one line tall, so the offset says which is on top.
            let offset = -f32::from(state.scroll.0.borrow().base_handle.offset().y);
            let top = (offset / f32::from(LINE_HEIGHT)).max(0.0) as usize;
            if top == 0 {
                continue;
            }
            let (line, at) = match &state.wrapped {
                // Off now, so it was wrapped: where that row starts.
                Some((_, rows)) if !wrap => rows
                    .get(top)
                    .map_or((top, 0), |(line, range)| (*line as usize, range.start)),
                _ => (top, 0),
            };
            let row = state.row_of(line, at, wrap);
            state
                .scroll
                .scroll_to_item_strict(row, gpui::ScrollStrategy::Top);
        }
        self.menu_dirty = true;
        cx.notify();
    }

    /// Whether a request tab is in front, and if so whether it wraps — for
    /// View › Wrap Lines.
    pub(crate) fn network_wrap_in_front(&self) -> Option<bool> {
        self.active_request().map(|_| self.network_panel.wrap)
    }

    /// The request a tab in front shows, if one is.
    fn active_request(&self) -> Option<RequestKey> {
        match &self.space()?.active_tab()?.kind {
            TabKind::NetworkRequest(key) => Some(key.clone()),
            _ => None,
        }
    }

    /// The rows `uniform_list` asked for, and only those.
    fn network_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = self.theme;
        let mono = self.font_family.clone();
        let Some(worktree) = self.network_panel.worktree.clone() else {
            return Vec::new();
        };
        let open = self.active_request();
        let shown = self.network_panel.shown.clone();

        range
            .filter_map(|index| {
                let index = match &shown {
                    Some(shown) => *shown.get(index)?,
                    None => index,
                };
                let row = self.network_panel.rows.get(index)?;
                let key = RequestKey {
                    worktree: worktree.clone(),
                    id: row.id.clone(),
                };
                let selected = open.as_ref() == Some(&key);
                let failed = row.status == 0 || row.status >= 400;
                // What this turn added, against what was sent again. A
                // request that carried on from nothing — the first, or a
                // conversation rewritten — was all new, and says so by its
                // colour rather than claiming an addition.
                let added: AnyElement = if row.continued {
                    div()
                        .text_color(paint(t.diff.added))
                        .child(format!("+{}", format_bytes(row.new_bytes as u64)))
                        .into_any_element()
                } else {
                    div()
                        .text_color(paint(t.text.dim))
                        .child(format_bytes(row.sent as u64))
                        .into_any_element()
                };
                let mark: Option<AnyElement> = if failed {
                    Some(
                        tinted_tag(
                            if row.status == 0 {
                                "no reply".to_owned()
                            } else {
                                row.status.to_string()
                            },
                            paint(t.status.failed),
                        )
                        .into_any_element(),
                    )
                } else if !row.complete {
                    Some(caption("cut off", &t).into_any_element())
                } else {
                    None
                };
                let flagged = !row.flags.is_empty();
                let flag = flagged.then(|| tinted_tag(flag_tag(&row.flags), paint(t.status.failed)));

                Some(
                    sized_row(("network-row", index), selected, ROW_HEIGHT, &t)
                        .when(flagged && !selected, |el| {
                            el.bg(alpha(paint(t.status.failed), 0.07))
                        })
                        .gap(px(8.0))
                        .w_full()
                        // In from the left as the worktree's name above it,
                        // and from the right as the ⋯ glyph's edge, so
                        // the sizes end under it.
                        .pl_3()
                        .pr(ROW_INSET_RIGHT)
                        .text_size(px(11.0))
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .text_size(px(10.5))
                                .text_color(paint(t.text.dim))
                                // Padded to the widest, `10:04:12 PM`, so the
                                // rows' columns line up in the mono face.
                                .child(format!("{:>11}", clock(row.at))),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(paint(t.text.primary))
                                .child(what(row)),
                        )
                        .child(div().flex_1())
                        .children(flag)
                        .children(mark)
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .gap(px(6.0))
                                .text_size(px(10.5))
                                .whitespace_nowrap()
                                .children(row.continued.then(|| {
                                    div()
                                        .text_color(paint(t.text.dim))
                                        .child(format_bytes(row.sent as u64))
                                }))
                                .child(added),
                        )
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            let preview = !matches!(event, ClickEvent::Mouse(click) if click.down.click_count >= 2);
                            this.open_network_request_tab(key.clone(), preview, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// Opens `key` as a tab in the panel's worktree, or brings its tab to
    /// the front. A preview tab for a single click, as a diff's is.
    pub(crate) fn open_network_request_tab(
        &mut self,
        key: RequestKey,
        preview: bool,
        cx: &mut Context<Self>,
    ) {
        let title = self
            .network_panel
            .rows
            .iter()
            .find(|row| row.id == key.id)
            .map_or_else(|| "Request".to_owned(), tab_title);
        self.load_network_request(&key, cx);
        let tab = Tab {
            title: title.into(),
            kind: TabKind::NetworkRequest(key.clone()),
            renamed: false,
            pinned: false,
            preview,
        };
        let space = self.spaces.entry(key.worktree).or_default();
        if preview {
            space.open_preview_tab(tab);
        } else {
            space.open_tab(tab);
        }
        self.prune_network_requests();
        self.persist_layout();
    }

    /// Reads `key`'s line of the log and its body, in the background.
    fn load_network_request(&mut self, key: &RequestKey, cx: &mut Context<Self>) {
        let state = self.network_requests.entry(key.clone()).or_default();
        if state.reading {
            return;
        }
        state.reading = true;
        let generation = state.generation;
        // The panel's row, when it has one, saves the summary being looked
        // for in the file; the body is on disk either way.
        let known = self
            .network_panel
            .rows
            .iter()
            .find(|row| row.id == key.id)
            .cloned();
        let read_key = key.clone();
        let read = cx.background_executor().spawn(async move {
            let summary = known.or_else(|| {
                store::summaries(read_key.worktree.as_str())
                    .into_iter()
                    .find(|row| row.id == read_key.id)
            });
            let body = store::body(read_key.worktree.as_str(), &read_key.id);
            let lines: Vec<SharedString> = body
                .as_deref()
                .unwrap_or_default()
                .lines()
                .map(|line| SharedString::from(line.to_owned()))
                .collect();
            // Found again in the body as it is shown, for the tab to mark:
            // only for a request the log flagged, so a clean one costs
            // nothing more.
            let flagged = summary
                .as_ref()
                .is_some_and(|summary| !summary.flags.is_empty());
            let marks: Marks = if flagged {
                lines.iter().map(|line| flags::find(line)).collect()
            } else {
                Vec::new()
            };
            // Where they came from, from the body as JSON: none for one cut
            // to fit, which no longer parses.
            let sources = flagged
                .then(|| {
                    body.as_deref()
                        .and_then(|body| serde_json::from_str(body).ok())
                })
                .flatten()
                .map(|kept| flags::sources(&kept))
                .unwrap_or_default();
            let mut issuers = Vec::new();
            for (line, ranges) in lines.iter().zip(&marks) {
                for (range, kind) in ranges {
                    if *kind == Kind::Secret
                        && let Some(issuer) = flags::issuer(&line[range.clone()])
                        && !issuers.contains(&issuer)
                    {
                        issuers.push(issuer);
                    }
                }
            }
            let root = flagged
                .then(|| Shell::worktree_root(&read_key.worktree))
                .flatten();
            let read = Read {
                body: body.unwrap_or_default().into(),
                lines,
                marks,
                sources,
                issuers,
                root,
            };
            (summary, read)
        });
        let key = key.clone();
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let (summary, read) = read.await;
            let _ = shell.update(cx, |shell, cx| {
                // Closed while it was being read.
                let Some(state) = shell.network_requests.get_mut(&key) else {
                    return;
                };
                state.reading = false;
                // Its log was deleted while it was being read: what came
                // back is gone, and the tab already says so.
                if state.generation != generation {
                    cx.notify();
                    return;
                }
                match summary {
                    Some(summary) => {
                        // Open on what was flagged, not the system prompt
                        // above it.
                        state.reveal = read
                            .marks
                            .iter()
                            .enumerate()
                            .find_map(|(line, marks)| Some((line, marks.first()?.0.start)));
                        state.wrapped = None;
                        state.pick = None;
                        state.drag = None;
                        state.summary = Some(summary);
                        state.body = read.body;
                        state.lines = Arc::new(read.lines);
                        state.marks = Arc::new(read.marks);
                        state.sources = read.sources;
                        state.issuers = read.issuers;
                        state.root = read.root;
                        state.error = None;
                    }
                    None => {
                        state.error = Some(
                            "This request is no longer in the log: it was older than five days, \
                             or the log was deleted."
                                .into(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Settings deleted `worktree`'s logs, or every worktree's for `None`:
    /// the panel showing them empties, and a request tab open on one says
    /// it is gone the next time it is read.
    pub(crate) fn network_logs_deleted(&mut self, worktree: Option<&str>) {
        let shown = self
            .network_panel
            .worktree
            .as_ref()
            .is_some_and(|shown| worktree.is_none_or(|gone| gone == shown.as_str()));
        if shown {
            self.network_panel.rows.clear();
            self.network_panel.arrived.clear();
            self.network_panel.generation += 1;
        }
        for (key, state) in &mut self.network_requests {
            if worktree.is_none_or(|gone| gone == key.worktree.as_str()) {
                state.generation += 1;
                state.summary = None;
                state.lines = Arc::default();
                state.marks = Arc::default();
                state.pick = None;
                state.drag = None;
                state.error = Some("This request's log was deleted.".into());
            }
        }
    }

    /// The context row's ⋯ button and, while open, its menu.
    fn network_more_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let trigger = icon_button("network-more", Icon::Ellipsis)
            .bare()
            .dense()
            .pressed(self.network_panel.menu.is_some())
            .render(&self.theme)
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_network_menu();
                cx.notify();
            }));
        let panel = self.network_panel.menu.as_ref().map(|menu| {
            menu.view(
                &self.theme,
                cx,
                |shell, action, cx| shell.run_network_action(*action, cx),
                |shell, cx| {
                    shell.network_panel.menu = None;
                    cx.notify();
                },
            )
        });
        dropdown("network-more-menu", trigger, panel)
            .flex_none()
            // The shell root closes every menu on a press; without this the
            // press that should close this one would close it on the way
            // down, and the click would open a fresh one.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }

    /// Opens the ⋯ menu, or closes it when it is already open.
    fn toggle_network_menu(&mut self) {
        if self.network_panel.menu.take().is_some() {
            return;
        }
        // One menu at a time: two open is two things claiming the keyboard.
        self.menu = None;
        self.worktree_menu = None;
        self.path_menu = None;
        self.project_menu = None;
        self.tucked_menu = None;
        self.sidebar_menu = None;
        self.popup = None;

        // What acts on the log is there only while there is one: an empty
        // worktree has no folder to show and nothing to clear.
        let logged = !self.network_panel.rows.is_empty();
        let enabled = |item: MenuItem<NetworkAction>, on: bool| {
            MenuEntry::Item(if on { item } else { item.disabled() })
        };
        let mut entries = vec![
            MenuEntry::Item(MenuItem::new(NetworkAction::Reload, "Reload").icon(Icon::Refresh)),
            MenuEntry::Separator,
        ];
        if let Some(name) = ket_core::surface::FILE_MANAGER {
            entries.push(enabled(
                MenuItem::new(NetworkAction::Reveal, format!("Show logs in {name}"))
                    .icon(Icon::Folder),
                logged,
            ));
        }
        entries.extend([
            MenuEntry::Item(
                MenuItem::new(NetworkAction::Settings, "Network settings\u{2026}")
                    .icon(Icon::Sliders),
            ),
            MenuEntry::Separator,
            enabled(
                MenuItem::new(NetworkAction::Clear, "Clear log")
                    .icon(Icon::Trash)
                    .subtitle(match self.network_panel.rows.len() {
                        1 => "1 request from this worktree".to_owned(),
                        n => format!("{n} requests from this worktree"),
                    })
                    .danger(),
                logged,
            ),
        ]);
        // Hung under the button with its right edge on the button's.
        self.network_panel.menu = Some(
            OpenMenu::new("network-more".into(), None, MENU_W, entries)
                .uncapped()
                .offset(WELL_XS - MENU_W, px(6.0)),
        );
    }

    /// Keys for the ⋯ menu while it is open. `true` when it took the key.
    pub(crate) fn network_menu_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.network_panel.menu.as_mut() else {
            return false;
        };
        match menu.key(event) {
            MenuKey::Ignored => return false,
            MenuKey::Consumed => {}
            MenuKey::Close => self.network_panel.menu = None,
            MenuKey::Run(action) => self.run_network_action(action, cx),
        }
        true
    }

    /// Runs one pick from the ⋯ menu.
    fn run_network_action(&mut self, action: NetworkAction, cx: &mut Context<Self>) {
        self.network_panel.menu = None;
        match action {
            NetworkAction::Reload => {
                self.network_panel.read = false;
                self.refresh_network_panel(cx);
            }
            NetworkAction::Reveal => {
                let dir = self
                    .network_panel
                    .worktree
                    .as_ref()
                    .and_then(|worktree| store::worktree_dir(worktree.as_str()));
                if let Some(dir) = dir
                    && let Err(error) = ket_core::surface::reveal_in_file_manager(&dir)
                {
                    let name = ket_core::surface::FILE_MANAGER.unwrap_or("the file manager");
                    self.toast_detail(
                        Tone::Error,
                        format!("Could not open in {name}"),
                        error.to_string(),
                        cx,
                    );
                }
            }
            NetworkAction::Settings => {
                self.open_preferences_at(crate::preferences::Section::Network, cx);
            }
            NetworkAction::Clear => self.clear_network_log(cx),
        }
        cx.notify();
    }

    /// Deletes the shown worktree's whole log, in the background, then
    /// empties the panel and any of its request tabs, as Settings' Delete
    /// does.
    fn clear_network_log(&mut self, cx: &mut Context<Self>) {
        let Some(worktree) = self.network_panel.worktree.as_ref() else {
            return;
        };
        let worktree = worktree.as_str().to_owned();
        let work = cx.background_executor().spawn({
            let worktree = worktree.clone();
            async move { store::remove_worktree(&worktree) }
        });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            work.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.network_logs_deleted(Some(&worktree));
                shell.network_logs_shown(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether `key`'s request, as read, had anything flagged.
    pub(crate) fn network_request_flagged(&self, key: &RequestKey) -> bool {
        self.network_requests
            .get(key)
            .and_then(|state| state.summary.as_ref())
            .is_some_and(|summary| !summary.flags.is_empty())
    }

    /// Drops the state of request tabs no longer open anywhere.
    pub(crate) fn prune_network_requests(&mut self) {
        if self.network_requests.is_empty() {
            return;
        }
        let open = open_requests(&self.spaces);
        self.network_requests.retain(|key, _| open.contains(key));
    }

    /// A request tab: what was flagged in it, and what to do about it, then
    /// the body as kept.
    pub(crate) fn network_request_pane(
        &mut self,
        key: &RequestKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let wrap = self.network_panel.wrap;
        if !self.network_requests.contains_key(key) {
            self.load_network_request(key, cx);
        }
        let Some(state) = self.network_requests.get(key) else {
            return div().into_any_element();
        };

        let Some(summary) = state.summary.as_ref() else {
            let text: SharedString = match &state.error {
                Some(error) => error.clone(),
                None => "Reading…".into(),
            };
            return div()
                .size_full()
                .child(crate::ui::picker::note(text, &t))
                .into_any_element();
        };
        let flagged = !summary.flags.is_empty();

        let copy_key = key.clone();
        let delete_key = key.clone();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.0))
            .h(px(44.0))
            .pl(px(16.0))
            .pr_3()
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(paint(t.text.primary))
                    .child(what(summary)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(paint(t.text.dim))
                    .child(clock(summary.at)),
            )
            .children(failure_tag(summary, &t))
            .child(div().flex_1())
            .child(
                icon_button("network-request-wrap", Icon::WrapText)
                    .bare()
                    .dense()
                    .pressed(wrap)
                    .render(&t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_network_wrap(cx);
                    })),
            )
            .child(
                icon_button("network-request-copy", Icon::Copy)
                    .bare()
                    .dense()
                    .render(&t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(state) = this.network_requests.get(&copy_key) {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                state.body.to_string(),
                            ));
                            this.toast(Tone::Success, "Request copied", cx);
                        }
                    })),
            )
            .child(
                icon_button("network-request-delete", Icon::Trash)
                    .bare()
                    .dense()
                    .render(&t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.delete_network_request(delete_key.clone(), cx);
                    })),
            );

        let alert = flagged.then(|| self.flag_alert(key, cx));

        let gutter = state.gutter();
        let mono = self.font_family.clone();
        let cell_width = crate::editor::code_cell_width(window, &mono, BODY_TEXT);
        let Some(state) = self.network_requests.get_mut(key) else {
            return div().into_any_element();
        };
        state.cell_width = cell_width;
        let wrapped = wrap.then(|| state.wrapped());
        // Wait for the real width before scrolling to what was flagged: the
        // rows cut at the fallback are not the rows that will be drawn.
        if let Some((line, at)) = state.reveal
            && (!wrap || state.columns.is_some())
        {
            state.reveal = None;
            state.scroll_to(line, at, wrap);
        }
        let Some(state) = self.network_requests.get(key) else {
            return div().into_any_element();
        };
        let Some(summary) = state.summary.as_ref() else {
            return div().into_any_element();
        };

        let body: AnyElement = if state.lines.is_empty() {
            crate::ui::picker::note(
                if summary.body {
                    "The body could not be read"
                } else {
                    "Nothing was kept for this request: it carried no body"
                },
                &t,
            )
            .into_any_element()
        } else {
            let lines = state.lines.clone();
            let marks = state.marks.clone();
            let pick = state.pick.map(|pick| pick.range());
            let dragging = state.drag.is_some();
            let count = wrapped.as_ref().map_or(lines.len(), |rows| rows.len());
            let list = uniform_list(
                SharedString::from(format!("network-body-{}", key.id)),
                count,
                cx.processor(move |this, range: Range<usize>, _, _| {
                    let t = this.theme;
                    let mono = this.font_family.clone();
                    let red = paint(t.status.failed);
                    let marked = HighlightStyle {
                        color: Some(red.into()),
                        background_color: Some(alpha(red, 0.18).into()),
                        ..Default::default()
                    };
                    let picked = HighlightStyle {
                        background_color: Some(paint(t.selection).into()),
                        ..Default::default()
                    };
                    range
                        .filter_map(|index| {
                            // The line this row is in, the text it shows, and
                            // the bytes of the line that text starts at and
                            // covers, for the marks.
                            let (line_index, text, shown) = match &wrapped {
                                Some(rows) => {
                                    let (line, range) = rows.get(index)?.clone();
                                    let text = lines.get(line as usize)?[range.clone()].to_owned();
                                    (line as usize, SharedString::from(text), range)
                                }
                                None => {
                                    let (text, end) = cut_line(lines.get(index)?);
                                    (index, text, 0..end)
                                }
                            };
                            let hit = marks.get(line_index).filter(|ranges| !ranges.is_empty());
                            let here: Vec<Range<usize>> = hit
                                .into_iter()
                                .flatten()
                                .filter(|(range, _)| {
                                    range.start < shown.end && range.end > shown.start
                                })
                                .map(|(range, _)| {
                                    let start = range.start.max(shown.start) - shown.start;
                                    let end = range.end.min(shown.end) - shown.start;
                                    start..end
                                })
                                .collect();
                            let chosen = row_pick(line_index, &shown, pick);
                            let highlights = runs(&here, chosen.as_ref(), marked, picked);
                            let text: AnyElement = if highlights.is_empty() {
                                text.into_any_element()
                            } else {
                                StyledText::new(text)
                                    .with_highlights(highlights)
                                    .into_any_element()
                            };
                            // A wrapped line's number is on its first row only.
                            let number = (shown.start == 0).then(|| (line_index + 1).to_string());
                            Some(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .h(LINE_HEIGHT)
                                    .pr_2()
                                    .font_family(mono.clone())
                                    .text_size(BODY_TEXT)
                                    .text_color(paint(t.text.primary))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex_none()
                                            .h_full()
                                            .w(gutter)
                                            .pr(px(12.0))
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .border_l_2()
                                            .border_color(match hit {
                                                Some(_) => red,
                                                None => gpui::transparent_black().into(),
                                            })
                                            .text_size(px(10.5))
                                            .text_color(match hit {
                                                Some(_) => red,
                                                None => alpha(paint(t.text.dim), 0.6),
                                            })
                                            .children(number),
                                    )
                                    .child(text)
                                    .into_any_element(),
                            )
                        })
                        .collect()
                }),
            )
            .track_scroll(state.scroll.clone())
            .flex_grow()
            .size_full();

            // Measures how wide the rows were painted, so the next frame
            // wraps at the body's real width rather than at the fallback.
            let measured = key.clone();
            let shell = cx.entity().downgrade();
            let probe = canvas(
                move |bounds, _, cx| {
                    let _ = shell.update(cx, |this, cx| {
                        // Less the gutter and the row's right inset.
                        let text_width = f32::from(bounds.size.width - gutter) - 8.0;
                        let columns = (text_width / f32::from(cell_width).max(1.0)).floor();
                        let columns = (columns.max(0.0) as usize).max(MIN_COLUMNS);
                        if let Some(state) = this.network_requests.get_mut(&measured)
                            && state.columns != Some(columns)
                        {
                            state.columns = Some(columns);
                            // Only when it moved, or this would repaint forever.
                            cx.notify();
                        }
                    });
                },
                |_, _: (), _, _| {},
            )
            .absolute()
            .size_full();

            // While a pick is dragged, the pointer is followed past the
            // body's edges too — the window's own moves, not the rows' — so
            // the pick runs on and the body scrolls when it leaves the top
            // or bottom.
            let follow = dragging.then(|| {
                let shell = cx.weak_entity();
                let key = key.clone();
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let (moved, moved_key) = (shell.clone(), key.clone());
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            let _ = moved.update(cx, |this, cx| {
                                let changed = match event.pressed_button {
                                    Some(MouseButton::Left) => {
                                        this.drag_network_pick(&moved_key, event.position, cx)
                                    }
                                    _ => this.end_network_drag(&moved_key),
                                };
                                if changed {
                                    cx.notify();
                                }
                            });
                        });
                        let (released, released_key) = (shell.clone(), key.clone());
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            let _ = released.update(cx, |this, cx| {
                                if this.end_network_drag(&released_key) {
                                    cx.notify();
                                }
                            });
                        });
                    },
                )
                .absolute()
                .size_full()
            });

            let (press_key, up_key) = (key.clone(), key.clone());
            div()
                .relative()
                .flex()
                .flex_grow()
                .size_full()
                .overflow_hidden()
                .cursor_text()
                .child(list)
                .child(probe)
                .children(follow)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.press_network_body(&press_key, event);
                        cx.notify();
                    }),
                )
                // Here as well as on the window: a quick click can let go
                // before the frame that starts listening there is drawn.
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                        if this.end_network_drag(&up_key) {
                            cx.notify();
                        }
                    }),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(header)
            .children(alert)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .when(flagged, |el| el.mt_3())
                    .pt(px(4.0))
                    .border_t_1()
                    .border_color(paint(t.rule))
                    .child(body),
            )
            .into_any_element()
    }

    /// A press on `key`'s body: a pick starts under the pointer — the word
    /// there on a double-click, the line on a triple — or, with ⇧, the pick
    /// runs to it. The pick then follows the pointer until the button comes
    /// up.
    fn press_network_body(&mut self, key: &RequestKey, event: &MouseDownEvent) {
        let wrap = self.network_panel.wrap;
        let Some(state) = self.network_requests.get_mut(key) else {
            return;
        };
        let Some(pos) = state.pos_at(event.position, wrap) else {
            return;
        };
        let (line, at) = pos;
        let anchor = match event.click_count {
            1 => (pos, pos),
            2 => {
                let word = word_at(&state.lines[line], at);
                ((line, word.start), (line, word.end))
            }
            _ => ((line, 0), (line, state.lines[line].len())),
        };
        state.pick = match state.pick {
            Some(mut pick) if event.modifiers.shift && event.click_count == 1 => {
                pick.head = pos;
                Some(pick)
            }
            _ => Some(Pick { anchor, head: pos }),
        };
        state.drag = Some(event.position);
    }

    /// The pointer moved with the button held: the pick runs to the byte
    /// under it, or to the body's edge when it is past one — and past it,
    /// the body scrolls to follow. Says whether the pick changed.
    fn drag_network_pick(
        &mut self,
        key: &RequestKey,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let wrap = self.network_panel.wrap;
        let Some(state) = self.network_requests.get_mut(key) else {
            return false;
        };
        if state.drag.is_none() {
            return false;
        }
        state.drag = Some(at);
        let before = state.pick;
        if let Some(pos) = state.pos_at(at, wrap)
            && let Some(pick) = state.pick.as_mut()
        {
            pick.head = pos;
        }
        let changed = state.pick != before;
        if edge_step(state, at.y) != px(0.0) {
            self.start_network_autoscroll(key.clone(), cx);
        }
        changed
    }

    /// The button came up, or a move arrived without it: the drag is over,
    /// and a pick that took nothing goes. Says whether there was a drag.
    fn end_network_drag(&mut self, key: &RequestKey) -> bool {
        let Some(state) = self.network_requests.get_mut(key) else {
            return false;
        };
        let dragging = state.drag.take().is_some();
        if state.pick.is_some_and(|pick| pick.is_empty()) {
            state.pick = None;
        }
        dragging
    }

    /// Scrolls `key`'s body a step each frame while a drag holds the
    /// pointer near or past its top or bottom. One loop per tab: a second
    /// call while one runs does nothing.
    fn start_network_autoscroll(&mut self, key: RequestKey, cx: &mut Context<Self>) {
        let Some(state) = self.network_requests.get_mut(&key) else {
            return;
        };
        if std::mem::replace(&mut state.autoscrolling, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL_TICK).await;
                let going = this
                    .update(cx, |this, cx| this.network_autoscroll_step(&key, cx))
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// One frame of [`Shell::start_network_autoscroll`]: scrolls, and runs
    /// the pick to the byte now under the pointer. Says whether to keep
    /// going.
    fn network_autoscroll_step(&mut self, key: &RequestKey, cx: &mut Context<Self>) -> bool {
        let wrap = self.network_panel.wrap;
        let Some(state) = self.network_requests.get_mut(key) else {
            return false;
        };
        let step = state.drag.map_or(px(0.0), |at| edge_step(state, at.y));
        let Some(at) = state.drag.filter(|_| step != px(0.0)) else {
            state.autoscrolling = false;
            return false;
        };
        {
            let handle = &state.scroll.0.borrow().base_handle;
            let (offset, max) = (handle.offset(), handle.max_offset());
            let y = (offset.y - step).max(-max.height).min(px(0.0));
            handle.set_offset(point(offset.x, y));
        }
        if let Some(pos) = state.pos_at(at, wrap)
            && let Some(pick) = state.pick.as_mut()
        {
            pick.head = pos;
        }
        cx.notify();
        true
    }

    /// Escape and ⌘C while text is picked in the request tab in front.
    pub(crate) fn network_request_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(key) = self.active_request() else {
            return false;
        };
        let Some(state) = self.network_requests.get_mut(&key) else {
            return false;
        };
        if state.pick.is_none() {
            return false;
        }
        let keys = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => {
                state.pick = None;
                state.drag = None;
                true
            }
            "c" if keys.platform || keys.control => self.copy_network_pick(cx),
            _ => false,
        }
    }

    /// ⌘C and Edit › Copy: the text picked in the request tab in front to
    /// the clipboard. Says whether there was any.
    pub(crate) fn copy_network_pick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.active_request() else {
            return false;
        };
        let Some(text) = self
            .network_requests
            .get(&key)
            .and_then(RequestState::picked_text)
        else {
            return false;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        true
    }

    /// Edit › Select All in a request tab: the whole body. Says whether a
    /// request tab was in front.
    pub(crate) fn select_network_body(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.active_request() else {
            return false;
        };
        let Some(state) = self.network_requests.get_mut(&key) else {
            return false;
        };
        if let Some(last) = state.lines.len().checked_sub(1) {
            let end = (last, state.lines[last].len());
            state.pick = Some(Pick {
                anchor: ((0, 0), end),
                head: (0, 0),
            });
            cx.notify();
        }
        true
    }

    /// What was flagged in `key`'s request, in a sentence, a chip a kind and
    /// where it came from, with what can be done about it.
    fn flag_alert(&self, key: &RequestKey, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let red = paint(t.status.failed);
        let Some(state) = self.network_requests.get(key) else {
            return div().into_any_element();
        };
        let Some(summary) = state.summary.as_ref() else {
            return div().into_any_element();
        };
        let found: usize = summary.flags.iter().map(|flag| flag.count).sum();
        let title = format!(
            "{found} {} personal or secret went to {}",
            if found == 1 {
                "thing that looks"
            } else {
                "things that look"
            },
            provider(&summary.upstream),
        );

        let chips = summary.flags.iter().map(|flag| {
            let serious = matches!(flag.kind, Kind::Secret | Kind::Ssn);
            let mut label = chip_label(flag.kind);
            if flag.count > 1 {
                label.push_str(&format!(" ×{}", flag.count));
            }
            let kind = flag.kind;
            let key = key.clone();
            div()
                .id(SharedString::from(format!("network-flag-{label}")))
                .flex()
                .flex_none()
                .items_center()
                .h(px(22.0))
                .px(px(9.0))
                .rounded(px(11.0))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(serious, |el| el.bg(red).text_color(paint(t.surface)))
                .when(!serious, |el| el.bg(alpha(red, 0.16)).text_color(red))
                .hover(|style| style.opacity(0.85))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    // To the first line carrying one of this kind.
                    let wrap = this.network_panel.wrap;
                    if let Some(state) = this.network_requests.get_mut(&key)
                        && let Some((line, at)) =
                            state.marks.iter().enumerate().find_map(|(line, marks)| {
                                let (range, _) = marks.iter().find(|(_, k)| *k == kind)?;
                                Some((line, range.start))
                            })
                    {
                        state.scroll_to(line, at, wrap);
                        cx.notify();
                    }
                }))
        });

        // "From fixtures/patients.csv and your message": a file is a link.
        let mono = self.font_family.clone();
        // Files by name, up to three, as links; everything else in words,
        // once each, so a turn of twenty commands is not twenty clauses.
        let files: Vec<&String> = state
            .sources
            .iter()
            .filter_map(|source| match source {
                Source::File(path) => Some(path),
                _ => None,
            })
            .collect();
        let mut named: Vec<AnyElement> = files
            .iter()
            .take(3)
            .map(|path| {
                let shown = relative(path, state.root.as_deref());
                let open = absolute(path, state.root.as_deref());
                div()
                    .id(SharedString::from(format!("network-source-{path}")))
                    .font_family(mono.clone())
                    .text_size(px(11.5))
                    .text_color(paint(t.text.primary))
                    .underline()
                    .cursor_pointer()
                    .child(shown)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_editor_tab(open.clone(), cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        if files.len() > 3 {
            named.push(
                div()
                    .child(format!("{} more files", files.len() - 3))
                    .into_any_element(),
            );
        }
        let commands = state
            .sources
            .iter()
            .filter(|source| matches!(source, Source::Command(_)))
            .count();
        let mut words: Vec<String> = Vec::new();
        for source in &state.sources {
            let said = match source {
                Source::File(_) => continue,
                Source::Command(_) if commands > 1 => format!("{commands} commands' output"),
                other => source_words(other),
            };
            if !words.contains(&said) {
                words.push(said);
            }
        }
        named.extend(
            words
                .into_iter()
                .map(|said| div().child(said).into_any_element()),
        );
        // "From a, b and c": a comma rides on the part before it, so the
        // gap between parts never opens up before one.
        let mut from: Vec<AnyElement> = Vec::new();
        let count = named.len();
        for (i, part) in named.into_iter().enumerate() {
            if i == 0 {
                from.push(div().child("From").into_any_element());
            } else if i + 1 == count {
                from.push(div().child("and").into_any_element());
            }
            from.push(
                div()
                    .flex()
                    .child(part)
                    .when(i + 2 < count, |el| el.child(","))
                    .into_any_element(),
            );
        }

        let project = self.project_of_worktree(&key.worktree);
        let mut actions: Vec<AnyElement> = Vec::new();
        for &path in files.iter().take(2) {
            let name = Path::new(path)
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
            if state.refused.contains(path) {
                actions.push(
                    button(
                        SharedString::from(format!("network-refused-{path}")),
                        format!("Agents can't read {name}"),
                    )
                    .small()
                    .leading(Icon::Check)
                    .enabled(false)
                    .render(&t)
                    .into_any_element(),
                );
                continue;
            }
            let Some(project) = project.clone() else {
                continue;
            };
            let key = key.clone();
            let path = path.clone();
            actions.push(
                button(
                    SharedString::from(format!("network-refuse-{path}")),
                    format!("Stop agents reading {name}"),
                )
                .small()
                .leading(Icon::Lock)
                .render(&t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.refuse_network_reads(&key, &project, &path, cx);
                }))
                .into_any_element(),
            );
        }
        for &(issuer, page) in &state.issuers {
            actions.push(
                button(
                    SharedString::from(format!("network-rotate-{issuer}")),
                    format!("Rotate the {issuer} key"),
                )
                .small()
                .leading(Icon::RotateCw)
                .trailing(Icon::ExternalLink)
                .render(&t)
                .on_click(move |_, _, cx| cx.open_url(page))
                .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(10.0))
            .mx_3()
            .pt(px(14.0))
            .pb(px(12.0))
            .px(px(16.0))
            .rounded(px(8.0))
            .bg(alpha(red, 0.09))
            .border_1()
            .border_color(alpha(red, 0.30))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(sized_icon(Icon::TriangleAlert, px(16.0), red))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(paint(t.text.primary))
                            .child(title),
                    ),
            )
            .child(div().flex().flex_wrap().gap(px(6.0)).children(chips))
            .when(!from.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(12.0))
                        .text_color(paint(t.text.dim))
                        .child(sized_icon(Icon::File, px(13.0), paint(t.text.dim)))
                        .children(from),
                )
            })
            .when(!actions.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .pt(px(2.0))
                        .children(actions),
                )
            })
            .into_any_element()
    }

    /// Adds a project rule refusing agents' reads of `path`, flagged in
    /// `key`'s request: the person's own rule, kept and undone in Settings →
    /// Permissions.
    fn refuse_network_reads(
        &mut self,
        key: &RequestKey,
        project: &ket_core::id::ProjectId,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let root = self
            .network_requests
            .get(key)
            .and_then(|state| state.root.clone());
        let pattern = permissions::literal(&relative(path, root.as_deref()));
        let mut rule = Rule::for_tool("read", Verdict::Deny);
        rule.path = Patterns::one(pattern);
        rule.reason =
            Some("The network inspector flagged personal or secret data in this file.".to_owned());
        rule.added_by = Some("network".to_owned());
        let name = Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        match permissions::remember(project.as_str(), rule) {
            Ok(()) => {
                if let Some(state) = self.network_requests.get_mut(key) {
                    state.refused.push(path.to_owned());
                }
                self.toast_detail(
                    Tone::Success,
                    format!("Agents can't read {name} now"),
                    "Undo it in Settings → Permissions",
                    cx,
                );
            }
            Err(error) => self.toast(Tone::Error, format!("Could not save the rule: {error}"), cx),
        }
        cx.notify();
    }

    /// Deletes `key`'s request from its log, and closes its tab.
    fn delete_network_request(&mut self, key: RequestKey, cx: &mut Context<Self>) {
        let worktree = key.worktree.clone();
        let id = key.id.clone();
        let removed = cx
            .background_executor()
            .spawn(async move { store::remove_request(worktree.as_str(), &id) });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let removed = removed.await;
            let _ = shell.update(cx, |shell, cx| {
                match removed {
                    Ok(()) => {
                        shell.network_panel.rows.retain(|row| row.id != key.id);
                        if let Some(space) = shell.spaces.get_mut(&key.worktree)
                            && let Some((pane, index)) =
                                space.root.find_tab(&TabKind::NetworkRequest(key.clone()))
                        {
                            space.close_tab(pane, index);
                        }
                        shell.prune_network_requests();
                        shell.persist_layout();
                        shell.toast(Tone::Success, "Request deleted from the log", cx);
                    }
                    Err(error) => shell.toast(
                        Tone::Error,
                        format!("Could not delete the request: {error}"),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Who a request went to, by its host: `Anthropic` for `api.anthropic.com`.
fn provider(upstream: &str) -> &str {
    match upstream {
        "api.anthropic.com" => "Anthropic",
        "api.openai.com" | "chatgpt.com" => "OpenAI",
        other => other,
    }
}

/// Kinds in a sentence: `API key, SSN and card`, then `and 3 more`.
fn kind_list(kinds: &[Kind]) -> String {
    let words: Vec<&str> = kinds
        .iter()
        .map(|kind| match kind {
            Kind::Secret => "API key",
            other => other.label(),
        })
        .collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [first, second, third] => format!("{first}, {second} and {third}"),
        [first, second, third, rest @ ..] => {
            format!("{first}, {second}, {third} and {} more", rest.len())
        }
    }
}

/// A chip's word for a kind: the tag's, capitalised.
fn chip_label(kind: Kind) -> String {
    let label = match kind {
        Kind::Secret => "API key",
        other => other.label(),
    };
    let mut chars = label.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A source that is not a file, as the sentence names it.
fn source_words(source: &Source) -> String {
    match source {
        Source::Message => "your message".to_owned(),
        Source::Context => "the agent's context".to_owned(),
        Source::Command(_) => "a command's output".to_owned(),
        Source::Tool(name) if name.is_empty() => "a tool's output".to_owned(),
        Source::Tool(name) => format!("{name}'s output"),
        Source::File(path) => path.clone(),
    }
}

/// `path` as the worktree sees it: relative when it is inside `root`.
fn relative(path: &str, root: Option<&Path>) -> String {
    root.and_then(|root| Path::new(path).strip_prefix(root).ok())
        .map_or_else(|| path.to_owned(), |rel| rel.display().to_string())
}

/// `path` on disk: as given when absolute, else under `root`.
fn absolute(path: &str, root: Option<&Path>) -> PathBuf {
    let path = Path::new(path);
    match root {
        Some(root) if path.is_relative() => root.join(path),
        _ => path.to_owned(),
    }
}

/// What the panel says in place of the list: a line, what it means if that
/// needs saying, inset as the rows are. The caller adds a button under it.
fn notice(title: &'static str, detail: Option<&'static str>, t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .items_start()
        .gap(px(4.0))
        .px_3()
        .py(px(16.0))
        .border_t_1()
        .border_color(paint(t.rule))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.primary))
                .child(title),
        )
        .children(detail.map(|detail| caption(detail, t)))
}

/// The reply's status, as a tag, when it failed, never came or was cut
/// off; nothing for one that came back whole.
fn failure_tag(summary: &RequestSummary, t: &Theme) -> Option<AnyElement> {
    let (text, colour) = match summary.status {
        0 => ("no reply".to_owned(), t.status.failed),
        status if status >= 400 => (status.to_string(), t.status.failed),
        status if !summary.complete => (format!("{status} · cut off"), t.status.attention),
        _ => return None,
    };
    Some(tinted_tag(text, paint(colour)).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(line: &str, columns: usize) -> Vec<&str> {
        let mut out = Vec::new();
        push_segments(0, line, columns, &mut out);
        out.into_iter().map(|(_, range)| &line[range]).collect()
    }

    #[test]
    fn a_line_that_fits_is_one_row() {
        assert_eq!(rows("short", 10), ["short"]);
        assert_eq!(rows("", 10), [""]);
    }

    #[test]
    fn a_long_line_breaks_after_the_last_space_that_fits() {
        assert_eq!(
            rows("one two three four", 9),
            ["one two ", "three ", "four"]
        );
    }

    #[test]
    fn a_word_wider_than_the_body_is_cut() {
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        assert_eq!(rows("ééééé", 2), ["éé", "éé", "é"]);
    }

    #[test]
    fn every_byte_of_the_line_is_in_exactly_one_row() {
        let line = "{\"content\": \"we need to set up the email address\\n\\n<pasted>\"}";
        assert_eq!(rows(line, 7).concat(), line);
    }

    #[test]
    fn a_flag_deep_in_a_long_line_is_found_in_its_own_row() {
        let mut state = RequestState {
            lines: Arc::new(vec!["{".into(), "aaaa bbbb cccc dddd".into(), "}".into()]),
            columns: Some(5),
            ..Default::default()
        };
        // Line 1 wraps as "aaaa " "bbbb " "cccc " "dddd": rows 1 to 4.
        assert_eq!(state.row_of(1, 0, true), 1);
        assert_eq!(state.row_of(1, 11, true), 3);
        assert_eq!(state.row_of(1, 18, true), 4);
        assert_eq!(state.row_of(2, 0, true), 5);
        assert_eq!(state.row_of(1, 11, false), 1);
    }

    #[test]
    fn unwrapped_lines_are_cut_at_the_cap() {
        let long = SharedString::from("x".repeat(LINE_CAP + 5));
        let (text, shown) = cut_line(&long);
        assert_eq!(shown, LINE_CAP);
        assert!(text.ends_with(" … 5 more characters on this line"));
    }

    #[test]
    fn a_double_click_takes_the_run_under_the_pointer() {
        let line = "dsn: 'https://6f00ab@o45.example.com/'";
        let word = |at: usize| &line[word_at(line, at)];
        assert_eq!(word(0), "dsn");
        assert_eq!(word(2), "dsn");
        assert_eq!(word(3), ":");
        assert_eq!(word(5), "'");
        assert_eq!(word(16), "6f00ab");
        assert_eq!(word(11), "://");
        // Past the end: the last character's run.
        assert_eq!(word(line.len()), "/'");
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn marks_and_the_pick_become_runs_that_do_not_overlap() {
        let mark = HighlightStyle {
            color: Some(gpui::red()),
            background_color: Some(gpui::yellow()),
            ..Default::default()
        };
        let pick = HighlightStyle {
            background_color: Some(gpui::blue()),
            ..Default::default()
        };
        let out = runs(std::slice::from_ref(&(2..6)), Some(&(4..9)), mark, pick);
        let ranges: Vec<Range<usize>> = out.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, [2..4, 4..6, 6..9]);
        assert_eq!(out[0].1, mark);
        // Marked text in the pick keeps its colour on the pick's ground.
        assert_eq!(out[1].1.color, mark.color);
        assert_eq!(out[1].1.background_color, pick.background_color);
        assert_eq!(out[2].1, pick);

        let apart = runs(std::slice::from_ref(&(0..2)), Some(&(5..7)), mark, pick);
        let ranges: Vec<Range<usize>> = apart.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, [0..2, 5..7]);
        assert!(runs(&[], None, mark, pick).is_empty());
    }

    #[test]
    fn the_text_picked_runs_across_lines_either_way_round() {
        let state = RequestState {
            lines: Arc::new(vec!["ab".into(), "cd".into(), "ef".into()]),
            pick: Some(Pick {
                anchor: ((0, 1), (0, 1)),
                head: (2, 1),
            }),
            ..Default::default()
        };
        assert_eq!(state.picked_text().as_deref(), Some("b\ncd\ne"));
        let backwards = RequestState {
            pick: Some(Pick {
                anchor: ((2, 1), (2, 1)),
                head: (0, 1),
            }),
            ..state
        };
        assert_eq!(backwards.picked_text().as_deref(), Some("b\ncd\ne"));
        let nothing = RequestState {
            pick: Some(Pick {
                anchor: ((1, 1), (1, 1)),
                head: (1, 1),
            }),
            ..backwards
        };
        assert_eq!(nothing.picked_text(), None);
        assert!(nothing.pick.unwrap().is_empty());
    }

    #[test]
    fn a_rows_part_of_the_pick_stays_within_the_row() {
        // Line 1 is wrapped as bytes 0..10, 10..20 and 20..30.
        let pick = Some(((1, 3), (1, 12)));
        assert_eq!(row_pick(1, &(0..10), pick), Some(3..10));
        assert_eq!(row_pick(1, &(10..20), pick), Some(0..2));
        // A later row of the line the pick ends on, which it never reaches.
        assert_eq!(row_pick(1, &(20..30), pick), None);
        // An earlier row of the line it starts on.
        let late = Some(((1, 25), (2, 4)));
        assert_eq!(row_pick(1, &(0..10), late), None);
        assert_eq!(row_pick(1, &(20..30), late), Some(5..10));
        assert_eq!(row_pick(2, &(0..8), late), Some(0..4));
        assert_eq!(row_pick(0, &(0..8), late), None);
        assert_eq!(row_pick(3, &(0..8), late), None);
        assert_eq!(row_pick(1, &(0..8), None), None);
    }

    #[test]
    fn a_word_picked_stays_whole_when_dragged_back_past_it() {
        let pick = Pick {
            anchor: ((0, 4), (0, 8)),
            head: (0, 1),
        };
        assert_eq!(pick.range(), ((0, 1), (0, 8)));
    }
}
