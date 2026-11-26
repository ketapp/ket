//! What one agent is told when it takes over another's task.
//!
//! No agent can open another's session, so a task moves between them as
//! words: a [`Brief`] built from the conversation ket can read
//! ([`crate::conversation`]), typed into the new agent as its first message.
//! The working tree carries the rest — the changes are on disk — so every
//! brief starts by sending the new agent to `git status` and `git diff`.
//!
//! How much of the conversation goes is the person's choice, [`Depth`]:
//! the task alone, the task and the last few exchanges, or everything ket
//! can read. Whatever is chosen is shown to them before it is sent, since it
//! goes to another company's model.
//!
//! Two things shape the text beyond what it says:
//!
//! - **It is typed into a TUI.** A line starting `$ ` reads as a command to
//!   paste back into a shell, and `/`, `!` or `#` at the start of the first
//!   line is a command, a shell escape or a memory note to Claude Code. So
//!   tool lines are reworded ([`tool_said`]) and the brief opens with plain
//!   words, at least [`OPENING_CHARS`] of them: Codex's composer is only
//!   known to hold a draft once that much of its start is on screen.
//! - **It is read by a model, not a person.** Turns are quoted, oldest
//!   first, each with who said it, and the stop that ended the old agent's
//!   turn — its usage limit — is left out: the new agent has nothing to do
//!   about it.

use crate::conversation::{Turn, TurnKind};
use crate::limits::{Failure, classify};

/// How much of the conversation a brief carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    /// The task as it was first asked, and nothing since.
    Task,
    /// The task, and the last few things said either way.
    #[default]
    Recent,
    /// Everything ket can read of the conversation, to [`FULL_MAX`].
    Full,
}

/// Why the task is moving: what the brief says happened to the old agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// It ran out of usage.
    Limit,
    /// The person chose to move it.
    Chosen,
}

/// A brief, with what it costs to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    /// The words typed into the new agent.
    pub text: String,
    /// How many of the conversation's turns it quotes, the task included.
    pub turns: usize,
    /// About how many tokens it is, for the person to weigh: four
    /// characters to a token, the rule of thumb for English and code.
    pub approx_tokens: usize,
}

/// Fewest plain characters the brief opens with — see the module docs.
pub const OPENING_CHARS: usize = 12;

/// Most exchanges after the task that [`Depth::Recent`] quotes.
const RECENT_TURNS: usize = 6;

/// Most of one turn quoted in a [`Depth::Recent`] brief.
const RECENT_TURN_CHARS: usize = 700;

/// Most of one turn quoted in a [`Depth::Full`] brief.
const FULL_TURN_CHARS: usize = 2_000;

/// Most of a whole [`Depth::Full`] brief. Past it the oldest exchanges go,
/// never the task: a paste this size goes into a composer in one piece, and
/// is still a fraction of any model's window.
pub const FULL_MAX: usize = 24_000;

/// Most files a brief names as changed.
const FILES_MAX: usize = 12;

/// The brief for handing `turns` — `from`'s conversation, oldest first —
/// to another agent, at `depth`. `from` is the agent's name as ket knows
/// it, put in words with [`label`]; the brief speaks to the new agent as
/// "you", so it needs no name for it.
pub fn brief(from: &str, turns: &[Turn], depth: Depth, why: Why) -> Brief {
    let from_name = label(from);
    // The stop itself, in the old agent's words, is not the task: its last
    // word, when that is a limit, and the same refusal given to a retry.
    // Anything else it said about limits is the conversation.
    let stop = turns
        .iter()
        .rev()
        .find(|turn| turn.kind == TurnKind::Assistant && !turn.text.trim().is_empty())
        .map(|turn| turn.text.trim())
        .filter(|last| classify(from, &Failure::said(*last)).is_some());
    let said: Vec<&Turn> = turns
        .iter()
        .filter(|turn| match turn.kind {
            TurnKind::User => !turn.text.trim().is_empty(),
            TurnKind::Assistant => !turn.text.trim().is_empty() && Some(turn.text.trim()) != stop,
            TurnKind::Tool => false,
        })
        .collect();
    let task = said.iter().position(|turn| turn.kind == TurnKind::User);

    let happened = match why {
        Why::Limit => "ran out of usage",
        Why::Chosen => "is handing it over",
    };
    let mut text = format!(
        "{from_name} was working on a task in this worktree and {happened}, so you are taking it \
         over. Its changes so far are in the working tree: read `git status` and `git diff` first, \
         then carry on where it left off.",
    );
    let mut quoted = 0;
    if let Some(at) = task {
        text.push_str("\n\nThe task, as it was asked:\n");
        text.push_str(&quote(&said[at].text, FULL_TURN_CHARS));
        quoted += 1;
    }

    let since: Vec<&Turn> = task.map_or_else(Vec::new, |at| said[at + 1..].to_vec());
    let exchanges: Vec<String> = match depth {
        Depth::Task => Vec::new(),
        Depth::Recent => since
            .iter()
            .skip(since.len().saturating_sub(RECENT_TURNS))
            .map(|turn| exchange(turn, &from_name, RECENT_TURN_CHARS))
            .collect(),
        Depth::Full => since
            .iter()
            .map(|turn| exchange(turn, &from_name, FULL_TURN_CHARS))
            .collect(),
    };
    if !exchanges.is_empty() {
        let mut kept = exchanges;
        if depth == Depth::Full {
            // The oldest go first: the task stays, and so does the end.
            let room = FULL_MAX.saturating_sub(text.len() + 200);
            while kept.len() > 1 && kept.iter().map(|said| said.len() + 2).sum::<usize>() > room {
                kept.remove(0);
            }
        }
        let skipped = since.len() - kept.len();
        text.push_str(if skipped > 0 {
            "\n\nThe most recent of what was said since, oldest first:\n"
        } else {
            "\n\nWhat was said since, oldest first:\n"
        });
        quoted += kept.len();
        text.push_str(&kept.join("\n\n"));
    }

    let files = changed(turns);
    if !files.is_empty() && depth != Depth::Task {
        text.push_str(&format!(
            "\n\nFiles {from_name} changed: {}.",
            files.join(", ")
        ));
    }
    let approx_tokens = text.chars().count().div_ceil(4);
    Brief {
        text,
        turns: quoted,
        approx_tokens,
    }
}

/// The brief for `agent`'s session `session`, read from its transcript as
/// ket finds it. `Err` says, in words for the person, why there is none.
pub fn brief_of_session(
    agent: &str,
    session: &str,
    depth: Depth,
    why: Why,
) -> Result<Brief, String> {
    let transcript = crate::conversation::Transcript::locate(agent, session)
        .ok_or_else(|| format!("{}'s conversation isn't readable here", label(agent)))?;
    let mut turns = transcript
        .read(0)
        .map_err(|error| format!("the conversation couldn't be read: {error}"))?
        .turns;
    // That read keeps a long session's recent end: the task as it was first
    // asked comes from its start.
    if let Some(first) = transcript.first_ask() {
        let asked = turns
            .iter()
            .find(|turn| turn.kind == TurnKind::User && !turn.text.trim().is_empty());
        if asked.is_none_or(|asked| asked.text != first.text) {
            turns.insert(0, first);
        }
    }
    Ok(brief(agent, &turns, depth, why))
}

/// One exchange, quoted with who said it.
fn exchange(turn: &Turn, from_name: &str, most: usize) -> String {
    let who = match turn.kind {
        TurnKind::User => "The person",
        _ => from_name,
    };
    format!("{who}:\n{}", quote(&turn.text, most))
}

/// `text` cut to `most` characters, each line indented two spaces so no
/// line of it starts with anything a TUI reads as a command, and a tool
/// line reworded — see [`tool_said`].
fn quote(text: &str, most: usize) -> String {
    let text = text.trim();
    let cut: String = if text.chars().count() > most {
        let mut cut: String = text.chars().take(most.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        text.to_owned()
    };
    cut.lines()
        .map(|line| {
            let line = tool_said(line);
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A line as it reads in a brief: `$ cargo test` becomes `Ran: cargo
/// test`, so nothing in it reads as a command to run.
fn tool_said(line: &str) -> String {
    match line.trim_start().strip_prefix("$ ") {
        Some(command) => format!("Ran: {command}"),
        None => line.to_owned(),
    }
}

/// The files the conversation's tools changed, by name, first change first.
fn changed(turns: &[Turn]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for turn in turns.iter().filter(|turn| turn.kind == TurnKind::Tool) {
        let name = turn
            .text
            .strip_prefix("Edited ")
            .or_else(|| turn.text.strip_prefix("Wrote "));
        if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty())
            && !files.iter().any(|seen| seen == name)
        {
            files.push(name.to_owned());
        }
    }
    if files.len() > FILES_MAX {
        let more = files.len() - FILES_MAX;
        files.truncate(FILES_MAX);
        files.push(format!("and {more} more"));
    }
    files
}

/// An agent's name as a brief says it: `Claude`, `Codex`, or the model of a
/// local one, `qwen3-coder (OpenCode)`.
pub fn label(agent: &str) -> String {
    let name = agent.trim();
    if let Some(model) = name.strip_prefix("opencode:ollama/") {
        return format!("{model} (OpenCode)");
    }
    match crate::agents::harness(name).to_ascii_lowercase().as_str() {
        "claude" => "Claude".to_owned(),
        "codex" => "Codex".to_owned(),
        "opencode" => "OpenCode".to_owned(),
        "grok" => "Grok".to_owned(),
        "gemini" => "Gemini".to_owned(),
        _ => {
            let mut chars = name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_else(|| "The agent".to_owned())
        }
    }
}
