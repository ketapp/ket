//! The shell's global keyboard shortcuts — the chords that mean something
//! regardless of which pane, dialog, or menu is on screen.
//!
//! This is deliberately just a table. Deciding *whether* a chord reaches this
//! table at all — an open menu, a modal dialog, a focused text field all get
//! first refusal — stays in `main.rs`'s `on_key_down`, where the ordering is
//! visible as one chain. What lives here is only "which chord means which
//! shortcut," kept separate so that chain does not keep growing a new
//! `keystroke.key == "…"` check every time a shortcut is added.

use gpui::Keystroke;

use crate::tabs::Toward;

/// One global keyboard shortcut, independent of what it does once matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlobalShortcut {
    /// Cmd-Shift-P / Ctrl-Shift-P: open the quick prompt.
    OpenQuickPrompt,
    /// Cmd-/ / Ctrl-/: pick a snippet and send it to the selected
    /// worktree's agent.
    SendSnippet,
    /// Cmd-K / Ctrl-K: open the command palette.
    OpenPalette,
    /// Cmd-P / Ctrl-P: open the file finder.
    OpenFinder,
    /// Cmd-O / Ctrl-O: open the sidebar's search.
    OpenSearch,
    /// Cmd-Shift-F / Ctrl-Shift-F: search text across the worktree.
    OpenWorkspaceSearch,
    /// Cmd-Shift-G / Ctrl-Shift-G: show the right panel's Git view.
    OpenGitPanel,
    /// Cmd-| / Ctrl-|: toggle the sidebar.
    ToggleSidebar,
    /// Cmd-J / Ctrl-J: toggle the right panel.
    TogglePanel,
    /// Ctrl-`: open or focus the selected worktree's terminal.
    OpenTerminalTab,
    /// Cmd-Shift-[ / Cmd-Shift-]: cycle the focused pane's active tab.
    ///
    /// `-1` or `1`, the direction to step.
    CycleTab(isize),
    /// Ctrl-Alt-Arrow: move focus to the neighbouring pane.
    FocusPane(Toward),
    /// Cmd-Shift-M: commit the selected worktree's work and merge it.
    MergeWorktree,
    /// Cmd-Shift-B / Ctrl-Shift-B: a browser tab in the focused pane.
    NewBrowserTab,
    /// Cmd-Shift-N / Ctrl-Shift-N: the Create worktree dialog, for the
    /// project in context — the one the current terminal belongs to, else
    /// the first expanded project.
    NewWorktree,
    /// Cmd-Shift-A / Ctrl-Shift-A: a note for a project's backlog, taking
    /// the terminal's selection with it.
    CaptureToBacklog,
    /// Cmd-Alt-1…9 / Ctrl-Alt-1…9: a fresh session with the runnable agent
    /// at this zero-based position — the order the `+` menu lists them in.
    NewAgentSession(usize),
}

/// The chord that commits and merges the selected worktree.
///
/// Named rather than inlined so the settings pane can draw the same chord this
/// table matches, and cannot promise one the shell does not answer to. ⌘M is
/// the system's minimise, so this takes the shifted one.
#[cfg(target_os = "macos")]
pub(crate) const MERGE_WORKTREE_CHORD: &str = "cmd+shift+m";
#[cfg(not(target_os = "macos"))]
pub(crate) const MERGE_WORKTREE_CHORD: &str = "ctrl+shift+m";

/// The chord that opens a browser tab, drawn by the `+` menu's Browser row.
/// ⌘B is left alone for an editor's bold; the shifted one is free.
#[cfg(target_os = "macos")]
pub(crate) const NEW_BROWSER_CHORD: &str = "cmd+shift+b";
#[cfg(not(target_os = "macos"))]
pub(crate) const NEW_BROWSER_CHORD: &str = "ctrl+shift+b";

/// The chord that opens the Create worktree dialog, drawn by the project
/// menu's New Worktree row. ⌘N is the blank document, so the shifted one is
/// the worktree — the same shape as ⌘⇧B's browser tab. Not ⌘⇧W, which reads
/// as "close window" wherever it is pressed.
#[cfg(target_os = "macos")]
pub(crate) const NEW_WORKTREE_CHORD: &str = "cmd+shift+n";
#[cfg(not(target_os = "macos"))]
pub(crate) const NEW_WORKTREE_CHORD: &str = "ctrl+shift+n";

/// The chord that takes a note for the backlog from anywhere, as the
/// Keybindings pane draws it. ⌘A is select-all; the shifted one is free.
#[cfg(target_os = "macos")]
pub(crate) const CAPTURE_CHORD: &str = "cmd+shift+a";
#[cfg(not(target_os = "macos"))]
pub(crate) const CAPTURE_CHORD: &str = "ctrl+shift+a";

/// The same chord in gpui's keymap spelling, for the File menu's item.
/// macOS only: the item needs it as a key equivalent only where a native
/// view — a web page — can hold the keyboard.
#[cfg(target_os = "macos")]
pub(crate) const CAPTURE_BINDING: &str = "cmd-shift-a";

/// The same chord as the backlog's empty list names it, in a sentence.
#[cfg(target_os = "macos")]
pub(crate) const CAPTURE_KEYS: &str = "\u{21e7}\u{2318}A";
#[cfg(not(target_os = "macos"))]
pub(crate) const CAPTURE_KEYS: &str = "Ctrl+Shift+A";

/// The chord that opens the snippet picker, drawn by the Keybindings pane.
#[cfg(target_os = "macos")]
pub(crate) const SEND_SNIPPET_CHORD: &str = "cmd+/";
#[cfg(not(target_os = "macos"))]
pub(crate) const SEND_SNIPPET_CHORD: &str = "ctrl+/";

/// The chord that starts a session with the agent at `index` in the `+`
/// menu, for the first nine. Positional rather than one letter per provider,
/// so an agent enabled later gets a chord without a new rule here — and not
/// ⌘⇧digit, which macOS keeps for screenshots.
pub(crate) fn agent_session_chord(index: usize) -> Option<String> {
    let digit = index.checked_add(1).filter(|digit| *digit <= 9)?;
    #[cfg(target_os = "macos")]
    return Some(format!("cmd+alt+{digit}"));
    #[cfg(not(target_os = "macos"))]
    return Some(format!("ctrl+alt+{digit}"));
}

/// The sidebar toggle's chord, as a tooltip names it. ⌘| is matched too —
/// see [`global_shortcut`] — but the unshifted key is the one to show.
#[cfg(target_os = "macos")]
pub(crate) const SIDEBAR_KEYS: &str = "⌘\\";
#[cfg(not(target_os = "macos"))]
pub(crate) const SIDEBAR_KEYS: &str = "Ctrl+\\";

/// The right panel toggle's chord, as a tooltip names it.
#[cfg(target_os = "macos")]
pub(crate) const PANEL_KEYS: &str = "⌘J";
#[cfg(not(target_os = "macos"))]
pub(crate) const PANEL_KEYS: &str = "Ctrl+J";

#[cfg(target_os = "macos")]
pub(crate) const SEARCH_HINT: &str = "Search (⌘O)";
#[cfg(not(target_os = "macos"))]
pub(crate) const SEARCH_HINT: &str = "Search (Ctrl+O)";

#[cfg(target_os = "macos")]
pub(crate) const QUICK_PROMPT_HINT: &str = "Quick prompt (⌘⇧P)";
#[cfg(not(target_os = "macos"))]
pub(crate) const QUICK_PROMPT_HINT: &str = "Quick prompt (Ctrl+Shift+P)";

/// The key hints along the bottom of the window: the keys first, then what
/// they do. Only chords this table and the tab strip actually answer to.
#[cfg(target_os = "macos")]
pub(crate) const FOOTER_HINTS: &[(&str, &str)] = &[
    ("⌘O", "search"),
    ("⌘⇧P", "quick prompt"),
    ("⌘T", "new tab"),
    ("⌘⇧M", "merge"),
];
#[cfg(not(target_os = "macos"))]
pub(crate) const FOOTER_HINTS: &[(&str, &str)] = &[
    ("Ctrl+O", "search"),
    ("Ctrl+Shift+P", "quick prompt"),
    ("Ctrl+T", "new tab"),
    ("Ctrl+Shift+M", "merge"),
];

/// Matches a keystroke against the shell's global shortcuts.
///
/// Returns `None` for anything unrecognised — most keystrokes, including
/// every plain character a text field would want — so a caller can fall
/// through to whatever handles those instead.
pub(crate) fn global_shortcut(keystroke: &Keystroke) -> Option<GlobalShortcut> {
    let m = keystroke.modifiers;
    let cmd_or_ctrl = m.platform || m.control;

    // macOS reports Cmd-Shift-[ / Cmd-Shift-] by the shifted glyph rather
    // than the physical key, and folds the shift back out of `modifiers`
    // when it does — the same quirk documented in gpui's `parse_keystroke`
    // (cmd-shift-s arrives shifted but cmd-shift-[ arrives as cmd-{ with
    // `shift: false`) — so this checks the glyph alone, not `shift` plus
    // `[`.
    if cmd_or_ctrl && keystroke.key == "{" {
        return Some(GlobalShortcut::CycleTab(-1));
    }
    if cmd_or_ctrl && keystroke.key == "}" {
        return Some(GlobalShortcut::CycleTab(1));
    }

    // Cmd-Shift-M / Ctrl-Shift-M. Unlike `{`/`}`/`|` above, `m` is a letter,
    // so macOS leaves `shift` in the modifiers and it can be checked directly.
    if cmd_or_ctrl && m.shift && keystroke.key == "m" {
        return Some(GlobalShortcut::MergeWorktree);
    }
    if cmd_or_ctrl && m.shift && keystroke.key == "f" {
        return Some(GlobalShortcut::OpenWorkspaceSearch);
    }
    if cmd_or_ctrl && m.shift && keystroke.key == "b" {
        return Some(GlobalShortcut::NewBrowserTab);
    }
    if cmd_or_ctrl && m.shift && keystroke.key == "n" {
        return Some(GlobalShortcut::NewWorktree);
    }
    if cmd_or_ctrl && m.shift && keystroke.key == "a" {
        return Some(GlobalShortcut::CaptureToBacklog);
    }
    // Ahead of the Ctrl-Alt pane moves below, which claim every Ctrl-Alt
    // chord and answer `None` for the ones that are not arrows. macOS reports
    // the digit itself as the key while ⌘ is held, not the ⌥ glyph.
    if cmd_or_ctrl
        && m.alt
        && !m.shift
        && let Some(digit) = keystroke
            .key
            .parse::<usize>()
            .ok()
            .filter(|d| (1..=9).contains(d))
    {
        return Some(GlobalShortcut::NewAgentSession(digit - 1));
    }
    // Cmd-Shift-G: VS Code's chord for its source control view.
    if cmd_or_ctrl && m.shift && keystroke.key == "g" {
        return Some(GlobalShortcut::OpenGitPanel);
    }
    if cmd_or_ctrl && m.shift && keystroke.key == "p" {
        return Some(GlobalShortcut::OpenQuickPrompt);
    }

    // Cmd-/ / Ctrl-/: the chord the quick prompt already gives its snippet
    // menu, so one chord means snippets wherever it is pressed.
    if cmd_or_ctrl && keystroke.key == "/" {
        return Some(GlobalShortcut::SendSnippet);
    }
    if cmd_or_ctrl && keystroke.key == "k" {
        return Some(GlobalShortcut::OpenPalette);
    }
    if cmd_or_ctrl && !m.shift && keystroke.key == "p" {
        return Some(GlobalShortcut::OpenFinder);
    }
    // Cmd-O / Ctrl-O: the sidebar's search. Not ⌘K, which the command palette
    // has, and not ⌘J, which is the right panel — both were the obvious
    // choices and both are taken. "Open" is what this does to a worktree, a
    // tab or a settings page, and ⌘O is what every application on the
    // platform calls opening something.
    if cmd_or_ctrl && keystroke.key == "o" {
        return Some(GlobalShortcut::OpenSearch);
    }
    // Cmd-| / Ctrl-|: the VS Code chord for the same action. Observed in
    // practice reported as cmd-\ with `shift` folded to `false` — the same
    // glyph-over-physical-key quirk as `{`/`}` above, just landing on the
    // unshifted backslash instead of the shifted pipe — so both glyphs are
    // matched here.
    if cmd_or_ctrl && (keystroke.key == "|" || keystroke.key == "\\") {
        return Some(GlobalShortcut::ToggleSidebar);
    }
    // Cmd-J / Ctrl-J: the VS Code chord for toggling the bottom/right panel.
    if cmd_or_ctrl && keystroke.key == "j" {
        return Some(GlobalShortcut::TogglePanel);
    }
    // Ctrl-` opens or focuses the selected worktree's terminal — the same
    // chord VS Code and most editors use, so it costs nothing to discover.
    // Platform-only, like every other chord here, would also fire on the
    // bare backtick key on some layouts, so this one stays Ctrl-only to
    // match what it always has been.
    if keystroke.key == "`" && m.control {
        return Some(GlobalShortcut::OpenTerminalTab);
    }
    // Ctrl-Alt-Arrow moves between panes. Not Cmd-Alt-Arrow, which browsers
    // take for switching tabs and which ket has a browser pane of its own to
    // live beside; not a plain Alt-Arrow, which is word navigation in every
    // shell and editor there is. This pair is claimed by neither macOS nor
    // anything commonly run in a terminal, which is what a chord read ahead of
    // the panes has to be.
    if m.control && m.alt {
        let toward = match keystroke.key.as_str() {
            "left" => Toward::Left,
            "right" => Toward::Right,
            "up" => Toward::Up,
            "down" => Toward::Down,
            _ => return None,
        };
        return Some(GlobalShortcut::FocusPane(toward));
    }

    None
}
