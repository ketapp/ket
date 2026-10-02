//! An agent, drawn: whose it is, and what it is doing.
//!
//! Two pieces the sidebar's selected worktree needs and nothing else in the
//! chrome had. Both are about a row you are *looking at* rather than one you
//! are scanning past, which is why they can afford a logo and a moving
//! gradient where the rest of the tree gets a six-pixel dot.
//!
//! The provider marks are a fixed table for the same reason
//! [`super::filetype`]'s colours are: a vendor's colour identifies the vendor
//! and does not change meaning when the theme does. Four more tokens in
//! `Theme` for something no theme would sensibly override is the worse trade.

use gpui::{Pixels, Rgba, px};
use ket_core::theme::Theme;

use super::icon::{Icon, sized_icon};
use crate::paint::{on_ground, paint};

/// How big a provider's mark is drawn beside its status line.
///
/// A shade under the chrome's [`super::ICON`]: these are filled silhouettes
/// among hairline strokes, and a filled mark at the same nominal size reads
/// heavier than everything around it.
pub(crate) const MARK: Pixels = px(13.0);

/// Anthropic's terracotta.
const CLAUDE: Rgba = Rgba {
    r: 0.851,
    g: 0.467,
    b: 0.341,
    a: 1.0,
};

/// OpenAI's grey.
///
/// The green this replaced was never OpenAI's: their mark is monochrome, and
/// OpenAI draws it near-black on a light ground. ket
/// cannot use near-black, because the same fixed value has to sit on both an
/// `#ffffff` card and a `#1d2126` one — so this is the grey from OpenAI's own
/// palette, which clears 3:1 against both and reads as deliberate rather than
/// as a mark that has been dimmed.
const CODEX: Rgba = Rgba {
    r: 0.431,
    g: 0.431,
    b: 0.502,
    a: 1.0,
};

/// OpenCode's amber.
const OPENCODE: Rgba = Rgba {
    r: 0.910,
    g: 0.702,
    b: 0.224,
    a: 1.0,
};

/// Gemini's violet.
const GEMINI: Rgba = Rgba {
    r: 0.557,
    g: 0.482,
    b: 0.961,
    a: 1.0,
};

/// Not a vendor's own colour — Ollama has none to speak of — but a cool
/// slate distinct from every cloud CLI's mark and from the dim fallback a
/// truly unknown agent gets, so a local model reads as "this machine" at a
/// glance.
const OLLAMA: Rgba = Rgba {
    r: 0.549,
    g: 0.620,
    b: 0.690,
    a: 1.0,
};

/// A human label for a provider's raw agent name.
///
/// The catalogue's name is what a shell runs (`claude`, `codex`, `opencode`);
/// this is what a person reads in a dialog or menu.
pub(crate) fn agent_label(name: &str) -> String {
    match name {
        "claude" => "Claude Code".to_owned(),
        "codex" => "Codex".to_owned(),
        "grok" => "Grok".to_owned(),
        "opencode" => "OpenCode".to_owned(),
        // The bare model tag, e.g. `qwen2.5-coder:7b` — the same spelling
        // `ollama list` already trained the person to recognise, rather
        // than a friendly-name table ket would have to keep up with every
        // model anyone pulls.
        other if other.starts_with("ollama:") => other["ollama:".len()..].to_owned(),
        other => other.to_owned(),
    }
}

/// The mark and colour an agent is drawn in.
///
/// Matched on the name loosely — an agent is `claude`, `claude-code` or
/// `@anthropic-ai/claude-code` depending on who is naming it, and a row that
/// falls back to a terminal glyph because the catalogue spelled it the second
/// way is a bug nobody would think to look for.
pub(crate) fn provider(agent: &str, t: &Theme) -> (Icon, Rgba) {
    let name = agent.to_ascii_lowercase();
    let is = |wanted: &str| name == wanted || name.contains(wanted);

    if is("claude") {
        (Icon::Claude, on_ground(CLAUDE, t))
    } else if is("codex") {
        // Chosen to clear 3:1 on a light card and a dark one as it is.
        (Icon::Codex, CODEX)
    } else if is("opencode") {
        (Icon::OpenCode, on_ground(OPENCODE, t))
    } else if is("gemini") {
        (Icon::Gemini, on_ground(GEMINI, t))
    } else if is("grok") {
        // xAI's mark is white on black and has no colour of its own, so the
        // silhouette does the distinguishing: the theme's own text colour,
        // which is white wherever white would read and dark where it would not.
        (Icon::Grok, paint(t.text.primary))
    } else if is("ollama") {
        // No logo of its own yet — a terminal glyph is still the honest
        // answer, since this *is* a local process in a shell. The tint is
        // what tells it apart from "ket has no mark for this at all".
        (Icon::Terminal, on_ground(OLLAMA, t))
    } else {
        // Something is running that ket has no mark for — a build, a test run,
        // an agent added since. A terminal glyph is the honest answer: it says
        // "a program in your shell", which is exactly what is known.
        (Icon::Terminal, paint(t.text.dim))
    }
}

/// A provider's mark at the size a status line wants it.
pub(crate) fn mark(agent: &str, t: &Theme) -> gpui::Svg {
    let (which, tint) = provider(agent, t);
    sized_icon(which, MARK, tint)
}
