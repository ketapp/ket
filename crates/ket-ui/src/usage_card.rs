//! What one worktree's agents are spending, hung off the row's own ring.
//!
//! This started on the status bar, as `Session 15% context · $1.24` beside the
//! provider quotas. Two things were wrong with it there. It was anchored to
//! nothing a person could point at — the bar followed whichever worktree the
//! sidebar had selected, so the number belonged to a row somewhere else on the
//! screen — and it was one slot per worktree, so two agents in one checkout
//! overwrote each other several times a second and the strip showed whichever
//! had drawn last. A figure that is correct and belongs to nobody reads as
//! arbitrary, which is exactly how it read.
//!
//! So it moved onto the token-reduction ring: the mark is already on the row
//! whose spending this is, and hovering a thing to learn about that thing needs
//! no explaining.
//!
//! **The ring opens it; the ring is not its subject.** For a while the card
//! led with the level — a hero ring, the level's name, its description. That
//! made one panel answer two unrelated questions, and the settings half
//! crowded out the half anyone opened it for. What the ring is now is a
//! handle: this card reports **what the agent's API actually charged**, and
//! nothing about how ket asked for it. The level lives on the row's own mark
//! and in the worktree menu that sets it.
//!
//! **Cost is followed by what the level saved.** That is a rule rather than a
//! layout choice: wherever a cost is shown, the saving against the reader's
//! own default-level sessions sits beside it, because a cost on its own says
//! nothing about whether the dial is doing anything. The saving is the one
//! thing about the level this card keeps. It is measured in tokens so it stays
//! honest for subscriptions and providers that do not report dollar cost.
//!
//! **Context pressure is deliberately not here.** The card carried it for a
//! while: a segmented bar splitting the window into fresh, cache-read and
//! cache-written tokens, which is genuinely the shape of the bill. It made the
//! panel a dashboard. Everything below earns its place against one question,
//! and how full the window is answers a different one.
//!
//! **Cache health is here, and passes the same test.** It looks like the thing
//! that was cut — both are read off the same payload, both are about tokens —
//! but they answer opposite kinds of question. How full the window is describes
//! a session; whether its cache is warm is a *setting that is wrong*, and one a
//! reader can fix in the next minute. It is also the largest lever there is: an
//! agent loop resends the whole conversation every turn, and a cache reprices
//! that at a tenth rather than preventing it, which is why a cold cache costs
//! more than every level on this dial can save. A row that changes what someone
//! does next stays; one that only informs does not.

use gpui::{
    AnyElement, Context, CursorStyle, FontWeight, Pixels, SharedString, Window, div, prelude::*,
    px, relative,
};
use ket_core::event::Money;
use ket_core::theme::Theme;
use ket_core::usage::{EconomyEstimate, Reading};

use crate::Shell;
use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::terminal::TerminalId;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::popup::{self, Placement};

/// How long a reading is drawn as current.
///
/// Claude redraws its status line as the session works, so a reading that has
/// stopped arriving usually means the session is gone — the pane was closed,
/// the agent exited. Past this the card dims its own footer rather than hiding
/// the numbers: the last thing a session spent is worth knowing, as long as it
/// does not claim to be live.
///
/// Generous, because the failure to avoid is dimming a session that is simply
/// sitting at its prompt with nobody typing: a redraw is driven by something
/// happening, and nothing happening is not the same as being gone.
const FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(120);

/// One session's latest reading, and everything that says whose it is.
#[derive(Debug, Clone)]
pub(crate) struct SessionReading {
    /// Everything the status line payload said — see [`Reading`], which is
    /// where the session id the history is keyed by lives.
    pub(crate) reading: Reading,
    /// The agent that drew it, from the launch environment.
    pub(crate) agent: Option<String>,
    /// The terminal it was launched into, when ket opened that terminal.
    pub(crate) pane: Option<TerminalId>,
    /// When ket heard it, so the card can tell live from left over.
    pub(crate) at_ms: u64,
    /// Whether this was rebuilt from ket's own history rather than heard from
    /// the session itself — see [`ket_core::usage::Record::restored_reading`].
    ///
    /// The card needs it to keep from claiming things a record cannot know: a
    /// restored reading has no pane, and "no pane" said of a live session
    /// means somebody started it in a shell of their own.
    pub(crate) restored: bool,
}

impl SessionReading {
    /// What identifies this reading among a worktree's others.
    ///
    /// Claude's session id when there is one, and the pane otherwise: two
    /// agents in one worktree are two readings, and a payload that named
    /// neither is the only case where they have to share a slot.
    fn key(&self) -> Option<String> {
        self.reading
            .session
            .clone()
            .or_else(|| self.pane.map(|pane| format!("pane-{}", pane.0)))
    }
}

/// Every session ket has heard from in one worktree.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorktreeSessions {
    /// Newest first, so the head is what the card draws.
    readings: Vec<SessionReading>,
}

/// How many sessions one worktree remembers.
///
/// Panes come and go; a worktree that has had thirty agents in it over a day
/// does not need thirty of them counted in a hover card, and the ones that
/// matter are the ones that reported most recently.
const MAX_SESSIONS: usize = 8;

impl WorktreeSessions {
    /// Files one reading, replacing whatever that session said before.
    ///
    /// A reading that identifies itself replaces its own; one that identifies
    /// itself as nothing replaces the last such reading rather than joining
    /// it. Appending those would let a single anonymous agent, redrawing
    /// several times a second, fill the list with copies of itself and push
    /// every named session out of it.
    pub(crate) fn record(&mut self, reading: SessionReading) {
        let key = reading.key();
        // Anything actually reporting supersedes what was restored. The
        // restored entry stands in for "the last thing ket heard here", and
        // once a session is speaking it is not that any more — left in place
        // it would be counted as a second session in the checkout, which is
        // the one thing this card exists to stop being vague about.
        if !reading.restored {
            self.readings.retain(|held| !held.restored);
        }
        self.readings.retain(|held| held.key() != key);
        self.readings.insert(0, reading);
        self.readings
            .sort_by_key(|held| std::cmp::Reverse(held.at_ms));
        self.readings.truncate(MAX_SESSIONS);
    }

    /// The reading the card speaks for: the one that reported most recently.
    pub(crate) fn current(&self) -> Option<&SessionReading> {
        self.readings.first()
    }

    /// How many others have reported from the same checkout.
    fn other_count(&self) -> usize {
        self.readings.len().saturating_sub(1)
    }
}

/// What each worktree last spent, from the history on disk.
///
/// The window opens with nothing in `Shell::session_usage`: it is filled by
/// status lines, and a status line only arrives while something is working.
/// Every worktree whose agent is idle — which after a restart is all of them —
/// therefore had a ring with nothing behind it, and a hover that answers
/// nothing reads as a hover that is broken. This seeds one restored reading per
/// worktree so the ring always has the last thing ket heard to show, marked as
/// exactly that. See [`ket_core::usage::Record::restored_reading`] for what a
/// record can and cannot say.
pub(crate) fn restored_sessions(
    history: &ket_core::usage::History,
) -> std::collections::HashMap<ket_core::id::WorktreeId, WorktreeSessions> {
    history
        .latest_per_worktree()
        .into_iter()
        .map(|(worktree, record)| {
            let mut sessions = WorktreeSessions::default();
            sessions.record(SessionReading {
                reading: record.restored_reading(),
                // A record does not keep which agent drew it, and one guessed
                // from the model name would be a provider mark that is
                // sometimes simply wrong. The slot stays empty.
                agent: None,
                pane: None,
                at_ms: record.last_seen_ms,
                restored: true,
            });
            (worktree.clone(), sessions)
        })
        .collect()
}

// ---- formatting --------------------------------------------------------------

/// `"$1.24"`, or `"<$0.01"` for an amount too small to round to a cent but not
/// actually nothing — a session that has spent something should never read as
/// having spent nothing.
///
/// A currency ket has not been taught keeps its own code rather than being
/// forced into a `$`, for the reason [`Money::currency`] is not an enum.
pub(crate) fn money(amount: &Money) -> String {
    let units = amount.micros as f64 / 1_000_000.0;
    let symbol = amount.currency == "USD";
    match (symbol, amount.micros) {
        (true, 1..=9_999) => "<$0.01".to_owned(),
        (true, _) => format!("${units:.2}"),
        (false, _) => format!("{units:.2} {}", amount.currency),
    }
}

/// A level's name without its number, lowercased for mid-sentence.
///
/// The table's label is "4 — No reduction", written for a picker row; "at No
/// reduction" mid-sentence reads as a stumble.
pub(crate) fn level_name(label: &str) -> String {
    let name = label.split_once(" — ").map_or(label, |(_, name)| name);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// How long ago something was heard, in two units at most.
fn ago(now_ms: u64, at_ms: u64) -> String {
    let age = now_ms.saturating_sub(at_ms);
    if age < 2_000 {
        return "just now".to_owned();
    }
    if age < 60_000 {
        return format!("{}s ago", age / 1_000);
    }
    format!("{} ago", crate::status_bar::compact_duration(age))
}

/// A session id, at the two ends a person actually matches against.
///
/// Counted in characters rather than bytes. These are UUIDs in practice, but
/// the value comes from an agent's payload, and a hover card is the last place
/// that should panic on a byte index landing inside a character.
fn short_session(id: &str) -> String {
    let count = id.chars().count();
    if count <= 16 {
        return id.to_owned();
    }
    let head: String = id.chars().take(8).collect();
    let tail: String = id.chars().skip(count - 4).collect();
    format!("{head}…{tail}")
}

// ---- metrics -----------------------------------------------------------------

/// The card's width.
///
/// Narrow for a panel with this much in it, and deliberately so: every line is
/// one fact, and a wider card would let two share a row.
pub(crate) const CARD_W: Pixels = px(336.0);

/// How far past the ring the pointer still counts as on it.
///
/// The ring is 14px of ink at the end of a row you select by clicking and
/// reorder by dragging, and a target that size is one the pointer misses. Four
/// pixels on every side makes it 22 — the merge mark's cell, near enough —
/// without moving the ink.
const RING_SLACK: Pixels = px(4.0);

/// The inset of every block — and, since there is only one column, where every
/// row's text begins.
///
/// Getting to one column took two passes. The first gave each marked row a
/// fixed gutter so a 5px dot and a 13px icon at least lined up with each other,
/// which turned four left edges into three. Three still reads as a mistake.
///
/// A mark cannot lead a row without pushing its text right, and nothing can sit
/// left of this padding, so the only route to one column was to stop leading
/// rows with marks at all. The terminal icon, the file icon and both status
/// dots are gone. None carried anything the row's own words did not already say
/// — except the footer's live-or-stale dot, whose colour moved onto the age it
/// used to sit beside.
///
/// The hero is the one exception, and deliberately: its ring is an avatar, and
/// a title indented past one is a header rather than a ragged row.
const PAD: Pixels = px(14.0);

/// Metadata: the footer, and the asides under a figure. A step under
/// [`CAPTION`], because these are things a person reads once and stops seeing.
const META: Pixels = px(10.5);

/// Lifts a small label onto the baseline of the larger figure beside it.
///
/// `items_baseline()` does not align baselines. A `gpui` text element is a
/// taffy leaf that reports no baseline of its own, and taffy then falls back to
/// the node's height — `child.baseline = baseline.unwrap_or(height)` — so the
/// row really aligns the *bottoms* of the two line boxes. Every line box keeps
/// its descent space below the baseline, and a smaller font keeps less of it,
/// so the small label lands low by the difference. At 20px beside 9.5px that
/// was a 2.5px drop under the figure, which is what made the label beside it
/// read as a slipped line rather than as a label.
///
/// Padding below the label puts the space back: it grows the box without moving
/// the text inside it, so the text rises by exactly what is added. The
/// proportion is measured off a rendering rather than derived — the ratios
/// `gpui` gets from the font are not the ones in its tables — so it holds for
/// the chrome face at the sizes this card uses, and no further.
///
/// Should `gpui` ever report a real baseline, this becomes inert rather than
/// wrong: taffy would align the text directly and the extra height below it
/// changes nothing.
fn lift(figure: Pixels, label: Pixels) -> Pixels {
    if figure > label {
        (figure - label) * 0.24
    } else {
        px(0.0)
    }
}

/// A block of the card, separated from the one above by a hairline.
fn block(rows: Vec<AnyElement>, first: bool, t: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .px(PAD)
        .py(px(11.0))
        .when(!first, |block| {
            block.border_t_1().border_color(paint(t.border))
        })
        .children(rows)
        .into_any_element()
}

/// A quiet sentence under a figure, which wraps.
fn aside(text: impl Into<SharedString>, t: &Theme) -> AnyElement {
    div()
        .text_size(META)
        // The one place in the card that is a sentence rather than a row, so
        // the one place that wants air between lines.
        .line_height(relative(1.45))
        .text_color(paint(t.text.dim))
        .child(text.into())
        .into_any_element()
}

// ---- the card ----------------------------------------------------------------

/// What ket knows about the session that the payload cannot say itself.
pub(crate) struct SessionContext {
    /// The tab holding the terminal this reading came from, when it is still
    /// open and ket opened it.
    pub(crate) terminal: Option<SharedString>,
    /// Prompts ket has counted for this session, and what it has spent since
    /// ket started counting them. `None` when ket has no record of it at all.
    pub(crate) counted: Option<(u32, Money)>,
    /// What the session cost at the organisation's own contracted rates, when a rate
    /// table is in force and covers the model.
    ///
    /// `None` on every install that has not been given a contract, which is
    /// the default — see [`ket_core::rates`].
    pub(crate) contracted: Option<i64>,
    /// What this worktree's own instruction files cost every session, when
    /// that is worth saying at all — see
    /// [`ket_core::worktree::ContextWeight::warning`], which is silent unless a
    /// budget is actually exceeded.
    pub(crate) weight: Option<String>,
    /// What Economy is estimated to have saved this session, and its prompts
    /// — see [`ket_core::usage::Record::economy_estimate`]. `None` when the
    /// session started at the default level or could not be priced.
    pub(crate) economy: Option<(EconomyEstimate, u32)>,
    /// The level the worktree is set to. What says *why* there is no
    /// saving when there is none: the dial is off, or it is on and there
    /// is nothing yet to measure it against.
    pub(crate) level: u8,
    /// The level the session itself started at, when ket has a record of it.
    /// A session keeps that level, so one begun before Economy was turned on
    /// has nothing to measure however long it runs.
    pub(crate) started_at: Option<u8>,
}

/// What this worktree's agent has spent, as its own API reported it.
pub(crate) fn usage_card(
    sessions: &WorktreeSessions,
    context: &SessionContext,
    now_ms: u64,
    t: &Theme,
) -> Option<AnyElement> {
    let held = sessions.current()?;
    let reading = &held.reading;
    let stale = now_ms.saturating_sub(held.at_ms) > FRESH_FOR.as_millis() as u64;

    // What the session cost, and beside it what the level saved. The two are
    // deliberately adjacent, with a hairline and a word over each, so neither
    // can be read as the other: whatever else a reader takes in, they take in
    // that there are two numbers and which is which.
    let money_block = {
        let cost = reading.usage.cost.as_ref();
        let economy = context.economy;
        let default_level = context.level == ket_core::worktree::default_token_reduction();

        // The word, the figure, the one clause that qualifies it. Equal halves
        // rather than sized to their contents — a saving that shifted the
        // cost figure sideways as it appeared would be the two reading as one
        // row again.
        let cell = |word: &'static str, figure: AnyElement, note: String, t: &Theme| {
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(9.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(alpha(paint(t.text.dim), 0.72))
                        .child(word),
                )
                .child(figure)
                .child(
                    div()
                        .text_size(META)
                        .line_height(relative(1.35))
                        .text_color(paint(t.text.dim))
                        .child(note),
                )
        };

        let figure = |text: String, tint: gpui::Rgba| {
            div()
                .text_size(px(19.0))
                .line_height(relative(1.05))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tint)
                .child(text)
                .into_any_element()
        };

        let cost_cell = cell(
            "Cost",
            match cost {
                Some(cost) => figure(money(cost), paint(t.text.primary)),
                // The status line carries cost on nearly every payload, so
                // this is a session ket has heard from before it said what it
                // spent. Better an empty slot than a block that comes and goes.
                None => figure("—".to_owned(), alpha(paint(t.text.dim), 0.6)),
            },
            // The same clause however this reader is billed: what separates a
            // plan from a metered key is what the estimate is *of*, and that
            // needs a sentence rather than a cell note. The aside below has
            // the room for it.
            match cost {
                Some(_) => "estimate, list price",
                None => "not reported yet",
            }
            .to_owned(),
            t,
        );

        // The saving is drawn whether or not there is one, and says which of
        // the reasons there is not. Telling somebody at the default level
        // to collect sessions they are already sitting at would be teaching
        // them the wrong thing.
        let saved_cell = match economy {
            Some((estimate, turns)) => {
                let tint = paint(t.diff.added);
                let trim = ket_core::usage::assumed_output_reduction(
                    context.started_at.unwrap_or(context.level),
                );
                cell(
                    "Saved",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(sized_icon(Icon::ChevronDown, px(13.0), tint))
                        .child(figure(
                            format!(
                                "≈{}",
                                ket_core::rates::money(
                                    estimate.saved_micros / i64::from(turns.max(1))
                                )
                            ),
                            tint,
                        ))
                        .into_any_element(),
                    format!(
                        "estimated per prompt · trims ~{:.0}% of output",
                        trim * 100.0
                    ),
                    t,
                )
            }
            None => cell(
                "Saved",
                figure("—".to_owned(), alpha(paint(t.text.dim), 0.6)),
                if default_level {
                    "Economy is off".to_owned()
                } else if context.started_at == Some(ket_core::worktree::default_token_reduction())
                {
                    "started with Economy off".to_owned()
                } else {
                    "not priced yet".to_owned()
                },
                t,
            ),
        };

        let mut rows = vec![
            div()
                .flex()
                // No `items_start`: flex stretches by default, which is what
                // runs the hairline the height of the taller cell whichever
                // that turns out to be. The cells are columns, so their own
                // content still sits at the top.
                .gap(px(14.0))
                .child(cost_cell)
                // The hairline is what makes these two things rather than
                // four lines.
                .child(div().flex_none().w(px(1.0)).bg(paint(t.border)))
                .child(saved_cell)
                .into_any_element(),
        ];

        // What the two figures are and how the saving is made is on the
        // website, as it is for the header's Economy popup: the notes under
        // each figure are all the card keeps, so it stays one glance.

        // The organisation's own number, under the estimate, because the whole
        // value of it is the contrast: "you are being quoted this and paying
        // that". Rebuilt from token counts rather than
        // corrected from the figure above it — see `Record::contracted_micros`,
        // which explains why the correction is impossible rather than merely
        // awkward.
        if let Some(micros) = context.contracted {
            rows.push(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(7.0))
                    .pt(px(1.0))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(paint(t.text.primary))
                            .child(ket_core::rates::money(micros)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .pb(lift(px(13.0), px(11.0)))
                            .text_color(paint(t.text.dim))
                            .child("at your rates"),
                    )
                    .into_any_element(),
            );
        }

        // The resume gap, in words. It was a split bar with a second dollar
        // figure under it, and on a session ket saw from the start both halves
        // said the same thing: a full bar, and the session's own cost written
        // out twice. A bar that is always full measures nothing.
        //
        // So it is a line now, and only when the two actually differ — which
        // is the case it existed for, a conversation resumed from before ket
        // was watching. Then it is worth a sentence, because the prompt count
        // beside it is the denominator of everything else on this card.
        if let Some((turns, attributed)) = context.counted.as_ref()
            && cost.is_some_and(|cost| attributed.micros < cost.micros)
        {
            rows.push(aside(
                format!(
                    "{} of it since ket attached, over {}.",
                    money(attributed),
                    match turns {
                        1 => "1 prompt".to_owned(),
                        turns => format!("{turns} prompts"),
                    }
                ),
                t,
            ));
        }

        // `first`: with the hero gone this is the top of the card, and a
        // hairline above the first block would be drawing the card's own edge
        // a second time.
        block(rows, true, t)
    };

    // Cache health, under the money rather than over it. It is the largest
    // lever there is, and the only row on this card a reader can act on today:
    // a cold cache or a stripped one is a setting to change, not a habit to
    // form — which is why it stays on a card that has cut everything merely
    // informative.
    let cache = reading
        .cache
        .as_ref()
        .map(|cache| cache_block(cache, reading.model.as_deref(), t));

    // Which of the worktree's agents this actually is.
    let whose = div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .px(PAD)
        .py(px(9.0))
        .border_t_1()
        .border_color(paint(t.border))
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(paint(if context.terminal.is_some() {
                    t.text.primary
                } else {
                    t.text.dim
                }))
                .child(match (context.terminal.as_ref(), held.pane) {
                    (Some(tab), _) => tab.to_string(),
                    // Launched into a pane that has since closed. The session
                    // is still reporting, so say so rather than leave the row
                    // out and let the reader assume it is one they can see.
                    (None, Some(pane)) => format!("pane {} — closed", pane.0),
                    // Nothing opened it this run: these are the numbers the
                    // history kept for the last session ket heard here, and
                    // the age in the footer says how long ago that was.
                    (None, None) if held.restored => "from ket's history".to_owned(),
                    // Every pane ket opens carries its key. Without one this is
                    // an agent someone started in a shell of their own.
                    (None, None) => "not opened by ket".to_owned(),
                }),
        )
        .child(div().flex_grow())
        .children(held.agent.as_deref().map(|agent| {
            let (mark, tint) = crate::ui::agent::provider(agent, t);
            div().flex_none().child(sized_icon(
                mark,
                px(11.0),
                if stale { alpha(tint, 0.5) } else { tint },
            ))
        }))
        .child(
            div()
                .flex_none()
                .max_w(px(140.0))
                .truncate()
                .text_size(META)
                .text_color(paint(t.text.dim))
                .child(
                    match (reading.name.as_deref(), reading.session.as_deref()) {
                        (Some(name), _) => name.to_owned(),
                        (None, Some(id)) => short_session(id),
                        (None, None) => "unnamed".to_owned(),
                    },
                ),
        )
        .into_any_element();

    // What this worktree charges every session before a word is typed. Silent
    // unless a budget is actually crossed — see `ContextWeight::warning` — so
    // this is a row most worktrees never grow.
    let weight = context.weight.as_deref().map(|warning| {
        div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .px(PAD)
            .py(px(9.0))
            .border_t_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(META)
                    .line_height(relative(1.4))
                    .text_color(paint(t.text.dim))
                    .child(warning.to_owned()),
            )
            .into_any_element()
    });

    // The ambiguity the status bar used to hide.
    let others = sessions.other_count();
    let crowd = (others > 0).then(|| {
        div()
            .flex()
            .items_center()
            .gap(px(7.0))
            .px(PAD)
            .py(px(9.0))
            .border_t_1()
            .border_color(paint(t.border))
            .bg(alpha(paint(t.status.attention), 0.07))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(META)
                    .line_height(relative(1.4))
                    .text_color(paint(t.status.attention))
                    .child(format!(
                        "{others} other {} here — this is whichever spoke last.",
                        if others == 1 { "session" } else { "sessions" }
                    )),
            )
            .into_any_element()
    });

    let mut footer = vec![
        // Live or stale was a coloured dot until the card was put on one text
        // column; a mark cannot lead a row without pushing its text right, and
        // nothing may sit left of the block's padding. So the age carries its
        // own state instead — green while the session is reporting, dim once it
        // has stopped — which is the same signal in one fewer element.
        div()
            .text_color(paint(if stale { t.text.dim } else { t.status.running }))
            .child(ago(now_ms, held.at_ms))
            .into_any_element(),
    ];
    // Ahead of the model and effort, and in the warning colour: those two are
    // settings a reader chose and can see in their own session, and this is a
    // surcharge that is easy to leave on and invisible from inside the
    // terminal.
    if reading.fast_mode {
        footer.push(div().child("·").into_any_element());
        footer.push(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.status.attention))
                .child("fast mode")
                .into_any_element(),
        );
    }
    for part in [reading.model.as_deref(), reading.effort.as_deref()]
        .into_iter()
        .flatten()
    {
        footer.push(div().child("·").into_any_element());
        footer.push(div().truncate().child(part.to_owned()).into_any_element());
    }
    footer.push(div().flex_grow().into_any_element());
    if reading.work.lines_added > 0 || reading.work.lines_removed > 0 {
        footer.push(
            div()
                .flex_none()
                .child(format!(
                    "+{} −{}",
                    reading.work.lines_added, reading.work.lines_removed
                ))
                .into_any_element(),
        );
    }

    Some(
        div()
            .prose()
            .flex()
            .flex_col()
            // gpui's default line height is `phi()` — 1.618 — which is right
            // for prose and wrong for a dense card: it makes an 11px row an
            // 18px box, and this card is a dozen rows of one line each. Stated
            // once at the root and inherited, so the card matches the design
            // rather than every row being a sixth taller than it was drawn.
            .line_height(relative(1.3))
            // Money first, then the lever.
            .child(money_block)
            .children(cache)
            .child(whose)
            .children(weight)
            .children(crowd)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(PAD)
                    .py(px(8.0))
                    .bg(paint(t.panel))
                    .border_t_1()
                    .border_color(paint(t.border))
                    .text_size(px(10.0))
                    .text_color(alpha(paint(t.text.dim), 0.7))
                    .children(footer),
            )
            .into_any_element(),
    )
}

/// A token count at the width a hover row can spare.
pub(crate) fn tokens(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=999_999 => format!("{:.0}K", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

/// What Claude Code's name for a cache miss means, in one clause a reader can
/// act on.
///
/// The cause names come from the payload and the remedies from the documented
/// list of what invalidates a cache. An unrecognised cause falls through to
/// itself rather than to a guess: a name ket has never seen is still worth
/// showing, and inventing advice for it would be worse than showing none.
/// Whether a miss cause is something on this machine that could be changed.
///
/// `likely_server_side` is the one that is not: Claude Code is saying the
/// provider expired the entry early, and there is no setting here to fix. A
/// warning about it would be a row telling someone to do something they cannot
/// do, every session, forever.
fn avoidable(cause: &str) -> bool {
    cause != "likely_server_side"
}

fn miss_remedy(cause: &str) -> Option<&'static str> {
    match cause {
        "tools_changed" => Some("an MCP server connected or disconnected"),
        "system_prompt_changed" => {
            Some("the system prompt changed — an upgrade, or an output style")
        }
        "ttl_expired_5m" => Some("idle past the five-minute cache lifetime"),
        "ttl_expired_1h" => Some("idle past the one-hour cache lifetime"),
        "model_changed" => Some("the model changed — each one has its own cache"),
        "effort_changed" => Some("the effort level changed mid-session"),
        "likely_server_side" => Some("expired early at the provider; nothing here to fix"),
        _ => None,
    }
}

/// How the session's prompt cache is doing.
///
/// Three states that want three different things said, and collapsing any two
/// of them would give the wrong advice: caching that was never reported at all
/// is a gateway or a setting, a cache that has gone cold is a pause that cost
/// something, and a warm one is just a number. Only the first two are drawn as
/// warnings — the card's rule is that the verdict row is the only *figure* in
/// colour, and these are bands, which is what the multi-session notice below
/// already is.
fn cache_block(cache: &ket_core::usage::CacheHealth, model: Option<&str>, t: &Theme) -> AnyElement {
    let warn = |lead: SharedString, detail: Option<String>, t: &Theme| {
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(PAD)
            .py(px(9.0))
            .border_t_1()
            .border_color(paint(t.border))
            .bg(alpha(paint(t.status.attention), 0.07))
            .child(
                div().flex().items_center().gap(px(7.0)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.0))
                        .line_height(relative(1.4))
                        .text_color(paint(t.status.attention))
                        .child(lead),
                ),
            )
            .children(detail.map(|detail| aside(detail, t)))
            .into_any_element()
    };

    // Never reported at all: caching is off, or something between ket and the
    // model is dropping the markers. Not a ratio of zero — a different problem
    // with a different answer.
    if !cache.observed {
        return warn(
            "No prompt caching reported".into(),
            Some(
                "Caching is off, or a gateway is stripping it — every turn is billed as \
                 fresh input."
                    .to_owned(),
            ),
            t,
        );
    }

    // Reported, but not right now. What the next request costs is the whole
    // point, and the payload knows it.
    if !cache.warm {
        return warn(
            "Cache cold".into(),
            Some(match cache.recache_if_cold {
                // Priced in the organisation's own money when a rate table covers this
                // model, and in tokens alone otherwise. Never in list price:
                // quoting a number that is not what the reader pays is worse
                // than quoting no number, because it is the one they would
                // repeat to somebody else.
                //
                // Charged at the cache-write rate rather than at input: a
                // re-cache is a cache being rebuilt, which is what the payload
                // is counting, and pricing it as fresh input would overstate
                // the cheaper half of it.
                Some(count) => match model
                    .and_then(ket_core::rates::for_model)
                    .map(|rate| rate.cache_write_micros(count, cache.ttl.as_deref()))
                {
                    Some(micros) => format!(
                        "The next request re-reads the whole history — about {} tokens, \
                         around {}.",
                        tokens(count),
                        ket_core::rates::money(micros)
                    ),
                    None => format!(
                        "The next request re-reads the whole history — about {} tokens.",
                        tokens(count)
                    ),
                },
                None => "The next request re-reads the whole history.".to_owned(),
            }),
            t,
        );
    }

    let mut rows = vec![
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(paint(t.text.dim))
                    .child("Cache"),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(paint(t.text.primary))
                    .child(match cache.hit_percent {
                        Some(percent) => format!("{percent}% from cache"),
                        // Warm, observed, and no ratio yet: one request in, and
                        // the denominator is still zero.
                        None => "warm".to_owned(),
                    }),
            )
            .child(div().flex_grow())
            .children(cache.ttl.as_deref().map(|ttl| {
                div()
                    .flex_none()
                    .text_size(META)
                    .text_color(paint(t.text.dim))
                    .child(format!("{ttl} lifetime"))
            }))
            .into_any_element(),
    ];

    // A cause that has fired more than once is a habit, and habits are what a
    // setting can fix. This is the only forward-looking thing on the card: the
    // aside below explains a miss that happened, and this says it is going to
    // keep happening.
    let recurring = cache
        .miss_causes
        .iter()
        .filter(|(cause, count)| **count > 1 && avoidable(cause))
        .max_by_key(|(_, count)| **count);

    if let Some((cause, count)) = recurring {
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(META)
                        .line_height(relative(1.4))
                        .text_color(paint(t.status.attention))
                        .child(match miss_remedy(cause) {
                            Some(remedy) => {
                                format!(
                                    "{count} rebuilds so far — {remedy}. It will keep happening."
                                )
                            }
                            None => format!("{count} rebuilds so far, all from {cause}."),
                        }),
                )
                .into_any_element(),
        );
    }

    // What has already cost this session a rebuild, on one line rather than a
    // row apiece. Compactions sit here beside misses because they are the same
    // fact to a reader — the prefix was rebuilt and paid for again — and differ
    // only in whether the session chose it. Skipped once the warning above has
    // named the same cause, which would otherwise say it twice.
    let mut history = Vec::new();
    if cache.misses > 0 && recurring.is_none() {
        history.push(match cache.last_miss_cause.as_deref().map(miss_remedy) {
            Some(Some(remedy)) => format!(
                "{} {} · last: {remedy}",
                cache.misses,
                if cache.misses == 1 { "miss" } else { "misses" }
            ),
            // A cause ket has no sentence for still gets its name; a miss with
            // no cause at all is still worth counting.
            Some(None) => format!(
                "{} {} · last: {}",
                cache.misses,
                if cache.misses == 1 { "miss" } else { "misses" },
                cache.last_miss_cause.as_deref().unwrap_or("cause unknown")
            ),
            None => format!(
                "{} {}",
                cache.misses,
                if cache.misses == 1 { "miss" } else { "misses" }
            ),
        });
    }
    // Not a fault and not a warning: a compaction is the session rewriting its
    // own history, and the rebuild is what that costs. Worth knowing when it is
    // happening often, which is the argument for `/clear` between tasks rather
    // than letting the window fill.
    if cache.expected_rebuilds > 0 {
        history.push(format!(
            "{} {} from compaction",
            cache.expected_rebuilds,
            if cache.expected_rebuilds == 1 {
                "rebuild"
            } else {
                "rebuilds"
            }
        ));
    }
    if !history.is_empty() {
        rows.push(aside(history.join(" · "), t));
    }

    block(rows, false, t)
}

impl Shell {
    /// What the tab holding `pane` is called, wherever it is open.
    ///
    /// Every space, not just the selected one: terminal ids are minted from a
    /// single counter, so the first space that claims one is the right one.
    fn terminal_tab_title(&self, pane: TerminalId) -> Option<SharedString> {
        let kind = crate::tabs::TabKind::Terminal(pane);
        self.spaces
            .values()
            .find_map(|space| space.tab_title(&kind))
    }

    /// ket's own side of the current reading.
    fn session_context(
        &self,
        worktree: &ket_core::id::WorktreeId,
        level: u8,
        sessions: &WorktreeSessions,
    ) -> SessionContext {
        let terminal = sessions
            .current()
            .and_then(|held| held.pane)
            .and_then(|pane| self.terminal_tab_title(pane));

        let counted = sessions
            .current()
            .and_then(|held| held.reading.session.as_deref())
            .and_then(|session| self.usage_history.record(session))
            .map(|record| {
                (
                    record.turns,
                    Money {
                        micros: record.attributed_micros(),
                        currency: "USD".to_owned(),
                    },
                )
            });

        let contracted = sessions
            .current()
            .and_then(|held| held.reading.session.as_deref())
            .and_then(|session| self.usage_history.record(session))
            .and_then(ket_core::usage::Record::contracted_micros);

        let default = ket_core::worktree::default_token_reduction();
        let economy = sessions
            .current()
            .and_then(|held| held.reading.session.as_deref())
            .and_then(|session| self.usage_history.record(session))
            .filter(|record| record.level != default)
            .and_then(|record| Some((record.economy_estimate()?, record.turns)));

        let started_at = sessions
            .current()
            .and_then(|held| held.reading.session.as_deref())
            .and_then(|session| self.usage_history.record(session))
            .map(|record| record.level);

        SessionContext {
            terminal,
            counted,
            contracted,
            weight: self
                .context_weights
                .get(worktree)
                .and_then(ket_core::worktree::ContextWeight::warning),
            economy,
            level,
            started_at,
        }
    }

    /// The ring on one worktree row, with its card when the pointer is on it.
    ///
    /// The ring is the row's token-reduction mark and stays that. What it
    /// opens is not about the level at all — see `usage_card`, which reports
    /// only what the agent's API charged. It is the handle because it is
    /// already on the row whose spending that is, not because the two are the
    /// same subject.
    ///
    /// Anchored below the ring rather than beside it: `ui::popup` measures and
    /// clamps to the window, and the sidebar is a scroll container that clips —
    /// which the deferred paint escapes, the same bargain `ui::tooltip` makes.
    pub(crate) fn token_reduction_ring(
        &self,
        node: &crate::tree::WorktreeNode,
        row: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let ring = crate::ui::chip::token_reduction_ring(node.token_reduction, t);

        let sessions = self.session_usage.get(&node.id);
        let card = (self.hovered_ring.as_ref() == Some(&node.id))
            .then(|| {
                sessions.and_then(|sessions| {
                    usage_card(
                        sessions,
                        &self.session_context(&node.id, node.token_reduction, sessions),
                        ket_core::now_ms(),
                        t,
                    )
                })
            })
            .flatten();

        let node_id = node.id.clone();
        div()
            .id(("token-reduction-ring", row))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .child(popup::hover_anchored(
                ("token-reduction-card", row),
                ring,
                card,
                Placement::BelowStart,
                CARD_W,
                window,
                t,
            ))
            // What the pointer actually lands on, as a layer over the ring
            // rather than padding around it: the ring's *centre* sits on the
            // sidebar's trailing axis, with the row's right padding derived
            // from it, so a wider box would bend the column its neighbours
            // line up in. This costs no layout and adds [`RING_SLACK`] on
            // every side.
            //
            // It carries the cursor for the same reason it carries the hover.
            // The row beneath asks for `OpenHand` across its whole width — it
            // is reordered by dragging — so a ring with nothing to say about
            // the pointer reads as part of that grip, which is exactly how it
            // read. macOS shows the last hovered hitbox to ask for a cursor,
            // and this one is painted after the row.
            .child(
                div()
                    .id(("token-reduction-hit", row))
                    .absolute()
                    .top(-RING_SLACK)
                    .bottom(-RING_SLACK)
                    .left(-RING_SLACK)
                    .right(-RING_SLACK)
                    .cursor(CursorStyle::Arrow)
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if *hovered {
                            this.hovered_ring = Some(node_id.clone());
                        } else if this.hovered_ring.as_ref() == Some(&node_id) {
                            this.hovered_ring = None;
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
