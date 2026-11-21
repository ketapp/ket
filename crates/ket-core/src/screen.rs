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
//! Read for Codex panes waiting on a prompt, idle, or still starting, on
//! the host's one-second tick. Folder access is asked before any hook can
//! report a session and can sit above empty rows, so the visible screen is
//! read under the parser's lock. Nothing is read for a Codex pane at work.
//!
//! Grok's own permission menu is read here too, but only when a phone
//! answers it: its rows change with the prompt, so the number to press is
//! found by label at that moment — see [`grok_permission_menu`].
//!
//! OpenCode's `/clear` and `/new` leave the TUI on its home screen without
//! creating a session, so the pane still names the conversation that just
//! left. [`opencode_home`] is how the phone tells, and starts the chat over.

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

/// The heading of Codex's startup question about hooks — see
/// [`codex_hook_review`].
pub const HOOK_REVIEW: &str = "Hooks need review";

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
        // Hook review's own first option opens a panel only the terminal
        // can work, and is left out: what is offered starts at Codex's 2.
        let first = if self.title == HOOK_REVIEW { 2 } else { 1 };
        let mut keys = (at + first).to_string().into_bytes();
        // Trusting requires Enter after its number, but the other choice
        // leaves immediately. An Enter after that would land in what comes
        // next: the Agent Command Center, or the composer.
        let leaves = at == 1 && (self.title == "Folder access" || self.title == HOOK_REVIEW);
        if self.enter && !leaves {
            keys.push(b'\r');
        }
        keys
    }
}

/// Whichever Codex dialog is on the visible screen, if any — see
/// [`codex_folder_access`], [`codex_hook_review`], [`codex_rate_limit`] and
/// [`codex_plan_implementation`].
pub fn codex_dialog(rows: &[String]) -> Option<Dialog> {
    let footer = &rows[rows.len().saturating_sub(DIALOG_ROWS)..];
    codex_startup_question(rows)
        .or_else(|| codex_rate_limit(footer))
        .or_else(|| codex_plan_implementation(footer))
}

/// Codex's startup folder-access question. Unlike its in-session pickers,
/// this is drawn at the top of the screen with empty rows below it, so
/// callers must supply the whole visible screen. Keep the folder paths and
/// explanation in the question: the person must see what they are trusting.
pub fn codex_folder_access(rows: &[String]) -> Option<Dialog> {
    let at = rows.iter().rposition(|row| row.trim() == "Folder access")?;
    let end = rows.iter().rposition(|row| !row.trim().is_empty())?;
    let footer = rows[end].trim().to_lowercase();
    if !footer.contains("enter") || !footer.contains("continue") || !footer.contains("esc") {
        return None;
    }
    let choices = rows.get(at + 1..end)?;
    let first = choices
        .iter()
        .position(|row| numbered_option(row).is_some())?;
    let options: Vec<String> = choices[first..]
        .iter()
        .filter_map(|row| numbered_option(row))
        .collect();
    if options.len() != 2 || options[0] != "Trust and continue" {
        return None;
    }
    Some(Dialog {
        title: "Folder access".to_owned(),
        question: choices[..first].join("\n").trim().to_owned(),
        options,
        enter: true,
    })
}

/// Whichever of Codex's startup questions is up — [`codex_folder_access`]
/// or [`codex_hook_review`] — over the whole visible screen. Either holds a
/// first prompt back until somebody answers it, and drops a paste made
/// meanwhile.
pub fn codex_startup_question(rows: &[String]) -> Option<Dialog> {
    codex_folder_access(rows).or_else(|| codex_hook_review(rows))
}

/// Codex's startup question when a hook is new or changed since it was
/// last trusted, as Codex CLI 0.170 draws it:
///
/// ```text
/// Hooks need review
/// 1 hook is new or changed.
/// Hooks can run outside the sandbox after you trust them.
///
/// › 1. Review hooks
///   2. Trust all and continue
///   3. Continue without trusting (hooks won't run)
///
/// enter confirm · esc skip
/// ```
///
/// Drawn like folder access, at the top over empty rows and before the
/// composer, so callers must supply the whole visible screen, and a task's
/// prompt cannot go in until it is answered. "Review hooks" opens Codex's
/// own hook panel, which only the terminal can work, so the dialog offers
/// the other two (see [`Dialog::keys`]). The question is Codex's own words:
/// the screen does not say which hooks, and nothing outside Codex can tell
/// which of them it has trusted.
pub fn codex_hook_review(rows: &[String]) -> Option<Dialog> {
    let at = rows.iter().rposition(|row| row.trim() == HOOK_REVIEW)?;
    let end = rows.iter().rposition(|row| !row.trim().is_empty())?;
    let footer = rows[end].trim().to_lowercase();
    if !footer.contains("enter") || !footer.contains("esc") {
        return None;
    }
    let body = rows.get(at + 1..end)?;
    let first = body.iter().position(|row| numbered_option(row).is_some())?;
    let options: Vec<String> = body[first..]
        .iter()
        .filter_map(|row| numbered_option(row))
        .collect();
    if options.len() != 3 || options[0] != "Review hooks" || !options[1].starts_with("Trust all") {
        return None;
    }
    let question: Vec<&str> = body[..first]
        .iter()
        .map(|row| row.trim())
        .filter(|row| !row.is_empty())
        .collect();
    Some(Dialog {
        title: HOOK_REVIEW.to_owned(),
        question: question.join("\n"),
        options: options[1..].to_vec(),
        enter: true,
    })
}

/// The session id a phone is given while OpenCode is on its home screen.
/// Distinct from OpenCode's own `ses_…` ids, so the chat starts over.
pub const OPENCODE_HOME: &str = "home";

/// Whether OpenCode's home screen is drawn — `/clear` and `/new` land here
/// without creating a session.
///
/// The TUI draws the OPENCODE wordmark in block characters, or a smaller
/// mark when the terminal is narrow. A session view never uses those rows.
/// Shorter than 12 rows, the mark is omitted and this cannot tell.
pub fn opencode_home(rows: &[String]) -> bool {
    rows.iter().any(|row| {
        // Full and medium wordmark: two `█▀▀█` side by side, or the `█^^^`
        // row. The small mark is `█_^█`, or `█▀▀█` over `█__█` when narrow.
        row.contains("\u{2588}\u{2580}\u{2580}\u{2588} \u{2588}\u{2580}\u{2580}\u{2588}")
            || row.contains("\u{2588}^^^")
            || row.contains("\u{2588}_^\u{2588}")
    }) || {
        let mark = rows
            .iter()
            .any(|row| row.contains("\u{2588}\u{2580}\u{2580}\u{2588}"));
        let body = rows.iter().any(|row| row.contains("\u{2588}__\u{2588}"));
        mark && body
    }
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

/// Whether Codex is ready for a first prompt: a visible cursor just after
/// its empty `›` composer and its normal footer below, with paste enabled.
/// Placeholder text after the cursor is allowed. A menu, trust screen,
/// shell prompt or a draft someone has already typed is not an empty input.
pub fn codex_prompt_ready<T>(term: &Term<T>) -> bool {
    use alacritty_terminal::term::TermMode;

    if !term
        .mode()
        .contains(TermMode::BRACKETED_PASTE | TermMode::SHOW_CURSOR)
    {
        return false;
    }
    let grid = term.grid();
    let cursor = grid.cursor.point;
    let Ok(row) = usize::try_from(cursor.line.0) else {
        return false;
    };
    if row >= grid.screen_lines() || cursor.column.0 >= grid.columns() {
        return false;
    }
    let prefix: String = (0..cursor.column.0)
        .map(|col| grid[cursor.line][Column(col)].c)
        .collect();
    if prefix.trim() != "›" {
        return false;
    }
    let rows = bottom_rows(term, usize::MAX);
    // A selected menu option uses the same › as the composer. During a
    // redraw, its cursor and a footer from the preceding screen can still
    // be visible. Sending Enter there accepts the option (not the task).
    if numbered_option(&rows[row]).is_some() {
        return false;
    }
    // The footer's hints are separated by `·`: Codex 0.170 leads with
    // `← for agents` after a startup question, ahead of `? for shortcuts`,
    // and can draw a warning at the far end of the same row.
    rows.iter().skip(row + 1).any(|line| {
        line.split('\u{b7}')
            .any(|hint| hint.trim().starts_with("? for shortcuts"))
            || line.contains("context left")
    })
}

/// Whether Claude Code has drawn its input box, so a first prompt typed now
/// reaches it rather than the shell it starts from: a `❯` row just under
/// the second-last of the full-width rules it draws over and under the box,
/// with paste on. A selected menu option — the folder-trust screen's
/// `❯ 1. Yes, proceed` — is not the box, and a shell's `❯` prompt has no
/// rules round it.
pub fn claude_prompt_ready<T>(term: &Term<T>) -> bool {
    use alacritty_terminal::term::TermMode;

    if !term.mode().contains(TermMode::BRACKETED_PASTE) {
        return false;
    }
    let rows = bottom_rows(term, usize::MAX);
    let rules: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| crate::agent_view::is_rule(row))
        .map(|(at, _)| at)
        .collect();
    let [.., above, below] = rules.as_slice() else {
        return false;
    };
    if below - above < 2 {
        return false;
    }
    let first = &rows[above + 1];
    first.trim_start().starts_with('\u{276f}') && numbered_option(first).is_none()
}

/// Whether Enter can submit `prompt`, just pasted into Codex: a live
/// composer row at or above the cursor that holds the start of it, or the
/// `[Pasted Content …]` chip Codex draws for a long paste. The footer is not looked at: with a draft in the box Codex
/// (0.170) draws its model and folder there, not `? for shortcuts`.
pub fn codex_draft_ready<T>(term: &Term<T>, prompt: &str) -> bool {
    use alacritty_terminal::term::TermMode;
    if !term
        .mode()
        .contains(TermMode::BRACKETED_PASTE | TermMode::SHOW_CURSOR)
    {
        return false;
    }
    let rows = bottom_rows(term, usize::MAX);
    let cursor = term.grid().cursor.point;
    let Ok(row) = usize::try_from(cursor.line.0) else {
        return false;
    };
    if row >= rows.len() {
        return false;
    }
    // The nearest `›` at or above the cursor: a menu's selected option,
    // when one has come up, and that never starts with the prompt. On the
    // cursor's own row only what is before the cursor counts: what follows
    // it is the empty box's placeholder.
    let Some(draft) = rows[..=row]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, line)| {
            let line: String = match at == row {
                true => line.chars().take(cursor.column.0).collect(),
                false => line.clone(),
            };
            line.trim_start().strip_prefix('›').map(str::to_owned)
        })
    else {
        return false;
    };
    // The start of its first line: later lines sit on rows of their own.
    let start: String = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(DRAFT_START)
        .collect();
    let draft = draft.split_whitespace().collect::<Vec<_>>().join(" ");
    // Images attached ahead of the prompt stand before it as chips.
    let mut draft = draft.as_str();
    while let Some((_, rest)) = draft
        .strip_prefix("[Image #")
        .and_then(|chip| chip.split_once(']'))
    {
        draft = rest.trim_start();
    }
    !start.is_empty() && (draft.starts_with(&start) || draft.starts_with("[Pasted Content"))
}

/// How much of a prompt's start has to be seen in Codex's composer: short
/// enough to sit on the first row of a narrow pane, long enough not to be
/// the placeholder.
const DRAFT_START: usize = 12;

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

    #[test]
    fn codex_input_requires_an_empty_live_composer_not_a_menu_or_draft() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::vte::ansi::Processor;

        let ready = "\x1b[?2004h\x1b[?25h\x1b[22;1H› Explain this codebase\x1b[24;1H? for shortcuts\x1b[22;3H";
        let check = |ansi: &str| {
            let mut term = Term::new(
                Default::default(),
                &crate::terminal::TerminalSize::new(80, 24),
                VoidListener,
            );
            let mut processor: Processor = Processor::new();
            processor.advance(&mut term, ansi.as_bytes());
            codex_prompt_ready(&term)
        };
        assert!(check(ready));
        assert!(check(
            &ready.replace("? for shortcuts", "100% context left")
        ));
        // As Codex 0.170 draws it once its hook review is answered.
        assert!(check(&ready.replace(
            "? for shortcuts",
            "  \u{2190} for agents \u{b7} ? for shortcuts"
        )));
        // With a warning at the row's far end, as Codex 0.170 drew it once
        // its hook review was answered without trusting.
        assert!(check(&ready.replace(
            "? for shortcuts",
            "  \u{2190} for agents \u{b7} ? for shortcuts     \u{26a0} 1 warning \u{b7} f2 to view"
        )));
        assert!(!check("shell prompt › "));
        assert!(!check(&ready.replace(
            "? for shortcuts",
            "Enter to confirm or Esc to cancel"
        )));
        assert!(!check(&format!("{ready}\x1b[?25l")));
        assert!(!check(&format!("{ready}\x1b[?2004l")));
        assert!(!check(&format!("{ready}Already typing")));
        // Startup pickers can leave the old footer visible while drawing.
        // Their selected › is a choice, never an empty composer, including
        // when a different update choice is selected.
        for option in [
            "1. Update now (runs `npm install -g @openai/codex`)",
            "2. Skip",
            "3. Skip until next version",
            "1. Trust and continue",
            "2. Back to Agent Command Center",
        ] {
            assert!(!check(&format!(
                "{ready}\x1b[18;1HUpdate available\x1b[22;1H\x1b[2K› {option}\x1b[22;3H"
            )));
        }
    }

    #[test]
    fn an_update_picker_between_paste_and_enter_refuses_submission() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::vte::ansi::Processor;
        let mut term = Term::new(
            Default::default(),
            &crate::terminal::TerminalSize::new(80, 24),
            VoidListener,
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, "\x1b[?2004h\x1b[?25h\x1b[22;1H› Explain this codebase\x1b[24;1H? for shortcuts\x1b[22;3H".as_bytes());
        assert!(codex_prompt_ready(&term));
        // The placeholder is not the draft, even for a task that reads as it.
        assert!(!codex_draft_ready(&term, "Build the app"));
        assert!(!codex_draft_ready(&term, "Explain this codebase"));
        // With a draft in the box Codex 0.170 draws its model and folder in
        // the footer, and wraps a long prompt under the composer's row.
        parser.advance(
            &mut term,
            "\x1b[22;1H\x1b[2K› Build the app and then\x1b[23;1H\x1b[2K  run it\x1b[24;1H\x1b[2K  GPT-6.2-Sol medium · ~/project\x1b[23;9H"
                .as_bytes(),
        );
        assert!(codex_draft_ready(&term, "Build the app  and then run it"));
        assert!(codex_draft_ready(
            &term,
            "\nBuild the app\n1. and then run it"
        ));
        assert!(!codex_draft_ready(&term, "Something else entirely"));
        parser.advance(
            &mut term,
            "\x1b[23;1H\x1b[2K\x1b[22;1H\x1b[2K› 1. Update now".as_bytes(),
        );
        assert!(!codex_prompt_ready(&term));
        assert!(!codex_draft_ready(&term, "Build the app and then run it"));
        // A task worded as the selected option is still not in the box.
        assert!(!codex_draft_ready(&term, "Update now"));
        parser.advance(&mut term, b"\x1b[2J\x1b[HUpdating and restarting");
        assert!(!codex_draft_ready(&term, "Build the app"));
    }

    fn rows(text: &str) -> Vec<String> {
        text.lines().map(|row| row.trim_end().to_owned()).collect()
    }

    const FOLDER_ACCESS: &str = "\
Folder access
/tmp/worktrees/new-task

Note: You're in a subdirectory of a Git
project. Trusting will apply to the
repository root:
/tmp/project

Trust this folder? Codex can read, edit,
and run files here. Your trust decision
will be saved.

› 1. Trust and continue
  2. Back to Agent Command Center

enter continue · esc back



";

    #[test]
    fn folder_access_above_empty_rows_keeps_both_paths_and_the_trust_explanation() {
        let dialog = codex_dialog(&rows(FOLDER_ACCESS)).expect("folder access");
        assert_eq!(dialog.title, "Folder access");
        assert!(dialog.question.contains("/tmp/worktrees/new-task"));
        assert!(dialog.question.contains("repository root:\n/tmp/project"));
        assert!(
            dialog
                .question
                .contains("Your trust decision\nwill be saved.")
        );
        assert_eq!(
            dialog.options,
            ["Trust and continue", "Back to Agent Command Center"]
        );
        assert_eq!(dialog.keys(0), b"1\r");
        assert_eq!(dialog.keys(1), b"2");
        // A direct CLI launch offers Quit rather than returning to the
        // Agent Command Center; it is the same folder trust question.
        let quit = FOLDER_ACCESS
            .replace("Back to Agent Command Center", "Quit")
            .replace("esc back", "esc quit");
        assert!(codex_dialog(&rows(&quit)).is_some());
    }

    /// As Codex CLI 0.170 drew it under a real pty, a new hook in the
    /// project's `.codex/hooks.json`.
    const HOOK_REVIEW_SCREEN: &str = "
  Hooks need review
  1 hook is new or changed.
  Hooks can run outside the sandbox after you trust them.


› 1. Review hooks
  2. Trust all and continue
  3. Continue without trusting (hooks won't run)

  enter confirm · esc skip




";

    #[test]
    fn hook_review_offers_trust_or_skip_but_not_codex_own_panel() {
        let dialog = codex_dialog(&rows(HOOK_REVIEW_SCREEN)).expect("hook review");
        assert_eq!(dialog.title, HOOK_REVIEW);
        assert_eq!(
            dialog.question,
            "1 hook is new or changed.\nHooks can run outside the sandbox after you trust them."
        );
        assert_eq!(
            dialog.options,
            [
                "Trust all and continue",
                "Continue without trusting (hooks won't run)"
            ]
        );
        // Codex's 2 moves the mark until Enter takes it; its 3 leaves at
        // once, and an Enter after it would land in the composer.
        assert_eq!(dialog.keys(0), b"2\r");
        assert_eq!(dialog.keys(1), b"3");
    }

    #[test]
    fn hook_review_is_only_the_live_question_with_its_own_choices() {
        // Answered, the composer's footer is the last row: not the question.
        let gone = format!("{HOOK_REVIEW_SCREEN}\n› Ask Codex to do anything\n\n  ? for shortcuts");
        assert!(codex_hook_review(&rows(&gone)).is_none());
        // Codex's hook panel, opened from Review hooks, is a different screen.
        let panel = "Hooks\nLifecycle hooks from config and enabled plugins.\n\n\
                     ⚠ 1 hook needs review before it can run.\n\n\
                     › PreToolUse 1/2 !\n\nt trust all · enter review · esc close";
        assert!(codex_hook_review(&rows(panel)).is_none());
        // Other choices than Codex's are not this question.
        let other = HOOK_REVIEW_SCREEN.replace("Review hooks", "Something else");
        assert!(codex_hook_review(&rows(&other)).is_none());
    }

    #[test]
    fn folder_access_requires_its_choices_and_live_footer() {
        let screen = rows(FOLDER_ACCESS);
        assert!(
            codex_dialog(&rows(
                &FOLDER_ACCESS.replace("enter continue · esc back", "")
            ))
            .is_none()
        );
        assert!(
            codex_dialog(&rows(
                &FOLDER_ACCESS.replace("Trust and continue", "Update now")
            ))
            .is_none()
        );
        let mut answered = screen;
        answered.extend(["› Explain this codebase", "? for shortcuts"].map(str::to_owned));
        assert!(codex_dialog(&answered).is_none());
    }

    #[test]
    fn old_dialog_titles_above_the_footer_do_not_cover_a_live_question_form() {
        for title in ["Implement this plan?", "Approaching rate limits"] {
            let mut screen = vec![title.to_owned()];
            screen.extend(vec![String::new(); DIALOG_ROWS]);
            screen.extend(rows(
                "Question 1/1 (1 unanswered)\nWhich crate?\n\n\
                 › 1. ket-core\n\x20 2. ket-ui\n\n\
                 tab to add notes | enter to submit answer | esc to interrupt",
            ));
            assert_eq!(codex_dialog(&screen), None);
        }
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

    #[test]
    fn opencodes_home_wordmark_is_home() {
        let full = rows(
            "\
                   █▀▀█ █▀▀█ █▀▀█ █▀▀▄  █▀▀▀ █▀▀█ █▀▀█ █▀▀█
                   █__█ █__█ █^^^ █__█  █___ █__█ █__█ █^^^
                   ▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀~~▀  ▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀

Ask anything, / for commands, @ for context",
        );
        assert!(opencode_home(&full));
        assert!(opencode_home(&rows("█_^█")));
        assert!(opencode_home(&rows("█▀▀█\n█__█")));
        assert!(!opencode_home(&rows(
            "Ask anything, / for commands, @ for context\n\n› hello"
        )));
        assert!(!opencode_home(&rows("")));
    }
}
