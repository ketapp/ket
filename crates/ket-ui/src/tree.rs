//! The projects sidebar — the primary interaction.
//!
//! A tree of project to worktree that you live in, rather than a switcher you
//! summon. Discovered worktrees appear demoted rather than hidden, and the
//! repository's own checkout is shown badged because it is where
//! `ket collapse` merges *to*.

use gpui::{
    AnyElement, BoxShadow, ClickEvent, Context, CursorStyle, Div, DragMoveEvent, ElementId,
    FontWeight, MouseButton, MouseDownEvent, Pixels, Point, Render, SharedString, Stateful,
    StyledText, TextRun, Window, div, font, point, prelude::*, px,
};
use ket_core::id::{ProjectId, WorktreeId};
use ket_core::theme::{Color, Theme};
use ket_core::workspace::Workspace;

use crate::Shell;
use crate::paint::{alpha, paint};
use crate::projects::default_color;
use crate::ui::agent::{MARK, mark, provider};
use crate::ui::button::{button, icon_button};
use crate::ui::chip::{Motion, badge, readout, status_dot, tag};
use crate::ui::group::{button_group, segment};
use crate::ui::icon::{Icon, icon, sized_icon};
use crate::ui::row::{BLOCK_LEAD, block_row, heading_row, row};
use ket_core::activity::{Activity, Foreground, Signal};
use ket_core::agent_hooks::{self, Listener};
use ket_core::subagents::{Subagent, SubagentState};

use crate::fonts::Prose;
use crate::ui::{ICON_GAP, WELL_XS};

/// How often the usage history is written.
///
/// A status line arrives several times a second and the file is rewritten
/// whole, so this is a debounce rather than a schedule: work that is worth
/// keeping is worth keeping within half a minute of happening, and worth
/// nothing if the cost of keeping it is a rewrite per frame.
pub(crate) const USAGE_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// How often the sidebar re-asks which branches are fully merged.
///
/// Deliberately far slower than the git status check on the tick, because the
/// two questions cost different things. "Is this dirty" is one walk of a
/// working tree; "has this landed" is a content comparison against the base —
/// several git processes per worktree, since a squash-merge cannot be seen by
/// reading a flag. And the answer only changes when someone merges something,
/// which is not a thing that happens between two ticks.
pub(crate) const MERGED_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Narrowest sidebar that leaves project names usable rather than clipped away.
const MIN_SIDEBAR_SIZE: Pixels = px(180.0);

/// The sidebar width from which the sort shows both its answers. Under it,
/// the filter, the sort and the actions no longer fit on one line, so the
/// sort folds to one button that flips between the two.
const SORT_UNFOLDS_AT: Pixels = px(340.0);

/// Widest sidebar worth keeping when long paths could otherwise consume the window.
const MAX_SIDEBAR_SIZE: Pixels = px(600.0);

/// Content remains reachable even if the sidebar is dragged across the window.
pub(crate) const MIN_CONTENT_SIZE: Pixels = px(240.0);

/// The sidebar's edge: the gutter of desk between its card and the panes',
/// which is the grab for a resize.
const SIDEBAR_DIVIDER_SIZE: Pixels = crate::header::GUTTER;

/// How far either side of the line the pointer still counts as on it.
///
/// The grab area is *overlaid*, not laid out: an element wide enough to hit
/// is also wide enough to see, and a transparent one between two differently
/// coloured regions shows the window behind it — which reads as a border, a
/// gap, and then another border. So the divider occupies only
/// [`DIVIDER_LINE`], and this much hit area hangs over its neighbours.
pub(crate) const DIVIDER_GRAB: Pixels = px(6.0);

/// How wide the line itself is.
///
/// Half a point, which is one device pixel on a 2x display — a true hairline.
/// A whole point is two device pixels there, which is what kept reading as a
/// thick border however much the constant was halved.
pub(crate) const DIVIDER_LINE: Pixels = px(0.5);

/// Height of the strip along the top of a card — the tab bar and the right
/// panel's view strip share it so the two read as one line.
pub(crate) const TOP_STRIP: Pixels = px(46.0);

/// How far a worktree row is inset from its project.
///
/// A margin rather than padding: rows are rounded pills now, and padding would
/// indent the text while leaving the pill itself full width.
///
/// Small, because the indent is not what says a row belongs to the project
/// above it — the heading does that, and a branch name is the longest thing in
/// the sidebar. Every pixel spent here is one taken off the end of the name it
/// is indenting.
const INDENT: Pixels = px(8.0);

/// How far the column of rows is inset from the sidebar's own edges.
///
/// Named rather than written into the column's padding, because the drag ghost
/// has to work out where a row sits to hold itself over one.
///
/// Nothing, so a row's hairline runs the full width of the sidebar. An inset
/// line stops short of both edges and reads as an underline under the text
/// rather than as a division of the panel.
const COLUMN_INSET: Pixels = px(8.0);

/// The room inside a tinted project group's edge, taken out of
/// [`COLUMN_INSET`] so its rows do not move.
const OVERRIDE_PAD: Pixels = px(3.0);

/// How far a tinted mark's well reaches past the mark on each side.
const MARK_WELL_BLEED: Pixels = px(4.0);

/// How far apart one project's rows sit.
///
/// Nothing: a row carries a hairline along its bottom edge, and a gap as well
/// as a line is how a list comes to look like a stack of cards. The insertion
/// line centres itself on the edge either way — see [`insertion_line`] — and
/// the drop target's slack going to zero is correct now that there is no dead
/// space between two rows to be in.
const ROW_GAP: Pixels = px(2.0);

/// How far apart one project's group sits from the next.
const GROUP_GAP: Pixels = px(12.0);

/// How wide the caps on an insertion line are, which is also how tall it draws.
const CAP: Pixels = px(6.0);

/// How tall the thing under the cursor draws.
///
/// Only the ghost needs this, and only to keep its last few pixels off the
/// bottom of the window — a row is two lines and this is one, so measuring the
/// real thing would be measuring something else.
const GHOST_HEIGHT: Pixels = px(26.0);

/// How far the state rail sits from a row's leading edge.
///
/// Enough that the bar reads as sitting inside the pill rather than pinned to
/// its edge — the rail is four pixels wide, and a mark that thin needs the
/// space beside it to look deliberate.
const RAIL_INSET: Pixels = px(8.0);

/// The sidebar's leading inset: where "Projects" starts, and where every
/// project's badge under it starts.
///
/// The search field above them sits at 8, not here — it is a bordered box,
/// and its inset is measured to the box, not to the ink inside it.
const LEAD: Pixels = px(12.0);

/// The trailing column, measured from the sidebar's right edge to the
/// *centre* of whatever ends a row: the strip's `+`, a project's `+`, a
/// worktree's ring.
///
/// A centre rather than a padding, because the things on it are different
/// sizes — a 24px well in the strip, a 22px well on a project, a 14px ring on
/// a worktree — and one shared padding would put their centres up to five
/// pixels apart, which is a column of plusses with a bend in it. Each row
/// derives its own right padding from this instead. The worktree rows'
/// [`block_row`] already pads 9, which is this minus half the ring.
const TRAIL_AXIS: Pixels = px(16.0);

/// A project heading's name size at rest: the same as the branch names under
/// it, since the badge is what marks the row as the heading.
const NAME: Pixels = px(13.5);

#[derive(Clone, Copy)]
pub(crate) struct SidebarResize;

impl Render for SidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A worktree row picked up to be dropped elsewhere in its own project.
///
/// Carries the row's position rather than its id because that is what the
/// drop needs — the id is on the node at that position — and the branch,
/// which is all the thing under the cursor has to say while it is moving.
#[derive(Clone)]
pub(crate) struct WorktreeDrag {
    /// Index into [`Shell::projects`] of the project the row belongs to.
    ///
    /// A drag never leaves its project: worktrees belong to a repository, and
    /// a row dropped under another project's heading would be a move nobody
    /// asked for and no store operation can perform.
    pub(crate) project: usize,
    /// Index into that project's worktrees.
    pub(crate) worktree: usize,
    /// Branch name, drawn under the cursor.
    pub(crate) branch: SharedString,
    /// The theme, so the thing under the cursor is painted like the row it
    /// came from. It renders in its own view, which has no access to the
    /// shell's.
    pub(crate) theme: Theme,
    /// Where inside the row the pointer was when the drag began.
    ///
    /// gpui draws the thing under the cursor with its top-left at
    /// `pointer - grab`, which is wherever the row itself would be if the row
    /// had come along — so this is what lets the ghost know where it is about
    /// to be put, and put itself somewhere else.
    pub(crate) grab: Point<Pixels>,
    /// How wide the sidebar is, which is as far as the ghost may go.
    pub(crate) sidebar: Pixels,
}

/// A project heading picked up to be dropped elsewhere in the sidebar.
///
/// Unlike a worktree, a project has nowhere it may not go: the list is one
/// sequence and any project can sit anywhere in it.
#[derive(Clone)]
pub(crate) struct ProjectDrag {
    /// Index into [`Shell::projects`].
    pub(crate) project: usize,
    /// Display name, drawn under the cursor.
    pub(crate) name: SharedString,
    /// Badge colour, so the thing moving is recognisably the row you took.
    pub(crate) color: Color,
    /// Badge contents — the project's icon, or its initial.
    pub(crate) icon: SharedString,
    /// The theme. Its own view has no access to the shell's.
    pub(crate) theme: Theme,
    /// Where inside the heading the pointer was when the drag began.
    pub(crate) grab: Point<Pixels>,
    /// How wide the sidebar is, which is as far as the ghost may go.
    pub(crate) sidebar: Pixels,
}

/// `value`, held between `low` and `high`.
///
/// An empty range answers `low`: the sidebar has a minimum width and the window
/// a minimum height, so this is unreachable rather than a case worth a policy,
/// and returning the near edge is the answer that cannot put a row off-screen.
fn clamp(value: Pixels, low: Pixels, high: Pixels) -> Pixels {
    if high < low || value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// The thing under the cursor while a sidebar row is being dragged.
///
/// Held inside the sidebar, because a drop can only land on another row and
/// every one of those is in there. A ghost that follows the pointer out over
/// the editor is offering somewhere it cannot go, so instead it takes the
/// column it came from and its width, and rides the pointer vertically —
/// pinned at the top and bottom of the window the same way. Wander off with
/// it and the row stays where the drop is.
///
/// `grab` is where in the row the pointer went down: gpui draws a drag view
/// with its top-left at `pointer - grab`, so subtracting it is what tells the
/// ghost where it is about to be put, and the difference from where it belongs
/// is applied to an absolutely positioned child — which moves it without the
/// root having to know its own size.
fn ghost(
    content: AnyElement,
    indent: Pixels,
    grab: Point<Pixels>,
    sidebar: Pixels,
    t: Theme,
    window: &Window,
) -> impl IntoElement {
    let origin = window.mouse_position() - grab;
    let left = COLUMN_INSET + indent;
    let width = clamp(
        sidebar - COLUMN_INSET * 2.0 - indent,
        px(80.0),
        MAX_SIDEBAR_SIZE,
    );
    let top = clamp(
        origin.y,
        TOP_STRIP,
        window.viewport_size().height - GHOST_HEIGHT,
    );

    div().relative().child(
        div()
            .absolute()
            .left(left - origin.x)
            .top(top - origin.y)
            .w(width)
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .px(crate::ui::PAD_X)
            .py(px(4.0))
            .rounded(crate::ui::RADIUS_LG)
            .bg(paint(t.panel))
            .border_1()
            .border_color(paint(t.border))
            .shadow(vec![BoxShadow {
                color: crate::paint::shadow(0.5, &t),
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(12.0),
                spread_radius: px(0.0),
            }])
            .text_size(px(11.0))
            .text_color(paint(t.text.primary))
            .child(content),
    )
}

impl Render for WorktreeDrag {
    /// The branch, and nothing else — the row itself stays where it was until
    /// the drop lands, so what follows the cursor only has to say which row is
    /// moving.
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The ghost is the row's width, and a branch name is the longest thing
        // in the sidebar: cut it at the edge rather than letting it push the
        // pill back out over the window `ghost` exists to keep it off.
        let branch = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .child(self.branch.clone())
            .into_any_element();
        ghost(branch, INDENT, self.grab, self.sidebar, self.theme, window)
    }
}

impl Render for ProjectDrag {
    /// The badge and the name, which is what a heading row is.
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let content = div()
            .flex()
            .items_center()
            .gap(ICON_GAP)
            .w_full()
            .min_w_0()
            .child(badge(self.color, self.icon.clone(), &t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.name.clone()),
            )
            .into_any_element();
        // No indent: a heading heads the column rather than sitting inside it.
        ghost(content, px(0.0), self.grab, self.sidebar, t, window)
    }
}

/// Where a dragged sidebar row would land if it were dropped now.
///
/// A *gap* rather than a row: "before the row at this index", with an index
/// one past the end meaning the bottom of the list. That is what an insertion
/// line draws, and it is the one thing a highlighted target row could never
/// say — dropping onto a row is ambiguous about which side of it you meant,
/// and with two rows the ambiguity is the whole question.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropTarget {
    /// A slot among one project's worktrees.
    Worktree {
        /// Index into [`Shell::projects`].
        project: usize,
        /// The slot the dragged row would take.
        before: usize,
    },
    /// A slot among the projects themselves.
    Project {
        /// The slot the dragged heading would take.
        before: usize,
    },
}

/// The line drawn in the gap a dragged row would drop into.
///
/// Absolutely positioned, and that is the point: the column's rows are two
/// pixels apart, and a line that took part in the layout would push every row
/// below it down by its own height as the pointer moved between two gaps —
/// which is a list that squirms away from the cursor. This one costs nothing
/// and lands in the gap that is already there.
///
/// Capped at both ends, so it reads as a marker placed between two rows rather
/// than as a border one of them has grown.
///
/// `gap` is how far apart the two things it sits between are, which is what
/// centres it: worktree rows are two pixels apart and whole projects twelve,
/// and a line placed at one distance for both would hug the wrong group.
///
/// `bleed` is how far past its parent's padding it reaches, because an
/// absolute inset is measured from the padding edge and a row's padding is not
/// where the row ends. A line that stopped at the text would read as
/// underlining it rather than as marking the space beneath it.
pub(crate) fn insertion_line(below: bool, gap: Pixels, bleed: Pixels, t: &Theme) -> AnyElement {
    let ink = alpha(paint(t.text.primary), 0.65);
    let cap = || div().flex_none().size(CAP).rounded_full().bg(ink);
    let out = gap / 2.0 + CAP / 2.0;

    div()
        .absolute()
        .left(-bleed)
        .right(-bleed)
        // Centred on the gap: half the cap on the row's side of the edge it is
        // marking, half on the other.
        .when(below, |el| el.bottom(-out))
        .when(!below, |el| el.top(-out))
        .h(CAP)
        .flex()
        .items_center()
        .child(cap())
        .child(div().flex_1().h(px(2.0)).bg(ink))
        .child(cap())
        .into_any_element()
}

fn clamped_sidebar_width(wanted: Pixels, extent: Pixels) -> Option<Pixels> {
    let available = extent - MIN_CONTENT_SIZE - SIDEBAR_DIVIDER_SIZE;
    let maximum = if available < MAX_SIDEBAR_SIZE {
        available
    } else {
        MAX_SIDEBAR_SIZE
    };
    if maximum < MIN_SIDEBAR_SIZE {
        return None;
    }
    Some(if wanted < MIN_SIDEBAR_SIZE {
        MIN_SIDEBAR_SIZE
    } else if wanted > maximum {
        maximum
    } else {
        wanted
    })
}

/// How many distinct paths a status reports, for a count a person reads.
///
/// A file with both staged and unstaged edits is two entries in the status
/// and one file to anybody looking at it.
pub(crate) fn changed_paths(status: &ket_core::status::WorktreeStatus) -> usize {
    let mut paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    paths.len()
}

/// Which worktrees the sidebar lists: every one, or only those in one state.
///
/// The segments' order is the order of the counts `Shell::sidebar` keeps, so
/// the discriminant doubles as the index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SidebarFilter {
    /// Everything, grouped by project.
    #[default]
    All = 0,
    /// Something is happening: an agent at work, a busy shell, a merge.
    Running = 1,
    /// Blocked on a person.
    Waiting = 2,
    /// Nothing happening, including a run that ended badly.
    Idle = 3,
}

impl SidebarFilter {
    /// Every filter, in the switch's order.
    pub(crate) const ALL: [SidebarFilter; 4] = [
        SidebarFilter::All,
        SidebarFilter::Running,
        SidebarFilter::Waiting,
        SidebarFilter::Idle,
    ];

    /// What its segment says.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SidebarFilter::All => "All",
            SidebarFilter::Running => "Running",
            SidebarFilter::Waiting => "Waiting",
            SidebarFilter::Idle => "Idle",
        }
    }

    /// The one filter, other than `All`, that a row in this state falls in.
    pub(crate) fn of(signal: Signal) -> Self {
        match signal {
            Signal::Working | Signal::Running | Signal::Merging => SidebarFilter::Running,
            Signal::Blocked => SidebarFilter::Waiting,
            Signal::Quiet | Signal::Failed => SidebarFilter::Idle,
        }
    }

    /// Whether a row in this state is listed under this filter.
    pub(crate) fn admits(self, signal: Signal) -> bool {
        self == SidebarFilter::All || SidebarFilter::of(signal) == self
    }
}

/// Which paths in a worktree git reports as changed, as the file tree needs
/// them: a letter per changed file, and a dot on every directory above one.
///
/// Built once per status read and shared, since the tree asks it for every
/// row it draws and a status can name a few thousand paths.
#[derive(Default)]
pub(crate) struct ChangeMarks {
    /// Each changed path, relative to the worktree root, with how it changed.
    /// An untracked directory is listed without its trailing slash.
    files: std::collections::HashMap<String, ket_core::status::ChangeKind>,
    /// Every directory with a changed path somewhere under it.
    dirs: std::collections::HashSet<String>,
}

impl ChangeMarks {
    /// The marks for one status read.
    pub(crate) fn of(status: &ket_core::status::WorktreeStatus) -> Self {
        let mut marks = Self::default();
        for file in &status.files {
            let path = file.path.trim_end_matches('/');
            // A path both staged and unstaged is listed twice; the first
            // answer is as good as the second for a one-letter mark.
            marks.files.entry(path.to_owned()).or_insert(file.kind);
            let mut rest = path;
            while let Some((dir, _)) = rest.rsplit_once('/') {
                if !marks.dirs.insert(dir.to_owned()) {
                    break;
                }
                rest = dir;
            }
        }
        marks
    }

    /// How the file at `rela` changed, if it did.
    pub(crate) fn file(&self, rela: &str) -> Option<ket_core::status::ChangeKind> {
        self.files.get(rela).copied()
    }

    /// Whether anything under the directory at `rela` changed.
    pub(crate) fn dir(&self, rela: &str) -> bool {
        self.dirs.contains(rela)
    }
}

/// A worktree ket created, as the tree needs it.
pub struct WorktreeNode {
    /// Stable id, used to keep selection, tabs and activity attached to the row.
    pub(crate) id: WorktreeId,
    /// Branch name.
    pub(crate) branch: SharedString,
    /// Display name, falling back to the branch when absent.
    pub(crate) name: Option<SharedString>,
    /// Which agent this worktree is for, when it says.
    pub(crate) agent: Option<SharedString>,
    /// Where it lives.
    ///
    /// Carried on the node rather than looked up per click: the checkout is
    /// not in the workspace's worktree list, so there is nothing there to look
    /// it up in — and a row that knows its own path does not need to reopen
    /// the workspace to answer where it is.
    pub(crate) path: std::path::PathBuf,
    /// Whether the checkout has uncommitted work in it.
    ///
    /// Only that. The design's sidebar shows a green light for a *running*
    /// agent, which needs session state the tree does not carry yet — so this
    /// is the honest half of that signal: the row has changes, or it does not.
    pub(crate) dirty: bool,
    /// Lines added and removed against `HEAD`, when the sweep has read them.
    ///
    /// `None` until the first status tick that finds this worktree dirty, and
    /// back to `None` when it stops being — it is a summary of exactly what
    /// `dirty` is true about, so the two are read and cleared together. See
    /// [`ket_core::git::Git::line_changes`] for what it counts and what it
    /// deliberately does not.
    pub(crate) line_changes: Option<(u32, u32)>,
    /// How many paths have uncommitted changes, for the Git view's badge.
    ///
    /// Read on the same status tick as [`Self::dirty`] and zero exactly when
    /// that is false. An untracked directory counts once here, as status
    /// reports it; the Git view itself expands it.
    pub(crate) changes: usize,
    /// The same read path by path, for the file tree's marks. Replaced
    /// together with [`Self::changes`].
    pub(crate) marks: std::sync::Arc<ChangeMarks>,
    /// The revision this worktree's branch is measured against, and the name
    /// to show for it — `main`, usually.
    ///
    /// `None` for the repository's own checkout, which is the base rather than
    /// something branched from one; see [`ket_core::worktree::Worktree::base_rev`].
    pub(crate) base: Option<(SharedString, SharedString)>,
    /// A merge, rebase, cherry-pick, revert, or bisect left mid-flight in this
    /// checkout — including one run by hand in the worktree's own terminal,
    /// not just `ket collapse`.
    pub(crate) in_progress: Option<ket_core::status::GitOperation>,
    /// How aggressively the agent launched here should cut its own token
    /// usage — see [`ket_core::worktree::Worktree::token_reduction`].
    pub(crate) token_reduction: u8,
    /// Whether it is pinned — see [`ket_core::worktree::Worktree::pinned`].
    pub(crate) pinned: bool,
    /// Whether the checkout is gone from disk.
    ///
    /// The registry outlives the directory: a worktree removed by hand, or by
    /// a `git worktree remove` ket did not run, leaves its record behind. The
    /// CLI has always flagged these with a `!`; the sidebar drew them as
    /// ordinary rows, so clicking one opened a shell in a directory that is not
    /// there and the pane came up empty with nothing saying why.
    pub(crate) missing: bool,
    /// Whether this is the repository's own checkout rather than a worktree
    /// ket created.
    ///
    /// It behaves like any other row — it opens, it gets a terminal, it shows
    /// its files — and only differs in wearing a badge that says what it is.
    pub(crate) primary: bool,
    /// Whether the branch holds nothing its base does not already have.
    ///
    /// The row's way of saying "this one is finished". Without it a merged
    /// worktree and a live one look identical, so the only way to find the
    /// gigabytes worth reclaiming is to go through them by hand — which is
    /// exactly how this repository reached eighty of them.
    ///
    /// False until a sweep says otherwise, and swept on its own slow cadence
    /// rather than with the git status tick: see [`Shell::refresh_merged`].
    pub(crate) merged: bool,
}

impl WorktreeNode {
    pub(crate) fn label(&self) -> SharedString {
        self.name.clone().unwrap_or_else(|| self.branch.clone())
    }
}

/// A worktree git knows about that ket did not create.
///
/// Shown demoted rather than hidden: omitting these makes ket look broken in any
/// repository where something else made a worktree.
pub struct DiscoveredNode {
    /// Branch name, or the directory when the checkout is detached.
    pub(crate) label: SharedString,
    /// Where it lives.
    pub(crate) path: SharedString,
}

/// A project and everything under it.
pub struct ProjectNode {
    /// Owning project.
    pub(crate) id: ProjectId,
    /// Display name — the user's chosen one, else the directory's.
    pub(crate) name: SharedString,
    /// Badge colour.
    pub(crate) color: Color,
    /// Icon, verbatim from core. Shown in the badge instead of the initial.
    pub(crate) icon: Option<SharedString>,
    /// Whether discovered worktrees are being hidden. Drives the menu label.
    pub(crate) hide_discovered: bool,
    /// Whether the project's worktrees are shown.
    pub(crate) expanded: bool,
    /// Worktrees ket manages.
    pub(crate) worktrees: Vec<WorktreeNode>,
    /// Worktrees git knows about that ket does not.
    pub(crate) discovered: Vec<DiscoveredNode>,
    /// Whether the discovered group is expanded.
    pub(crate) discovered_expanded: bool,
    /// Backlog notes not started yet, for the count beside the name.
    pub(crate) backlog: usize,
    /// When the backlog file had last changed as `backlog` was counted, so a
    /// note a phone adds through the host is counted on the next tick.
    pub(crate) backlog_seen: Option<std::time::SystemTime>,
    /// The project's own launch command for an agent, by agent name, where it
    /// has one. Every session ket starts in this project's worktrees launches
    /// through it — see [`ket_core::project::ProjectSettings::agents`].
    pub(crate) agent_overrides: std::collections::BTreeMap<String, ket_core::config::AgentLaunch>,
}

impl ProjectNode {
    /// The project's own commands, by first word — `claude-work` — for the
    /// places that say a project is launching differently. `None` when it
    /// launches every agent as Settings says.
    pub(crate) fn override_summary(&self) -> Option<SharedString> {
        (!self.agent_overrides.is_empty()).then(|| {
            self.agent_overrides
                .values()
                .map(crate::agent_override::short)
                .collect::<Vec<_>>()
                .join(", ")
                .into()
        })
    }
}

/// Which worktree is selected, as a path through the tree.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Index into [`Shell::projects`].
    pub(crate) project: usize,
    /// Index into that project's worktrees.
    pub(crate) worktree: usize,
}

/// Takes every discovered worktree that has an agent session into the project.
///
/// The rule is "have you worked here", and an agent transcript is the only
/// durable evidence of that ket has — a directory's timestamps say when files
/// changed, not whether you were the one changing them. Anything promoted is
/// removed from `discovered`, so a worktree never appears twice.
///
/// Failures are left alone rather than reported: a detached checkout has no
/// branch to register and legitimately stays where it is, and a project whose
/// repository has gone missing has larger problems than this list.
/// A worktree's state as its row says it: a dot and a word in the state's
/// hue, right-aligned in the chrome's mono. `None` for a row with nothing to
/// report, which is most of them at rest — "idle" on every quiet row would be
/// a column of the same word.
///
/// Replaces the state rail that ran down each row's leading edge: that edge
/// now belongs to the selection marker, and a word is what the eye can read
/// at a glance down a list without learning a colour code first.
pub(crate) fn state_word(node: &WorktreeNode, signal: Signal, t: &Theme) -> Option<Div> {
    let (word, colour, motion): (&str, Color, Option<Motion>) = match node.in_progress {
        Some(op) => (
            git_operation_label(op),
            t.status.merging,
            Some(match signal {
                Signal::Working => Motion::Travel,
                Signal::Blocked => Motion::Pulse,
                _ => Motion::Still,
            }),
        ),
        None => match signal {
            Signal::Working => ("running", t.status.running, Some(Motion::Travel)),
            Signal::Blocked => ("waiting", t.status.attention, Some(Motion::Pulse)),
            Signal::Failed => ("failed", t.status.failed, Some(Motion::Still)),
            Signal::Merging => ("merging", t.status.merging, Some(Motion::Travel)),
            // Something is running that is not known to be an agent making
            // progress: worth a word, not a colour or a beat.
            Signal::Running => ("busy", t.text.dim, None),
            Signal::Quiet => return None,
        },
    };
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .text_size(px(11.5))
            .text_color(paint(colour))
            .children(motion.map(|motion| status_dot(paint(colour), motion, px(6.0))))
            .child(word),
    )
}

/// The mark at the head of a worktree row.
///
/// This is the slot the state light used to occupy, and moving the state to
/// the rail is what paid for it. A row can now say *who* as well as *what*
/// without being selected first — which matters exactly when it is hard, with
/// four worktrees running and one of them wanting something.
///
/// A worktree with no agent of any kind gets the branch glyph rather than the
/// terminal one: a terminal mark on a row where no terminal is running says
/// something that is not true.
pub(crate) fn row_mark(activity: &Activity, dirty: bool, missing: bool, t: &Theme) -> AnyElement {
    match activity {
        // A conversation on disk with nothing running. The agent's own mark,
        // most of the way out: enough to say this worktree remembers a
        // session, not enough to compete with the rows that are live.
        Activity::Recorded { agent, .. } => {
            let (which, tint) = provider(agent, t);
            sized_icon(which, MARK, alpha(tint, 0.35)).into_any_element()
        }
        Activity::Session { agent, .. }
        | Activity::Observed { agent, .. }
        | Activity::Ended { agent, .. } => mark(agent, t).into_any_element(),
        // A program ket watched start rather than one it launched, so there is
        // no agent name recorded to look up — but the command *is* one, and
        // `provider` matches on exactly that. It answers with the terminal
        // glyph for anything it does not recognise, so an unknown build still
        // draws what it drew before, and a `claude` the catalogue never
        // matched stops being a terminal on a row that plainly says claude.
        // Git changing the worktree's history gets its own glyph in the
        // merging colour — the same hue the rail beside it is carrying, and
        // deliberately not the running green, which on this row would claim
        // an agent is working. Anything else busy falls back to `provider`,
        // which has no mark for a build or a test run and answers with the
        // honest dim terminal glyph.
        Activity::Busy { mutating: true, .. } => {
            sized_icon(Icon::GitBranch, MARK, paint(t.status.merging)).into_any_element()
        }
        Activity::Busy { command, .. } => {
            let (which, tint) = provider(command, t);
            sized_icon(which, MARK, tint).into_any_element()
        }
        Activity::Idle => {
            // Dirty is the one quiet state worth a brighter glyph: the rail
            // beside it is white and still, and the two together are what say
            // *there is something here to read*.
            let ink = if dirty {
                alpha(paint(t.text.primary), 0.62)
            } else if missing {
                alpha(paint(t.text.dim), 0.30)
            } else {
                alpha(paint(t.text.dim), 0.34)
            };
            sized_icon(Icon::GitBranch, MARK, ink).into_any_element()
        }
    }
}

/// The tag text for a git operation caught mid-flight.
///
/// Present tense and lower case, to read the same as `primary`/`missing`
/// beside it rather than announcing itself as a different kind of label.
fn git_operation_label(op: ket_core::status::GitOperation) -> &'static str {
    use ket_core::status::GitOperation;
    match op {
        GitOperation::Merge => "merging",
        GitOperation::Rebase => "rebasing",
        GitOperation::CherryPick => "cherry-picking",
        GitOperation::Revert => "reverting",
        GitOperation::Bisect => "bisecting",
    }
}

/// Where a worktree row's mark is centred, measured from the row's left edge.
///
/// The subagent tree hangs from here. It is derived rather than stated because
/// it is four things added up — [`BLOCK_LEAD`], the rail, the gap, half the
/// mark — and the tree's guide landing a few pixels off the mark it claims to
/// grow out of is exactly the detached look the tree exists to fix.
fn mark_axis() -> Pixels {
    BLOCK_LEAD + MARK / 2.0
}

/// A subagent row's height: a step taller than the text needs, so each elbow
/// has room to turn before it reaches the next one.
const SUBAGENT_ROW_H: Pixels = px(24.0);

/// How far the elbow runs out from the guide before it meets the state dot.
///
/// Short on purpose. The label has to land on the worktree's own text column —
/// that shared edge is what makes the two levels read as one list — and that
/// leaves half a mark and a gap between the guide and the words for the elbow,
/// the dot and a breath between them.
const ELBOW_RUN: Pixels = px(6.0);

/// Where the elbow turns. Nearly the whole run, so the branch reads as one
/// curve into its dot rather than a corner followed by a stub.
const ELBOW_RADIUS: Pixels = px(5.0);

/// The state dot at the end of each branch.
const NODE: Pixels = px(5.0);

/// Space between the guide's own start and the bottom of the mark it drops
/// from, so the line visibly leaves the mark rather than touching it.
const GUIDE_CLEARANCE: Pixels = px(3.0);

/// The tree's line colour: the same faint ink the guide has always been
/// drawn in, stated once because four elements now draw it.
fn guide(t: &Theme) -> gpui::Rgba {
    alpha(paint(t.text.primary), 0.22)
}

/// Draws the small amount of Markdown an agent's one-line report can carry.
///
/// These reports are deliberately not full Markdown: they stay one line and
/// truncate with the row. Inline code is useful even there, though — commit
/// ids, branches and commands otherwise keep their literal backticks and read
/// like punctuation in prose. Paired backticks are removed and their contents
/// take the chrome's identifier face and a faint code wash. An unmatched tick
/// is left alone, so a report cannot silently lose text while it is arriving.
fn detail_text(text: String, ink: gpui::Rgba) -> StyledText {
    fn push_run(runs: &mut Vec<(usize, bool)>, len: usize, code: bool) {
        if len == 0 {
            return;
        }
        if let Some((last, last_code)) = runs.last_mut()
            && *last_code == code
        {
            *last += len;
        } else {
            runs.push((len, code));
        }
    }

    let mut rendered = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            rendered.push_str(rest);
            push_run(&mut spans, rest.len(), false);
            rest = "";
            break;
        };
        let code = &after_open[..close];
        if code.is_empty() {
            let literal = &rest[..open + close + 2];
            rendered.push_str(literal);
            push_run(&mut spans, literal.len(), false);
        } else {
            let prose = &rest[..open];
            rendered.push_str(prose);
            push_run(&mut spans, prose.len(), false);
            rendered.push_str(code);
            push_run(&mut spans, code.len(), true);
        }
        rest = &after_open[close + 1..];
    }
    rendered.push_str(rest);
    push_run(&mut spans, rest.len(), false);

    let prose = font(crate::fonts::prose());
    let mut code = font(crate::fonts::chrome());
    code.weight = FontWeight::MEDIUM;
    let color = ink.into();
    let code_background = alpha(ink, 0.12).into();
    let runs = spans
        .into_iter()
        .map(|(len, is_code)| TextRun {
            len,
            font: if is_code { code.clone() } else { prose.clone() },
            color,
            background_color: is_code.then_some(code_background),
            underline: None,
            strikethrough: None,
        })
        .collect();

    StyledText::new(rendered).with_runs(runs)
}

/// The top of the tree's trunk, drawn inside the worktree row itself.
///
/// It starts just under the mark and runs out through the row's bottom edge
/// — and through its hairline, which the row keeps but draws clear while it
/// heads a tree — so it meets the first elbow without a seam. Positioned from
/// the row's middle, where `items_center` puts the mark, plus half the mark
/// and the clearance, as a margin: a percentage and a length cannot be added
/// in one inset.
fn trunk_head(t: &Theme) -> AnyElement {
    div()
        .absolute()
        .left(mark_axis() - px(0.5))
        .top(gpui::relative(0.5))
        .mt(MARK / 2.0 + GUIDE_CLEARANCE)
        .bottom(px(-1.0))
        .w(px(1.0))
        .bg(guide(t))
        .into_any_element()
}

/// The hover group a worktree row and its meta share.
///
/// Named here rather than at each row, because the meta reveals part of
/// itself on the row's hover and has to name the same group the row does —
/// wherever that row is drawn.
pub(crate) fn worktree_row_group(project: usize, worktree: usize) -> SharedString {
    SharedString::from(format!("wt-{project}-{worktree}"))
}

impl Shell {
    /// The trailing facts and actions shared by both worktree list views.
    ///
    /// `leading` is reserved for structure owned by one view, currently the
    /// grouped view's subagent fold. Everything after it describes or acts on
    /// the worktree itself and must therefore be identical wherever that
    /// worktree is listed.
    pub(crate) fn worktree_meta(
        &self,
        node: &WorktreeNode,
        project: usize,
        worktree: usize,
        leading: Option<AnyElement>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let row = project * 1000 + worktree;
        let can_merge = !node.primary && !node.missing && node.dirty;
        let merging = self.merging.as_ref() == Some(&node.id);
        let mid_operation = node.in_progress.is_some();
        let merge_button = can_merge.then(|| {
            let idle = !merging && !mid_operation;
            let hovered = self.hovered_merge.as_ref() == Some(&node.id);
            let node_id = node.id.clone();
            // A button while a merge can start; the glyph alone, with the
            // tooltip still saying why, while one cannot.
            let mark = match idle {
                true => icon_button(("worktree-merge-button", row), Icon::GitMerge)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request_merge_worktree(project, worktree, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .into_any_element(),
                false => div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(crate::ui::WELL_XS)
                    .child(sized_icon(Icon::GitMerge, px(12.0), paint(t.text.dim)))
                    .into_any_element(),
            };
            let label = if merging {
                "merging\u{2026}"
            } else if mid_operation {
                "busy"
            } else {
                "commit and merge"
            };

            crate::ui::tooltip::tooltip(
                ("worktree-merge", row),
                mark,
                label,
                crate::ui::tooltip::Side::Top,
                hovered,
                t,
            )
            .flex_none()
            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                if *is_hovered {
                    this.hovered_merge = Some(node_id.clone());
                } else if this.hovered_merge.as_ref() == Some(&node_id) {
                    this.hovered_merge = None;
                }
                cx.notify();
            }))
        });

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .children(leading)
            .children(merge_button)
            // The row's facts — what it weighs on disk — show only while the
            // pointer is on the row. At rest every row ends in one mark, the
            // ring, and the trailing column stops reading as three unrelated
            // things that happen to share a right edge. Absent rather than
            // hidden: a hidden figure still held its width, and every row sat
            // its state word in front of a blank the size of a number it was
            // not showing. The state word steps aside when the figure comes.
            //
            // What stays visible is state rather than fact: a missing
            // checkout, an operation under way. Those are things the row is
            // telling you, not things you came to ask it.
            .children(
                (self.hovered_worktree.as_ref() == Some(&node.id) && !node.missing)
                    .then_some(())
                    .and_then(|()| {
                        let footprint = self
                            .footprint(&node.id)
                            .filter(|footprint| footprint.total >= crate::storage::SHOWN_FROM)?;
                        let size = div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(paint(t.text.dim))
                            .child(ket_core::storage::format_bytes(footprint.total))
                            .into_any_element();
                        // The figure is only here because it is large, so
                        // its tooltip says what can be done about it —
                        // and points at the menu that does it, when there
                        // is build output to clear.
                        let tip = match footprint.build {
                            0 => "On disk, none of it build output".to_owned(),
                            build => format!(
                                "{} is build output \u{2014} right-click \u{203a} Clear build output",
                                ket_core::storage::format_bytes(build)
                            ),
                        };
                        let node_id = node.id.clone();
                        Some(
                            crate::ui::tooltip::tooltip(
                                ("worktree-footprint", row),
                                size,
                                tip,
                                crate::ui::tooltip::Side::Top,
                                self.hovered_footprint.as_ref() == Some(&node.id),
                                t,
                            )
                            .flex_none()
                            .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                                if *is_hovered {
                                    this.hovered_footprint = Some(node_id.clone());
                                } else if this.hovered_footprint.as_ref() == Some(&node_id) {
                                    this.hovered_footprint = None;
                                }
                                cx.notify();
                            })),
                        )
                    }),
            )
            .when(node.missing, |el| {
                el.child(tag("missing", t).text_size(px(8.5)))
            })
            // Only a level someone chose earns a mark: the default is what
            // every worktree has, and a ring on every row says nothing. Left
            // out rather than hidden, so the rest of the row ends flush with
            // the right edge.
            .when(
                node.token_reduction != ket_core::worktree::default_token_reduction(),
                |el| el.child(self.token_reduction_ring(node, row, window, cx)),
            )
            .into_any_element()
    }

    /// The rows under a worktree for the subagents its agent has running.
    ///
    /// A tree rather than a list beside one: the guide leaves from under the
    /// worktree's own mark (see `trunk_head`), and each row takes a rounded
    /// elbow off it that ends in the subagent's state dot. The label then sits
    /// on the worktree's text column, so parent and children share one left
    /// edge. No fill and no rule under the group: the worktree above is a
    /// rounded card, and a band beneath it read as a second, squarer one.
    ///
    /// Otherwise as before: a dot, what it was sent to do, how long it has been
    /// going, nothing to dismiss. A click still goes to the pane its lead runs
    /// in, but the rows do not dress as buttons for it: when that pane is
    /// already the one in front, which is most of the time, nothing happens.
    fn subagent_rows(
        &self,
        pi: usize,
        wi: usize,
        worktree: &WorktreeId,
        subagents: &[Subagent],
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let axis = mark_axis();
        let line = guide(t);
        let last = subagents.len().saturating_sub(1);
        // Where the worktree's branch name starts, and so where every label
        // does: half the mark and the row's gap past the axis.
        let label_x = axis + MARK / 2.0 + ICON_GAP;
        let node_x = axis + ELBOW_RUN;
        // Where the worktree's state word ends, and so where every figure
        // does: past the row's own end padding, its trailing column and the
        // gap before it. The fold alone until that column has been measured.
        let trailing = self.meta_widths.get(worktree).copied().unwrap_or(WELL_XS);
        let end = px(10.0) + trailing + ICON_GAP;

        let rows: Vec<_> = subagents
            .iter()
            .enumerate()
            .map(|(i, sub)| {
                let key = SharedString::from(format!("sub-{worktree}-{}", sub.id));
                let label = SharedString::from(sub.label.clone());
                let (node, ink) = match sub.state {
                    SubagentState::Working => {
                        (div().bg(paint(t.status.running)), paint(t.text.dim))
                    }
                    SubagentState::Blocked => (
                        div().bg(paint(t.status.attention)),
                        paint(t.status.attention),
                    ),
                    // Finished, or waiting to be told something: hollow, so a
                    // column of dots still reads as which ones are live.
                    SubagentState::Idle => (
                        div().border_1().border_color(alpha(paint(t.text.dim), 0.7)),
                        alpha(paint(t.text.dim), 0.7),
                    ),
                };
                let age = now.saturating_sub(sub.started_at_ms);
                let pane = sub.pane.clone();

                div()
                    .id(ElementId::from(key.clone()))
                    .relative()
                    .flex()
                    .items_center()
                    .h(SUBAGENT_ROW_H)
                    .pl(node_x)
                    .pr(end)
                    .text_size(px(11.0))
                    // No hover and no pointer: a click here lands where a
                    // click on the worktree above would, so a row that lit up
                    // promised something of its own and then did not do it.
                    // ╰ from the guide into this row, turning into the dot.
                    .child(
                        div()
                            .absolute()
                            .left(axis - px(0.5))
                            .top_0()
                            .h(SUBAGENT_ROW_H / 2.0 + px(0.5))
                            .w(ELBOW_RUN + px(0.5))
                            .border_l_1()
                            .border_b_1()
                            .border_color(line)
                            .rounded_bl(ELBOW_RADIUS),
                    )
                    // │ on to the next sibling. Not under the last, which is
                    // where the tree ends.
                    .when(i < last, |el| {
                        el.child(
                            div()
                                .absolute()
                                .left(axis - px(0.5))
                                .top(SUBAGENT_ROW_H / 2.0)
                                .bottom_0()
                                .w(px(1.0))
                                .bg(line),
                        )
                    })
                    .child(
                        node.flex_none()
                            .size(NODE)
                            .rounded_full()
                            .mr(label_x - node_x - NODE),
                    )
                    .child(
                        div().flex().flex_1().min_w_0().prose().child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ink)
                                .child(label),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(6.0))
                            .ml(px(10.0))
                            .text_size(px(10.0))
                            .text_color(paint(t.text.dim))
                            .children(sub.model.clone())
                            .child(
                                div()
                                    // A subagent that is waiting on you has
                                    // been waiting this long, which is the
                                    // number worth noticing.
                                    .when(sub.state == SubagentState::Blocked, |el| {
                                        el.text_color(paint(t.status.attention))
                                    })
                                    .child(if age < 60_000 {
                                        "<1m".to_owned()
                                    } else {
                                        crate::status_bar::compact_duration(age)
                                    }),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(
                            Selection {
                                project: pi,
                                worktree: wi,
                            },
                            cx,
                        );
                        this.focus_agent_pane(pane.as_deref());
                        cx.notify();
                    }))
            })
            .collect();

        div()
            .flex_none()
            .flex()
            .flex_col()
            .pb(px(5.0))
            .children(rows)
            .into_any_element()
    }
}

fn promote_worked_in(
    workspace: &Workspace,
    project: &ProjectId,
    mut managed: Vec<ket_core::worktree::Worktree>,
    discovered: &mut Vec<ket_core::worktree::DiscoveredWorktree>,
) -> Vec<ket_core::worktree::Worktree> {
    let paths: Vec<&std::path::Path> = discovered
        .iter()
        .map(|found| found.path.as_path())
        .collect();
    if paths.is_empty() {
        return managed;
    }

    let worked_in = ket_core::sessions::Sessions::new().scan(&paths);
    if worked_in.is_empty() {
        return managed;
    }

    let mut adopted = Vec::new();
    discovered.retain(|found| {
        if !worked_in.contains_key(&found.path) {
            return true;
        }
        match workspace.adopt_worktree(project, &found.path) {
            Ok(worktree) => {
                adopted.push(worktree);
                false
            }
            Err(e) => {
                tracing::debug!(path = %found.path.display(), %e, "not promoting");
                true
            }
        }
    });

    managed.extend(adopted);
    managed.sort_by_key(|worktree| std::cmp::Reverse(worktree.created_at_ms));
    managed
}

impl Shell {
    /// Builds the tree from core.
    pub(crate) fn load_projects(workspace: &Workspace) -> Vec<ProjectNode> {
        let Ok(projects) = workspace.projects() else {
            return Vec::new();
        };

        projects
            .into_iter()
            .map(|project| {
                let report = workspace.worktree_report(Some(&project.id)).ok();
                let (managed, mut discovered) = match report {
                    Some(report) => (report.managed, report.discovered),
                    None => (Vec::new(), Vec::new()),
                };
                let settings = workspace.project_settings(&project.id).unwrap_or_default();

                // A checkout an agent has a conversation in is one you have
                // worked in, and it belongs in the project rather than in the
                // list of things git merely knows about. Promoted on load, so
                // the row you want is where you left it instead of behind a
                // disclosure you have to open and read through first.
                //
                // Gated on the setting, and the order matters: promotion
                // *writes* to the registry, so running it before the check let
                // `hide_discovered_worktrees` hide the rows while still
                // adopting every one of them. Someone who has said they do not
                // want git's other checkouts in this project has said it about
                // the record, not just the display.
                let managed = if settings.hide_discovered_worktrees {
                    discovered.clear();
                    managed
                } else {
                    promote_worked_in(workspace, &project.id, managed, &mut discovered)
                };

                let mut node = ProjectNode {
                    name: settings.name_for(&project).to_owned().into(),
                    color: settings
                        .color
                        .as_deref()
                        .and_then(|hex| Color::from_hex(hex).ok())
                        .unwrap_or_else(default_color),
                    icon: settings.icon.clone().map(SharedString::from),
                    hide_discovered: settings.hide_discovered_worktrees,
                    agent_overrides: settings.agents.clone(),
                    expanded: true,
                    worktrees: std::iter::once({
                        // Not in the registry `worktree_status` reads — this
                        // is the repository's own checkout, not a worktree
                        // ket created — so its status is read straight off
                        // the path instead. Skipped when the checkout itself
                        // is gone: opening a missing repository is the error
                        // `missing` already exists to report.
                        let primary_missing = !project.root.is_dir();
                        let primary_status = (!primary_missing)
                            .then(|| ket_core::status::of_worktree(&project.root, None).ok())
                            .flatten();

                        WorktreeNode {
                            // A stable id derived from the project, so its
                            // tabs and its terminal survive a reload the same
                            // way a real worktree's do. Nothing in core ever
                            // sees it.
                            id: WorktreeId::new(format!("primary-{}", project.id)),
                            line_changes: None,
                            changes: primary_status.as_ref().map(changed_paths).unwrap_or(0),
                            marks: std::sync::Arc::new(
                                primary_status
                                    .as_ref()
                                    .map(ChangeMarks::of)
                                    .unwrap_or_default(),
                            ),
                            base: None,
                            branch: project.default_base.clone().into(),
                            name: None,
                            agent: None,
                            dirty: primary_status.as_ref().is_some_and(|s| s.total_changes > 0),
                            in_progress: primary_status.and_then(|s| s.in_progress),
                            token_reduction: ket_core::worktree::default_token_reduction(),
                            pinned: false,
                            missing: primary_missing,
                            path: project.root.clone(),
                            primary: true,
                            // The base branch is not a thing that gets merged
                            // away, and a badge on it would say nothing.
                            merged: false,
                        }
                    })
                    .chain(managed.iter().map(|worktree| {
                        // A status that will not read is not an error worth a
                        // row saying so — the light simply stays off and the
                        // operation badge simply does not appear.
                        let status = workspace.worktree_status(&worktree.id).ok();
                        let dirty = status.as_ref().is_some_and(|s| s.total_changes > 0);
                        let changes = status.as_ref().map(changed_paths).unwrap_or(0);
                        let marks = std::sync::Arc::new(
                            status.as_ref().map(ChangeMarks::of).unwrap_or_default(),
                        );
                        let in_progress = status.and_then(|s| s.in_progress);
                        let base = worktree.base_rev().map(|rev| {
                            // A base named `HEAD` resolved to a commit when the
                            // worktree was cut; the commit is all there is to
                            // call it by.
                            let label = if worktree.base != "HEAD" && !worktree.base.is_empty() {
                                worktree.base.clone()
                            } else {
                                rev.chars().take(7).collect()
                            };
                            (
                                SharedString::from(label),
                                SharedString::from(rev.to_owned()),
                            )
                        });

                        WorktreeNode {
                            id: worktree.id.clone(),
                            branch: worktree.branch.clone().into(),
                            name: worktree.name.clone().map(SharedString::from),
                            line_changes: None,
                            changes,
                            marks,
                            base,
                            agent: worktree.agent.clone().map(SharedString::from),
                            dirty,
                            in_progress,
                            token_reduction: ket_core::worktree::resolve_level(
                                worktree.token_reduction,
                                worktree.token_reduction_id.as_deref(),
                                worktree.token_reduction_pack_id.as_deref(),
                            ),
                            pinned: worktree.pinned,
                            missing: !worktree.exists(),
                            path: worktree.path.clone(),
                            primary: false,
                            // Carried over by `Shell::reload`, so a reload does
                            // not blink every badge off until the next sweep.
                            merged: false,
                        }
                    }))
                    .collect(),
                    discovered: discovered
                        .iter()
                        .map(|found| DiscoveredNode {
                            label: found
                                .branch
                                .clone()
                                .unwrap_or_else(|| "detached".to_owned())
                                .into(),
                            path: found.path.display().to_string().into(),
                        })
                        .collect(),
                    discovered_expanded: false,
                    backlog_seen: ket_core::backlog::Backlog::modified(&project.id),
                    backlog: ket_core::backlog::Backlog::load(&project.id)
                        .map(|backlog| backlog.open_count())
                        .unwrap_or(0),
                    id: project.id,
                };
                // Pinned rows lead the project, behind only the repository's own
                // checkout. Stable, so everything else keeps the order it had.
                node.worktrees.sort_by_key(|w| (!w.primary, !w.pinned));
                node
            })
            .collect()
    }

    /// Where the selected row lives on disk.
    ///
    /// Read off the node rather than looked up in the workspace: the
    /// repository's own checkout is a row without being a worktree core knows
    /// about, so a lookup would come back empty for exactly the row people
    /// click first.
    pub(crate) fn selected_path(&self) -> Option<std::path::PathBuf> {
        let selection = self.selection?;
        self.projects
            .get(selection.project)?
            .worktrees
            .get(selection.worktree)
            .map(|node| node.path.clone())
    }

    /// A worktree's token-reduction level, for the agent about to launch in
    /// it.
    ///
    /// Looked up by id rather than off the current selection: the terminal a
    /// tab opens is not always the one currently selected, and a level that
    /// silently followed the wrong row would be worse than one that fell
    /// back to the default.
    pub(crate) fn token_reduction_for(&self, id: &WorktreeId) -> u8 {
        self.projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|node| &node.id == id)
            .map(|node| node.token_reduction)
            .unwrap_or_else(ket_core::worktree::default_token_reduction)
    }

    /// Selects a worktree and loads its space.
    pub(crate) fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        let Some(node) = self
            .projects
            .get(selection.project)
            .and_then(|p| p.worktrees.get(selection.worktree))
        else {
            return;
        };
        let id = node.id.clone();
        let missing = node.missing;
        let project_id = self.projects[selection.project].id.clone();
        self.selection = Some(selection);
        self.project_focus.insert(project_id, id.clone());
        self.menu = None;
        // The strip a rename field is drawn in belongs to the worktree being
        // left, and it goes with it. Dropped rather than carried across,
        // because the field is the only thing holding the keyboard and one
        // left behind on a strip nothing draws is a field nothing can finish.
        self.tab_rename = None;
        // Choosing a worktree hands the file panel back to the selection.
        // Without this, clicking the repository's own checkout would pin the
        // panel there and every worktree picked afterwards would show that
        // checkout's files instead of its own.
        self.explorer.pinned = false;
        // Whether this worktree needs a shell opening for it.
        //
        // Deliberately "has no terminal", not "has never been visited". A
        // restored layout creates the space before anything is selected, so a
        // first-visit test is false for every worktree that has ever been
        // arranged — and since a terminal cannot survive a restart, that meant
        // clicking such a worktree opened nothing at all, and the session
        // resume that rides along with opening never ran either.
        self.spaces.entry(id.clone()).or_default();
        // A shell cannot start in a directory that is not there. Spawning one
        // anyway is what produced an empty pane with nothing explaining it, so
        // the row says what is wrong instead and leaves the space alone — the
        // record is still selectable, which is what makes it removable.
        // Terminal tabs the saved layout wants back — with the ket host, which
        // may still be running them. One that ran an agent is the session this
        // worktree would otherwise open, so its being there answers that.
        let restores = match missing {
            true => Vec::new(),
            false => self.owed_restores.remove(&id).unwrap_or_default(),
        };
        // Settings → General can turn this off. Asked last, so arriving at a
        // worktree that already has a terminal never reads the file.
        let needs_terminal = !missing
            && !restores.iter().any(|restore| restore.agent.is_some())
            && self
                .spaces
                .get(&id)
                .is_some_and(|space| space.terminal_ids().is_empty())
            && ket_core::config::Config::load().map_or(true, |c| c.general.open_terminal);

        // Panes a saved layout left owing a shell — see `Shell::owed_terminals`.
        // Taken rather than read, so a worktree revisited later does not get a
        // second shell in every pane; and only once the directory is known to
        // be there, since a shell cannot start in one that is not.
        let owed = match missing {
            true => Vec::new(),
            false => self.owed_terminals.remove(&id).unwrap_or_default(),
        };

        tracing::debug!(
            worktree = %id,
            path = %node.path.display(),
            missing,
            needs_terminal,
            "selecting a worktree"
        );

        if missing {
            self.note = Some(
                format!(
                    "{} is gone from disk — remove it from the project, or restore the directory",
                    node.path.display()
                )
                .into(),
            );
            return;
        }

        // Opening a worktree puts you in it, which means a shell in its
        // directory — the thing you were going to open anyway. Only when it has
        // none: coming back to a worktree that already has one should give you
        // back your arrangement, not stack another terminal on it.
        self.note = None;
        self.open_owed_terminals(needs_terminal, &owed, cx);
        self.restore_terminals(restores, cx);
    }

    /// Takes a discovered worktree into its project, then opens it.
    ///
    /// A discovered row is a checkout git knows about and ket does not —
    /// usually one made by hand or by another tool. Registering it is the whole
    /// difference between the two kinds of row, so a click does that and then
    /// behaves exactly as if the row had always been managed: reload, find it
    /// by id, select it, which opens its shell and resumes whatever session its
    /// agent left behind.
    pub(crate) fn adopt_discovered(
        &mut self,
        project: usize,
        which: usize,
        cx: &mut Context<Self>,
    ) {
        let Some((project_id, path)) = self.projects.get(project).and_then(|node| {
            let found = node.discovered.get(which)?;
            Some((
                node.id.clone(),
                std::path::PathBuf::from(found.path.as_ref()),
            ))
        }) else {
            return;
        };

        let Ok(workspace) = Workspace::open() else {
            return;
        };

        let adopted = match workspace.adopt_worktree(&project_id, &path) {
            Ok(worktree) => worktree.id,
            Err(e) => {
                // A detached checkout, or one git has since dropped. Adoption
                // is refused, so the click falls back to what it used to do —
                // show the files — and says why it could do no more. Either
                // way it accounts for itself; doing nothing is what was wrong
                // with the old behaviour.
                self.note = Some(format!("could not add {}: {e}", path.display()).into());
                self.open_files_at(path, cx);
                return;
            }
        };

        self.reload(cx);
        // Otherwise this row sits with no size at all until the next
        // launch's sweep, the same gap `create_worktree` closes for a
        // freshly-created one.
        self.remeasure_worktree(adopted.clone(), cx);

        let Some(found) = self
            .projects
            .iter()
            .position(|node| node.id == project_id)
            .and_then(|pi| {
                let wi = self.projects[pi]
                    .worktrees
                    .iter()
                    .position(|node| node.id == adopted)?;
                Some(Selection {
                    project: pi,
                    worktree: wi,
                })
            })
        else {
            return;
        };

        self.select(found, cx);
    }

    /// Reloads the tree, keeping what the reader had expanded.
    ///
    /// Expansion is remembered by project **id**, not by position. Projects are
    /// ordered by recency, so acting on one reorders the list — keyed by index,
    /// a refresh would silently collapse the project you were working in and
    /// expand some other one.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let Ok(workspace) = Workspace::open() else {
            return;
        };

        let was: Vec<(ProjectId, bool, bool)> = self
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.expanded, p.discovered_expanded))
            .collect();
        let selected = self.selected_id();

        // Merged state is swept on its own slow cadence, so a reload that
        // dropped it would blink every badge off until the next sweep came
        // round — up to a minute of the sidebar claiming work is unfinished.
        let merged: Vec<WorktreeId> = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .filter(|node| node.merged)
            .map(|node| node.id.clone())
            .collect();

        self.projects = Self::load_projects(&workspace);

        for node in &mut self.projects {
            for worktree in &mut node.worktrees {
                worktree.merged = merged.contains(&worktree.id);
            }
            if let Some((_, expanded, discovered)) = was.iter().find(|(id, _, _)| id == &node.id) {
                node.expanded = *expanded;
                node.discovered_expanded = *discovered;
            }
        }

        // The selection is re-found by worktree id, not kept by position:
        // removing a project shifts every index after it, and a selection
        // kept by index would land on some other project's worktree.
        self.selection = selected.and_then(|id| self.position_of(&id));
        if let Some(selection) = self.selection {
            self.select(selection, cx);
        }
        if self
            .project_menu
            .as_ref()
            .is_some_and(|menu| menu.project >= self.projects.len())
        {
            self.project_menu = None;
        }
        // A level just changed, or a worktree came or went: either moves what
        // the savings rollup lists.
        self.refresh_savings();
    }

    /// Starts the hook listener and registers ket with the agents.
    ///
    /// Writing into another application's configuration is not something to do
    /// on every launch, so the installers are asked first and only run when
    /// ket is not already registered. A failure here is not fatal and not worth
    /// a modal: it costs live status, and the scanner and the terminal's
    /// foreground still answer.
    /// Puts the cached pack in force, and asks for a fresher one in the
    /// background.
    ///
    /// Cache first and network never: the levels a launch uses come off the
    /// disk, synchronously, before anything renders. A fetch that succeeds
    /// writes the cache for *next* time and changes nothing about this run —
    /// see `ket_core::worktree::activate_pack` for why the table does not move
    /// under a session that is already reading it.
    ///
    /// With no pack URL configured this does nothing at all, makes no network
    /// request, and leaves the built-in levels in force. That is every install
    /// until someone sets one.
    pub(crate) fn activate_pack(&mut self, cx: &mut Context<Self>) {
        // Litter from a process killed between the temp write and the rename.
        // Here rather than on its own timer: the cache directory is about to be
        // read anyway, and this is the one moment ket is already thinking about
        // these files.
        ket_core::pack::sweep_temps();

        if let Some(pack) = ket_core::pack::load() {
            let id = pack.pack_id.clone();
            let version = pack.pack_version;
            // Named before it is activated: a record stamped between the two
            // would claim the built-in table while reading a pack's levels.
            if ket_core::worktree::activate_pack(
                id.clone(),
                version,
                pack.default_level(),
                pack.levels(),
            ) {
                tracing::info!(pack = %id, version, "level pack in force");
            }
            self.active_pack = Some(pack);
        }

        let Some(url) = Workspace::open()
            .ok()
            .and_then(|workspace| workspace.config().pack.url.clone())
        else {
            self.pack_refresh_status = crate::PackRefreshStatus::NotConfigured;
            return;
        };

        self.pack_refresh_status = crate::PackRefreshStatus::Checking;
        let refresh = cx
            .background_executor()
            .spawn(async move { ket_core::pack::refresh(&url) });
        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let result = refresh.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.pack_refresh_status = match result {
                    Ok(pack) => {
                        tracing::info!(
                            pack = %pack.pack_id,
                            version = pack.pack_version,
                            "level pack fetched"
                        );
                        crate::PackRefreshStatus::Succeeded
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not refresh the level pack");
                        crate::PackRefreshStatus::Failed(error.to_string())
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Works out the telemetry variables once, for every pane this run opens.
    ///
    /// Config is read here rather than at each launch because the answer cannot
    /// change without a restart: an agent's environment is fixed when its
    /// process starts, so a collector configured mid-session reaches nothing
    /// already running either way.
    ///
    /// With no endpoint configured this leaves the map empty and no agent is
    /// launched with a single OTel variable — which is every install until
    /// someone sets one.
    /// Puts the organisation's rate table in force, when one is configured.
    ///
    /// A `OnceLock` in `ket_core::rates` rather than a field here, for the
    /// reason the pack's levels are one: the card reads rates from several
    /// places that have no path back to this struct, and money must not change
    /// denomination under a reader mid-session.
    ///
    /// Unconfigured — every install until someone fills in a contract — leaves
    /// ket quoting nothing in dollars anywhere.
    pub(crate) fn activate_rates(&mut self, workspace: &Workspace) {
        let rates = workspace.config().rates.clone();
        let models: Vec<String> = rates
            .models
            .iter()
            .filter(|rate| rate.usable())
            .map(|rate| rate.model.clone())
            .collect();
        if ket_core::rates::activate(rates) {
            tracing::info!(?models, "contracted rates in force");
        }
    }

    pub(crate) fn configure_telemetry(&mut self, workspace: &Workspace) {
        self.telemetry_env = workspace.config().telemetry.env();
        if !self.telemetry_env.is_empty() {
            tracing::info!(
                endpoint = self.telemetry_env["OTEL_EXPORTER_OTLP_ENDPOINT"],
                "exporting usage telemetry"
            );
        }
    }

    pub(crate) fn install_agent_hooks(&mut self) {
        self.hook_token = agent_hooks::new_token();

        match Listener::start(self.hook_token.clone()) {
            Ok(listener) => self.hooks = Some(listener),
            Err(e) => {
                tracing::warn!(%e, "agent hooks: could not listen; status will be inferred");
                return;
            }
        }

        let script = match agent_hooks::install_script() {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(%e, "agent hooks: could not write the hook script");
                return;
            }
        };

        let installers: [Box<dyn agent_hooks::HookInstaller>; 3] = [
            Box::new(agent_hooks::CodexHooks::default()),
            Box::new(agent_hooks::OpenCodeHooks::default()),
            Box::new(agent_hooks::GrokHooks::default()),
        ];
        let claude = claude_hook_installers();
        for installer in claude.into_iter().chain(installers) {
            install_hooks(installer.as_ref(), &script);
        }

        self.install_status_line();
        self.pick_up_spool();
    }

    /// Puts ket's script in front of Claude's status line, for the quota in it.
    ///
    /// Kept apart from the hooks above because it is a different kind of
    /// intervention and fails differently: a hook ket cannot install is a row
    /// that stays dim, and a status line ket installs badly is a line the
    /// person is looking at. See [`agent_hooks::ClaudeStatusLine`], which
    /// carries whatever it displaced rather than replacing it.
    fn install_status_line(&mut self) {
        let status_line = agent_hooks::ClaudeStatusLine::default();
        let installed = match status_line.is_installed() {
            Ok(installed) => installed,
            Err(e) => {
                tracing::warn!(%e, "status line: unreadable settings");
                return;
            }
        };

        // Written on every launch, not only the first. The settings entry and
        // the script are two different things to be out of date, and only the
        // settings entry is what `is_installed` sees: returning early on it
        // left everyone who had ever installed ket running whichever script
        // their first launch wrote, forever. A field added to the envelope
        // would then reach new readers only.
        let script = match agent_hooks::install_statusline_script() {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(%e, "status line: could not write the script");
                return;
            }
        };

        if installed {
            return;
        }

        if let Err(e) = status_line.install(&script) {
            tracing::warn!(%e, "status line: install failed");
        }
    }

    /// Reads anything the hooks spooled while ket was not listening.
    ///
    /// This is the reopened-app case the spool exists for: an agent that
    /// reported while the window was closed should not have that history
    /// vanish because nothing was there to receive it.
    fn pick_up_spool(&mut self) {
        let path = agent_hooks::spool_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };

        self.sync_status_worktrees();
        // Filed under the panes they name, although no terminal of this run
        // has those keys yet: the spool is exactly the reports whose pane was
        // not listening, and dropping them as strangers would lose what it
        // was kept for. They age out on the ordinary linger.
        for report in agent_hooks::reports_from_spool(&text) {
            self.activity.report(&report, |_| true);
        }

        // Taken once. Leaving it would replay the same history on every launch.
        let _ = std::fs::remove_file(&path);
    }

    /// Takes whatever the agents have reported since it was last asked.
    ///
    /// Called two ways. The periodic tick passes `full`, and also does the
    /// work that is worth doing on a timer whether or not anything arrived —
    /// following Codex rollouts, weighing instruction files. The quick wake
    /// between ticks does not, and returns at once when nothing is new, so a
    /// report lands on screen when it arrives rather than up to a tick later.
    /// Returns whether anything did.
    pub(crate) fn drain_hook_reports(&mut self, full: bool) -> bool {
        // Two sources: this window's own listener, for agents it runs itself,
        // and the ket host, for agents in terminals it runs — which report to
        // the host, since the window that launched them may be gone.
        let local = self
            .hooks
            .as_ref()
            .map(|listener| listener.drain())
            .unwrap_or_default();
        let hosted = ket_core::host::take_hook_reports();
        let mut statuslines = self
            .hooks
            .as_ref()
            .map(|listener| listener.drain_statuslines())
            .unwrap_or_default();
        statuslines.extend(ket_core::host::take_status_lines());
        // The host's status is the status of its terminals, whole. Its raw
        // hooks still come, but only for what is not status: this window
        // applying them as well would be a second authority on the same
        // panes, drifting from the first.
        let snapshot = ket_core::host::take_agent_status()
            .is_some_and(|snapshot| self.activity.accept_snapshot(snapshot));

        let arrived = !local.is_empty() || !hosted.is_empty() || !statuslines.is_empty();
        if !full && !arrived {
            return snapshot;
        }

        if !local.is_empty() || !hosted.is_empty() {
            // Before routing anything: a report arriving ahead of the first
            // tick would otherwise find no worktrees to be placed in.
            self.sync_status_worktrees();
        }
        if !local.is_empty() {
            let known = self.pane_keys();
            for report in &local {
                self.activity.report(report, |pane| known.contains(pane));
            }
        }

        let now = ket_core::now_ms();
        let mut counted = Vec::new();
        let mut codex_seen = std::collections::BTreeSet::new();
        for mut report in local.into_iter().chain(hosted) {
            self.activity.route(&mut report);
            // Turns and tool calls, which no status line reports: they are what
            // the spend has to be divided by before two sessions of different
            // sizes can be compared at all.
            // Codex has no status line, so a hook report is the only sign that
            // there is a rollout worth following in this worktree.
            if report.agent.eq_ignore_ascii_case("codex")
                && let Some(worktree) = report.worktree.as_deref()
            {
                codex_seen.insert(WorktreeId::new(worktree.to_owned()));
            }
            if let (Some(session), Some(worktree)) =
                (report.session.clone(), report.worktree.clone())
            {
                counted.push((
                    report.event,
                    session,
                    WorktreeId::new(worktree),
                    report.tool_name.clone(),
                ));
            }
        }
        for (event, session, worktree, tool) in counted {
            let level = self.token_reduction_for(&worktree);
            match event {
                agent_hooks::HookEvent::UserPrompt => self
                    .usage_history
                    .note_turn(&session, &worktree, level, now),
                agent_hooks::HookEvent::PreTool => {
                    // `PreTool` rather than `PostTool`: a call the model asked
                    // for has already cost its schema's place in the prefix,
                    // whether or not it came back.
                    let server = tool.as_deref().and_then(agent_hooks::mcp_server);
                    self.usage_history
                        .note_tool_call(&session, &worktree, level, server, now)
                }
                _ => {}
            }
        }

        // Two readings out of one payload, from a session that was drawing its
        // status line anyway. Most payloads carry no quota and are dropped
        // without disturbing the numbers already cached — see
        // `snapshot_from_statusline`.
        for line in statuslines {
            if let Some(snapshot) = ket_core::rate_limits::snapshot_from_statusline(&line.payload) {
                self.rate_limits.accept(snapshot);
            }

            // The account's quota is the same wherever it came from; what the
            // session itself is using is not, so this half needs to know which
            // worktree drew it. A payload without one is still worth its quota
            // and cannot be worth anything here.
            let Some(worktree) = line.worktree.as_ref().map(|id| WorktreeId::new(id.clone()))
            else {
                continue;
            };
            let Some(reading) = ket_core::usage::reading_from_statusline(&line.payload) else {
                continue;
            };

            if let Some(session) = reading.session.as_deref() {
                self.usage_history.observe(
                    session,
                    &worktree,
                    self.token_reduction_for(&worktree),
                    &reading,
                    now,
                );
            }

            let pane = line.pane.as_deref().and_then(|pane| self.pane_for(pane));
            self.session_usage.entry(worktree).or_default().record(
                crate::usage_card::SessionReading {
                    agent: line.agent,
                    pane,
                    reading,
                    at_ms: now,
                    restored: false,
                },
            );
        }

        for worktree in codex_seen {
            self.follow_codex(&worktree);
        }
        if full {
            self.weigh_context();
            self.poll_codex_rollouts(now);
        }

        self.save_usage_history(now);
        arrived || snapshot
    }

    /// The pane keys of this window's terminals, for filing what reaches its
    /// own listener. See [`ket_core::agent_status::StatusStore::report`].
    ///
    /// Hosted terminals too. Their agents normally report to the host, which
    /// hands them its own listener's address; one reaching this window's is
    /// from a host that could not start a listener, left its agents with
    /// ours, and so holds no status for them — this window is the only one
    /// hearing them.
    fn pane_keys(&self) -> std::collections::HashSet<String> {
        self.terminals
            .keys()
            .map(|id| {
                self.terminal_keys
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| id.0.to_string())
            })
            .collect()
    }

    /// Tells both status owners — this window's store and the ket host's —
    /// where the worktrees are, so a Codex report can be placed by its
    /// working directory. Only when the list has changed: the store resolves
    /// each path on disk when it takes a new one.
    pub(crate) fn sync_status_worktrees(&mut self) {
        let worktrees: Vec<(String, std::path::PathBuf)> = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .map(|node| (node.id.to_string(), node.path.clone()))
            .collect();
        if worktrees == self.status_worktrees {
            return;
        }
        self.activity.set_worktrees(worktrees.clone());
        ket_core::host::publish_worktrees(worktrees.clone());
        self.status_worktrees = worktrees;
    }

    /// Reads the instruction files of any worktree now reporting, once each.
    ///
    /// Once per worktree per run, on a tick rather than a repaint — see
    /// `Shell::context_weights`. A worktree ket has no reading for is skipped:
    /// nothing is going to draw the answer, so nothing should pay for it.
    fn weigh_context(&mut self) {
        let wanted: Vec<WorktreeId> = self
            .session_usage
            .keys()
            .filter(|id| !self.context_weights.contains_key(*id))
            .cloned()
            .collect();

        for worktree in wanted {
            let Some(path) = self
                .projects
                .iter()
                .flat_map(|project| project.worktrees.iter())
                .find(|node| node.id == worktree)
                .map(|node| node.path.clone())
            else {
                continue;
            };
            self.context_weights
                .insert(worktree, ket_core::worktree::context_weight(&path));
        }
    }

    /// Starts following a worktree's Codex rollout, if it is not already.
    ///
    /// The walk of Codex's dated store happens once per worktree here, not per
    /// poll: the tailer keeps the path it found.
    fn follow_codex(&mut self, worktree: &WorktreeId) {
        if self.codex_rollouts.contains_key(worktree) {
            return;
        }
        let Some(path) = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .find(|node| &node.id == worktree)
            .map(|node| node.path.clone())
        else {
            return;
        };
        if let Some(rollout) = ket_core::sessions::CodexRollout::for_directory(&path) {
            self.codex_rollouts.insert(worktree.clone(), rollout);
        }
    }

    /// Reads whatever each followed rollout has appended, and files it the same
    /// way a status line's reading is filed.
    ///
    /// The two agents converge here: a Codex reading and a Claude one become the
    /// same [`ket_core::usage::Reading`], land in the same `session_usage`, and
    /// are drawn by the same card. Nothing downstream asks which agent it came
    /// from — see `ket_core::usage::reading_from_codex` for what Codex cannot
    /// fill in, and why those stay empty rather than zero.
    fn poll_codex_rollouts(&mut self, now: u64) {
        let mut filed = Vec::new();
        for (worktree, rollout) in self.codex_rollouts.iter_mut() {
            let Some(tokens) = rollout.poll() else {
                continue;
            };
            let Some(reading) = ket_core::usage::reading_from_codex(
                &tokens,
                rollout.session(),
                rollout.cwd(),
                rollout.model(),
                rollout.effort(),
            ) else {
                continue;
            };
            filed.push((worktree.clone(), reading));
        }

        for (worktree, reading) in filed {
            if let Some(session) = reading.session.as_deref() {
                self.usage_history.observe(
                    session,
                    &worktree,
                    self.token_reduction_for(&worktree),
                    &reading,
                    now,
                );
            }
            self.session_usage.entry(worktree).or_default().record(
                crate::usage_card::SessionReading {
                    agent: Some("codex".to_owned()),
                    // Codex's own transcript says nothing about which pane ket
                    // opened it in. The hook reports carry that, and joining the
                    // two is work for the day something needs it.
                    pane: None,
                    reading,
                    at_ms: now,
                    restored: false,
                },
            );
        }
    }

    /// Reads a pane key back into the id that wrote it.
    ///
    /// The key crosses into a shell script and back as text, so a value that
    /// is not a number is a session launched by something other than this
    /// version of ket, not an error worth reporting.
    /// The terminal an agent's hooks mean by `pane` — see
    /// [`ket_core::agent_hooks::PANE_ENV`].
    ///
    /// The terminal's key, for anything launched since the key existed: it is
    /// kept across a restart, so an agent the ket host kept running still
    /// finds its tab. A bare number is the tab's id in the window that
    /// launched it, from before.
    pub(crate) fn pane_for(&self, pane: &str) -> Option<crate::terminal::TerminalId> {
        self.terminal_keys
            .iter()
            .find(|(_, key)| key.as_str() == pane)
            .map(|(terminal, _)| *terminal)
            .or_else(|| pane.parse().ok().map(crate::terminal::TerminalId))
    }

    /// Writes the usage history out, at most every
    /// [`USAGE_SAVE_INTERVAL`](crate::tree::USAGE_SAVE_INTERVAL).
    ///
    /// The file is rewritten whole, and a status line arrives several times a
    /// second — so this is debounced rather than written on every reading. A
    /// failure is logged and dropped: losing a spending record is not a reason
    /// to interrupt anyone's work, and the next tick will try again.
    fn save_usage_history(&mut self, now_ms: u64) {
        if !self.usage_history.dirty()
            || now_ms.saturating_sub(self.usage_saved_at_ms)
                < USAGE_SAVE_INTERVAL.as_millis() as u64
        {
            return;
        }
        self.usage_saved_at_ms = now_ms;
        if let Err(e) = self.usage_history.save() {
            tracing::warn!(error = %e, "could not write the usage history");
        }
    }

    /// The environment an agent needs to report back to ket.
    ///
    /// Correlation is by injection, not by guessing: the hook echoes these
    /// back with its payload, which is how a report finds its worktree.
    pub(crate) fn hook_env(
        &self,
        worktree: &WorktreeId,
        agent: &str,
        pane: crate::terminal::TerminalId,
    ) -> std::collections::BTreeMap<String, String> {
        let mut env = std::collections::BTreeMap::new();
        let Some(listener) = self.hooks.as_ref() else {
            return env;
        };

        env.insert(
            agent_hooks::PORT_ENV.to_owned(),
            listener.port().to_string(),
        );
        env.insert(agent_hooks::TOKEN_ENV.to_owned(), self.hook_token.clone());
        env.insert(agent_hooks::WORKTREE_ENV.to_owned(), worktree.to_string());
        env.insert(
            agent_hooks::PANE_ENV.to_owned(),
            self.terminal_keys
                .get(&pane)
                .cloned()
                .unwrap_or_else(|| pane.0.to_string()),
        );
        env.insert(
            agent_hooks::VERSION_ENV.to_owned(),
            agent_hooks::HOOK_VERSION.to_owned(),
        );
        env.insert("KET_AGENT_NAME".to_owned(), agent.to_owned());
        env
    }

    /// Re-reads what every worktree is doing.
    ///
    /// Two sources, per [`ket_core::activity`]: the session store, which knows
    /// about agents ket launched, and each open terminal's foreground process,
    /// which is the only evidence of an agent a person started themselves.
    ///
    /// Runs on the same tick as the quota poll. Both are "ask, because nothing
    /// will tell you", and giving them separate timers would mean two wakeups
    /// to answer one repaint.
    pub(crate) fn refresh_activity(&mut self, cx: &mut Context<Self>) {
        self.sync_status_worktrees();
        // Work a phone asked this window to start, handed over by the host,
        // and backlog notes a phone wrote through it.
        self.take_phone_work(cx);
        self.refresh_backlog_counts();

        // A worktree can have several terminals now, so its foreground is the
        // busiest of them: one pane running a build is the worktree being busy,
        // whatever the other three are sitting at.
        let foreground_span = crate::frametrace::tick("  activity:foreground");
        for node in self.projects.iter().flat_map(|p| p.worktrees.iter()) {
            let busiest = self
                .spaces
                .get(&node.id)
                .map(|space| space.terminal_ids())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|id| self.terminals.get(&id))
                .filter_map(|handle| handle.term.foreground())
                .reduce(|a, b| match (&a, &b) {
                    (Foreground::Running(_), _) => a,
                    _ => b,
                });

            self.activity.observe(&node.id, busiest);
        }
        drop(foreground_span);

        // The store is shared with every other `ket` process, so this is a read
        // of the truth rather than a cache of it. A failure here is not worth a
        // note in the window: the next tick is ten seconds away.
        {
            let _t = crate::frametrace::tick("  activity:store");
            if let Ok(workspace) = Workspace::open()
                && let Ok(sessions) = workspace.sessions(None)
            {
                self.activity.sync_sessions(sessions);
            }
        }

        {
            let _t = crate::frametrace::tick("  activity:records");
            self.refresh_records(cx);
        }
        {
            let _t = crate::frametrace::tick("  activity:git");
            // Only what has a reason to be read — see `crate::git_watch`,
            // which also refreshes the Git view and an open diff when the
            // worktree they show is read.
            self.follow_selected_worktree(cx);
            self.read_git_status(cx);
            // Bookkeeping only: diff tabs closed by a route that did not
            // come through `close_tab_now`.
            self.prune_diffs();
            self.refresh_merged(cx);
        }
        {
            let _t = crate::frametrace::tick("  activity:file_tree");
            self.refresh_file_tree();
        }
    }

    /// Re-asks which worktrees hold nothing their base does not already have.
    ///
    /// Its own sweep rather than a field on the status one, and on its own
    /// much slower clock — see [`MERGED_SWEEP_INTERVAL`] for why the two
    /// cannot share a cadence.
    ///
    /// What this drives is the whole reason it exists: a merged worktree and a
    /// live one are otherwise identical in the sidebar, so the several
    /// gigabytes a finished Rust checkout is sitting on are invisible until
    /// someone goes looking branch by branch. Nobody does, which is how they
    /// accumulate.
    fn refresh_merged(&mut self, cx: &mut Context<Self>) {
        let now_ms = ket_core::now_ms();
        if self.reading_merged
            || now_ms.saturating_sub(self.merged_swept_at_ms)
                < MERGED_SWEEP_INTERVAL.as_millis() as u64
        {
            return;
        }
        self.merged_swept_at_ms = now_ms;
        self.reading_merged = true;

        let read = cx.background_executor().spawn(async move {
            let Ok(workspace) = Workspace::open() else {
                return Vec::new();
            };
            let (Ok(projects), Ok(worktrees)) = (workspace.projects(), workspace.worktrees(None))
            else {
                return Vec::new();
            };

            worktrees
                .into_iter()
                .filter_map(|worktree| {
                    // Asked in the *primary* checkout, never in the worktree
                    // itself. `HEAD` inside a worktree is the very branch being
                    // asked about, and every branch contains itself — so asking
                    // there would report all of them merged.
                    let root = &projects
                        .iter()
                        .find(|project| project.id == worktree.project_id)?
                        .root;
                    // `branch_has_landed`, not `branch_is_fully_merged`: the
                    // latter is the cleanup's question — "would deleting this
                    // lose anything" — which a branch with no commits of its
                    // own passes vacuously, so every worktree wore the badge
                    // from the moment it was created. The commit the branch
                    // was cut at is what tells those two apart.
                    let merged = ket_core::branch_cleanup::branch_has_landed(
                        &ket_core::git::Git::new(root),
                        &worktree.branch,
                        worktree.base_commit.as_deref(),
                    );
                    Some((worktree.id, merged))
                })
                .collect::<Vec<_>>()
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let rows = read.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.reading_merged = false;
                // By id, not by position, for the reason the status sweep
                // writes back that way too.
                for (id, merged) in rows {
                    for node in shell
                        .projects
                        .iter_mut()
                        .flat_map(|project| project.worktrees.iter_mut())
                        .filter(|node| node.id == id)
                    {
                        node.merged = merged;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-reads what each agent has on disk for every worktree.
    ///
    /// This is the reopened-app case: the process is gone, the conversation is
    /// not, and a row that says nothing about it looks identical to a worktree
    /// nobody has ever worked in. Throttled by the tracker rather than run on
    /// the tick — see [`ket_core::activity::RECORDS_EVERY_MS`].
    ///
    /// **Off the window's thread.** The scan reads every agent's transcripts
    /// and measured 150ms here, a dropped-frames stall that landed in the middle
    /// of typing. One scan at a time; the throttle restarts when it lands.
    fn refresh_records(&mut self, cx: &mut Context<Self>) {
        let now = ket_core::now_ms();
        if self.reading_records || !self.activity.wants_records(now) {
            return;
        }

        let nodes: Vec<(WorktreeId, std::path::PathBuf)> = self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .filter(|node| !node.missing)
            .map(|node| (node.id.clone(), node.path.clone()))
            .collect();

        self.reading_records = true;
        let scan = cx.background_executor().spawn(async move {
            let _t = crate::frametrace::tick("  records:scan (background)");
            let paths: Vec<&std::path::Path> =
                nodes.iter().map(|(_, path)| path.as_path()).collect();
            let found = ket_core::sessions::Sessions::new().scan(&paths);

            let records = nodes
                .iter()
                .filter_map(|(id, path)| {
                    // Newest first out of `scan`, so the first is the one to offer.
                    let newest = found.get(path)?.first()?;
                    Some((id.clone(), newest.clone()))
                })
                .collect::<Vec<_>>();

            let mut renamed = Vec::new();
            if let Ok(workspace) = Workspace::open() {
                for (id, session) in &records {
                    let Some(title) = session.explicit_title.as_deref() else {
                        continue;
                    };
                    if workspace
                        .sync_worktree_name_from_agent(id, title)
                        .unwrap_or(false)
                    {
                        renamed.push((id.clone(), title.to_owned()));
                    }
                }
            }

            (records, renamed)
        });

        cx.spawn(async move |shell: gpui::WeakEntity<Shell>, cx| {
            let (records, renamed) = scan.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.reading_records = false;
                for (id, title) in renamed {
                    for node in shell
                        .projects
                        .iter_mut()
                        .flat_map(|project| project.worktrees.iter_mut())
                        .filter(|node| node.id == id)
                    {
                        node.name = Some(title.clone().into());
                    }
                }
                shell.activity.sync_records(now, records);
                cx.notify();
            });
        })
        .detach();
    }

    /// Where a worktree sits in the tree, if it is still there.
    pub(crate) fn position_of(&self, id: &WorktreeId) -> Option<Selection> {
        self.projects
            .iter()
            .enumerate()
            .find_map(|(project, node)| {
                node.worktrees
                    .iter()
                    .position(|w| &w.id == id)
                    .map(|worktree| Selection { project, worktree })
            })
    }

    /// Turns a gap into the index a removed row is put back at.
    ///
    /// `before` names a slot in the list as it stands, with the row still in
    /// it. Taking the row out shifts every slot after it down by one, so a
    /// drop below where the row came from lands one place earlier than the gap
    /// the line was drawn in. Answers `None` when the move is a no-op — the
    /// two gaps either side of a row both put it back where it was.
    fn landing(from: usize, before: usize) -> Option<usize> {
        let to = if before > from { before - 1 } else { before };
        (to != from).then_some(to)
    }

    /// Moves a worktree row into the gap it was dropped in, and records the
    /// order.
    ///
    /// The list is rearranged here rather than reloaded from core, so the row
    /// lands under the cursor on the frame the mouse came up; the store is
    /// told afterwards, and a store that refuses leaves the sidebar showing
    /// an order the next reload will undo — which is the honest outcome, and
    /// the only one that does not make the drag feel like it stuck.
    ///
    /// The repository's own checkout does not move and nothing moves above
    /// it. It is not a worktree, so it has no place in the stored order, and
    /// a row dropped over it would have to be given one.
    pub(crate) fn reorder_worktree(
        &mut self,
        project: usize,
        from: usize,
        before: usize,
        cx: &mut Context<Self>,
    ) {
        // Read before the move, while the selection's indices still name the
        // row they were recorded against.
        let selected = self.selected_id();

        let Some(node) = self.projects.get_mut(project) else {
            return;
        };
        if from >= node.worktrees.len() || before > node.worktrees.len() {
            return;
        }
        if node.worktrees[from].primary {
            return;
        }
        // The first slot a row may take: the one after the repository's own
        // checkout, which heads the group and stays there.
        let first = node
            .worktrees
            .iter()
            .position(|node| !node.primary)
            .unwrap_or(0);
        let Some(to) = Self::landing(from, before.max(first)) else {
            return;
        };

        let moved = node.worktrees.remove(from);
        node.worktrees.insert(to, moved);

        let id = node.id.clone();
        let order: Vec<WorktreeId> = node
            .worktrees
            .iter()
            .filter(|node| !node.primary)
            .map(|node| node.id.clone())
            .collect();

        // Both are held by index, and every index in this project past the
        // move has just changed meaning. The selection is re-found by id; the
        // menu is simply closed, because a menu whose row moved out from under
        // it is acting on whatever took its place.
        self.selection = selected.and_then(|selected| self.position_of(&selected));
        self.worktree_menu = None;

        if let Ok(workspace) = Workspace::open()
            && let Err(e) = workspace.set_worktree_order(&id, &order)
        {
            tracing::warn!(%e, "could not record the sidebar's worktree order");
        }
        cx.notify();
    }

    /// Moves a project heading into the gap it was dropped in, and records the
    /// order.
    ///
    /// The same bargain a worktree's rows are moved under: rearranged here so
    /// the heading lands under the cursor on the frame the mouse came up, and
    /// the store told afterwards.
    ///
    /// Every index the shell holds into `projects` changes meaning here, and
    /// there are more of them than a worktree move disturbs — the selection,
    /// both menus, and whatever dialog either of them opened. The selection is
    /// re-found by worktree id; the rest are closed, because a menu whose row
    /// has moved out from under it is acting on whatever took its place.
    pub(crate) fn reorder_project(&mut self, from: usize, before: usize, cx: &mut Context<Self>) {
        let selected = self.selected_id();

        if from >= self.projects.len() || before > self.projects.len() {
            return;
        }
        let Some(to) = Self::landing(from, before) else {
            return;
        };

        let moved = self.projects.remove(from);
        self.projects.insert(to, moved);

        let order: Vec<ProjectId> = self.projects.iter().map(|node| node.id.clone()).collect();

        self.selection = selected.and_then(|selected| self.position_of(&selected));
        self.project_menu = None;
        self.worktree_menu = None;

        if let Ok(workspace) = Workspace::open()
            && let Err(e) = workspace.set_project_order(&order)
        {
            tracing::warn!(%e, "could not record the sidebar's project order");
        }
        cx.notify();
    }

    /// Notes where a dragged row would land, from a pointer that has moved.
    ///
    /// The gap is chosen by which half of the row the pointer is in: above its
    /// middle means the row goes before it, below means after. Rows are two
    /// pixels apart, so halves rather than edges is what makes every pixel of
    /// the list name a gap.
    ///
    /// A pointer outside the row answers `false` and changes nothing. Every
    /// row's listener runs on every move — gpui does not filter these by
    /// hitbox — so a row that is not the one under the cursor has to keep its
    /// hands off, and the sidebar clears the target before the rows are asked.
    ///
    /// `slack` is half the gap to the next row, and it is what makes the space
    /// *between* two rows belong to one of them: without it, the twelve pixels
    /// between two projects are nobody's, and the line the pointer was
    /// following blinks out every time it crosses from one group to the next.
    /// Half each means the two bands meet exactly and never overlap.
    fn note_drop_target<T: 'static>(
        &mut self,
        target: impl Fn(bool) -> DropTarget,
        slack: Pixels,
        event: &DragMoveEvent<T>,
    ) -> bool {
        let pointer = event.event.position;
        let bounds = event.bounds;
        let inside = pointer.x >= bounds.left()
            && pointer.x <= bounds.right()
            && pointer.y >= bounds.top() - slack
            && pointer.y <= bounds.bottom() + slack;
        if !inside {
            return false;
        }
        let below = event.event.position.y > event.bounds.center().y;
        let target = target(below);
        if self.drop_target == Some(target) {
            return false;
        }
        self.drop_target = Some(target);
        true
    }

    /// The projects tree.
    pub(crate) fn sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let selection = self.selection;
        let now = ket_core::now_ms();
        // Asked of gpui rather than of the field, because a drag released over
        // nothing at all is a drag nothing tells the shell about: the field
        // would still be holding the last gap the pointer crossed, and the
        // line would sit there until somebody dragged something else.
        let drop = cx.has_active_drag().then_some(self.drop_target).flatten();
        let projects = self.projects.len();

        // The top of the sidebar's card: which worktrees to show, then the
        // search. The filter's counts are across every project, so a filter
        // that holds nothing says so before it is pressed.
        let mut counts = [0usize; 4];
        for node in self
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
        {
            counts[0] += 1;
            counts[SidebarFilter::of(self.row_signal(node, now)) as usize] += 1;
        }
        // The filter, the sort and the actions share one line — the filter a
        // dense group whose answers past All are a status dot and a count,
        // named in a tooltip; the sort a two-glyph group; the actions bare
        // glyphs pushed to the right. Where the sidebar is too narrow for all
        // of it, the sort folds to one button that flips, and past that the
        // line wraps rather than clipping. The search opens *over* the line
        // rather than beneath it: a field added below pushed the whole tree
        // down the moment it appeared, and pushed it back up on Escape.
        //
        // Each glyph says what it does on hover: a sidebar header is exactly
        // where a person looks for "what does this one do" before pressing.
        let search_field = self.search.open.then(|| self.search_field(window, cx));
        let hovered_action = self.hovered_strip_action;
        let tip = |id: &'static str, button: Stateful<Div>, label: SharedString| {
            crate::ui::tooltip::tooltip(
                id,
                button.into_any_element(),
                label,
                crate::ui::tooltip::Side::Bottom,
                hovered_action == Some(id),
                t,
            )
            .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
                if *is_hovered {
                    this.hovered_strip_action = Some(id);
                } else if this.hovered_strip_action == Some(id) {
                    this.hovered_strip_action = None;
                }
                cx.notify();
            }))
            .into_any_element()
        };
        let tip = &tip;

        let chosen = self.sidebar_filter;
        let filter = button_group("sidebar-filter")
            .dense()
            .children(SidebarFilter::ALL.map(|which| {
                let count = counts[which as usize];
                // Past All, a state is its dot, and its name is the tooltip.
                let signal = match which {
                    SidebarFilter::All => None,
                    SidebarFilter::Running => Some((t.status.running, "filter-running")),
                    SidebarFilter::Waiting => Some((t.status.attention, "filter-waiting")),
                    SidebarFilter::Idle => Some((t.text.dim, "filter-idle")),
                };
                let label = if signal.is_some() { "" } else { which.label() };
                let answer = segment(("sidebar-filter", which as usize), label)
                    .count(count.to_string())
                    .selected(chosen == which)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sidebar_filter = which;
                        cx.notify();
                    }));
                match signal {
                    None => answer,
                    Some((dot, id)) => answer.dot(paint(dot)).wrap(move |cell| {
                        tip(id, cell, format!("{} · {count}", which.label()).into())
                    }),
                }
            }))
            .render(t);

        let status_view = self.activity_view;
        let show = |status: bool| {
            cx.listener(
                move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    if this.activity_view != status {
                        this.activity_view = status;
                        this.persist_view();
                        cx.notify();
                    }
                },
            )
        };
        let view_toggle = if self.sidebar_width >= SORT_UNFOLDS_AT {
            button_group("view-toggle")
                .child(
                    segment("view-status", "")
                        .glyph(Icon::ViewStatus)
                        .selected(status_view)
                        .on_click(show(true))
                        .wrap(|cell| tip("view-status", cell, "Sort worktrees by status".into())),
                )
                .child(
                    segment("view-project", "")
                        .glyph(Icon::ViewProject)
                        .selected(!status_view)
                        .on_click(show(false))
                        .wrap(|cell| tip("view-project", cell, "Group by project".into())),
                )
                .render(t)
        } else {
            let (which, label) = if status_view {
                (Icon::ViewStatus, "Sorted by status · group by project")
            } else {
                (Icon::ViewProject, "Grouped by project · sort by status")
            };
            tip(
                "view-flip",
                icon_button("view-flip", which)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(show(!status_view)),
                label.into(),
            )
        };

        let open = self.search.open;
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .child(tip(
                "strip-add-project",
                icon_button("strip-add-project", Icon::FolderPlus)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_project(cx);
                        cx.notify();
                    })),
                "Add project".into(),
            ))
            .child(tip(
                "strip-quick-prompt",
                icon_button("strip-quick-prompt", Icon::Pencil)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_quick_prompt(cx);
                        cx.notify();
                    })),
                crate::shortcuts::QUICK_PROMPT_HINT.into(),
            ))
            .child(tip(
                "strip-search",
                icon_button("strip-search", Icon::Search)
                    .bare()
                    .dense()
                    .render(t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_search(cx);
                        cx.notify();
                    })),
                crate::shortcuts::SEARCH_HINT.into(),
            ));

        let strip = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(8.0))
            .px(px(10.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(4.0))
                            // As tall as the field that covers it, so opening
                            // the search moves nothing below.
                            .min_h(crate::ui::FIELD_H)
                            // Kept in the layout while the field is up, for
                            // the height it holds; hidden, it paints nothing
                            // and takes no clicks.
                            .when(open, |el| el.invisible())
                            .child(filter)
                            .child(view_toggle)
                            .child(div().flex_1())
                            .child(actions),
                    )
                    .children(search_field.map(|field| {
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .flex()
                            .child(field)
                    })),
            );

        let list = div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            // Every row's `on_drag_move` runs on every move — gpui filters
            // these by drag type, not by hitbox — so the gap is cleared here
            // and then claimed by whichever row the pointer is actually in.
            // The capture phase runs back to front, so this parent is asked
            // before its children, which is the whole reason it works.
            .on_drag_move::<WorktreeDrag>(cx.listener(|this, _, _, cx| {
                if this.drop_target.take().is_some() {
                    cx.notify();
                }
            }))
            .on_drag_move::<ProjectDrag>(cx.listener(|this, _, _, cx| {
                if this.drop_target.take().is_some() {
                    cx.notify();
                }
            }))
            // And both drops land here rather than on the row they were
            // released over, because the gap is already worked out: a drop
            // anywhere in the sidebar goes wherever the line is showing. Put
            // on the rows instead, the space *between* two of them belonged to
            // neither — the line would be drawn in a gap that could not be
            // dropped into, which is the sort of near-miss that reads as the
            // drag not having worked.
            .on_drop::<WorktreeDrag>(cx.listener(|this, drag: &WorktreeDrag, _, cx| {
                let Some(DropTarget::Worktree { project, before }) = this.drop_target.take() else {
                    return;
                };
                if drag.project == project {
                    this.reorder_worktree(project, drag.worktree, before, cx);
                }
            }))
            .on_drop::<ProjectDrag>(cx.listener(|this, drag: &ProjectDrag, _, cx| {
                let Some(DropTarget::Project { before }) = this.drop_target.take() else {
                    return;
                };
                this.reorder_project(drag.project, before, cx);
            }))
            // A drag let go of somewhere else in the window drops nowhere and
            // tells nobody, so the gap it was last over is forgotten here.
            // Without this it survives the drag that set it, and the next
            // thing dragged anywhere — the divider between the sidebar and the
            // editor will do — finds the sidebar drawing a line for a move
            // that ended some time ago.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.drop_target.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            // The search field lives inside `strip` now — merged into the
            // same bordered row as "Projects" and the view controls. See
            // `crate::search` for how it collapses to a chunk at rest and
            // takes the row once open.
            .child(strip)
            .when(self.activity_view, |el| {
                el.children(self.activity_project_rows(now, window, cx))
            })
            .when(!self.activity_view, |el| {
                el.children(self.projects.iter().enumerate().map(|(pi, project)| {
                    // Filtered, a project is listed only for the rows it has in
                    // that state, and lists them whether or not it is folded:
                    // the filter is a question about worktrees, and a match
                    // hidden under a folded heading is not an answer.
                    let filter = self.sidebar_filter;
                    let filtering = filter != SidebarFilter::All;
                    if filtering
                        && !project
                            .worktrees
                            .iter()
                            .any(|node| filter.admits(self.row_signal(node, now)))
                    {
                        return div();
                    }
                    // The margin underneath is what separates one project from
                    // the next: groups sat flush, so the last worktree of one
                    // project and the heading of the next were as far apart as
                    // two rows of the same project.
                    //
                    // A project's group is what a project drag aims between, so
                    // the whole column names the gap rather than its heading: the
                    // space under the last project is under its worktrees, not
                    // under the name at the top of them.
                    // A project launching an agent with its own command is
                    // drawn as one tinted group, heading and rows together, so
                    // which sessions run differently is plain from across the
                    // room. The inset is split around the edge rather than
                    // added to it: every row stays at the x it has in an
                    // untinted project. See `crate::agent_override`.
                    let launches = project.override_summary();
                    let tint = crate::agent_override::hue(t);
                    let mut rows = div()
                        .relative()
                        .flex()
                        .flex_col()
                        .gap(ROW_GAP)
                        .when(launches.is_none(), |el| el.px(COLUMN_INSET))
                        .when(launches.is_some(), |el| {
                            el.mx(COLUMN_INSET - OVERRIDE_PAD - px(1.0))
                                .p(OVERRIDE_PAD)
                                .rounded(crate::ui::RADIUS_MD)
                                .border_1()
                                .border_color(alpha(tint, 0.35))
                                .bg(alpha(tint, 0.05))
                        })
                        .mb(GROUP_GAP)
                        .on_drag_move::<ProjectDrag>(cx.listener(
                            move |this, event: &DragMoveEvent<ProjectDrag>, _, cx| {
                                let moved = this.note_drop_target(
                                    |below| DropTarget::Project {
                                        before: if below { pi + 1 } else { pi },
                                    },
                                    GROUP_GAP / 2.0,
                                    event,
                                );
                                if moved {
                                    cx.notify();
                                }
                            },
                        ));

                    // The row restores the last worktree visited in this project;
                    // expansion belongs to the chevron so clicking a project never
                    // merely hides the tabs the reader expected to see.
                    // A project heads its group; it is never itself the selection,
                    // so it does not wear the selected fill. Its disclosure sits
                    // on the right rather than leading the row — the badge is
                    // what the eye looks for first.
                    // The badge's contents, worked out once: the ghost that follows
                    // the cursor wears the same one, so a heading being dragged is
                    // the heading you took hold of.
                    let mark: SharedString = project.icon.clone().unwrap_or_else(|| {
                        project
                            .name
                            .chars()
                            .next()
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_default()
                            .into()
                    });

                    // The caret says what it does on hover, the way the strip's
                    // buttons do: it is a glyph with no word beside it, and easy
                    // to mistake for decoration.
                    let backlog = self.backlog_count(project);
                    let hovered_action = self.hovered_project_action;
                    let tip = |id: &'static str, button: Stateful<Div>, label: &'static str| {
                        crate::ui::tooltip::tooltip(
                            (id, pi),
                            button.into_any_element(),
                            label,
                            crate::ui::tooltip::Side::Top,
                            hovered_action == Some((id, pi)),
                            t,
                        )
                        .on_hover(cx.listener(
                            move |this, is_hovered: &bool, _, cx| {
                                if *is_hovered {
                                    this.hovered_project_action = Some((id, pi));
                                } else if this.hovered_project_action == Some((id, pi)) {
                                    this.hovered_project_action = None;
                                }
                                cx.notify();
                            },
                        ))
                    };

                    rows = rows.child(
                        heading_row(("project", pi), t)
                            .relative()
                            // Everything else a project offers — New Worktree,
                            // what it costs, its settings — is in the menu a
                            // right-click opens, the way a worktree row's is.
                            // See `crate::projects`.
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.open_project_menu(pi, event.position);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                            // The badge starts where "Projects" does, and the caret
                            // at the far end lands on the trailing axis.
                            .pl(LEAD)
                            .pr(TRAIL_AXIS - WELL_XS / 2.0)
                            // The same size as the branch names under it: the
                            // badge is what marks this row as the heading, not
                            // a larger word.
                            .text_size(NAME)
                            .cursor(CursorStyle::OpenHand)
                            // Projects reorder by dragging their headings, the way
                            // worktree rows do. `on_drag` starts only once the
                            // pointer moves with the button down, so a heading that
                            // is clicked is still a heading that folds its project.
                            .on_drag(
                                ProjectDrag {
                                    project: pi,
                                    name: project.name.clone(),
                                    color: project.color,
                                    icon: mark.clone(),
                                    theme: *t,
                                    // Filled in by the listener, which is where
                                    // gpui says how far into the row the pointer
                                    // was.
                                    grab: point(px(0.0), px(0.0)),
                                    sidebar: self.sidebar_width,
                                },
                                |drag, grab, _, cx| {
                                    let mut drag = drag.clone();
                                    drag.grab = grab;
                                    cx.new(|_| drag)
                                },
                            )
                            // No hover effect on the badge or the name: the
                            // heading is still under the pointer, and the
                            // caret has its own well.
                            .child(
                                badge(project.color, mark, t).when(launches.is_some(), |el| {
                                    // A ring rather than a border: a border is inside
                                    // the box and would shrink the badge, and a ring
                                    // set off by a gap of the sidebar's own ground
                                    // reads as worn rather than as part of the colour.
                                    el.shadow(vec![
                                        BoxShadow {
                                            color: tint.into(),
                                            offset: point(px(0.0), px(0.0)),
                                            blur_radius: px(0.0),
                                            spread_radius: px(3.0),
                                        },
                                        BoxShadow {
                                            color: paint(t.panel).into(),
                                            offset: point(px(0.0), px(0.0)),
                                            blur_radius: px(0.0),
                                            spread_radius: px(1.5),
                                        },
                                    ])
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(ICON_GAP)
                                    .flex_1()
                                    .min_w_0()
                                    // Prose: a project is named by whoever added
                                    // it, and what they typed is a word rather
                                    // than a ref. The branch names under it are
                                    // the identifiers, and they keep the
                                    // chrome's mono.
                                    .child(
                                        div()
                                            .prose()
                                            .min_w_0()
                                            // One line, cut with an ellipsis:
                                            // the row is a fixed height, and a
                                            // name that wraps pushes its
                                            // buttons off the baseline.
                                            .truncate()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(project.name.clone()),
                                    )
                                    // The command itself, beside the name: the
                                    // tint says *something* is different, and
                                    // this says what.
                                    .children(launches.clone().map(|launches| {
                                        div()
                                            .flex_none()
                                            .font_family(crate::fonts::chrome())
                                            .text_size(px(11.0))
                                            .text_color(tint)
                                            .child(launches)
                                    }))
                                    // What is waiting in the backlog, beside the
                                    // name it is waiting on; a click opens it.
                                    // Nothing at all for an empty backlog.
                                    .when(backlog > 0, |el| {
                                        let id = project.id.clone();
                                        let label = if backlog > 99 {
                                            "99+".to_owned()
                                        } else {
                                            backlog.to_string()
                                        };
                                        // The phone's mark for it: the list
                                        // glyph and the count in a small
                                        // recessed well, faint until pointed
                                        // at.
                                        let faint = alpha(paint(t.text.dim), 0.72);
                                        let bright = paint(t.text.primary);
                                        el.child(tip(
                                            "project-backlog",
                                            readout(t)
                                                .id(("project-backlog", pi))
                                                .gap(px(5.0))
                                                .text_color(faint)
                                                .cursor_pointer()
                                                .hover(move |s| s.text_color(bright))
                                                .child(sized_icon(
                                                    Icon::ListTodo,
                                                    px(13.0),
                                                    paint(t.text.dim),
                                                ))
                                                .child(label)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.open_backlog(&id, cx);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                })),
                                            "Backlog",
                                        ))
                                    }),
                            )
                            .child(tip(
                                "project-expander",
                                icon_button(("project-expander", pi), {
                                    if project.expanded {
                                        Icon::ChevronDown
                                    } else {
                                        Icon::ChevronRight
                                    }
                                })
                                .bare()
                                .dense()
                                .render(t)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(node) = this.projects.get_mut(pi) {
                                            node.expanded = !node.expanded;
                                        }
                                        this.persist_view();
                                        cx.stop_propagation();
                                        cx.notify();
                                    },
                                )),
                                if project.expanded {
                                    "Collapse"
                                } else {
                                    "Expand"
                                },
                            ))
                            // The whole heading folds the project, the same as the
                            // caret: the badge and name are the biggest target on
                            // the row, and the caret is the smallest.
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(node) = this.projects.get_mut(pi) {
                                    node.expanded = !node.expanded;
                                }
                                this.persist_view();
                                cx.notify();
                            })),
                    );

                    if project.expanded || filtering {
                        rows = rows.children(
                            project
                                .worktrees
                                .iter()
                                .enumerate()
                                .filter(|(_, node)| filter.admits(self.row_signal(node, now)))
                                .flat_map(|(wi, node)| {
                                    let is_selected = selection
                                        == Some(Selection {
                                            project: pi,
                                            worktree: wi,
                                        });

                                    // The row's shape: a state light, the branch, and —
                                    // only where it means something — the `primary`
                                    // pill. No trailing labels: a change count and a
                                    // divergence arrow are things you go and look at,
                                    // not things worth carrying on every row of a list
                                    // you scan. The light says what the worktree is
                                    // doing, not what is in it: working, blocked on a
                                    // person, or finished badly. Uncommitted changes
                                    // only colour it when nothing is running, since
                                    // files changed on disk are the least urgent thing
                                    // a row can report.
                                    let activity = self.activity.activity(&node.id, now);
                                    let overridden = activity
                                        .agent()
                                        .or(node.agent.as_ref().map(SharedString::as_ref))
                                        .is_some_and(|agent| {
                                            project.agent_overrides.contains_key(agent)
                                        });
                                    let reported = activity.signal();
                                    let subagents = self.activity.subagents(&node.id, now);
                                    let folded = self.collapsed_subagents.contains(&node.id);
                                    // Whether this row heads a visible subagent tree,
                                    // which changes the row as well as what follows it.
                                    // Not while a row is being dragged: the drop targets
                                    // are worktree rows, and a gap under one full of
                                    // subagents is a gap that says nothing about where
                                    // it would land.
                                    let grouped =
                                        !folded && !subagents.is_empty() && !cx.has_active_drag();

                                    // The row's state in words at its far end, in the
                                    // state's own hue — see `state_word`. The subagents
                                    // roll up into it: one waiting on a person is the
                                    // worktree waiting on one.
                                    let state = state_word(
                                        node,
                                        ket_core::activity::roll_up(reported, &subagents),
                                        t,
                                    );
                                    // What it is doing, without the agent's name in it:
                                    // the provider's own mark is drawn beside this, and
                                    // "claude · thinking" next to Claude's logo says the
                                    // same thing twice.
                                    let detail = activity.detail();

                                    // What state looks like on a selected row: nothing
                                    // extra. The rail beside the branch name is already
                                    // the hue, and `block_row`'s wash is already "here" —
                                    // a tinted hairline and a halo on top of both was the
                                    // row saying the same two things three times. What
                                    // that cost was a selected row that changed shape as
                                    // an agent started and stopped.
                                    // Everything that trails the name lives in one column
                                    // at the row's right edge, centred against both of the
                                    // row's lines rather than riding the branch's. Two
                                    // things follow from that: a status line coming or
                                    // going never shifts the marks beside it, and a long
                                    // branch name is cut by the column rather than pushing
                                    // it off the row.
                                    //
                                    // Whether there is anything to put in it, asked before
                                    // it is built: the row spaces its children with a gap,
                                    // and an empty column still takes one — every plain row
                                    // would pay nine pixels of branch name for a column
                                    // holding nothing.
                                    //
                                    // The row's own group, so the merge mark can come up
                                    // under the pointer without a `hovered_*` field of its
                                    // own on the shell.
                                    let row_group = worktree_row_group(pi, wi);

                                    // Folds the subagent rows away, and says how many
                                    // it is hiding while it does. Only while there are
                                    // any: a chevron on every row would be a control
                                    // for nothing.
                                    let fold = (!subagents.is_empty()).then(|| {
                                        let node_id = node.id.clone();
                                        let button = icon_button(
                                            ("subagent-fold", pi * 1000 + wi),
                                            if folded {
                                                Icon::ChevronRight
                                            } else {
                                                Icon::ChevronDown
                                            },
                                        )
                                        .bare()
                                        .dense();
                                        match folded {
                                            true => button.count(format!("+{}", subagents.len())),
                                            false => button,
                                        }
                                        .render(t)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.collapsed_subagents.remove(&node_id) {
                                                this.collapsed_subagents.insert(node_id.clone());
                                            }
                                            // The row selects on click too.
                                            cx.stop_propagation();
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                    });

                                    let meta = self.worktree_meta(node, pi, wi, fold, window, cx);
                                    // Heading a tree, the column is measured for the
                                    // subagent rows below, which end their figures
                                    // under the state word — see `meta_widths`. Asks
                                    // for another frame only when the width moved.
                                    let meta = if grouped {
                                        let shell = cx.entity();
                                        let node_id = node.id.clone();
                                        let probe = gpui::canvas(
                                            move |bounds, _, cx| {
                                                shell.update(cx, |this, cx| {
                                                    let width = bounds.size.width;
                                                    if this.meta_widths.get(&node_id)
                                                        != Some(&width)
                                                    {
                                                        this.meta_widths
                                                            .insert(node_id.clone(), width);
                                                        cx.notify();
                                                    }
                                                });
                                            },
                                            |_, _: (), _, _| {},
                                        )
                                        .absolute()
                                        .size_full();
                                        div()
                                            .relative()
                                            .flex()
                                            .flex_none()
                                            .child(meta)
                                            .child(probe)
                                            .into_any_element()
                                    } else {
                                        meta
                                    };
                                    // The primary checkout says so beside its name, the
                                    // way a pinned row does: which row is the repository
                                    // itself is worth knowing at a glance, not on hover.
                                    let primary_mark = node.primary.then(|| {
                                        let hovered =
                                            self.hovered_primary.as_ref() == Some(&node.id);
                                        let node_id = node.id.clone();
                                        crate::ui::tooltip::tooltip(
                                            ("primary-mark", pi * 1000 + wi),
                                            sized_icon(Icon::Primary, px(10.0), paint(t.text.dim))
                                                .flex_none()
                                                .into_any_element(),
                                            "primary",
                                            crate::ui::tooltip::Side::Top,
                                            hovered,
                                            t,
                                        )
                                        .on_hover(
                                            cx.listener(move |this, is_hovered, _, cx| {
                                                if *is_hovered {
                                                    this.hovered_primary = Some(node_id.clone());
                                                } else if this.hovered_primary.as_ref()
                                                    == Some(&node_id)
                                                {
                                                    this.hovered_primary = None;
                                                }
                                                cx.notify();
                                            }),
                                        )
                                    });

                                    let last = project.worktrees.len().saturating_sub(1);

                                    let item =
                                        block_row(("worktree", pi * 1000 + wi), is_selected, t)
                                            .group(row_group.clone())
                                            .relative()
                                            // A step and a half under `LABEL`: a worktree row
                                            // carries a branch name, a status line and a run
                                            // of tags at once, and that density reads better
                                            // small. The sidebar as a whole was brought down a
                                            // size or two from the design's numbers on request.
                                            .text_size(px(13.5))
                                            // The rail stands in the row's own leading
                                            // padding, so the bar itself costs no width — and
                                            // what it frees is the slot the light used to
                                            // hold, which now carries the agent's own mark on
                                            // every row rather than only on the selected one.
                                            .flex_row()
                                            .items_center()
                                            .gap(ICON_GAP)
                                            // Heading a tree, the row keeps its hairline's
                                            // pixel but not its ink — the line belongs at the
                                            // foot of the group — and grows the guide the
                                            // subagents hang from out of its own mark.
                                            .when(grouped, |el| {
                                                el.border_color(gpui::transparent_black())
                                                    .child(trunk_head(t))
                                            })
                                            // The mark stands in a column of its own, exactly
                                            // as wide as it draws. Every branch name then
                                            // starts at the same x whichever mark the row turns
                                            // out to wear, and the status line beneath it needs
                                            // no indent of its own to sit under the name — it
                                            // is a sibling of the mark, not a thing tucked in
                                            // behind it.
                                            .child(
                                                div()
                                                    .relative()
                                                    .flex()
                                                    .flex_none()
                                                    .items_center()
                                                    .justify_center()
                                                    .w(MARK)
                                                    // An agent this project
                                                    // launches with its own
                                                    // command sits in a tinted
                                                    // well. Drawn behind the
                                                    // mark and outside its
                                                    // column, so the name beside
                                                    // it does not move.
                                                    .when(overridden, |el| {
                                                        el.child(
                                                            div()
                                                                .absolute()
                                                                .top(-MARK_WELL_BLEED)
                                                                .bottom(-MARK_WELL_BLEED)
                                                                .left(-MARK_WELL_BLEED)
                                                                .right(-MARK_WELL_BLEED)
                                                                .rounded(crate::ui::RADIUS_SM)
                                                                .bg(alpha(tint, 0.16))
                                                                .border_1()
                                                                .border_color(alpha(tint, 0.4)),
                                                        )
                                                    })
                                                    .child(row_mark(
                                                        &activity,
                                                        node.dirty,
                                                        node.missing,
                                                        t,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap(px(0.0))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .min_w_0()
                                                            // The branch takes the room that is
                                                            // left and gives it back when the
                                                            // row needs it: without a min width
                                                            // of zero a flex child refuses to
                                                            // shrink.
                                                            //
                                                            // Not `flex_1`, so a pinned row's
                                                            // pin sits against the end of the
                                                            // name rather than out at the row's
                                                            // far edge, where the meta already
                                                            // is.
                                                            .gap(px(4.0))
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    // One line, cut short with an
                                                                    // ellipsis. Without `flex_1`
                                                                    // a name that may shrink is
                                                                    // sized by its narrowest
                                                                    // wrap, and a wrapping one
                                                                    // collapsed to a column of
                                                                    // single letters: every row
                                                                    // hundreds of pixels tall and
                                                                    // no name showing.
                                                                    .truncate()
                                                                    // A record whose checkout is
                                                                    // gone is still a row you
                                                                    // can act on, so it is
                                                                    // dimmed rather than hidden
                                                                    // — but it must not look
                                                                    // like somewhere you can go.
                                                                    .when(node.missing, |el| {
                                                                        el.text_color(paint(
                                                                            t.text.dim,
                                                                        ))
                                                                    })
                                                                    .child(node.label()),
                                                            )
                                                            .children(primary_mark)
                                                            // Pinned rows lead their project,
                                                            // and this says why one is up
                                                            // there. Beside the name rather
                                                            // than among the meta, whose
                                                            // column is for what the row is
                                                            // doing, not where it is kept.
                                                            .when(node.pinned, |el| {
                                                                el.child(
                                                                    sized_icon(
                                                                        Icon::Pin,
                                                                        px(10.0),
                                                                        paint(t.text.dim),
                                                                    )
                                                                    .flex_none(),
                                                                )
                                                            }),
                                                    )
                                                    // What it is doing, spelled out — on every
                                                    // row that has something to say, selected
                                                    // or not. Selecting a row is not allowed
                                                    // to change anything but colour: it must
                                                    // never be what makes a label appear.
                                                    //
                                                    // Drawn on every row regardless, and
                                                    // hidden with `invisible` rather than left
                                                    // out when there is nothing to show — an
                                                    // absent element and a hidden one take the
                                                    // same width, but only the hidden one
                                                    // keeps its height, so a row with nothing
                                                    // to say can never be shorter than one
                                                    // that does and shove everything beneath
                                                    // it up and down as rows come and go.
                                                    .child({
                                                        let show = !detail.is_empty();
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            // No indent of its own: the mark
                                                            // sits in a column beside both
                                                            // lines, so this one starts where
                                                            // the branch does for free. It is
                                                            // not drawn a second time here —
                                                            // twice is how a sidebar comes to
                                                            // look unconsidered.
                                                            //
                                                            // A step under `CAPTION`, to sit
                                                            // beneath the branch line rather
                                                            // than beside it.
                                                            //
                                                            // And prose, where the branch name
                                                            // above it is not. This line is the
                                                            // reason the chrome is paired at
                                                            // all: "Claude Code · needs you" is
                                                            // words, and 9.5px is where a mono
                                                            // costs the most — widest way to
                                                            // set the one line that has least
                                                            // room, in the one place a reader
                                                            // skims rather than compares.
                                                            .prose()
                                                            .mt(px(2.0))
                                                            .text_size(px(12.0))
                                                            .when(!show, |el| el.invisible())
                                                            .child(if !show {
                                                                div()
                                                                    .child("\u{200b}")
                                                                    .into_any_element()
                                                            } else {
                                                                let ink = paint(match reported {
                                                                    Signal::Blocked => {
                                                                        t.status.attention
                                                                    }
                                                                    Signal::Failed => {
                                                                        t.status.failed
                                                                    }
                                                                    // A theme may give it a hue.
                                                                    _ => t
                                                                        .text
                                                                        .note
                                                                        .unwrap_or(t.text.dim),
                                                                });
                                                                div()
                                                                    .flex_1()
                                                                    .min_w_0()
                                                                    // Long titles are the norm
                                                                    // here — a recorded session
                                                                    // is named after whatever
                                                                    // was asked of it — so the
                                                                    // line is cut at the
                                                                    // sidebar's edge rather than
                                                                    // wrapped into a second one.
                                                                    .truncate()
                                                                    // The agent's own state,
                                                                    // not the rail's: under a
                                                                    // purple merge rail these
                                                                    // words are still "waiting
                                                                    // on you", and yellow is
                                                                    // what makes them read as
                                                                    // that rather than as a
                                                                    // caption.
                                                                    .text_color(ink)
                                                                    .child(detail_text(detail, ink))
                                                                    .into_any_element()
                                                            })
                                                    }),
                                            )
                                            .children(state)
                                            .child(meta)
                                            // Rows reorder by dragging, and only within their
                                            // own project. The repository's own checkout is
                                            // left out of every part of it: it is not a
                                            // worktree, there is nothing in the store to record
                                            // a position for it, and it heads the group because
                                            // that is what it is. Nothing may be dropped above
                                            // it either, which is why it takes no target — the
                                            // sidebar clears the gap and this row never claims
                                            // one back.
                                            //
                                            // `on_drag` starts only once the pointer moves
                                            // with the button down, so a row that is clicked
                                            // is still a row that is selected.
                                            .when(!node.primary, |el| {
                                                let drag = WorktreeDrag {
                                                    project: pi,
                                                    worktree: wi,
                                                    branch: node.label(),
                                                    theme: *t,
                                                    // Filled in by the listener below, which is
                                                    // where gpui says how far into the row the
                                                    // pointer was.
                                                    grab: point(px(0.0), px(0.0)),
                                                    sidebar: self.sidebar_width,
                                                };
                                                el.cursor(CursorStyle::OpenHand)
                                    .on_drag(drag, |drag, grab, _, cx| {
                                        let mut drag = drag.clone();
                                        drag.grab = grab;
                                        cx.new(|_| drag)
                                    })
                                    .on_drag_move::<WorktreeDrag>(cx.listener(
                                        move |this, event: &DragMoveEvent<WorktreeDrag>, _, cx| {
                                            // Only its own project's rows: a
                                            // worktree belongs to a repository,
                                            // and a gap under another project's
                                            // heading is a move no store
                                            // operation can perform.
                                            if event.drag(cx).project != pi {
                                                return;
                                            }
                                            let moved = this.note_drop_target(
                                                |below| DropTarget::Worktree {
                                                    project: pi,
                                                    before: if below { wi + 1 } else { wi },
                                                },
                                                ROW_GAP / 2.0,
                                                event,
                                            );
                                            if moved {
                                                cx.notify();
                                            }
                                        },
                                    ))
                                            })
                                            // The gap the dragged row would land in, drawn
                                            // above this one — and below it as well when it is
                                            // the last row, which is the one gap no row's top
                                            // edge can stand for.
                                            .when(
                                                drop == Some(DropTarget::Worktree {
                                                    project: pi,
                                                    before: wi,
                                                }),
                                                |el| {
                                                    el.child(insertion_line(
                                                        false, ROW_GAP, RAIL_INSET, t,
                                                    ))
                                                },
                                            )
                                            .when(
                                                wi == last
                                                    && drop
                                                        == Some(DropTarget::Worktree {
                                                            project: pi,
                                                            before: wi + 1,
                                                        }),
                                                |el| {
                                                    el.child(insertion_line(
                                                        true, ROW_GAP, RAIL_INSET, t,
                                                    ))
                                                },
                                            )
                                            // Right-click opens the row's context menu at the
                                            // pointer. `on_mouse_down` rather than a click:
                                            // a menu that waits for the button to come back up
                                            // feels late next to every other one on the
                                            // platform.
                                            .on_mouse_down(
                                                MouseButton::Right,
                                                cx.listener(
                                                    move |this, event: &MouseDownEvent, _, cx| {
                                                        this.open_worktree_menu(
                                                            pi,
                                                            wi,
                                                            event.position,
                                                        );
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    },
                                                ),
                                            )
                                            // Which row the pointer is on, for the facts
                                            // `worktree_meta` shows only then.
                                            .on_hover({
                                                let node_id = node.id.clone();
                                                cx.listener(
                                                    move |this, is_hovered: &bool, _, cx| {
                                                        if *is_hovered {
                                                            this.hovered_worktree =
                                                                Some(node_id.clone());
                                                        } else if this.hovered_worktree.as_ref()
                                                            == Some(&node_id)
                                                        {
                                                            this.hovered_worktree = None;
                                                        }
                                                        cx.notify();
                                                    },
                                                )
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let target = Selection {
                                                    project: pi,
                                                    worktree: wi,
                                                };
                                                // Clicking the row you are already in used to do
                                                // nothing at all. It means "back to the
                                                // worktree", so it goes back to its shell —
                                                // whatever you had opened over the top of it.
                                                // Only for that row: arriving from somewhere
                                                // else still hands back the arrangement the
                                                // worktree was left in.
                                                let reselected = this.selection == Some(target);
                                                this.select(target, cx);
                                                if reselected {
                                                    this.focus_main_tab();
                                                }
                                                cx.notify();
                                            }));

                                    let mut out = vec![item.into_any_element()];
                                    if grouped {
                                        out.push(
                                            self.subagent_rows(
                                                pi, wi, &node.id, &subagents, now, cx,
                                            ),
                                        );
                                    }
                                    out
                                }),
                        );

                        if !project.discovered.is_empty() && !filtering {
                            let count = project.discovered.len();

                            rows = rows.child(
                                row(("discovered", pi), false, t)
                                    .ml(INDENT)
                                    .text_size(px(10.0))
                                    .text_color(paint(t.text.dim))
                                    .child(icon(
                                        if project.discovered_expanded {
                                            Icon::ChevronDown
                                        } else {
                                            Icon::ChevronRight
                                        },
                                        paint(t.text.dim),
                                    ))
                                    .child(format!(
                                        "{count} discovered worktree{}",
                                        if count == 1 { "" } else { "s" }
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(node) = this.projects.get_mut(pi) {
                                            node.discovered_expanded = !node.discovered_expanded;
                                        }
                                        this.persist_view();
                                        cx.notify();
                                    })),
                            );

                            if project.discovered_expanded {
                                rows = rows.children(project.discovered.iter().enumerate().map(
                                    |(di, found)| {
                                        block_row(("discovered", pi * 1000 + di), false, t)
                                            .ml(INDENT * 2.0)
                                            .text_size(px(11.0))
                                            .text_color(paint(t.text.dim))
                                            // Clicking takes it into the project
                                            // and opens it. It used to only reveal
                                            // the files, which from the outside was
                                            // indistinguishable from the row doing
                                            // nothing at all — and a worktree you
                                            // can see but cannot enter is worse
                                            // than one that was never listed.
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.adopt_discovered(pi, di, cx);
                                                cx.notify();
                                            }))
                                            .child(found.label.clone())
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(paint(t.text.dim))
                                                    .child(found.path.clone()),
                                            )
                                    },
                                ));
                            }
                        }
                    }

                    // Last, so it is painted over the rows rather than under them.
                    // Its offsets are larger than a worktree's: groups are twelve
                    // pixels apart, not two, and a line four pixels off the edge
                    // would read as belonging to the group it was nearest.
                    rows.when(drop == Some(DropTarget::Project { before: pi }), |el| {
                        el.child(insertion_line(false, GROUP_GAP, px(0.0), t))
                    })
                    .when(
                        pi + 1 == projects
                            && drop == Some(DropTarget::Project { before: projects }),
                        |el| el.child(insertion_line(true, GROUP_GAP, px(0.0), t)),
                    )
                }))
            });

        // The merge card sits outside the scrolling list, so it stays docked
        // at the sidebar's foot however long or short the list above it is.
        div()
            .flex()
            .flex_col()
            .flex_none()
            .size_full()
            // No fill of its own: the card paints the panel colour, rounded.
            // A square one here, flush with the card's edge, covered its
            // corners — gpui clips to rectangles — which showed wherever the
            // desk and the panel differ, as they do in Monokai.
            .child(list)
            .child(self.new_worktree_foot(cx))
            .into_any_element()
    }

    /// A worktree's state as its row reports it: the lead agent's signal with
    /// its subagents rolled up, and a git operation mid-flight on top. The
    /// filter and its counts ask this, so they agree with the rows.
    pub(crate) fn row_signal(&self, node: &WorktreeNode, now: u64) -> Signal {
        if node.in_progress.is_some() {
            return Signal::Merging;
        }
        let reported = self.activity.activity(&node.id, now).signal();
        ket_core::activity::roll_up(reported, &self.activity.subagents(&node.id, now))
    }

    /// The button at the sidebar's foot: a new worktree in the selected
    /// project, or the first one when nothing is selected. With no projects
    /// at all there is nowhere to put a worktree, so it adds a project
    /// instead and says so.
    fn new_worktree_foot(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let project = self
            .selection
            .map(|selection| selection.project)
            .filter(|&index| index < self.projects.len())
            .or((!self.projects.is_empty()).then_some(0));
        let label = match project {
            Some(_) => "New worktree",
            None => "Add project",
        };
        div()
            .flex()
            .flex_none()
            .p(px(10.0))
            .child(
                button("sidebar-new-worktree", label)
                    .leading(Icon::Plus)
                    .render(t)
                    .flex_1()
                    .justify_center()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match project {
                            Some(index) => this.open_new_worktree(index, true, cx),
                            None => this.add_project(cx),
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// The worktrees with finished work waiting in them, as (project,
    /// worktree) indices — what the header's Merge figure counts and lists.
    /// See `crate::header`.
    ///
    /// The gate is `can_merge`'s — not the repository's own checkout, still
    /// on disk, and dirty — plus the agent being done: a worktree whose agent
    /// or subagents are still working, or waiting on a person, has changes in
    /// it that are not finished yet. That gate's cost is worth repeating here,
    /// because this is the surface that now looks complete: a worktree whose
    /// agent committed as it went has nothing uncommitted, so it is not
    /// listed, and `ket merge` is still the way to take it.
    ///
    pub(crate) fn ready_to_merge(&self, now: u64) -> Vec<(usize, usize)> {
        self.projects
            .iter()
            .enumerate()
            .flat_map(|(pi, project)| {
                project
                    .worktrees
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| {
                        !node.primary
                            && !node.missing
                            && node.dirty
                            && matches!(self.activity.signal(&node.id, now), Signal::Quiet)
                    })
                    .map(move |(wi, _)| (pi, wi))
            })
            .collect()
    }

    /// The explicit edge and resize cursor make resizing discoverable without
    /// spending scarce sidebar width on explanatory UI.
    pub(crate) fn sidebar_divider(&self) -> AnyElement {
        div()
            .id("sidebar-divider")
            .relative()
            .w(SIDEBAR_DIVIDER_SIZE)
            .h_full()
            .flex_none()
            // No line of its own: the cards' borders either side of it are
            // the edge, and the desk between them is the divider.
            .child(
                div()
                    .id("sidebar-divider-grab")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(-DIVIDER_GRAB)
                    .right(-DIVIDER_GRAB)
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_drag(SidebarResize, |drag, _, _, cx| cx.new(|_| *drag)),
            )
            .into_any_element()
    }

    /// Clamping both sides prevents a drag from hiding either the tree or the
    /// only pane through which the user can recover the layout.
    pub(crate) fn resize_sidebar(&mut self, event: &DragMoveEvent<SidebarResize>) -> bool {
        let wanted = event.event.position.x - event.bounds.left();
        let Some(width) = clamped_sidebar_width(wanted, event.bounds.size.width) else {
            return false;
        };
        if width == self.sidebar_width {
            return false;
        }
        self.sidebar_width = width;
        true
    }
}

/// Claude's hooks, once for every config directory a session ket launches
/// can run under — the configured one and each a project's own `claude`
/// command points at. A directory without them is a project whose rows stay
/// dim, with nothing to say why.
fn claude_hook_installers() -> Vec<Box<dyn agent_hooks::HookInstaller>> {
    ket_core::sessions::claude_config_dirs()
        .into_iter()
        .map(|dir| {
            Box::new(agent_hooks::ClaudeHooks::at(dir.join("settings.json")))
                as Box<dyn agent_hooks::HookInstaller>
        })
        .collect()
}

/// Installs one agent's hooks unless they are already there.
fn install_hooks(installer: &dyn agent_hooks::HookInstaller, script: &std::path::Path) {
    match installer.is_installed() {
        Ok(true) => {}
        Ok(false) => {
            if let Err(e) = installer.install(script) {
                tracing::warn!(agent = installer.agent(), %e, "agent hooks: install failed");
            }
        }
        Err(e) => {
            tracing::warn!(agent = installer.agent(), %e, "agent hooks: unreadable settings")
        }
    }
}

/// Puts Claude's hooks into any config directory a project's own `claude`
/// command has just started pointing at.
///
/// Off the window's thread: a command not seen before is a login-shell
/// startup to find out where it keeps its state.
pub(crate) fn install_claude_hooks_in_background() {
    std::thread::spawn(|| {
        let Ok(script) = agent_hooks::install_script() else {
            return;
        };
        for installer in claude_hook_installers() {
            install_hooks(installer.as_ref(), &script);
        }
    });
}

#[cfg(test)]
mod resize_tests {
    use super::*;

    #[test]
    fn sidebar_resizing_keeps_both_the_tree_and_content_reachable() {
        assert_eq!(
            clamped_sidebar_width(px(10.0), px(1200.0)),
            Some(MIN_SIDEBAR_SIZE)
        );
        // Past the widest the window allows: all of it but the content's
        // least and the divider.
        let widest = px(700.0) - MIN_CONTENT_SIZE - SIDEBAR_DIVIDER_SIZE;
        assert_eq!(clamped_sidebar_width(px(900.0), px(700.0)), Some(widest));
    }

    #[test]
    fn a_window_too_narrow_for_both_sides_keeps_the_previous_width() {
        assert_eq!(clamped_sidebar_width(px(200.0), px(400.0)), None);
    }
}
