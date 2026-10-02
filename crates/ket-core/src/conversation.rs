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
//! agents that write a JSONL file the cursor is a byte offset; for OpenCode,
//! which keeps a SQLite database, it is a place in the session's messages.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::{KetError, Result};

/// How much of a transcript a first read looks at: the recent end, not the
/// whole history.
const FIRST_READ_BYTES: u64 = 2 * 1024 * 1024;

/// How much of a transcript's end is searched for messages still queued.
const QUEUE_READ_BYTES: u64 = 1024 * 1024;

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

/// The transcript of Claude Code session `session`, wherever Claude keeps
/// its projects — the one file named after it.
pub fn claude_transcript(session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    let file = format!("{session}.jsonl");
    // Every config directory, not just the default one: a project whose own
    // `claude` command points somewhere else files its transcripts there.
    crate::sessions::claude_config_dirs()
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir.join("projects")).ok())
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// The rollout of Codex session `session`: the one file in Codex's dated
/// sessions store whose name ends in its id. Remembered once found — the
/// store is a directory per day, and a phone asks every couple of seconds.
pub fn codex_transcript(session: &str) -> Option<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, PoisonError};

    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    static FOUND: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let found = FOUND.get_or_init(Mutex::default);
    if let Some(path) = found
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(session)
        .filter(|path| path.is_file())
    {
        return Some(path.clone());
    }

    // `<root>/<year>/<month>/<day>/rollout-<when>-<id>.jsonl`, newest first:
    // a session being looked at is almost always a recent one.
    let suffix = format!("-{session}.jsonl");
    let newest_first = |dir: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .collect();
        entries.sort_unstable_by(|a, b| b.cmp(a));
        entries
    };
    let root = crate::sessions::codex_sessions_dir();
    let path = newest_first(&root)
        .iter()
        .flat_map(|year| newest_first(year))
        .flat_map(|month| newest_first(&month))
        .flat_map(|day| newest_first(&day))
        .find(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&suffix))
        })?;
    found
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(session.to_owned(), path.clone());
    Some(path)
}

/// The transcript of Grok session `session`: `chat_history.jsonl` in its own
/// directory, under whichever cwd's directory holds it — see
/// [`crate::sessions::GrokSessions`] for the layout. Only the cwd directories
/// are listed, never the sessions directory's parent, which holds Grok's
/// credentials.
pub fn grok_transcript(session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    let root = crate::sessions::grok_sessions_dir();
    std::fs::read_dir(&root)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|cwd| cwd.path().join(session).join("chat_history.jsonl"))
        .find(|path| path.is_file())
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
        match agent.to_ascii_lowercase().as_str() {
            "claude" => claude_transcript(session).map(Self::Claude),
            "codex" => codex_transcript(session).map(Self::Codex),
            "grok" => grok_transcript(session).map(Self::Grok),
            "opencode" => {
                let database = crate::sessions::opencode_database();
                (opencode_id(session) && database.is_file()).then(|| Self::OpenCode {
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
            agent.to_ascii_lowercase().as_str(),
            "claude" | "codex" | "grok" | "opencode"
        )
    }

    /// What the session has said since cursor `after` — `0` for a first
    /// read, which keeps the last [`FIRST_TURNS`] turns.
    pub fn read(&self, after: u64) -> Result<Read_> {
        match self {
            Self::Claude(path) => read(path, after),
            Self::Codex(path) => read_codex(path, after),
            Self::Grok(path) => read_with(path, after, grok_turns_of),
            Self::OpenCode { database, session } => read_opencode(database, session, after),
        }
    }

    /// Messages sent that the agent has not taken in yet. Only Claude records
    /// its queue; the others show a message once it is read.
    pub fn queued(&self) -> Vec<String> {
        match self {
            Self::Claude(path) => queued(path).unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

/// Whether `session` looks like an OpenCode session id — `ses_` and letters
/// and digits. Checked before it goes into a statement.
fn opencode_id(session: &str) -> bool {
    session.len() > 4
        && session.starts_with("ses_")
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// How many of a session's newest messages a first OpenCode read looks at:
/// enough for [`FIRST_TURNS`] turns, since most steps are a tool or two.
const OPENCODE_FIRST_MESSAGES: usize = 400;

/// Most rows (message/part pairs, so several per message) one incremental
/// read may ask `sqlite3` for. The fresh path above is already bounded by
/// its own `OFFSET`; a read from an arbitrary cursor has nothing else
/// capping how far back it could start, so this is that cap.
const INCREMENTAL_OPENCODE_ROWS: u64 = 2_000;

/// Reads OpenCode session `session` from cursor `after`.
///
/// Through the `sqlite3` binary, one read-only statement naming columns of
/// `message` and `part` only — the way [`crate::sessions::OpencodeSessions`]
/// reads the same database, which also holds OpenCode's credentials.
///
/// The cursor is a message's creation time in milliseconds times 1000, plus
/// how many messages at that same millisecond were already read: creation
/// times never change, where a row's `rowid` might if OpenCode rewrote it,
/// and two messages created in one millisecond are still read once each.
///
/// Only finished messages are read, and the read stops at the first one that
/// is not — an assistant step still streaming, or the person's message before
/// its words are written — so a turn is never sent half-said and then again.
/// A message with any after it is finished whatever it says: OpenCode works
/// one step at a time, and a step it abandoned may never be marked complete.
fn read_opencode(database: &Path, session: &str, after: u64) -> Result<Read_> {
    let fresh = after == 0;
    let (since, skip) = (after / 1000, after % 1000);
    let from = match fresh {
        true => format!(
            "COALESCE((SELECT time_created FROM message WHERE session_id = '{session}' \
             ORDER BY time_created DESC, id DESC LIMIT 1 OFFSET {}), 0)",
            OPENCODE_FIRST_MESSAGES - 1
        ),
        false => since.to_string(),
    };
    let output = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg("-json")
        .arg(database)
        .arg(format!(
            "SELECT m.time_created AS at, m.data AS message, p.data AS part \
             FROM message m LEFT JOIN part p ON p.message_id = m.id \
             WHERE m.session_id = '{session}' AND m.time_created >= {from} \
             ORDER BY m.time_created, m.id, p.id {};",
            if fresh {
                String::new()
            } else {
                format!("LIMIT {INCREMENTAL_OPENCODE_ROWS}")
            }
        ))
        .output()
        .map_err(|e| KetError::io(database, e))?;
    if !output.status.success() {
        return Err(KetError::Agent {
            agent: "opencode".to_owned(),
            why: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    // No rows prints nothing at all rather than `[]`.
    let rows: Vec<serde_json::Value> = match output.stdout.iter().all(u8::is_ascii_whitespace) {
        true => Vec::new(),
        false => serde_json::from_slice(&output.stdout).map_err(|e| KetError::Agent {
            agent: "opencode".to_owned(),
            why: e.to_string(),
        })?,
    };

    // Rows are a message's columns once per part; one entry per message.
    let mut messages: Vec<(u64, serde_json::Value, Vec<serde_json::Value>)> = Vec::new();
    let json = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(|v| v.as_str())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
    };
    for row in &rows {
        let at = row
            .get("at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let Some(message) = json(row, "message") else {
            continue;
        };
        let part = json(row, "part");
        let same = messages.last().is_some_and(|(last_at, last, _)| {
            *last_at == at && last.get("id") == message.get("id")
        });
        if !same {
            messages.push((at, message, Vec::new()));
        }
        if let (Some(part), Some((_, _, parts))) = (part, messages.last_mut()) {
            parts.push(part);
        }
    }

    let (mut next_at, mut next_skip) = (since, skip);
    let mut skipped = 0;
    let mut turns = Vec::new();
    let count = messages.len();
    for (index, (at, message, parts)) in messages.iter().enumerate() {
        if !fresh && *at == since && skipped < skip {
            skipped += 1;
            continue;
        }
        let last = index + 1 == count;
        let finished = match message.get("role").and_then(|r| r.as_str()) {
            Some("user") => !parts.is_empty(),
            _ => message
                .get("time")
                .and_then(|t| t.get("completed"))
                .is_some_and(|c| !c.is_null()),
        };
        if last && !finished {
            break;
        }
        opencode_turns_of(message, parts, &mut turns);
        if *at == next_at {
            next_skip += 1;
        } else {
            (next_at, next_skip) = (*at, 1);
        }
    }
    if fresh && turns.len() > FIRST_TURNS {
        turns.drain(..turns.len() - FIRST_TURNS);
    }
    Ok(Read_ {
        turns,
        next: match next_at {
            0 => 0,
            at => at * 1000 + next_skip.min(999),
        },
        fresh,
    })
}

/// Reads Claude transcript `path` from byte `after` — `0` for a first read,
/// which looks at the recent end only and keeps the last [`FIRST_TURNS`]
/// turns. Stops at the last whole line, so a record still being written is
/// read next time.
pub fn read(path: &Path, after: u64) -> Result<Read_> {
    read_with(path, after, turns_of)
}

/// [`read`], for a Codex rollout.
pub fn read_codex(path: &Path, after: u64) -> Result<Read_> {
    read_with(path, after, codex_turns_of)
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

/// Messages the person has sent that the agent has not read yet, oldest
/// first: typed while it was working, they wait in Claude's queue until it
/// takes them in mid-turn or starts its next turn with them. Claude records
/// each step — `enqueue` with the text, `dequeue` taking the oldest,
/// `remove` taking one by its text — and what is left is still waiting.
/// Only the transcript's recent end is searched; a message queued before it
/// has long since been read.
pub fn queued(path: &Path) -> Result<Vec<String>> {
    let mut file = std::fs::File::open(path).map_err(|e| KetError::io(path, e))?;
    let length = file.metadata().map_err(|e| KetError::io(path, e))?.len();
    let start = length.saturating_sub(QUEUE_READ_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| KetError::io(path, e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| KetError::io(path, e))?;

    let marker = b"\"queue-operation\"";
    let mut waiting: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for line in bytes.split(|&b| b == b'\n') {
        if !line.windows(marker.len()).any(|w| w == marker) {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let content = record.get("content").and_then(|c| c.as_str());
        match record.get("operation").and_then(|o| o.as_str()) {
            Some("enqueue") => waiting.push_back(content.unwrap_or_default().to_owned()),
            Some("dequeue") => {
                waiting.pop_front();
            }
            Some("remove") => {
                if let Some(at) = waiting.iter().position(|w| Some(w.as_str()) == content) {
                    waiting.remove(at);
                }
            }
            _ => {}
        }
    }
    // Task notifications and the like queue too; only what a person typed.
    Ok(waiting
        .into_iter()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty() && !text.starts_with('<'))
        .map(|text| clip(&text))
        .collect())
}

/// The turns one transcript record holds, if it holds any.
fn turns_of(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    let kind = match record.get("type").and_then(|t| t.as_str()) {
        Some("user") => TurnKind::User,
        Some("assistant") => TurnKind::Assistant,
        Some("attachment") => return absorbed(record, turns),
        // What a local command printed, and sometimes the command itself,
        // filed as the harness's own note: read as the person's, where
        // `push_text` keeps only those two.
        Some("system") => {
            if record.get("subtype").and_then(|t| t.as_str()) == Some("local_command")
                && let Some(text) = record.get("content").and_then(|c| c.as_str())
            {
                push_text(turns, TurnKind::User, text);
            }
            return;
        }
        _ => return,
    };
    // A subagent's own conversation, and records the agent adds for itself.
    let flag = |name: &str| record.get(name).and_then(serde_json::Value::as_bool) == Some(true);
    if flag("isSidechain") || flag("isMeta") || flag("isCompactSummary") {
        return;
    }
    // Answers to the agent's own questions come back as its tool's result:
    // what was picked is the person's turn.
    if let Some(answers) = record
        .get("toolUseResult")
        .and_then(|r| r.get("answers"))
        .and_then(serde_json::Value::as_object)
    {
        let picked: Vec<&str> = answers.values().filter_map(|a| a.as_str()).collect();
        push_text(turns, TurnKind::User, &picked.join("\n"));
        return;
    }
    let Some(content) = record.get("message").and_then(|m| m.get("content")) else {
        return;
    };
    match content {
        serde_json::Value::String(text) => push_text(turns, kind, text),
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            push_text(turns, kind, text);
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("Tool");
                        if let Some(said) = spoken(name, block.get("input")) {
                            push_text(turns, TurnKind::Assistant, &said);
                        } else {
                            turns.push(Turn {
                                kind: TurnKind::Tool,
                                text: tool_line(name, block.get("input")),
                                tool: name.to_owned(),
                            });
                        }
                    }
                    // Thinking, tool results, images: not the conversation.
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// The turns one Codex rollout record holds.
///
/// Read from the `item_completed` events, which carry each finished item
/// once: what the person typed, what the agent said, the commands it ran and
/// the files it changed. The rollout's `response_item` records say the same
/// things again, alongside the instructions and context Codex feeds the
/// model as though the person had typed them.
fn codex_turns_of(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    if record.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
        return;
    }
    let Some(payload) = record.get("payload") else {
        return;
    };
    // A turn Codex ended on an error — a usage limit, most often — says so
    // in its TUI and nowhere in its items. Without this the conversation just
    // stopped, and read as a phone that had lost it.
    let ended_with = match payload.get("type").and_then(|t| t.as_str()) {
        Some("task_complete") => payload.get("error").and_then(|e| e.get("message")),
        Some("error") => payload.get("message"),
        _ => None,
    };
    if let Some(message) = ended_with.and_then(|m| m.as_str()) {
        push_text(turns, TurnKind::Assistant, message);
        return;
    }
    if payload.get("type").and_then(|t| t.as_str()) != Some("item_completed") {
        return;
    }
    let Some(item) = payload.get("item") else {
        return;
    };
    let words = || -> String {
        item.get("content")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let tool = |turns: &mut Vec<Turn>, name: &str, input: serde_json::Value| {
        turns.push(Turn {
            kind: TurnKind::Tool,
            text: tool_line(name, Some(&input)),
            tool: name.to_owned(),
        });
    };
    match item.get("type").and_then(|t| t.as_str()) {
        Some("UserMessage") => push_text(turns, TurnKind::User, &words()),
        Some("AgentMessage") => push_text(turns, TurnKind::Assistant, &words()),
        Some("CommandExecution") => {
            // `["/bin/zsh", "-lc", "<the command>"]`: the script is what was
            // run, the shell around it is Codex's.
            let command = match item.get("command") {
                Some(serde_json::Value::Array(parts)) => {
                    let parts: Vec<&str> = parts.iter().filter_map(|p| p.as_str()).collect();
                    match parts.as_slice() {
                        [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
                            (*script).to_owned()
                        }
                        _ => parts.join(" "),
                    }
                }
                Some(serde_json::Value::String(command)) => command.clone(),
                _ => return,
            };
            tool(turns, "Bash", serde_json::json!({ "command": command }));
        }
        Some("FileChange") => {
            for path in item
                .get("changes")
                .and_then(|c| c.as_object())
                .into_iter()
                .flat_map(|changes| changes.keys())
            {
                tool(turns, "Edit", serde_json::json!({ "file_path": path }));
            }
        }
        Some("Extension") if item.get("kind").and_then(|k| k.as_str()) == Some("web.search") => {
            if let Some(query) = item
                .get("query")
                .and_then(|q| q.as_str())
                .filter(|q| !q.trim().is_empty())
            {
                tool(turns, "WebSearch", serde_json::json!({ "query": query }));
            }
        }
        _ => {}
    }
}

/// The turns one Grok `chat_history.jsonl` record holds.
///
/// What the person typed carries a `prompt_index`, wrapped in
/// `<user_query>`; Grok files the context it
/// feeds the model — the project's instructions, its system reminders — as
/// user records too, without one. The agent's words are its `content`, and
/// each of its `tool_calls` is a tool, named and keyed the way Grok's hooks
/// are before [`crate::agent_hooks`] renames them to Claude's.
fn grok_turns_of(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    match record.get("type").and_then(|t| t.as_str()) {
        Some("user") if record.get("prompt_index").is_some() => {
            let words = match record.get("content") {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(serde_json::Value::Array(blocks)) => blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                _ => return,
            };
            // Grok wraps what was typed in `<user_query>`, which would read
            // as the harness talking.
            let words = tagged(&words, "user_query").unwrap_or(&words);
            push_text(turns, TurnKind::User, words);
        }
        Some("assistant") => {
            if let Some(text) = record.get("content").and_then(|c| c.as_str()) {
                push_text(turns, TurnKind::Assistant, text);
            }
            for call in record
                .get("tool_calls")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
            {
                let Some(name) = call.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                let mut input = call
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
                    .unwrap_or(serde_json::Value::Null);
                if let Some(input) = input.as_object_mut() {
                    crate::agent_hooks::grok_input(input);
                }
                let name = crate::agent_hooks::grok_tool(name).unwrap_or(name);
                turns.push(Turn {
                    kind: TurnKind::Tool,
                    text: tool_line(name, Some(&input)),
                    tool: name.to_owned(),
                });
            }
        }
        _ => {}
    }
}

/// The turns one OpenCode message holds, from its parts in order.
///
/// The person's words are the text parts of a user message, less the ones
/// OpenCode adds itself (`synthetic`). An assistant message is one step: its
/// text parts are what it said, and each tool part a tool — its `question`
/// asked out in full, as Claude's `AskUserQuestion` is. Thinking, the step
/// markers, snapshots and patches are not the conversation.
fn opencode_turns_of(
    message: &serde_json::Value,
    parts: &[serde_json::Value],
    turns: &mut Vec<Turn>,
) {
    let kind = match message.get("role").and_then(|r| r.as_str()) {
        Some("user") => TurnKind::User,
        Some("assistant") => TurnKind::Assistant,
        _ => return,
    };
    for part in parts {
        let synthetic = part.get("synthetic").and_then(serde_json::Value::as_bool) == Some(true);
        match part.get("type").and_then(|t| t.as_str()) {
            Some("text") if !synthetic => {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    push_text(turns, kind, text);
                }
            }
            Some("tool") if kind == TurnKind::Assistant => {
                let name = part.get("tool").and_then(|t| t.as_str()).unwrap_or("Tool");
                let input = part.get("state").and_then(|s| s.get("input"));
                let (name, input) = opencode_tool(name, input);
                if let Some(said) = spoken(name, input.as_ref()) {
                    push_text(turns, TurnKind::Assistant, &said);
                } else {
                    turns.push(Turn {
                        kind: TurnKind::Tool,
                        text: tool_line(name, input.as_ref()),
                        tool: name.to_owned(),
                    });
                }
            }
            Some("subtask") => {
                let input = serde_json::json!({ "description": part.get("description") });
                turns.push(Turn {
                    kind: TurnKind::Tool,
                    text: tool_line("Task", Some(&input)),
                    tool: "Task".to_owned(),
                });
            }
            _ => {}
        }
    }
}

/// An OpenCode tool by Claude's name, with its input keyed the way
/// [`tool_line`] and [`spoken`] read Claude's. The plugin ket installs in
/// OpenCode maps the same names for its hook reports — see `OPENCODE_PLUGIN`
/// in [`crate::agent_hooks`] — so a tool reads the same in both.
fn opencode_tool<'a>(
    name: &'a str,
    input: Option<&serde_json::Value>,
) -> (&'a str, Option<serde_json::Value>) {
    let claude = match name {
        "bash" => "Bash",
        "read" => "Read",
        "edit" | "multiedit" | "patch" => "Edit",
        "write" => "Write",
        "grep" => "Grep",
        "glob" => "Glob",
        "list" => "LS",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "task" => "Task",
        "todowrite" => "TodoWrite",
        "question" => "AskUserQuestion",
        other => other,
    };
    let mut input = input.cloned();
    if let Some(object) = input.as_mut().and_then(|i| i.as_object_mut())
        && !object.contains_key("file_path")
        && let Some(path) = object.get("filePath").cloned()
    {
        object.insert("file_path".to_owned(), path);
    }
    (claude, input)
}

/// A message typed while the agent was working: Claude folds it into the
/// turn in progress and records it as an attachment, not a user record.
fn absorbed(record: &serde_json::Value, turns: &mut Vec<Turn>) {
    let Some(attachment) = record.get("attachment") else {
        return;
    };
    let field = |name: &str| attachment.get(name).and_then(|v| v.as_str());
    if field("type") != Some("queued_command") || field("commandMode") != Some("prompt") {
        return;
    }
    match attachment.get("prompt") {
        Some(serde_json::Value::String(text)) => push_text(turns, TurnKind::User, text),
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(text) = block.get("text").and_then(|t| t.as_str())
                {
                    push_text(turns, TurnKind::User, text);
                }
            }
        }
        _ => {}
    }
}

/// A turn of text, unless it is the harness talking — a reminder, a caveat,
/// anything else that starts with a tag — or its note that a turn was
/// interrupted. Some of the harness's records are the person's after all: a
/// slash command, which Claude files as its `<command-name>` wrapper, and
/// what a local command printed, which reads as one line under it; and a
/// `!` command, which Claude files as `<bash-input>`, with what it printed as
/// a `<bash-stdout>` record of its own.
fn push_text(turns: &mut Vec<Turn>, kind: TurnKind, text: &str) {
    let text = text.trim();
    if kind == TurnKind::User {
        if let Some(command) = slash_command(text) {
            turns.push(Turn {
                kind: TurnKind::User,
                text: clip(&command),
                tool: String::new(),
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
fn tagged<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(&text[start..end])
}

/// `text` without its terminal colour codes: a local command prints for the
/// terminal, and its escapes are noise anywhere else.
fn strip_ansi(text: &str) -> String {
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
