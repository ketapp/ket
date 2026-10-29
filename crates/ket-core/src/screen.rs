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
//! Codex also asks things no hook reports at all. A plan finished in Plan
//! mode ends its turn — `Stop` fires — and only then does the TUI ask
//! "Implement this plan?", so the pane reads idle while it waits on a
//! person. That is read here the same way, and the pane waits on it.
//!
//! Read only for a Codex pane waiting on a prompt or idle, on the host's
//! one-second tick, [`DIALOG_ROWS`] rows under the parser's lock: nothing
//! for any other agent, and nothing for a Codex pane at work.
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
    /// Whether Enter follows the option's number. Codex's pickers take the
    /// number alone, and an Enter after it lands in whatever comes next —
    /// the composer, with any draft in it.
    pub enter: bool,
}

impl Dialog {
    /// The keys that pick option `at`, counted from zero.
    pub fn keys(&self, at: usize) -> Vec<u8> {
        let mut keys = (at + 1).to_string().into_bytes();
        if self.enter {
            keys.push(b'\r');
        }
        keys
    }
}

/// Whichever Codex dialog is up at the foot of the screen, if any — see
/// [`codex_rate_limit`] and [`codex_plan_implementation`].
pub fn codex_dialog(rows: &[String]) -> Option<Dialog> {
    codex_rate_limit(rows).or_else(|| codex_plan_implementation(rows))
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
    // Answered as #77 shipped it, number and Enter.
    codex_titled(rows, "Approaching rate limits", true)
}

/// Codex's question once a plan made in Plan mode is done, as Codex CLI
/// 0.164 draws it:
///
/// ```text
/// Implement this plan?
///
/// › 1. Yes, implement this plan          Switch to Default and start coding
///   2. Yes, clear context and implement  Fresh thread with this plan
///   3. No, stay in Plan mode             Continue planning with the model
///
/// Press enter to confirm or esc to go back
/// ```
///
/// It is drawn after the turn's `Stop`, and no hook says it is up. A digit
/// picks its option and takes it at once, so no Enter follows. The options
/// are their labels alone, without the descriptions beside them. `None` on
/// the same terms as [`codex_rate_limit`].
pub fn codex_plan_implementation(rows: &[String]) -> Option<Dialog> {
    codex_titled(rows, "Implement this plan?", false)
}

/// A Codex picker headed `title`: an optional line asking something, its
/// numbered options, and the footer that says it is live.
fn codex_titled(rows: &[String], title: &str, enter: bool) -> Option<Dialog> {
    let at = rows.iter().rposition(|row| row.contains(title))?;
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
        title: title.to_owned(),
        question: question.unwrap_or_default(),
        options,
        enter,
    })
}

/// `1. Switch to gpt-6-luna`, with or without a selection mark before the
/// number. A description drawn beside the label, a column over, is left
/// off.
fn numbered_option(row: &str) -> Option<String> {
    let row = row.trim_start_matches(['\u{203a}', '>', '\u{276f}', '\u{2022}', ' ']);
    let (number, rest) = row.split_once(". ")?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim();
    let label = rest.split_once("  ").map_or(rest, |(label, _)| label);
    (!label.is_empty()).then(|| label.to_owned())
}

/// Whether Codex's `request_user_input` form is up on its first question
/// of `count`, none of them answered yet: `Question 1/2 (2 unanswered)`,
/// as Codex CLI 0.164 heads it. Its numbers answer one question each, in
/// order, so they mean what the phone meant only from there.
pub fn codex_user_input_from_start(rows: &[String], count: usize) -> bool {
    let start = format!("Question 1/{count} ({count} unanswered)");
    rows.iter()
        .rev()
        .map(|row| row.trim())
        .find(|row| row.starts_with("Question ") && row.contains('/'))
        .is_some_and(|row| row.starts_with(&start))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<String> {
        text.lines().map(|row| row.trim_end().to_owned()).collect()
    }

    /// Codex CLI 0.164's plan question, at the foot of an 80-column screen.
    const PLAN: &str = "\
• Proposed plan: split the parser.

  Implement this plan?


› 1. Yes, implement this plan          Switch to Default and start coding
  2. Yes, clear context and implement  Start a fresh thread (current context:
                                       89% used)
  3. No, stay in Plan mode             Continue planning with the model

  Press enter to confirm or esc to go back";

    #[test]
    fn codexs_plan_question_is_read_with_its_labels_alone() {
        let dialog = codex_dialog(&rows(PLAN)).expect("the plan question");
        assert_eq!(dialog.title, "Implement this plan?");
        assert_eq!(dialog.question, "");
        assert_eq!(
            dialog.options,
            [
                "Yes, implement this plan",
                "Yes, clear context and implement",
                "No, stay in Plan mode"
            ]
        );
        // The number takes it; an Enter would go to the composer after.
        assert_eq!(dialog.keys(0), b"1");
    }

    #[test]
    fn a_plan_question_left_in_the_scrollback_is_not_up() {
        let mut screen = rows(PLAN);
        screen.extend(["", "", "", "› ", "", "  ? for shortcuts"].map(str::to_owned));
        assert_eq!(codex_dialog(&screen), None);
    }

    #[test]
    fn the_rate_limit_notice_keeps_its_enter() {
        let screen = rows(
            "\
  Approaching rate limits
  Switch to gpt-6-luna for lower credit usage?
› 1. Switch to gpt-6-luna
  2. Keep current model
  3. Keep current model (never show again)
  Press enter to confirm or esc to go back",
        );
        let dialog = codex_dialog(&screen).expect("the notice");
        assert_eq!(dialog.title, "Approaching rate limits");
        assert_eq!(
            dialog.question,
            "Switch to gpt-6-luna for lower credit usage?"
        );
        assert_eq!(dialog.options.len(), 3);
        assert_eq!(dialog.keys(1), b"2\r");
    }

    #[test]
    fn codexs_form_is_answered_only_from_its_first_question() {
        let form = |progress: &str| {
            rows(&format!(
                "\
  {progress}
  Which crate?

  › 1. ket-core  The engine
    2. ket-ui    The window

  tab to add notes | enter to submit answer | esc to interrupt"
            ))
        };
        assert!(codex_user_input_from_start(
            &form("Question 1/2 (2 unanswered)"),
            2
        ));
        assert!(codex_user_input_from_start(
            &form("Question 1/1 (1 unanswered) · auto-resolves in 1m 00s"),
            1
        ));
        // Moved on at the desk, partly answered, or another form entirely.
        assert!(!codex_user_input_from_start(
            &form("Question 2/2 (1 unanswered)"),
            2
        ));
        assert!(!codex_user_input_from_start(
            &form("Question 1/2 (1 unanswered)"),
            2
        ));
        assert!(!codex_user_input_from_start(
            &form("Question 1/3 (3 unanswered)"),
            2
        ));
        assert!(!codex_user_input_from_start(&rows("› "), 1));
    }
}
