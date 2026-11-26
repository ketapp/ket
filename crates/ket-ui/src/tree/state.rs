//! A worktree row's state: drawn on its mark, and said at the head of its
//! second line.
//!
//! The row's end used to carry the state as a dot and a word, and on a
//! sidebar of ordinary width that column was what cut the branch and the
//! detail short. So the state moved onto the mark, as the phone draws it:
//! the agent's mark takes the state's colour and wears a small dot on its
//! corner, breathing while the agent works, beating while it waits on a
//! person, still once it has failed. Where a word is worth having, the second
//! line leads with it in the same colour — "Needs you", "Failed", a git
//! operation caught half-way, "Unread" — and the agent's own words follow in
//! grey. A running agent gets no word: its lit mark and moving dot are the
//! whole of the news, and the line is left to say what it is doing.

use gpui::{AnyElement, Div, FontWeight, Pixels, Rgba, SharedString, div, prelude::*, px};
use ket_core::activity::{Activity, Signal};
use ket_core::status::GitOperation;
use ket_core::theme::Theme;

use super::WorktreeNode;
use crate::paint::{alpha, paint};
use crate::ui::agent::{MARK, provider};
use crate::ui::chip::{Motion, status_dot_on};
use crate::ui::icon::{Icon, sized_icon};

/// What a row says about its state.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowState {
    /// The state's colour: the dot's, the lead word's and, where it
    /// [`lights`](Self::lights), the mark's.
    pub(crate) colour: Rgba,
    /// How the dot moves.
    pub(crate) motion: Motion,
    /// The word that leads the second line, where the state has one.
    pub(crate) lead: Option<&'static str>,
    /// Whether the mark takes the state's colour. Not for an unread answer:
    /// the agent is done, and a lit mark would say it is still at something.
    pub(crate) lights: bool,
}

impl RowState {
    /// A state that says itself with its dot alone.
    pub(crate) fn dot(colour: Rgba, motion: Motion) -> Self {
        Self {
            colour,
            motion,
            lead: None,
            lights: false,
        }
    }
}

/// What `node`'s row says about its state. `None` for a row with nothing to
/// report, which is most of them at rest.
///
/// Something running that is not known to be an agent making progress — a
/// build, a test run — reports nothing here either: its command is already
/// the second line, and a dot would claim an agent it does not have.
pub(crate) fn row_state(
    node: &WorktreeNode,
    signal: Signal,
    unread: bool,
    t: &Theme,
) -> Option<RowState> {
    // A merge or rebase stopped half-way is a fact about the checkout, so it
    // outranks whatever the agent is doing; the dot still moves as the agent
    // does.
    if let Some(op) = node.in_progress {
        let motion = match signal {
            Signal::Working => Motion::Travel,
            Signal::Blocked => Motion::Pulse,
            _ => Motion::Still,
        };
        return Some(RowState {
            colour: paint(t.status.merging),
            motion,
            lead: Some(operation_word(op)),
            lights: true,
        });
    }
    let (colour, motion, lead) = match signal {
        Signal::Working => (t.status.running, Motion::Travel, None),
        Signal::Blocked => (t.status.attention, Motion::Pulse, Some("Needs you")),
        Signal::Failed => (t.status.failed, Motion::Still, Some("Failed")),
        // Git rewriting history from a terminal: the mark is git's own, in
        // the merging colour already, and the second line is the command.
        Signal::Merging => (t.status.merging, Motion::Travel, None),
        // An answer nobody has read: a still dot on a mark at rest.
        Signal::Quiet if unread => {
            return Some(RowState {
                colour: paint(t.status.unread),
                motion: Motion::Still,
                lead: Some("Unread"),
                lights: false,
            });
        }
        Signal::Running | Signal::Quiet => return None,
    };
    Some(RowState {
        colour: paint(colour),
        motion,
        lead,
        lights: true,
    })
}

/// The word for a git operation caught mid-flight, as it leads the line.
fn operation_word(op: GitOperation) -> &'static str {
    match op {
        GitOperation::Merge => "Merging",
        GitOperation::Rebase => "Rebasing",
        GitOperation::CherryPick => "Cherry-picking",
        GitOperation::Revert => "Reverting",
        GitOperation::Bisect => "Bisecting",
    }
}

/// The colour a row's mark is lit in, while its state lights it.
pub(crate) fn lit(state: Option<&RowState>) -> Option<Rgba> {
    state.filter(|state| state.lights).map(|state| state.colour)
}

/// The mark at the head of a worktree row, lit in `lit` while its row has a
/// state to show.
///
/// An agent's mark at rest is grey, as the phone draws it: the colour on a
/// row is its state's, and a column of vendor colours on rows where nothing
/// is happening was colour that said nothing.
///
/// A worktree with no agent of any kind gets the branch glyph rather than the
/// terminal one: a terminal mark on a row where no terminal is running says
/// something that is not true.
pub(crate) fn row_mark(
    activity: &Activity,
    lit: Option<Rgba>,
    dirty: bool,
    missing: bool,
    t: &Theme,
) -> AnyElement {
    let ink = lit.unwrap_or_else(|| paint(t.text.dim));
    match activity {
        // A conversation on disk with nothing running: most of the way out,
        // enough to say this worktree remembers a session.
        Activity::Recorded { agent, .. } => {
            sized_icon(provider(agent, t).0, MARK, alpha(ink, 0.35)).into_any_element()
        }
        Activity::Session { agent, .. }
        | Activity::Observed { agent, .. }
        | Activity::Ended { agent, .. } => {
            sized_icon(provider(agent, t).0, MARK, ink).into_any_element()
        }
        // Git changing the worktree's history gets its own glyph in the
        // merging colour, and deliberately not the running green, which on
        // this row would claim an agent is working.
        Activity::Busy { mutating: true, .. } => {
            sized_icon(Icon::GitBranch, MARK, paint(t.status.merging)).into_any_element()
        }
        // A program ket watched start: `provider` knows a `claude` when it
        // sees one, and answers anything else with the honest terminal glyph.
        Activity::Busy { command, .. } => {
            sized_icon(provider(command, t).0, MARK, ink).into_any_element()
        }
        Activity::Idle => {
            // Dirty is the one quiet state worth a brighter glyph: there is
            // something here to read.
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

/// The dot on a mark's corner: a step under the 7px the row's end used to
/// carry, since it sits on the mark rather than beside a word.
const DOT: Pixels = px(6.0);

/// The ring of the row's own fill round the dot, which keeps the mark under
/// it from reading through.
const RING: Pixels = px(2.0);

/// `mark` wearing `state`'s dot on its bottom-right corner.
///
/// The dot is ringed in `ground`, the row's fill, and fades towards it
/// rather than out, so it stays solid over the mark at the bottom of every
/// beat. `hover` is the ground while the pointer is on the row, for a row
/// drawn as a `group`. `tiled` for a mark on a session's coloured tile, which
/// is larger: the dot keeps to the tile's corner.
pub(crate) fn with_dot(
    mark: AnyElement,
    state: Option<&RowState>,
    tiled: bool,
    ground: Rgba,
    hover: Option<(SharedString, Rgba)>,
) -> Div {
    let across = DOT + RING * 2.0;
    // Its centre a little inside the corner, as the phone's tile dot sits.
    let offset = if tiled { px(2.5) } else { px(1.5) } - across / 2.0;
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .child(mark)
        .children(state.map(|state| {
            div()
                .absolute()
                .right(offset)
                .bottom(offset)
                .flex()
                .items_center()
                .justify_center()
                .size(across)
                .rounded_full()
                .bg(ground)
                .when_some(hover, |el, (group, hover)| {
                    el.group_hover(group, move |style| style.bg(hover))
                })
                .child(status_dot_on(state.colour, ground, state.motion, DOT))
        }))
}

/// The word leading a row's second line, in its state's colour, where the
/// state has one.
pub(crate) fn lead_word(state: Option<&RowState>) -> Option<Div> {
    let state = state?;
    Some(
        div()
            .flex_none()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(state.colour)
            .child(state.lead?),
    )
}

/// The word leading the second line of a row whose checkout is going or
/// gone: "Deleting", then "Missing". Grey, since neither is something to
/// act on in a hurry, and ahead of the state's own word: an agent's news
/// means little in a checkout that is not there.
pub(crate) fn checkout_lead(missing: bool, deleting: bool, t: &Theme) -> Option<Div> {
    let word = if deleting {
        "Deleting"
    } else if missing {
        "Missing"
    } else {
        return None;
    };
    Some(
        div()
            .flex_none()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(paint(t.text.dim))
            .child(word),
    )
}

/// How far the agent's words stand off the lead word: the change of colour
/// and weight already separates them, so no "·" between.
pub(crate) const LEAD_GAP: Pixels = px(4.0);

/// The agent's words after the lead, less a phrase that only said the lead
/// again: "Unread ready", "Needs you waiting on you", "Failed failed".
pub(crate) fn after_lead(state: Option<&RowState>, detail: String) -> String {
    const SAID: [&str; 5] = ["ready", "session ready", "waiting on you", "failed", "done"];
    match state.and_then(|state| state.lead) {
        Some(_) if SAID.contains(&detail.as_str()) => String::new(),
        _ => detail,
    }
}
