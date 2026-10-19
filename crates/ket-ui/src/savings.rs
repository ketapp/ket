//! What Economy sessions cost and saved, across every worktree that ran one.
//!
//! The hover card on a worktree's ring says what one session saved. It adds
//! nothing up, so it cannot answer whether the dial is worth having on at all.
//! This does: a figure in the header, and behind it the same figure by project
//! and by worktree.
//!
//! **Spent, and what was saved, with the measured kept apart from the
//! assumed.** Each session is priced from its own tokens at its model's rates.
//! What the output filter kept from the model is measured the same way and
//! drawn plainly. Shorter answers are the output a level is assumed to have
//! trimmed — see [`ket_core::usage::ASSUMED_OUTPUT_REDUCTION`] for why that is
//! never measured against other sessions — and carry `≈` and a tag that says
//! so.
//!
//! **The prompt cache is shown and not counted.** It saves more than both put
//! together, and nearly all of it would have been saved with Economy off —
//! see [`ket_core::usage::History::savings_by_worktree`]. So its tokens get a
//! line that says it is left out, and no dollar figure.
//!
//! **A worktree is listed if it is reduced now or ran Economy sessions in the
//! period.** One put back to the default keeps what it saved while it was on.
//!
//! **A removed worktree keeps what it saved.** Its reduced sessions go on a
//! line of their own and into the total: it stopped because the work was
//! done, not because the reduction was turned off, and a figure that fell
//! every time a worktree was pruned would say nothing about the dial.
//!
//! **The rollup is built on the poll tick, not per frame.** It walks every
//! recorded session, which is cheap once and wasteful sixty times a second for a figure that changes when a prompt
//! finishes. See [`Shell::refresh_savings`].

use gpui::{
    AnyElement, Context, FontWeight, Pixels, SharedString, Window, div, prelude::*, px, relative,
};
use ket_core::id::ProjectId;
use ket_core::rates::money;
use ket_core::theme::{Color, Theme};
use ket_core::usage::WorktreeSavings;

use crate::fonts::Prose;
use crate::paint::{alpha, paint};
use crate::ui::button::button;
use crate::ui::chip::{badge, caption, tag, token_reduction_ring};
use crate::ui::icon::{Icon, WELL_GLYPH, icon_well, sized_icon};
use crate::ui::popup::{self, Placement};
use crate::ui::{CAPTION, LABEL, RADIUS_LG, RADIUS_MD};
use crate::usage_card::{level_name, tokens};
use crate::{PopupKind, Shell};

/// The popup's width: a worktree line carries a name, its level, a figure
/// and a session count, and wants them on one line.
const POPUP_W: Pixels = px(460.0);

/// Inner padding, matching the Usage popup beside it.
const PAD: Pixels = px(12.0);

/// The header's height, matching the Usage popup's.
const HEADER_H: Pixels = px(34.0);

/// Small print: notes under figures, session counts, level names.
const META: Pixels = px(10.5);

/// The period's two totals.
const TOTAL: Pixels = px(18.0);

/// How far a worktree line sits in from its project's badge.
const INDENT: Pixels = px(28.0);

/// The Spent and Saved columns' width, and the gap before each.
const COLUMN_W: Pixels = px(76.0);
const COLUMN_GAP: Pixels = px(12.0);

/// A project's name in the list.
///
/// Set here rather than left to the window's rem: the list followed
/// `theme.ui_font_size` while everything around it was fixed, and at the
/// default 15 its lines came out larger than the totals' labels.
const NAME: Pixels = px(12.0);

/// A worktree's name and every figure on a list line.
const ITEM: Pixels = px(11.5);

/// A list line's height.
const LINE_H: Pixels = px(28.0);

/// Where the figures are explained.
const DOCS_URL: &str = "https://ketapp.dev/docs/concepts/economy/#what-it-saved";

/// How tall the project list may grow before it scrolls. A reader with a
/// dozen reduced projects should not get a popup taller than the window.
const LIST_MAX_H: Pixels = px(320.0);

/// A day, for the periods.
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The settings pane's saved figure: the point of its card.
const FIGURE: Pixels = px(34.0);

/// How much of the saving's green an estimate gets: the same hue, so it adds
/// up with the measured lines, but quieter, so it never reads as one of them.
const ESTIMATE_INK: f32 = 0.62;

/// A level's name, in the Economy pane's levels card.
const LEVEL_NAME_W: Pixels = px(68.0);

/// A level's bar, in the same card.
const LEVEL_BAR_H: Pixels = px(4.0);

/// The span the savings are totalled over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SavingsPeriod {
    /// The last seven days — the default, recent enough to be about the
    /// work in front of the reader.
    #[default]
    Week,
    /// The last thirty days.
    Month,
    /// Everything the history still holds — see
    /// [`ket_core::usage::MAX_RECORDS`].
    All,
}

impl SavingsPeriod {
    /// Every period, in the switch's order.
    const ALL: [(&'static str, SavingsPeriod); 3] = [
        ("7d", SavingsPeriod::Week),
        ("30d", SavingsPeriod::Month),
        ("All", SavingsPeriod::All),
    ];

    /// The earliest `last_seen_ms` a session may have to count.
    pub(crate) fn since_ms(self, now_ms: u64) -> u64 {
        match self {
            SavingsPeriod::Week => now_ms.saturating_sub(7 * DAY_MS),
            SavingsPeriod::Month => now_ms.saturating_sub(30 * DAY_MS),
            SavingsPeriod::All => 0,
        }
    }

    /// What the popup's switch calls it: `7d`, `30d`, `All`.
    pub(crate) fn short(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, period)| *period == self)
            .map_or("7d", |(short, _)| short)
    }

    /// Its stored name — see `crate::view`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            SavingsPeriod::Week => "week",
            SavingsPeriod::Month => "month",
            SavingsPeriod::All => "all",
        }
    }

    /// The period a stored name means; anything unknown is the default.
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "month" => SavingsPeriod::Month,
            "all" => SavingsPeriod::All,
            _ => SavingsPeriod::Week,
        }
    }
}

/// The rollup the item and the popup both draw from.
#[derive(Default)]
pub(crate) struct Savings {
    /// Projects with at least one reduced worktree, in the sidebar's order.
    projects: Vec<ProjectSavings>,
    /// Reduced sessions from worktrees no longer in the sidebar.
    removed: WorktreeSavings,
    /// Every listed worktree's figures and the removed ones', summed.
    total: WorktreeSavings,
}

impl Savings {
    /// Whether there is nothing to show: no worktree reduced now, and none
    /// removed with reduced sessions in the period.
    fn is_off(&self) -> bool {
        self.projects.is_empty() && self.removed.priced + self.removed.unpriced == 0
    }
}

/// One project's reduced worktrees.
struct ProjectSavings {
    /// Which project, for a caller asking about one by id.
    id: ProjectId,
    name: SharedString,
    color: Color,
    /// What its sidebar badge says, so the two are recognisably one project.
    mark: SharedString,
    worktrees: Vec<WorktreeLine>,
    /// Its worktrees' figures, summed.
    total: WorktreeSavings,
}

/// One reduced worktree.
struct WorktreeLine {
    label: SharedString,
    level: u8,
    savings: WorktreeSavings,
}

/// One of the Economy pane's two cards, side by side on the sheet.
fn economy_card(t: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(12.0))
        .px(px(18.0))
        .py(px(16.0))
        .rounded(RADIUS_LG)
        .border_1()
        .border_color(paint(t.border))
        .bg(paint(t.elevated))
}

/// A card's first line: its glyph in a well of `hue`, its name, and what
/// sits at the far end.
fn card_head(
    which: Icon,
    hue: gpui::Rgba,
    name: &'static str,
    end: impl IntoElement,
    t: &Theme,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(icon_well(Some(hue), false, t).child(sized_icon(which, WELL_GLYPH, hue)))
        .child(
            div()
                .flex_1()
                .text_size(LABEL)
                .font_weight(FontWeight::MEDIUM)
                .text_color(paint(t.text.primary))
                .child(name),
        )
        .child(end)
}

/// An estimated dollar figure, marked as one.
fn estimated(micros: i64) -> String {
    format!("≈{}", money(micros))
}

/// A saving as a figure and its ink: green when there is one, dim when there
/// is none. Never red: neither saving can go below zero.
pub(crate) fn saving(micros: i64, t: &Theme) -> (String, gpui::Rgba) {
    let ink = match micros > 0 {
        true => t.diff.added,
        false => t.text.dim,
    };
    (money(micros), paint(ink))
}

/// A figure that has the shorter-answers estimate in it, marked as an
/// estimate: a total is only as measured as its least measured part.
fn approx(savings: &WorktreeSavings, figure: String) -> String {
    match savings.estimated_micros > 0 {
        true => format!("≈{figure}"),
        false => figure,
    }
}

/// Everything a line saved as a figure and its ink — see [`saving`] — with
/// `≈` when the estimate is part of it.
fn whole_saving(savings: &WorktreeSavings, t: &Theme) -> (String, gpui::Rgba) {
    let (figure, ink) = saving(savings.saved_micros(), t);
    (approx(savings, figure), ink)
}

/// The saving as a share of what the same sessions would have cost with
/// Economy off.
fn percent(savings: &WorktreeSavings) -> Option<String> {
    savings
        .fraction()
        .map(|fraction| format!("{:.0}% less", (fraction * 100.0).round()))
}

/// "1 session" or "3 sessions".
fn sessions(count: usize) -> String {
    match count {
        1 => "1 session".to_owned(),
        n => format!("{n} sessions"),
    }
}

/// Why a figure has no saving to show: its sessions could not be priced, they
/// ran at the default level, or there are none.
fn unmeasured(savings: &WorktreeSavings) -> String {
    if savings.unpriced > 0 {
        format!("{} not priced", sessions(savings.unpriced))
    } else if savings.unreduced > 0 {
        format!("{} at {}", sessions(savings.unreduced), default_level())
    } else {
        "no sessions yet".to_owned()
    }
}

/// A clause as the start of a sentence: `no sessions yet` → `No sessions yet`.
fn capitalise(clause: &str) -> String {
    let mut chars = clause.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The default level's name, mid-sentence.
fn default_level() -> String {
    level_name(
        &ket_core::worktree::token_reduction(ket_core::worktree::default_token_reduction()).label,
    )
}

/// A quiet sentence, which wraps.
fn aside(text: impl Into<SharedString>, t: &Theme) -> AnyElement {
    div()
        .prose()
        .text_size(META)
        .line_height(relative(1.45))
        .text_color(paint(t.text.dim))
        .child(text.into())
        .into_any_element()
}

/// One of the two figure columns: right-aligned at a fixed width, so a
/// line's token count sits under the total however long its name.
fn column(text: impl Into<SharedString>, ink: gpui::Rgba) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .justify_end()
        .w(COLUMN_W)
        .text_color(ink)
        .child(text.into())
}

/// What a line spent, once anything in it could be priced.
fn spent_figure(savings: &WorktreeSavings) -> Option<String> {
    (savings.priced > 0 || savings.spent_micros > 0).then(|| money(savings.spent_micros))
}

/// What a line saved, both savings together, once one of its Economy
/// sessions could be priced.
fn saved_figure(savings: &WorktreeSavings, t: &Theme) -> Option<(String, gpui::Rgba)> {
    (savings.priced > 0).then(|| whole_saving(savings, t))
}

/// A line's spend and saving, or a dash for either it has none of.
fn figures(savings: &WorktreeSavings, t: &Theme) -> [gpui::Div; 2] {
    let dash = || column("\u{2014}", alpha(paint(t.text.dim), 0.6));
    [
        spent_figure(savings).map_or_else(dash, |spent| column(spent, paint(t.text.dim))),
        saved_figure(savings, t).map_or_else(dash, |(saved, ink)| {
            column(saved, ink).font_weight(FontWeight::MEDIUM)
        }),
    ]
}

/// "1 command" or "1,284 commands".
fn commands(count: u32) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match count {
        1 => "1 command".to_owned(),
        _ => format!("{grouped} commands"),
    }
}

/// What the output filter's saving is made of, measured: the tokens it kept
/// from the model, and the commands it saw.
fn filter_detail(total: &WorktreeSavings) -> String {
    format!(
        "{} tokens trimmed \u{b7} {}",
        tokens(total.filter_saved_tokens),
        commands(total.command_calls)
    )
}

/// One line under the totals: what it is, with a tag when it needs one, what
/// it was made of under that, and — for a saving — what it came to at the
/// end.
fn ledger_row(
    name: &'static str,
    label: Option<gpui::Div>,
    detail: String,
    figure: Option<(String, gpui::Rgba)>,
    t: &Theme,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(COLUMN_GAP)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .prose()
                                .text_size(NAME)
                                .text_color(paint(t.text.primary))
                                .child(name),
                        )
                        .children(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(META)
                        .text_color(paint(t.text.dim))
                        .child(detail),
                ),
        )
        .children(figure.map(|(figure, ink)| {
            column(figure, ink)
                .text_size(NAME)
                .font_weight(FontWeight::MEDIUM)
        }))
}

/// The two savings, the measured one first, and under them the prompt cache,
/// which is said and not counted.
///
/// The filter says it has nothing yet rather than `$0.00` until a command has
/// been measured: a session that never ran one is not a filter that saved
/// nothing. The cache gets no figure at all, so that nothing on the line can
/// be read as part of the total — see the module docs.
fn ledger(total: &WorktreeSavings, t: &Theme) -> Vec<gpui::Div> {
    let dim = paint(t.text.dim);
    let filter = match total.command_calls {
        0 => ledger_row(
            "Output filter",
            None,
            "no commands measured yet".to_owned(),
            Some(("\u{2014}".to_owned(), alpha(dim, 0.6))),
            t,
        ),
        _ => ledger_row(
            "Output filter",
            None,
            filter_detail(total),
            Some(saving(total.filter_saved_micros, t)),
            t,
        ),
    };
    let shorter = ledger_row(
        "Shorter answers",
        Some(tag("estimated", t)),
        "assumed output trim, by level".to_owned(),
        Some((
            estimated(total.estimated_micros),
            match total.estimated_micros > 0 {
                true => alpha(paint(t.diff.added), ESTIMATE_INK),
                false => dim,
            },
        )),
        t,
    );
    let cache = (total.cache_read_tokens + total.cache_write_tokens > 0).then(|| {
        ledger_row(
            "Prompt cache",
            Some(tag("not counted", t)),
            format!(
                "{} read \u{b7} {} written \u{b7} cached at every level",
                tokens(total.cache_read_tokens),
                tokens(total.cache_write_tokens)
            ),
            None,
            t,
        )
    });
    [Some(filter), Some(shorter), cache]
        .into_iter()
        .flatten()
        .collect()
}

/// One line of the list: what it is on the left, its two figures on the
/// right. Flush with the well's edges above it.
fn line(
    lead: Option<AnyElement>,
    name: AnyElement,
    savings: &WorktreeSavings,
    indent: Pixels,
    t: &Theme,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(COLUMN_GAP)
        .h(LINE_H)
        .pl(PAD + indent)
        .pr(PAD)
        .text_size(ITEM)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(8.0))
                .children(lead)
                .child(name),
        )
        .children(figures(savings, t))
}

/// A 20px slot for a line's leading mark, so every name starts at one x.
fn lead(mark: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(20.0))
        .child(mark)
        .into_any_element()
}

impl Shell {
    /// Rebuilds the rollup from the usage history and the sidebar's worktrees.
    ///
    /// Called on the poll tick, after a reload (a level changed, or a worktree
    /// came or went) and when the period changes.
    ///
    /// Whatever the history holds for a worktree the sidebar no longer has
    /// goes on the removed line — only one that ran reduced sessions, since
    /// one that never did explains nothing. Its spend counts at every level,
    /// as a listed worktree's does, so the line's Spent and Saved agree.
    pub(crate) fn refresh_savings(&mut self) {
        let since = self.savings_period.since_ms(ket_core::now_ms());
        let mut by_worktree = self.usage_history.savings_by_worktree(since);
        let default = ket_core::worktree::default_token_reduction();

        let mut rollup = Savings::default();
        for project in &self.projects {
            let worktrees: Vec<WorktreeLine> = project
                .worktrees
                .iter()
                .filter_map(|node| {
                    let savings = by_worktree.remove(&node.id).unwrap_or_default();
                    let ran = savings.priced + savings.unpriced > 0;
                    (node.token_reduction != default || ran).then(|| WorktreeLine {
                        label: node.name.clone().unwrap_or_else(|| node.branch.clone()),
                        level: node.token_reduction,
                        savings,
                    })
                })
                .collect();
            if worktrees.is_empty() {
                continue;
            }

            let mut total = WorktreeSavings::default();
            for line in &worktrees {
                total.add(&line.savings);
            }
            rollup.total.add(&total);

            // The same mark the sidebar's badge draws.
            let mark = project.icon.clone().unwrap_or_else(|| {
                project
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default()
                    .into()
            });
            rollup.projects.push(ProjectSavings {
                id: project.id.clone(),
                name: project.name.clone(),
                color: project.color,
                mark,
                worktrees,
                total,
            });
        }

        // Every sidebar worktree was taken above; what is left was removed.
        for savings in by_worktree.into_values() {
            if savings.priced + savings.unpriced > 0 {
                rollup.removed.add(&WorktreeSavings {
                    unreduced: 0,
                    ..savings
                });
            }
        }
        rollup.total.add(&rollup.removed);
        self.savings = rollup;
    }

    /// What Economy has saved one project over the chosen period, as the
    /// figure, its tint, and the clause under it — `None` when none of the
    /// project's worktrees is reduced or ran Economy sessions.
    ///
    /// The same numbers the header's Economy popup lists for the project, so the
    /// two can never disagree.
    pub(crate) fn project_economy(
        &self,
        id: &ProjectId,
    ) -> Option<(SharedString, gpui::Rgba, SharedString)> {
        let project = self
            .savings
            .projects
            .iter()
            .find(|project| &project.id == id)?;
        let total = &project.total;
        let period = self.savings_period.short();
        if total.priced == 0 {
            return Some((
                "—".into(),
                paint(self.theme.text.dim),
                format!("{} · {period}", capitalise(&unmeasured(total))).into(),
            ));
        }
        let note = match percent(total) {
            Some(percent) => format!("{percent} · {period}"),
            None => format!("{} · {period}", sessions(total.priced)),
        };
        let (figure, tint) = whole_saving(total, &self.theme);
        Some((figure.into(), tint, note.into()))
    }

    /// The Economy pane's first card: what Economy saved over the header's
    /// period, in green, with the switch that sets the period; the two
    /// savings it is made of and the cache it leaves out, as the popup's
    /// ledger has them; and what was spent against what it would have been.
    ///
    /// From the same rollup as the header, so the two can never disagree.
    pub(crate) fn economy_figures(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let savings = &self.savings;
        let total = &savings.total;
        let measured = total.priced > 0;
        let dim = paint(t.text.dim);

        let period = crate::status_bar::segmented(
            "settings-savings-period",
            &SavingsPeriod::ALL,
            self.savings_period,
            |this, period| {
                this.savings_period = period;
                this.persist_view();
                this.refresh_savings();
            },
            t,
            cx,
        );

        let (figure, ink) = match measured {
            true => whole_saving(total, t),
            false => ("\u{2014}".to_owned(), dim),
        };
        let figure = div()
            .flex()
            .items_end()
            .gap(px(10.0))
            .pt(px(4.0))
            .child(
                div()
                    .font_family(crate::fonts::chrome())
                    .text_size(FIGURE)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ink)
                    .whitespace_nowrap()
                    .child(figure),
            )
            .children(percent(total).filter(|_| measured).map(|share| {
                div()
                    .prose()
                    .pb(px(5.0))
                    .text_size(NAME)
                    .text_color(ink)
                    .child(share)
            }));

        // The popup's ledger, with a rule between its lines: on the sheet
        // they are not in a well that already sets them apart.
        let ledger = measured.then(|| {
            let rows = ledger(total, t);
            let last = rows.len().saturating_sub(1);
            div()
                .flex()
                .flex_col()
                .children(rows.into_iter().enumerate().map(|(i, row)| {
                    row.when(i > 0, |row| {
                        row.pt(px(8.0)).border_t_1().border_color(paint(t.rule))
                    })
                    .when(i < last, |row| row.pb(px(8.0)))
                }))
        });

        let pair = |label: &'static str, figure: String| {
            div().flex().gap(px(5.0)).child(label).child(
                div()
                    .font_family(crate::fonts::chrome())
                    .text_color(paint(t.text.primary))
                    .child(figure),
            )
        };
        let foot = if savings.is_off() {
            caption("No worktree has Economy on yet", t)
        } else if !measured {
            caption(capitalise(&unmeasured(total)), t)
        } else {
            div()
                .flex()
                .justify_between()
                .pt(px(2.0))
                .text_size(CAPTION)
                .text_color(dim)
                .child(pair("Spent", money(total.spent_micros)))
                .child(pair(
                    "Without Economy",
                    approx(
                        total,
                        money(total.spent_micros.saturating_add(total.saved_micros())),
                    ),
                ))
        };

        economy_card(t)
            .child(card_head(
                Icon::PiggyBank,
                paint(t.diff.added),
                "Saved",
                period,
                t,
            ))
            .child(figure)
            .children(ledger)
            .child(foot)
            .into_any_element()
    }

    /// The Economy pane's second card: how many worktrees sit at each level,
    /// as the ring the sidebar draws, a bar and a count.
    pub(crate) fn economy_levels(&self) -> AnyElement {
        let t = &self.theme;
        let dim = paint(t.text.dim);
        let green = paint(t.diff.added);
        let default = ket_core::worktree::default_token_reduction();
        let worktrees = || self.projects.iter().flat_map(|project| &project.worktrees);
        let all = worktrees().count();
        let on = worktrees()
            .filter(|node| node.token_reduction != default)
            .count();
        let levels = ket_core::worktree::token_reduction_levels();
        let counts: Vec<usize> = levels
            .iter()
            .map(|level| {
                worktrees()
                    .filter(|node| node.token_reduction == level.level)
                    .count()
            })
            .collect();
        let most = counts.iter().copied().max().unwrap_or(0).max(1);

        let summary = match all {
            0 => "No worktrees yet".to_owned(),
            _ => format!("{on} of {all} on Economy"),
        };
        let rows = levels.iter().zip(counts).map(|(level, count)| {
            let ink = paint(match count {
                0 => t.text.dim,
                _ => t.text.primary,
            });
            let fill = match level.level == default {
                true => alpha(dim, 0.5),
                false => green,
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(crate::ui::chip::tinted_token_reduction_ring(
                    level.level,
                    green,
                    t,
                ))
                .child(
                    div()
                        .flex_none()
                        .w(LEVEL_NAME_W)
                        .text_size(NAME)
                        .text_color(ink)
                        .child(level.name().to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .h(LEVEL_BAR_H)
                        .rounded(LEVEL_BAR_H)
                        .bg(alpha(dim, 0.14))
                        .child(
                            div()
                                .h_full()
                                .w(relative(count as f32 / most as f32))
                                .rounded(LEVEL_BAR_H)
                                .bg(fill),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .justify_end()
                        .w(px(18.0))
                        .font_family(crate::fonts::chrome())
                        .text_size(ITEM)
                        .text_color(ink)
                        .child(count.to_string()),
                )
        });

        economy_card(t)
            .gap(px(9.0))
            .child(card_head(
                Icon::Orbit,
                paint(t.status.merging),
                "Your worktrees",
                caption(summary, t),
                t,
            ))
            .children(rows)
            .into_any_element()
    }

    /// The header's Economy figure — what token reduction has saved over the
    /// chosen period — and, pressed, the popup that breaks it down by project
    /// and worktree.
    ///
    /// Three states, each worded rather than left blank: no worktree is
    /// reduced, one is but nothing could be priced yet, and the whole saving.
    pub(crate) fn economy_status(
        &self,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::header::{COMPACT_TRIGGER_PAD, TRIGGER_PAD, stat, trail};

        let t = &self.theme;
        let savings = &self.savings;
        let total = &savings.total;
        let measured = total.priced > 0;
        let dim = paint(t.text.dim);
        let (figure, ink, note): (String, gpui::Rgba, Option<String>) = if savings.is_off() {
            ("off".to_owned(), dim, None)
        } else if !measured {
            let note = match total.unpriced {
                0 => "no sessions",
                _ => "not priced",
            };
            ("\u{2014}".to_owned(), dim, Some(note.to_owned()))
        } else {
            // Plain ink up here: green is the popover's, where the figure
            // has its columns and labels to say what it means.
            let (figure, _) = whole_saving(total, t);
            (figure, paint(t.text.primary), percent(total))
        };

        let face = match compact {
            true => crate::ui::chip::readout(t).child(div().text_color(ink).child(figure)),
            false => stat(
                "Economy",
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(div().text_color(ink).child(figure))
                    .children(note.map(|note| trail(note, t))),
                t,
            ),
        };

        let trigger = crate::ui::button::pressable("economy-status", t)
            .when(compact, |el| {
                el.p(COMPACT_TRIGGER_PAD).mx(-COMPACT_TRIGGER_PAD)
            })
            .when(!compact, |el| {
                el.px(TRIGGER_PAD).mx(-TRIGGER_PAD).py(px(5.0))
            })
            .child(face)
            .on_click(cx.listener(|this, _, _, cx| {
                if this.popup == Some(PopupKind::Savings) {
                    this.popup = None;
                } else {
                    // One non-modal overlay at a time, as the header's other
                    // figures have it.
                    this.close_palette(cx);
                    this.close_finder(cx);
                    this.menu = None;
                    this.project_menu = None;
                    this.worktree_menu = None;
                    this.refresh_savings();
                    this.popup = Some(PopupKind::Savings);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element();

        let content = (self.popup == Some(PopupKind::Savings)).then(|| self.savings_popup(cx));

        popup::anchored(
            "savings-popup",
            trigger,
            content,
            Placement::BelowStart,
            POPUP_W,
            window,
            t,
        )
    }

    /// The popup's body.
    fn savings_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let savings = &self.savings;
        let total = &savings.total;

        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(HEADER_H)
            .pl(PAD)
            .pr(px(6.0))
            .border_b_1()
            .border_color(paint(t.border))
            .child(
                div()
                    .flex_none()
                    .text_size(LABEL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Economy"),
            )
            .child(div().flex_grow())
            .child(crate::status_bar::segmented(
                "savings-period",
                &SavingsPeriod::ALL,
                self.savings_period,
                |this, period| {
                    this.savings_period = period;
                    this.persist_view();
                    this.refresh_savings();
                },
                t,
                cx,
            ));

        let panel = div().flex().flex_col().child(header);

        if savings.is_off() {
            return panel
                .child(div().px(PAD).py(px(12.0)).child(aside(
                    "No worktree has Economy on. Set a level from a worktree's menu \
                     — Economy — and what it saves collects here.",
                    t,
                )))
                .children(self.pack_waiting(cx))
                .into_any_element();
        }

        // The whole period in a well, headed by the two columns the lines
        // below repeat — what was spent, and what Economy saved — with the
        // saving's two parts, and the cache it leaves out, under a rule.
        let totals = {
            let measured = total.priced > 0;
            let share = match percent(total) {
                Some(percent) if measured => format!("{percent} than {}", default_level()),
                _ => capitalise(&unmeasured(total)),
            };
            let [spent, saved] = figures(total, t);
            // The period's spend is read first, so it is not dimmed the way
            // a line's is.
            let spent = spent.when(spent_figure(total).is_some(), |spent| {
                spent.text_color(paint(t.text.primary))
            });
            let heading =
                |text: &'static str| column(text, paint(t.text.dim)).prose().text_size(CAPTION);
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .mx(PAD)
                .mt(PAD)
                .mb(px(4.0))
                .px(px(12.0))
                .pt(px(10.0))
                // The ledger's lines carry their own room below.
                .pb(px(if measured { 4.0 } else { 10.0 }))
                .rounded(RADIUS_MD)
                .bg(paint(t.sunken))
                .border_1()
                .border_color(paint(t.border))
                .child(
                    div()
                        .flex()
                        .gap(COLUMN_GAP)
                        .child(
                            caption(
                                match measured {
                                    true => sessions(total.priced),
                                    false => String::new(),
                                },
                                t,
                            )
                            .prose()
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(heading("Spent"))
                        .child(heading("Saved")),
                )
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap(COLUMN_GAP)
                        .child(
                            caption(share, t)
                                .prose()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(alpha(paint(t.text.dim), 0.8)),
                        )
                        .child(spent.text_size(TOTAL).font_weight(FontWeight::SEMIBOLD))
                        .child(saved.text_size(TOTAL).font_weight(FontWeight::SEMIBOLD)),
                )
                .when(measured, |well| {
                    well.child(
                        div()
                            .flex()
                            .flex_col()
                            .mt(px(8.0))
                            .pt(px(2.0))
                            .border_t_1()
                            .border_color(paint(t.border))
                            .children(ledger(total, t).into_iter().map(|row| row.py(px(6.0)))),
                    )
                })
        };

        let list = div()
            .id("savings-projects")
            .flex()
            .flex_col()
            .max_h(LIST_MAX_H)
            .overflow_y_scroll()
            .pt(px(2.0))
            .pb(px(6.0))
            .children(savings.projects.iter().flat_map(|project| {
                let heading = line(
                    Some(lead(badge(project.color, project.mark.clone(), t))),
                    div()
                        .prose()
                        .min_w_0()
                        .truncate()
                        .text_size(NAME)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(paint(t.text.primary))
                        .child(project.name.clone())
                        .into_any_element(),
                    &project.total,
                    px(0.0),
                    t,
                );
                std::iter::once(heading).chain(project.worktrees.iter().map(|worktree| {
                    line(
                        Some(lead(token_reduction_ring(worktree.level, t))),
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(paint(t.text.primary))
                            .child(worktree.label.clone())
                            .into_any_element(),
                        &worktree.savings,
                        INDENT,
                        t,
                    )
                }))
            }))
            .when(
                savings.removed.priced + savings.removed.unpriced > 0,
                |list| {
                    // Says what "removed" means in so many words: worktrees
                    // deleted since, whose reduced sessions still count. One
                    // quiet line under a rule, no mark: it is not a project.
                    list.child(
                        div()
                            .mt(px(4.0))
                            .mx(PAD)
                            .border_t_1()
                            .border_color(paint(t.rule))
                            .child(
                                line(
                                    None,
                                    div()
                                        .prose()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(NAME)
                                        .text_color(paint(t.text.dim))
                                        .child("Removed worktrees, still counted")
                                        .into_any_element(),
                                    &savings.removed,
                                    px(0.0),
                                    t,
                                )
                                .px(px(0.0)),
                            ),
                    )
                },
            );

        // How the figures are made lives on the website; all the popup
        // keeps is which of them is an estimate, and what they leave out.
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(HEADER_H)
            .px(PAD)
            .border_t_1()
            .border_color(paint(t.border))
            .child(
                button("savings-docs", "How this is calculated")
                    .link()
                    .small()
                    .trailing(Icon::ExternalLink)
                    .render(t)
                    .on_click(|_, _, cx| cx.open_url(DOCS_URL)),
            )
            .child(div().flex_grow())
            .child(
                caption(
                    match total.unpriced {
                        0 => "≈ estimated".to_owned(),
                        unpriced => format!("≈ estimated \u{b7} {unpriced} not priced"),
                    },
                    t,
                )
                .prose(),
            );

        panel
            .child(totals)
            .child(list)
            .children(self.pack_waiting(cx))
            .child(footer)
            .into_any_element()
    }

    /// A pack somebody published, not yet one anybody adopted, and the way to
    /// adopt it. The whole point of staging: a fetch must not be the same act
    /// as a deployment onto someone else's machine.
    fn pack_waiting(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = &self.theme;
        let staged = ket_core::pack::staged(self.active_pack.as_ref())?;
        let in_force = self
            .active_pack
            .as_ref()
            .map(|pack| format!("v{}", pack.pack_version))
            .unwrap_or_else(|| "the built-in levels".to_owned());
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(PAD)
                .py(px(9.0))
                .border_t_1()
                .border_color(paint(t.border))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .prose()
                                .text_size(CAPTION)
                                .text_color(paint(t.text.primary))
                                .child(format!(
                                    "{} v{} available",
                                    staged
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| staged.pack_id.clone()),
                                    staged.pack_version
                                )),
                        )
                        .child(caption(format!("In force: {in_force}"), t).prose()),
                )
                .child(
                    button("pack-adopt", "Use it")
                        .small()
                        .render(t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            match ket_core::pack::promote() {
                                // Deliberately not applied to this run: the levels
                                // in force do not move while ket is open. See
                                // `activate_pack`.
                                Ok(()) => {
                                    this.note =
                                        Some("Pack adopted — in force next time ket starts".into())
                                }
                                Err(e) => this.note = Some(format!("Could not adopt: {e}").into()),
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    //! The arithmetic and wording behind the savings figures, apart from
    //! their drawing.

    use super::*;

    fn savings(measured: usize, spent: i64, saved: i64) -> WorktreeSavings {
        WorktreeSavings {
            priced: measured,
            spent_micros: spent,
            estimated_micros: saved,
            ..WorktreeSavings::default()
        }
    }

    #[test]
    fn a_period_reaches_back_its_own_length_and_all_reaches_back_to_zero() {
        let now = 100 * DAY_MS;
        assert_eq!(SavingsPeriod::Week.since_ms(now), 93 * DAY_MS);
        assert_eq!(SavingsPeriod::Month.since_ms(now), 70 * DAY_MS);
        assert_eq!(SavingsPeriod::All.since_ms(now), 0);
        // Never below zero on a clock that has barely started.
        assert_eq!(SavingsPeriod::Month.since_ms(DAY_MS), 0);
    }

    #[test]
    fn a_period_round_trips_through_its_stored_name() {
        for period in [
            SavingsPeriod::Week,
            SavingsPeriod::Month,
            SavingsPeriod::All,
        ] {
            assert_eq!(SavingsPeriod::from_name(period.name()), period);
        }
        assert_eq!(SavingsPeriod::from_name("fortnight"), SavingsPeriod::Week);
        assert_eq!(SavingsPeriod::Week.short(), "7d");
        assert_eq!(SavingsPeriod::Month.short(), "30d");
        assert_eq!(SavingsPeriod::All.short(), "All");
    }

    #[test]
    fn the_saving_is_a_share_of_what_it_would_have_cost_without_economy() {
        // Spent $3, saved $1: $4 without Economy, so a quarter less.
        assert_eq!(
            percent(&savings(2, 3_000_000, 1_000_000)).as_deref(),
            Some("25% less")
        );
        // Nothing measured, or nothing to divide by: no figure at all.
        assert_eq!(percent(&savings(0, 3_000_000, 1_000_000)), None);
        assert_eq!(percent(&savings(1, 0, 0)), None);
    }

    #[test]
    fn the_share_counts_both_savings_and_never_the_cache() {
        let both = WorktreeSavings {
            priced: 1,
            spent_micros: 2_000_000,
            filter_saved_micros: 1_000_000,
            estimated_micros: 1_000_000,
            // A cache read back a million times over changes nothing here.
            cache_read_tokens: 1_000_000_000,
            ..WorktreeSavings::default()
        };
        assert_eq!(percent(&both).as_deref(), Some("50% less"));
    }

    #[test]
    fn a_saving_is_green_and_none_is_dim() {
        let t = Theme::dark();
        assert_eq!(
            saving(2_250_000, &t),
            ("$2.25".to_owned(), paint(t.diff.added))
        );
        assert_eq!(saving(0, &t), ("$0.00".to_owned(), paint(t.text.dim)));
        // Too small for a cent is still a saving, and still says so.
        assert_eq!(saving(1, &t), ("<$0.01".to_owned(), paint(t.diff.added)));
    }

    #[test]
    fn a_line_shows_spend_once_anything_was_priced_and_a_saving_once_economy_was() {
        let t = Theme::dark();
        let none = WorktreeSavings::default();
        assert_eq!(spent_figure(&none), None);
        assert_eq!(saved_figure(&none, &t), None);

        // Sessions at the default level only: real spend, nothing to credit.
        let unreduced = WorktreeSavings {
            unreduced: 2,
            spent_micros: 5_000_000,
            ..WorktreeSavings::default()
        };
        assert_eq!(spent_figure(&unreduced).as_deref(), Some("$5.00"));
        assert_eq!(saved_figure(&unreduced, &t), None);

        let reduced = WorktreeSavings {
            priced: 1,
            filter_saved_micros: 2_000_000,
            ..unreduced
        };
        assert_eq!(
            saved_figure(&reduced, &t),
            Some(("$2.00".to_owned(), paint(t.diff.added)))
        );

        // With the estimate in it, the whole figure is an estimate.
        let estimated = WorktreeSavings {
            estimated_micros: 1_000_000,
            ..reduced
        };
        assert_eq!(
            saved_figure(&estimated, &t),
            Some(("≈$3.00".to_owned(), paint(t.diff.added)))
        );
    }

    #[test]
    fn commands_are_counted_in_words_with_thousands_grouped() {
        assert_eq!(commands(0), "0 commands");
        assert_eq!(commands(1), "1 command");
        assert_eq!(commands(2), "2 commands");
        assert_eq!(commands(999), "999 commands");
        assert_eq!(commands(1_000), "1,000 commands");
        assert_eq!(commands(1_284), "1,284 commands");
        assert_eq!(commands(1_234_567), "1,234,567 commands");
    }

    #[test]
    fn a_total_is_marked_an_estimate_only_when_the_estimate_is_in_it() {
        let t = Theme::dark();
        let filtered = WorktreeSavings {
            priced: 1,
            filter_saved_micros: 1_500_000,
            ..WorktreeSavings::default()
        };
        assert_eq!(approx(&filtered, "$1.50".to_owned()), "$1.50");
        assert_eq!(
            whole_saving(&filtered, &t),
            ("$1.50".to_owned(), paint(t.diff.added))
        );

        let both = WorktreeSavings {
            estimated_micros: 500_000,
            ..filtered
        };
        assert_eq!(approx(&both, "$2.00".to_owned()), "≈$2.00");
        assert_eq!(
            whole_saving(&both, &t),
            ("≈$2.00".to_owned(), paint(t.diff.added))
        );

        // Nothing saved at all: plain, and dim.
        assert_eq!(
            whole_saving(&WorktreeSavings::default(), &t),
            ("$0.00".to_owned(), paint(t.text.dim))
        );
    }

    #[test]
    fn the_share_rounds_to_a_whole_percent() {
        // $38 saved on $322 spent: 10.6%, which reads as 11.
        assert_eq!(
            percent(&savings(1, 322_000_000, 38_000_000)).as_deref(),
            Some("11% less")
        );
        // Too small to round up to one is still said, as nothing.
        assert_eq!(
            percent(&savings(1, 1_000_000_000, 1)).as_deref(),
            Some("0% less")
        );
    }

    #[test]
    fn the_filter_says_so_when_it_has_trimmed_nothing_yet() {
        let unfiltered = WorktreeSavings {
            command_calls: 3,
            ..WorktreeSavings::default()
        };
        assert_eq!(
            filter_detail(&unfiltered),
            "0 tokens trimmed \u{b7} 3 commands"
        );
    }

    #[test]
    fn a_removed_worktree_with_only_unpriced_sessions_still_turns_the_rollup_on() {
        let removed = Savings {
            removed: WorktreeSavings {
                unpriced: 1,
                ..WorktreeSavings::default()
            },
            ..Savings::default()
        };
        assert!(!removed.is_off());
    }

    #[test]
    fn an_estimate_says_it_is_one() {
        assert_eq!(estimated(1_250_000), "≈$1.25");
        assert_eq!(estimated(1), "≈<$0.01");
    }

    #[test]
    fn sessions_are_counted_in_words() {
        assert_eq!(sessions(1), "1 session");
        assert_eq!(sessions(0), "0 sessions");
        assert_eq!(sessions(3), "3 sessions");
    }

    #[test]
    fn a_figure_with_no_saving_says_why() {
        let none = WorktreeSavings::default();
        assert_eq!(unmeasured(&none), "no sessions yet");

        let unpriced = WorktreeSavings {
            unpriced: 2,
            unreduced: 5,
            ..WorktreeSavings::default()
        };
        // Unpriced sessions are the more useful thing to say.
        assert_eq!(unmeasured(&unpriced), "2 sessions not priced");

        let default_level = WorktreeSavings {
            unreduced: 1,
            ..WorktreeSavings::default()
        };
        let said = unmeasured(&default_level);
        assert!(said.starts_with("1 session at "), "{said}");
    }

    #[test]
    fn a_clause_is_capitalised_at_the_start_of_a_sentence() {
        assert_eq!(capitalise("no sessions yet"), "No sessions yet");
        assert_eq!(capitalise(""), "");
        assert_eq!(capitalise("élan"), "Élan");
    }

    #[test]
    fn the_filter_says_what_it_trimmed_over_how_many_commands() {
        let one = WorktreeSavings {
            command_calls: 1,
            filter_saved_tokens: 2_500,
            ..WorktreeSavings::default()
        };
        assert_eq!(filter_detail(&one), "2K tokens trimmed \u{b7} 1 command");

        let many = WorktreeSavings {
            command_calls: 1_284,
            filter_saved_tokens: 612_000,
            ..WorktreeSavings::default()
        };
        assert_eq!(
            filter_detail(&many),
            "612K tokens trimmed \u{b7} 1,284 commands"
        );
    }

    #[test]
    fn a_rollup_is_off_until_something_was_reduced() {
        assert!(Savings::default().is_off());
        let removed = Savings {
            removed: WorktreeSavings {
                priced: 1,
                ..WorktreeSavings::default()
            },
            ..Savings::default()
        };
        assert!(!removed.is_off());
    }
}
