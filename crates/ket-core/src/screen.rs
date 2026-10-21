//! What a terminal's screen says that no hook does.
//!
//! An agent's TUI can draw a dialog of its own over a prompt — Codex's
//! "Approaching rate limits" notice, offering a cheaper model — and tell its
//! hooks nothing. The approval a phone was told about is then underneath,
//! and the keys an answer presses go to the notice instead. The host owns
//! the terminal, so it reads the rows the dialog is drawn in and puts the
//! dialog itself on the pane's status, as a question with the dialog's own
//! numbered options — see [`crate::agent_status::StatusStore::screen_dialog`].
//!
//! Read only for a Codex pane already waiting on a prompt, on the host's
//! one-second tick, [`DIALOG_ROWS`] rows under the parser's lock: nothing
//! for any other agent, and nothing for a Codex pane at work or idle.
//!
//! Grok's own permission menu is read here too, but only when a phone
//! answers it: its rows change with the prompt, so the number to press is
//! found by label at that moment — see [`grok_permission_menu`].

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;

/// Rows at the foot of the screen a dialog is read from.
pub const DIALOG_ROWS: usize = 16;

/// How close to the bottom a live dialog's footer sits. The TUI draws the
/// dialog at the foot of its viewport; the same lines left higher up are
/// scrollback, not a dialog.
const FOOTER_ROWS: usize = 3;

/// The tool name a screen-read dialog is reported under, in place of the
/// tool whose prompt it covers. Not a tool: a phone treats it as a question.
pub const DIALOG_TOOL: &str = "Dialog";

/// A dialog read off the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    /// Its heading: "Approaching rate limits".
    pub title: String,
    /// What it asks, in its own words.
    pub question: String,
    /// Its numbered options in order, without the numbers.
    pub options: Vec<String>,
}

/// The bottom `n` rows of the screen as text, each trimmed on the right.
pub fn bottom_rows<T>(term: &Term<T>, n: usize) -> Vec<String> {
    let grid = term.grid();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    (rows.saturating_sub(n)..rows)
        .map(|row| {
            let line = Line(i32::try_from(row).unwrap_or(i32::MAX));
            let text: String = (0..cols).map(|col| grid[line][Column(col)].c).collect();
            text.trim_end().to_owned()
        })
        .collect()
}

/// Codex's notice that a usage limit is near, as Codex CLI 0.156 draws it:
///
/// ```text
/// Approaching rate limits
/// Switch to gpt-6-luna for lower credit usage?
/// › 1. Switch to gpt-6-luna
///   2. Keep current model
///   3. Keep current model (never show again)
/// Press enter to confirm or esc to go back
/// ```
///
/// A digit picks its option and Enter confirms. The options are read from
/// the rows, not hard-coded, so the model it offers never has to be known
/// here. `None` unless the heading, at least two numbered options and the
/// footer are all on screen, with the footer within [`FOOTER_ROWS`] of the
/// bottom: that is what says the dialog is up now rather than left in the
/// scrollback above.
pub fn codex_rate_limit(rows: &[String]) -> Option<Dialog> {
    const TITLE: &str = "Approaching rate limits";
    let at = rows.iter().rposition(|row| row.contains(TITLE))?;
    let live_from = rows.len().saturating_sub(FOOTER_ROWS);
    let mut question = None;
    let mut options = Vec::new();
    let mut footer = false;
    for (index, row) in rows.iter().enumerate().skip(at + 1) {
        let row = row.trim();
        if row.is_empty() {
            continue;
        }
        if let Some(option) = numbered_option(row) {
            options.push(option);
            continue;
        }
        let lower = row.to_lowercase();
        if lower.contains("enter") && lower.contains("esc") {
            footer = index >= live_from;
            break;
        }
        if options.is_empty() && question.is_none() {
            question = Some(row.to_owned());
        }
    }
    (footer && options.len() >= 2).then(|| Dialog {
        title: TITLE.to_owned(),
        question: question.unwrap_or_default(),
        options,
    })
}

/// `1. Switch to gpt-6-luna`, with or without a selection mark before the
/// number.
fn numbered_option(row: &str) -> Option<String> {
    let row = row.trim_start_matches(['\u{203a}', '>', '\u{276f}', '\u{2022}', ' ']);
    let (number, rest) = row.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// Grok's permission menu, as Grok 1.0.62's TUI draws it: the options in
/// order, each with the digit that takes it.
///
/// ```text
/// ┃  1 (○) Yes, and don't ask again for anything (always-approve mode)
/// ┃  2 (○) Always allow: rm a.txt
/// ┃  3 (●) Yes, proceed
/// ┃  4 (○) No, reject (type to add feedback)
/// ┃  5 (○) Never allow: rm a.txt
///
/// 1/5:select  │  Tab:next option  │  ←/→:scope  │  e:edit pattern  │  …
/// ```
///
/// The rows are not fixed. A chained command, or one Grok cannot remember
/// a grant for, gets no Always allow and no Never allow, and the same
/// prompt is then `1` always-approve, `2` Yes, proceed, `3` No, reject —
/// which is why a number pressed blind turned a phone's Approve into a
/// rejection. `None` unless the options run from `1` without a gap and the
/// footer, within [`FOOTER_ROWS`] of the bottom, counts the same number of
/// them: that is what says the menu is up now, whole. Only the footer's
/// `n/N:select` is relied on, not the hints after it, which a terminal a
/// phone has narrowed cuts off.
pub fn grok_permission_menu(rows: &[String]) -> Option<Vec<(u8, String)>> {
    let live_from = rows.len().saturating_sub(FOOTER_ROWS);
    let footer = rows
        .iter()
        .rposition(|row| row.trim_start().split_once(":select").is_some())
        .filter(|at| *at >= live_from)?;
    let counted: usize = rows[footer]
        .trim()
        .split_once(":select")
        .and_then(|(count, _)| count.split_once('/'))
        .and_then(|(_, of)| of.trim().parse().ok())?;
    let mut options: Vec<(u8, String)> = Vec::new();
    for row in &rows[..footer] {
        let Some((digit, label)) = grok_option(row) else {
            continue;
        };
        // A `1` starts the menu over: anything numbered above it was not
        // this menu.
        if digit == 1 {
            options.clear();
        }
        let next = u8::try_from(options.len() + 1).ok()?;
        if digit != next {
            return None;
        }
        options.push((digit, label));
    }
    (!options.is_empty() && options.len() == counted).then_some(options)
}

/// `┃  3 (●) Yes, proceed`: the digit and the label, with or without the
/// bar and whichever row is selected.
fn grok_option(row: &str) -> Option<(u8, String)> {
    let row = row.trim_start_matches(['\u{2503}', '\u{2502}', ' ']);
    let (number, rest) = row.split_once(' ')?;
    let digit: u8 = number
        .parse()
        .ok()
        .filter(|digit| (1..=9).contains(digit))?;
    let rest = rest
        .trim_start()
        .strip_prefix("(\u{25cb})")
        .or_else(|| rest.trim_start().strip_prefix("(\u{25cf})"))?;
    let label = rest.trim();
    (!label.is_empty()).then(|| (digit, label.to_owned()))
}
