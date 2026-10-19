//! The keys that answer each agent's permission prompts, typed for a phone.

use ket_remote::proto;

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

/// Grok's permission menu, as Grok 1.0.44's TUI draws it: numbered options,
/// each taken by its digit with no Enter. `3` allows once and `4` rejects
/// and ends the turn. `2` always allows that command (a shell command), every
/// edit for the session (an edit or a new file), or that domain for the
/// project (a fetch). The menu can open on `1`, which turns off every prompt
/// for the session — never pressed. Escape does not decline: it moves focus
/// off the menu and leaves the prompt open.
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
    match choice {
        Choice::AllowOnce => Some(b"3"),
        Choice::AllowAlways => Some(b"2"),
        Choice::Deny => Some(b"4"),
        _ => None,
    }
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
