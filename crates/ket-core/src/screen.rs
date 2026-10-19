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
