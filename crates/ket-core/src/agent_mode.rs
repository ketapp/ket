//! The mode an agent is in — Claude's permission mode, OpenCode's agent,
//! Codex's collaboration mode — as a phone shows it and asks for another.
//!
//! None of the agents says which mode it is in to anyone. Claude writes a
//! `permission-mode` record to its transcript each time the mode changes;
//! OpenCode keeps the session's agent in its database; Codex keeps it to
//! itself. Each is switched with its own key — Shift+Tab for Claude and
//! Codex, Tab for OpenCode — which cycles, so a mode is reached by pressing
//! until the agent reports the one asked for.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::terminal::LocalTerminal;

/// One mode an agent offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    /// What a phone names to ask for it.
    pub id: String,
    /// The name shown to the person choosing a mode.
    pub label: String,
    /// A few words on what it does, or empty.
    pub detail: String,
}

/// An agent's modes, and the one it is in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Modes {
    /// Empty when the agent's mode can't be read.
    pub current: String,
    /// The modes this agent offers.
    pub choices: Vec<Mode>,
}

fn mode(id: &str, label: &str, detail: &str) -> Mode {
    Mode {
        id: id.to_owned(),
        label: label.to_owned(),
        detail: detail.to_owned(),
    }
}

/// Claude's modes by the ids its transcript records.
fn claude_mode_named(id: &str) -> Mode {
    match id {
        "default" => mode("default", "Default", "Asks before edits and commands"),
        "acceptEdits" => mode(
            "acceptEdits",
            "Accept edits",
            "Edits go in; commands still ask",
        ),
        "plan" => mode("plan", "Plan", "Reads and plans; changes nothing"),
        "auto" => mode("auto", "Auto", "Claude decides what to allow"),
        "bypassPermissions" => mode("bypassPermissions", "Bypass", "Nothing asks"),
        "dontAsk" => mode("dontAsk", "Don't ask", "Refuses what it would have asked"),
        other => mode(other, other, ""),
    }
}

/// The modes `agent` offers, with the one it is in: Claude's by its
/// transcript, OpenCode's by its session record, Codex's never known. `None`
/// for an agent with no modes, or a shell. `session` is the agent's session
/// id, where the host knows it.
pub fn modes(agent: &str, session: Option<&str>) -> Option<Modes> {
    match crate::agents::harness(agent).to_ascii_lowercase().as_str() {
        "claude" => {
            let current = session.and_then(claude_mode).unwrap_or_default();
            let mut choices: Vec<Mode> = ["default", "acceptEdits", "plan"]
                .into_iter()
                .map(claude_mode_named)
                .collect();
            // A mode only some installs reach — auto, bypass — is offered
            // once Claude is seen in it.
            if !current.is_empty() && !choices.iter().any(|choice| choice.id == current) {
                choices.push(claude_mode_named(&current));
            }
            Some(Modes { current, choices })
        }
        "codex" => Some(Modes {
            current: String::new(),
            choices: vec![
                mode("default", "Default", "Works as it goes"),
                mode("plan", "Plan", "Plans first; changes nothing"),
            ],
        }),
        "opencode" => {
            let current = session
                .and_then(|session| opencode_agent(session, true))
                .unwrap_or_default();
            let mut choices = vec![
                mode("build", "Build", "Edits and runs"),
                mode("plan", "Plan", "Reads and plans; changes nothing"),
            ];
            if !current.is_empty() && !choices.iter().any(|choice| choice.id == current) {
                let label = {
                    let mut chars = current.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                };
                choices.push(mode(&current, &label, ""));
            }
            Some(Modes { current, choices })
        }
        _ => None,
    }
}

/// How much of a transcript's end holds its newest mode record.
const MODE_TAIL: u64 = 256 * 1024;

/// Claude's permission mode in `session`: the newest `permission-mode`
/// record at its transcript's end, or `None` when none is there — a session
/// started in Default writes none until the mode first changes.
pub fn claude_mode(session: &str) -> Option<String> {
    let path = crate::conversation::claude_transcript(session)?;
    let bytes = tail(&path, MODE_TAIL)?;
    let marker = b"\"permission-mode\"";
    let mut found = None;
    for line in bytes.split(|&b| b == b'\n') {
        if !line.windows(marker.len()).any(|w| w == marker) {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("type").and_then(|t| t.as_str()) != Some("permission-mode") {
            continue;
        }
        if let Some(mode) = record.get("permissionMode").and_then(|m| m.as_str()) {
            found = Some(mode.to_owned());
        }
    }
    found
}

fn tail(path: &Path, n: u64) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(n))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// How long a read of OpenCode's agent is kept: a snapshot asks every
/// second, and each read is a `sqlite3` process.
const AGENT_TTL: Duration = Duration::from_secs(3);

type CachedAgent = (Instant, Option<String>);
type AgentCache = Mutex<HashMap<String, CachedAgent>>;
type ModeReader<'a> = Box<dyn Fn() -> Option<String> + 'a>;

fn agents() -> &'static AgentCache {
    static AGENTS: OnceLock<AgentCache> = OnceLock::new();
    AGENTS.get_or_init(Mutex::default)
}

/// The agent OpenCode session `session` runs — `build`, `plan` — from its
/// database; `cached` lets a recent read stand.
pub fn opencode_agent(session: &str, cached: bool) -> Option<String> {
    if !crate::conversation::opencode_id(session) {
        return None;
    }
    if cached
        && let Some((at, agent)) = agents()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
        && at.elapsed() < AGENT_TTL
    {
        return agent.clone();
    }
    let database = crate::sessions::opencode_database();
    let output = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg(&database)
        .arg(format!(
            "SELECT agent FROM session_v2 WHERE id = '{session}';"
        ))
        .output()
        .ok()?;
    let agent = output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|agent| !agent.is_empty());
    agents()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(session.to_owned(), (Instant::now(), agent.clone()));
    agent
}

/// How long one press is given to change the mode, and how often it is
/// looked for meanwhile.
const STEP_WAIT: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(100);
/// The most presses a mode is sought with: a cycle, and some slack.
const MAX_PRESSES: usize = 6;

/// Puts `agent` in mode `target` in `terminal`: its own key, pressed until
/// the agent reports that mode — read as [`modes`] reads it — or `Err` with
/// why not. Codex, which reports nothing, gets one press. Blocks while the
/// agent takes each press, so it runs on the terminal's typing worker.
pub fn set(
    terminal: &LocalTerminal,
    agent: &str,
    session: Option<&str>,
    target: &str,
) -> Result<(), String> {
    let gone = |_| "the terminal closed".to_owned();
    let harness = crate::agents::harness(agent).to_ascii_lowercase();
    let (name, keys, read): (&str, &[u8], ModeReader<'_>) = match harness.as_str() {
        "claude" => (
            "Claude",
            b"\x1b[Z",
            Box::new(move || session.and_then(claude_mode)),
        ),
        "opencode" => (
            "OpenCode",
            b"\t",
            Box::new(move || session.and_then(|session| opencode_agent(session, false))),
        ),
        "codex" => {
            return terminal.send_input(b"\x1b[Z".to_vec()).map_err(gone);
        }
        _ => return Err("this agent has no modes to switch".to_owned()),
    };
    if read().as_deref() == Some(target) {
        return Ok(());
    }
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..MAX_PRESSES {
        let before = read();
        terminal.send_input(keys.to_vec()).map_err(gone)?;
        let since = Instant::now();
        let mut now = None;
        while since.elapsed() < STEP_WAIT {
            std::thread::sleep(POLL);
            now = read();
            if now.is_some() && now != before {
                break;
            }
        }
        match now {
            Some(mode) if mode == target => return Ok(()),
            Some(mode) if mode == before.clone().unwrap_or_default() => {
                return Err(format!(
                    "{name} didn't take the key — switch it in the terminal"
                ));
            }
            Some(mode) => {
                if seen.contains(&mode) {
                    return Err(format!("{name} doesn't offer that mode here"));
                }
                seen.push(mode);
            }
            None => return Err(format!("{name} hasn't said which mode it is in yet")),
        }
    }
    Err(format!("{name} didn't settle on that mode"))
}
