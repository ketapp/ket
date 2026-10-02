//! A project's own agent command, drawn.
//!
//! A project can launch an agent with a command of its own — `claude-work`
//! where every other project runs `claude-personal` (see
//! [`ket_core::project::ProjectSettings::agents`]). Getting that wrong is a
//! session on the wrong account, so everything the command reaches wears one
//! tint: the project's group in the sidebar, its marks, the header, the tab
//! and pane of a session it launched, the agent picker and the agent's card
//! in Settings. One hue, so a glance anywhere says "this is not the default".

use gpui::{Rgba, SharedString};
use ket_core::config::AgentLaunch;
use ket_core::theme::Theme;

use crate::paint::paint;

/// The tint.
///
/// The theme's own terminal cyan rather than a new chrome token: no other
/// state in the chrome spends it, and every theme already picks one that
/// reads on its own ground — `#3fd2e0` on the dark desk, a deep teal on the
/// light one.
pub(crate) fn hue(t: &Theme) -> Rgba {
    paint(t.terminal.ansi.cyan)
}

/// What a strip has room for: the command's first word, `claude-work`.
///
/// The word is the part that names the account or wrapper; its arguments are
/// in Project Settings and on the agent's card, where there is room to read
/// them.
pub(crate) fn short(launch: &AgentLaunch) -> SharedString {
    launch
        .command
        .split_whitespace()
        .next()
        .unwrap_or(&launch.command)
        .to_owned()
        .into()
}

/// The whole line, the way the shell is given it.
pub(crate) fn line(launch: &AgentLaunch) -> SharedString {
    ket_core::shell::line(&launch.command, &launch.args).into()
}
