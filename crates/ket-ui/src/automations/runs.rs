//! The chosen automation's figures — whether it is on, when it starts next,
//! how it repeats, what the last week came to — and the Runs tab's table of
//! every run, newest first.

use gpui::{AnyElement, Context, IntoElement, ParentElement, Rgba, Styled, div, prelude::*, px};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use ket_core::automation::{Automation, Outcome, Run};
use ket_core::theme::Theme;

use super::{day_of, time_of};
use crate::Shell;
use crate::paint::paint;
use crate::ui::button::icon_button;
use crate::ui::chip::{caption, tag, tinted_tag};
use crate::ui::figure::figure;
use crate::ui::icon::{Icon, sized_icon};
use crate::ui::table::{Column, header, row, two_lines};

/// The runs table's columns.
const COLUMNS: [Column; 6] = [
    Column::fixed("Started", px(176.0)),
    Column::fixed("Result", px(112.0)),
    Column::fill("Worktree"),
    Column::fixed("Took", px(72.0)).at_end(),
    Column::fixed("Tokens", px(72.0)).at_end(),
    Column::fixed("", px(36.0)),
];

/// A week, for the strip's last figure.
const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// A start late by more than this says when it was due.
const LATE_MS: u64 = 2 * 60 * 1000;

/// The colour an outcome is drawn in: the signal it is.
pub(super) fn outcome_colour(outcome: &Outcome, t: &Theme) -> Rgba {
    paint(match outcome {
        Outcome::Started | Outcome::Done => t.status.running,
        Outcome::Failed { .. } | Outcome::CouldNotStart { .. } => t.status.failed,
        Outcome::Skipped { .. } | Outcome::Missed { .. } => t.text.dim,
    })
}

/// A run's result, as a tag in its signal's colour — a neutral one for a
/// start that was skipped or missed, which is a fact rather than a fault.
fn result_tag(outcome: &Outcome, t: &Theme) -> AnyElement {
    match outcome {
        Outcome::Skipped { .. } | Outcome::Missed { .. } => {
            tag(outcome.label(), t).into_any_element()
        }
        _ => tinted_tag(outcome.label(), outcome_colour(outcome, t)).into_any_element(),
    }
}

/// `Thu 19 Nov, 2:00 AM`.
fn when(ms: u64, tz: &TimeZone) -> String {
    let Some(at) = i64::try_from(ms)
        .ok()
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
    else {
        return String::new();
    };
    format!("{}, {}", day_of(at, tz), time_of(at, tz))
}

/// `38 min`, `26 h`.
fn took(run: &Run) -> Option<String> {
    let minutes = run.ended_ms?.saturating_sub(run.at_ms) / 60_000;
    Some(match minutes {
        0 => "<1 min".to_owned(),
        1..=99 => format!("{minutes} min"),
        _ => format!("{} h", minutes / 60),
    })
}

/// Why a start that didn't happen didn't.
fn reason(outcome: &Outcome) -> Option<&str> {
    match outcome {
        Outcome::Failed { why }
        | Outcome::Skipped { why }
        | Outcome::Missed { why }
        | Outcome::CouldNotStart { why } => Some(why),
        Outcome::Started | Outcome::Done => None,
    }
}

/// The strip of figures over the tabs, led by `toggle`, the switch that
/// pauses the automation, so it sits beside the next start it governs.
pub(super) fn strip(shell: &Shell, automation: &Automation, toggle: AnyElement) -> AnyElement {
    let t = &shell.theme;
    let tz = TimeZone::system();
    let Some(dialog) = shell.automations.as_ref() else {
        return div().into_any_element();
    };
    let runs: Vec<&Run> = dialog.runs.of(&automation.id).collect();
    let now_ms = ket_core::now_ms();

    let next = automation.next_start(Timestamp::now(), &tz).map_or_else(
        || {
            if automation.finished() {
                "Has had its start"
            } else {
                "Paused"
            }
            .to_owned()
        },
        |next| format!("{}, {}", day_of(next, &tz), time_of(next, &tz)),
    );
    let week: Vec<&&Run> = runs
        .iter()
        .filter(|run| now_ms.saturating_sub(run.at_ms) < WEEK_MS)
        .filter(|run| {
            matches!(
                run.outcome,
                Outcome::Started | Outcome::Done | Outcome::Failed { .. }
            )
        })
        .collect();
    let tokens: u64 = week.iter().filter_map(|run| run.tokens).sum();
    let week_said = match (week.len(), tokens) {
        (0, _) => "No runs".to_owned(),
        (1, 0) => "1 run".to_owned(),
        (n, 0) => format!("{n} runs"),
        (n, tokens) => format!("{n} runs \u{b7} {}k tokens", tokens / 1000),
    };
    let cell = |content: AnyElement, first: bool| {
        div()
            .flex_1()
            .px(px(14.0))
            .py(px(10.0))
            .when(!first, |el| el.border_l_1().border_color(paint(t.rule)))
            .child(content)
    };
    let schedule = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(toggle)
        .child(if automation.enabled { "On" } else { "Paused" });
    div()
        .flex()
        .flex_none()
        .rounded(crate::ui::RADIUS_MD)
        .border_1()
        .border_color(paint(t.border))
        .child(cell(
            figure("Schedule", schedule, t).into_any_element(),
            true,
        ))
        .child(cell(
            figure("Next start", next, t).into_any_element(),
            false,
        ))
        .child(cell(
            figure("Repeats", automation.when.describe(), t).into_any_element(),
            false,
        ))
        .child(cell(
            figure("Last 7 days", week_said, t).into_any_element(),
            false,
        ))
        .into_any_element()
}

/// The Runs tab for `automation`.
pub(super) fn runs_tab(
    shell: &Shell,
    automation: &Automation,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let t = &shell.theme;
    let tz = TimeZone::system();
    let Some(dialog) = shell.automations.as_ref() else {
        return div().into_any_element();
    };
    let runs: Vec<&Run> = dialog.runs.of(&automation.id).collect();

    let mut table = div()
        .id("automation-runs")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(header(&COLUMNS, t));
    for (at, run) in runs.iter().enumerate() {
        let started = when(run.at_ms, &tz);
        let late = run.at_ms.saturating_sub(run.due_ms) > LATE_MS
            && matches!(
                run.outcome,
                Outcome::Started | Outcome::Done | Outcome::Failed { .. }
            );
        let started: AnyElement = if late {
            two_lines(
                div().child(started),
                format!(
                    "Due {} \u{b7} the Mac was asleep",
                    time_of_ms(run.due_ms, &tz)
                ),
                t,
            )
            .into_any_element()
        } else {
            div().child(started).into_any_element()
        };
        let place: AnyElement = match (&run.branch, reason(&run.outcome)) {
            (Some(branch), None) => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .child(sized_icon(Icon::GitBranch, px(13.0), paint(t.text.dim)))
                .child(div().truncate().child(branch.clone()))
                .into_any_element(),
            (_, Some(why)) => caption(why.to_owned(), t).truncate().into_any_element(),
            (None, None) => caption("\u{2014}", t).into_any_element(),
        };
        let dash = || caption("\u{2014}", t).into_any_element();
        let took_cell = took(run).map_or_else(dash, |took| caption(took, t).into_any_element());
        let tokens_cell = run.tokens.map_or_else(dash, |tokens| {
            caption(format!("{}k", tokens.div_ceil(1000)), t).into_any_element()
        });
        let worktree = run.worktree.clone();
        let open: AnyElement = match worktree {
            Some(worktree) => icon_button(("automation-run-open", at), Icon::ExternalLink)
                .bare()
                .small()
                .render(t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(selection) = this.position_of(&worktree) {
                        this.close_automations();
                        this.select(selection, cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
            None => div().into_any_element(),
        };
        table = table.child(row(
            ("automation-run", at),
            &COLUMNS,
            vec![
                started,
                result_tag(&run.outcome, t),
                place,
                took_cell,
                tokens_cell,
                open,
            ],
            t,
        ));
    }
    if runs.is_empty() {
        let first = automation
            .next_start(Timestamp::now(), &tz)
            .map(|next| {
                format!(
                    " Its first start is {}, {}.",
                    day_of(next, &tz),
                    time_of(next, &tz)
                )
            })
            .unwrap_or_default();
        table = table.child(
            caption(format!("Not run yet.{first}"), t)
                .px(px(8.0))
                .pt(px(12.0)),
        );
    }

    table.into_any_element()
}

fn time_of_ms(ms: u64, tz: &TimeZone) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .map(|at| time_of(at, tz))
        .unwrap_or_default()
}
