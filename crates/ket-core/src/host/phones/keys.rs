//! The keys that answer each agent's permission prompts, typed for a phone.

use ket_remote::proto;

use crate::host::server::AnswerRefused;

/// The keys that answer an agent's permission prompt with `choice`, or `None`
/// when ket does not know that agent's menu well enough to press them.
///
/// Claude Code draws a numbered menu and takes the number: `1` allows,
/// `2` allows and stops asking where the prompt offers that, and Escape
/// declines. A plan to approve or a question to answer is not a permission,
/// and its `1` means something else, so those are left to the terminal.
/// Codex: see [`codex_keys`]; OpenCode: [`opencode_keys`]; Grok:
/// [`grok_keys`]. Any other agent
/// is not answered until its menu is checked.
pub(super) fn answer_keys(
    agent: &str,
    tool: &str,
    choice: proto::answer::Choice,
) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    // A local agent is OpenCode with a model named after it; its menu is
    // OpenCode's.
    let agent = crate::agents::harness(agent);
    if agent.eq_ignore_ascii_case("codex") {
        return codex_keys(tool, choice);
    }
    if agent.eq_ignore_ascii_case("opencode") {
        return opencode_keys(choice);
    }
    if agent.eq_ignore_ascii_case("grok") {
        return grok_keys(tool, choice);
    }
    if !agent.eq_ignore_ascii_case("claude") || matches!(tool, "ExitPlanMode" | "AskUserQuestion") {
        return None;
    }
    match choice {
        Choice::AllowOnce => Some(b"1"),
        Choice::AllowAlways if offers_always(tool) => Some(b"2"),
        Choice::Deny => Some(&[0x1b]),
        _ => None,
    }
}

/// Codex's approval menus, as Codex CLI 0.159's TUI draws them with its
/// default keymap (`[tui.keymap.approval]` in `~/.codex/config.toml` can
/// rebind them). Each option takes a letter: `y` accepts,
/// `a` accepts for the rest of the session. Escape is the menu's cancel,
/// checked before any option, so it works whatever the menu offers; it
/// cancels the request and ends the turn, as Claude Code's Escape does.
///
/// Its hooks name the prompt `Bash` for a command (a network request
/// included), `apply_patch` for a file edit and `request_permissions` for
/// more sandbox access — each with a `y` option. `a` is pressed only for a
/// file edit, whose menu always has it; a command's menu offers it only
/// sometimes. `write_stdin` and MCP tools are left to the terminal.
fn codex_keys(tool: &str, choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    if !matches!(tool, "Bash" | "apply_patch" | "request_permissions") {
        return None;
    }
    match choice {
        Choice::AllowOnce => Some(b"y"),
        Choice::AllowAlways if tool == "apply_patch" => Some(b"a"),
        Choice::Deny => Some(&[0x1b]),
        _ => None,
    }
}

/// OpenCode's permission prompt, as its TUI draws it in 1.18.33
/// (`tui/src/routes/session/permission.tsx`): three buttons — Allow once,
/// Allow always, Reject — with Allow once selected when it opens. Enter
/// takes the selected one; Escape rejects, which ends the turn. The same
/// dialog for every tool.
///
/// No Allow always: it is a move right, then Enter, then Enter again on a
/// confirmation, and which button is selected can change under a mouse. Only
/// the main session's prompts reach here — ket's plugin leaves a subagent's
/// out, whose Reject opens a feedback box instead. Keybinds for this dialog
/// live in OpenCode's `tui.json`; Enter and the arrows cannot be rebound.
fn opencode_keys(choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    match choice {
        Choice::AllowOnce => Some(b"\r"),
        Choice::Deny => Some(&[0x1b]),
        _ => None,
    }
}

/// Which of Grok's prompts a phone can answer, and with what. Grok's menu
/// takes each option by its digit, with no Enter, but which digit is which
/// changes from prompt to prompt (see [`crate::screen::grok_permission_menu`]),
/// so nothing here is pressed: [`READ_OFF_MENU`] says the digit is found on
/// the screen when the answer is given, by [`answer_on_screen`].
///
/// Checked for a shell command, an edit, a new file and a fetch — Grok's
/// `run_terminal_command`, `search_replace`, `write` and `web_fetch`, which
/// ket's hook reader spells as Claude's names. Any other tool, an MCP tool
/// among them, is left to the terminal.
fn grok_keys(tool: &str, choice: proto::answer::Choice) -> Option<&'static [u8]> {
    use proto::answer::Choice;
    if !matches!(tool, "Bash" | "Edit" | "Write" | "WebFetch") {
        return None;
    }
    matches!(
        choice,
        Choice::AllowOnce | Choice::AllowAlways | Choice::Deny
    )
    .then_some(READ_OFF_MENU)
}

/// What [`answer_keys`] gives for an agent whose keys are read off its menu
/// at the moment of answering: no keys of its own, so pressing it presses
/// nothing.
pub(super) const READ_OFF_MENU: &[u8] = &[];

/// The keys that answer terminal `rows`' prompt with `choice`: Grok's read
/// off the menu it is showing, every other agent's from [`answer_keys`].
///
/// For Grok, the option for each choice is found by its label: Yes, proceed
/// (or Yes, allow once) to allow; the prompt's own Always allow — the
/// command, every edit this session, or the domain for this project — to
/// always allow; No to decline. Never always-approve mode, Always allow on
/// all sessions or Never allow, which change more than the one prompt.
/// Refused rather than guessed: no menu on screen is a prompt that has
/// moved on, and a menu without the option asked for — a chained command
/// has no Always allow — is said so.
pub(super) fn answer_on_screen(
    agent: &str,
    tool: &str,
    choice: proto::answer::Choice,
    rows: &dyn Fn() -> Vec<String>,
) -> Result<Vec<u8>, AnswerRefused> {
    let keys = answer_keys(agent, tool, choice).ok_or(AnswerRefused::Unknown)?;
    if !crate::agents::harness(agent).eq_ignore_ascii_case("grok") {
        return Ok(keys.to_vec());
    }
    let menu = crate::screen::grok_permission_menu(&rows()).ok_or(AnswerRefused::Changed)?;
    let digit = grok_pick(&menu, choice).ok_or(AnswerRefused::NotOffered)?;
    Ok(vec![b'0' + digit])
}

/// The digit of the option in Grok's `menu` that answers with `choice`.
fn grok_pick(menu: &[(u8, String)], choice: proto::answer::Choice) -> Option<u8> {
    use proto::answer::Choice;
    let fits = |label: &str| match choice {
        Choice::AllowOnce => {
            label.starts_with("Yes, proceed") || label.starts_with("Yes, allow once")
        }
        Choice::AllowAlways => {
            label.starts_with("Always allow:")
                || label.starts_with("Yes, allow all edits during this session")
                || (label.starts_with("Yes, always allow") && label.contains("for this project"))
        }
        Choice::Deny => label.starts_with("No, reject") || label.starts_with("No, and tell Grok"),
        _ => false,
    };
    let mut found = menu.iter().filter(|(_, label)| fits(label));
    let (digit, _) = found.next()?;
    // Two rows that both fit is a menu read wrong: press neither.
    found.next().is_none().then_some(*digit)
}

/// Whether Claude Code's prompt for `tool` has the three-line menu whose
/// second line stops asking. Every other prompt's `2` is not that.
fn offers_always(tool: &str) -> bool {
    matches!(
        tool,
        "Bash" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "WebFetch"
    )
}

/// Whether a keystroke asks the program to stop: Ctrl-C, or a bare Escape,
/// which is how Claude Code and Codex cancel a turn. The same test a window
/// makes on its own keystrokes.
pub(super) fn is_interrupt(bytes: &[u8]) -> bool {
    matches!(bytes, [0x03] | [0x1b])
}
