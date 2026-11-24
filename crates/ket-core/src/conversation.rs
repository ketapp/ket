//! An agent session's conversation — Claude Code's, Codex's, Grok's or
//! OpenCode's — read from where the agent keeps it, for a phone to show as a
//! conversation rather than as a terminal.
//!
//! What a person said, what the agent said back, and each tool it used as
//! one line — `$ cargo test`, `Edited main.rs`. Not its thinking, not tool
//! output, not a subagent's side conversation, and not the transcript's
//! bookkeeping records, which outnumber the conversation several times over.
//!
//! Read incrementally: a caller holds the cursor the last read stopped at,
//! and the next read starts there — a transcript runs to megabytes, and a
//! phone asks every couple of seconds while it is looking. For the three
//! agents that write a JSONL file the cursor is a byte offset — Grok's with
//! one flag above it, see [`grok`]; for OpenCode, which keeps a
//! SQLite database, it is a place in the session's messages.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::{KetError, Result};

mod claude;
mod codex;
mod grok;
mod opencode;

pub use claude::{
    BACKGROUNDED, LastStep, last_step as claude_last_step, queued, read,
    transcript as claude_transcript,
};
pub use codex::{read as read_codex, transcript as codex_transcript};
pub use grok::transcript as grok_transcript;
pub(crate) use opencode::id as opencode_id;

/// How much of a transcript a first read looks at: the recent end, not the
/// whole history.
const FIRST_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Most transcript bytes one incremental phone read may inspect. The phone
/// continues from `next` on its own next poll; one request never scans from
/// its cursor all the way to end-of-file, however far that is.
const INCREMENTAL_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Most turns a first read returns.
const FIRST_TURNS: usize = 200;

/// Longest text one turn carries; the rest is cut with an ellipsis.
const TURN_TEXT_MAX: usize = 8_000;

/// Who a turn is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// The person — typed at the desktop or sent from a phone.
    User,
    /// The agent's words.
    Assistant,
    /// A tool the agent used, as one line.
    Tool,
}

/// One entry in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Who.
    pub kind: TurnKind,
    /// What — for a tool, the line that says what it did.
    pub text: String,
    /// For a tool, its name: `Bash`, `Edit`.
    pub tool: String,
    /// For the person, how many images the message carried. The agent's
    /// marks for them in the text, "[Image #1]", are left out of it.
    pub images: u32,
}

/// What one read found.
#[derive(Debug, Clone, Default)]
pub struct Read_ {
    /// New turns, oldest first.
    pub turns: Vec<Turn>,
    /// Where the next read starts.
    pub next: u64,
    /// Whether this read started over — a first read, or a transcript that
    /// has shrunk under the offset it was given — so the caller drops what it
    /// had.
    pub fresh: bool,
}

/// An agent's session, wherever that agent keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transcript {
    /// A Claude Code transcript.
    Claude(PathBuf),
    /// A Codex rollout.
    Codex(PathBuf),
    /// A Grok `chat_history.jsonl`.
    Grok(PathBuf),
    /// A session in OpenCode's database.
    OpenCode {
        /// The database file.
        database: PathBuf,
        /// The session's id, `ses_…`.
        session: String,
    },
}

impl Transcript {
    /// Where `agent` keeps session `session`, or `None` for an agent ket
    /// cannot read, or a session that is not there yet.
    pub fn locate(agent: &str, session: &str) -> Option<Self> {
        match crate::agents::harness(agent).to_ascii_lowercase().as_str() {
            "claude" => claude::transcript(session).map(Self::Claude),
            "codex" => codex::transcript(session).map(Self::Codex),
            "grok" => grok::transcript(session).map(Self::Grok),
            "opencode" => {
                let database = crate::sessions::opencode_database();
                (opencode::id(session) && database.is_file()).then(|| Self::OpenCode {
                    database,
                    session: session.to_owned(),
                })
            }
            _ => None,
        }
    }

    /// Whether ket can read `agent`'s sessions at all.
    pub fn readable(agent: &str) -> bool {
        matches!(
            crate::agents::harness(agent).to_ascii_lowercase().as_str(),
            "claude" | "codex" | "grok" | "opencode"
        )
    }

    /// What the session has said since cursor `after` — `0` for a first
    /// read, which keeps the last [`FIRST_TURNS`] turns.
    pub fn read(&self, after: u64) -> Result<Read_> {
        match self {
            Self::Claude(path) => claude::read(path, after),
            Self::Codex(path) => codex::read(path, after),
            Self::Grok(path) => grok::read(path, after),
            Self::OpenCode { database, session } => opencode::read(database, session, after),
        }
    }

    /// Messages sent that the agent has not taken in yet. Only Claude records
    /// its queue; the others show a message once it is read.
    pub fn queued(&self) -> Vec<String> {
        match self {
            Self::Claude(path) => claude::queued(path).unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

/// Reads whole records from `after` on, each through `parse`.
fn read_with(
    path: &Path,
    after: u64,
    parse: fn(&serde_json::Value, &mut Vec<Turn>),
) -> Result<Read_> {
    let mut file = std::fs::File::open(path).map_err(|e| KetError::io(path, e))?;
    let length = file.metadata().map_err(|e| KetError::io(path, e))?.len();
    let fresh = after == 0 || after > length;
    let start = if fresh {
        length.saturating_sub(FIRST_READ_BYTES)
    } else {
        after
    };
    file.seek(SeekFrom::Start(start))
        .map_err(|e| KetError::io(path, e))?;
    let limit = if fresh {
        FIRST_READ_BYTES
    } else {
        INCREMENTAL_READ_BYTES
    };
    let mut bytes = Vec::new();
    file.take((length - start).min(limit))
        .read_to_end(&mut bytes)
        .map_err(|e| KetError::io(path, e))?;

    // Only whole lines; and a read that began mid-file skips the partial
    // line it began in.
    let end = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let begin = if fresh && start > 0 {
        bytes[..end]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(end, |at| at + 1)
    } else {
        0
    };
    let mut turns = Vec::new();
    for line in bytes[begin..end].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) {
            parse(&record, &mut turns);
        }
    }
    if fresh && turns.len() > FIRST_TURNS {
        turns.drain(..turns.len() - FIRST_TURNS);
    }
    // If a single record exceeds the whole budget, no newline was found at
    // all: advance past the examined fragment anyway, so a phone's repeated
    // polling cannot get stuck re-reading the same oversized record forever.
    // An ordinary record still being written, short of the limit, is left
    // for next time exactly as before.
    let next = if end == 0 && bytes.len() as u64 == limit {
        start + bytes.len() as u64
    } else {
        start + end as u64
    };
    Ok(Read_ { turns, next, fresh })
}

/// Gives the person's turn among `turns[from..]` — what one record added —
/// the `images` that record carried, or a turn of its own when it held
/// images and no words.
fn carried(turns: &mut Vec<Turn>, from: usize, images: u32) {
    if images == 0 {
        return;
    }
    match turns
        .get_mut(from..)
        .and_then(|added| added.iter_mut().find(|turn| turn.kind == TurnKind::User))
    {
        Some(turn) => turn.images += images,
        None => turns.push(Turn {
            kind: TurnKind::User,
            text: String::new(),
            tool: String::new(),
            images,
        }),
    }
}

/// Whether a content block is an image, by its type: Claude's `image`,
/// Codex's `local_image` and `input_image`.
fn is_image(block: &serde_json::Value) -> bool {
    block
        .get("type")
        .and_then(|t| t.as_str())
        .is_some_and(|kind| kind == "image" || kind.ends_with("_image"))
}

/// `text` without the marks an agent writes into a message for each image
/// in it — Claude Code's and Codex's "[Image #1]", OpenCode's "[Image 1]" —
/// which [`Turn::images`] counts instead, and the space after each.
fn without_image_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("[Image ") {
        let after = &rest[at + "[Image ".len()..];
        let number = after.strip_prefix('#').unwrap_or(after);
        let digits = number.bytes().take_while(u8::is_ascii_digit).count();
        match number[digits..].strip_prefix(']') {
            Some(tail) if digits > 0 => {
                out.push_str(&rest[..at]);
                rest = tail.strip_prefix(' ').unwrap_or(tail);
            }
            _ => {
                out.push_str(&rest[..at + 1]);
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What Claude Code writes around each paste in a message:
/// `<pasted_content id="8e80">` on the line before it, and
/// `</pasted_content id="8e80">` on the line after.
const PASTE_MARK: &str = "pasted_content id=\"";

/// `text` with Claude Code's marks around each paste taken out — see
/// [`PASTE_MARK`] — leaving what was pasted, as the message read in the
/// prompt. Left in, a message that starts with a paste reads as the
/// harness's own and isn't shown at all.
fn without_paste_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(PASTE_MARK) {
        let past = at + PASTE_MARK.len();
        let id = rest[past..]
            .bytes()
            .take_while(u8::is_ascii_alphanumeric)
            .count();
        let tail = rest[past + id..].strip_prefix("\">").filter(|_| id > 0);
        let before = &rest[..at];
        match (tail, before.strip_suffix("</"), before.strip_suffix('<')) {
            // A paste's end, with the new line before it.
            (Some(tail), Some(before), _) => {
                out.push_str(before.strip_suffix('\n').unwrap_or(before));
                rest = tail;
            }
            // Its start, with the new line after it.
            (Some(tail), None, Some(before)) => {
                out.push_str(before);
                rest = tail.strip_prefix('\n').unwrap_or(tail);
            }
            _ => {
                out.push_str(&rest[..past]);
                rest = &rest[past..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A turn of text, unless it is the harness talking — a reminder, a caveat,
/// anything else that starts with a tag — or its note that a turn was
/// interrupted. Some of the harness's records are the person's after all: a
/// slash command, which Claude files as its `<command-name>` wrapper, and
/// what a local command printed, which reads as one line under it; and a
/// `!` command, which Claude files as `<bash-input>`, with what it printed as
/// a `<bash-stdout>` record of its own.
fn push_text(turns: &mut Vec<Turn>, kind: TurnKind, text: &str) {
    let unmarked;
    let marked = text.contains("[Image ") || text.contains(PASTE_MARK);
    let text = if kind == TurnKind::User && marked {
        unmarked = without_paste_marks(&without_image_marks(text));
        unmarked.trim()
    } else {
        text.trim()
    };
    if kind == TurnKind::User {
        if let Some(command) = slash_command(text) {
            turns.push(Turn {
                kind: TurnKind::User,
                text: clip(&command),
                tool: String::new(),
                images: 0,
            });
            return;
        }
        // Read back with its `!`, as it was typed: that is the text a phone
        // that sent it is waiting to see before it calls it delivered.
        if text.starts_with("<bash-input>")
            && let Some(command) = tagged(text, "bash-input")
        {
            turns.push(Turn {
                kind: TurnKind::User,
                text: clip(&format!("!{command}")),
                tool: String::new(),
                images: 0,
            });
            return;
        }
        if text.starts_with("<bash-stdout>") || text.starts_with("<bash-stderr>") {
            let printed: Vec<String> = ["bash-stdout", "bash-stderr"]
                .into_iter()
                .filter_map(|tag| tagged(text, tag))
                .map(|stream| as_shown(&unescape(&strip_ansi(stream))))
                .filter(|stream| !stream.is_empty())
                .collect();
            if !printed.is_empty() {
                turns.push(Turn {
                    kind: TurnKind::Tool,
                    text: clip(&printed.join("\n")),
                    tool: "Bash".to_owned(),
                    images: 0,
                });
            }
            return;
        }
        if let Some(printed) = tagged(text, "local-command-stdout") {
            let line = strip_ansi(printed);
            if let Some(line) = line.lines().map(str::trim).find(|line| !line.is_empty()) {
                turns.push(Turn {
                    kind: TurnKind::Tool,
                    text: clip(line),
                    tool: "Command".to_owned(),
                    images: 0,
                });
            }
            return;
        }
    }
    if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
        return;
    }
    // One reply arrives as several records; its words read as one turn.
    if kind == TurnKind::Assistant
        && let Some(last) = turns.last_mut()
        && last.kind == TurnKind::Assistant
    {
        last.text.push_str("\n\n");
        last.text.push_str(&clip(text));
        return;
    }
    turns.push(Turn {
        kind,
        text: clip(text),
        tool: String::new(),
        images: 0,
    });
}

/// `/compact` or `/review 42`, from the wrapper Claude records a slash
/// command as: `<command-name>/review</command-name>` with its
/// `<command-args>` beside it.
fn slash_command(text: &str) -> Option<String> {
    if !text.starts_with("<command-name>") {
        return None;
    }
    let name = tagged(text, "command-name")?.trim();
    if name.is_empty() {
        return None;
    }
    let slash = if name.starts_with('/') { "" } else { "/" };
    let args = tagged(text, "command-args").map_or("", str::trim);
    Some(match args {
        "" => format!("{slash}{name}"),
        args => format!("{slash}{name} {args}"),
    })
}

/// What sits between `<tag>` and `</tag>` in `text`.
pub(crate) fn tagged<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

/// `text` without its terminal colour codes: a local command prints for the
/// terminal, and its escapes are noise anywhere else.
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI: `ESC [`, parameters, then one final byte in `@`..`~`.
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// What a `!` command printed, with the `&lt;`, `&gt;` and `&amp;` Claude
/// writes into its output record turned back into the characters printed.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Output as a terminal would have left it: a line a progress bar redrew
/// with `\r` reads as its last drawing, and blank lines at either end go.
fn as_shown(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            line.rsplit('\r')
                .find(|part| !part.is_empty())
                .unwrap_or("")
                .trim_end()
        })
        .collect();
    lines.join("\n").trim_matches('\n').trim_end().to_owned()
}

fn clip(text: &str) -> String {
    match text.char_indices().nth(TURN_TEXT_MAX) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// One line for a tool the agent used: what it ran, read or changed.
fn tool_line(name: &str, input: Option<&serde_json::Value>) -> String {
    let field = |key: &str| {
        input
            .and_then(|i| i.get(key))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let leaf =
        |key: &str| field(key).map(|path| path.rsplit('/').next().unwrap_or(path).to_owned());
    let first_line = |text: &str| {
        let line = text.lines().next().unwrap_or(text);
        match line.char_indices().nth(160) {
            Some((at, _)) => format!("{}…", &line[..at]),
            None => line.to_owned(),
        }
    };
    let said = match name {
        "Bash" => field("command").map(|c| format!("$ {}", first_line(c))),
        "Read" => leaf("file_path").map(|f| format!("Read {f}")),
        "Edit" | "MultiEdit" => leaf("file_path").map(|f| format!("Edited {f}")),
        "Write" => leaf("file_path").map(|f| format!("Wrote {f}")),
        "NotebookEdit" => leaf("notebook_path").map(|f| format!("Edited {f}")),
        "Grep" => field("pattern").map(|p| format!("Searched for {}", first_line(p))),
        "Glob" => field("pattern").map(|p| format!("Listed {p}")),
        "WebFetch" => field("url").map(|u| format!("Fetched {u}")),
        "WebSearch" => field("query").map(|q| format!("Searched the web for {}", first_line(q))),
        "Task" | "Agent" => {
            field("description").map(|d| format!("Started a subagent: {}", first_line(d)))
        }
        "TodoWrite" => Some("Updated its plan".to_owned()),
        _ => None,
    };
    said.unwrap_or_else(|| name.to_owned())
}

/// What a tool says to the person, when the tool *is* the message: the
/// questions it asks with their options, or the plan it wants approved. The
/// agent stops on these and waits, so a one-line summary left the phone
/// showing nothing of what it was being asked.
fn spoken(name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    fn text<'a>(value: Option<&'a serde_json::Value>, key: &str) -> Option<&'a str> {
        value
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
    match name {
        "AskUserQuestion" => {
            let questions = input?.get("questions")?.as_array()?;
            let asked: Vec<String> = questions
                .iter()
                .filter_map(|question| {
                    let mut said = text(Some(question), "question")?.to_owned();
                    if question.get("multiSelect").and_then(|m| m.as_bool()) == Some(true) {
                        said.push_str(" (pick any)");
                    }
                    for option in question
                        .get("options")
                        .and_then(|o| o.as_array())
                        .into_iter()
                        .flatten()
                    {
                        let Some(label) = text(Some(option), "label") else {
                            continue;
                        };
                        said.push_str("\n• ");
                        said.push_str(label);
                        if let Some(description) = text(Some(option), "description") {
                            said.push_str(" — ");
                            said.push_str(description);
                        }
                    }
                    Some(said)
                })
                .collect();
            (!asked.is_empty()).then(|| asked.join("\n\n"))
        }
        "ExitPlanMode" => text(input, "plan").map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod local_agent_tests {
    use super::*;

    #[test]
    fn a_local_agents_transcript_is_read_as_opencodes() {
        assert!(Transcript::readable("opencode:ollama/qwen3-coder:30b"));
        assert!(Transcript::readable("opencode"));
        assert!(!Transcript::readable("ollama:qwen3:0.6b"));
    }
}
