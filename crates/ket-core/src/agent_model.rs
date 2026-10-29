//! The model and reasoning effort a terminal's agent runs with, and changing
//! them from a phone.
//!
//! Claude and Codex only, each by its own command, typed as a person would:
//! Claude's `/model <alias>` and `/effort <level>`, Codex's `/model <model>
//! --reasoning=<level>`. What the agent runs with is read off its transcript:
//! Claude writes the model and effort on every reply, Codex on every turn's
//! context. A change shows there only once the agent next answers, so one
//! made here is remembered until the transcript says otherwise.
//!
//! Claude keeps a change as its default for new sessions too ("saved as your
//! default for new sessions"); Codex's is "for this session only". Both
//! confirm on screen, which is how a change is known to have taken.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::terminal::LocalTerminal;

/// One thing that can be picked: the id the agent's command takes, and what
/// a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What the agent's command takes.
    pub id: String,
    /// What a person reads.
    pub label: String,
}

/// What an agent runs with, and what it can be switched to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The model's id, one of `models`; empty while the agent hasn't said.
    pub model: String,
    /// The effort's id, one of `efforts`; empty while unknown, or for a
    /// model without one.
    pub effort: String,
    /// The models it can be switched to, the one in use among them.
    pub models: Vec<Choice>,
    /// The effort levels it takes.
    pub efforts: Vec<Choice>,
    /// Whether a change also becomes the agent's default for new sessions.
    pub saves_default: bool,
}

/// Claude's model aliases, as `/model` and `--model` take them, newest
/// family first.
const CLAUDE_MODELS: [(&str, &str); 4] = [
    ("fable", "Fable"),
    ("opus", "Opus"),
    ("sonnet", "Sonnet"),
    ("haiku", "Haiku"),
];

/// Claude's effort levels, as `/effort` and `--effort` take them, as of
/// Claude Code 2.1.288.
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Codex's models and effort levels for when its model cache can't be read.
const CODEX_MODELS: [&str; 4] = ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-sol", "gpt-6-luna"];
const CODEX_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

fn choices(pairs: &[(&str, &str)]) -> Vec<Choice> {
    pairs
        .iter()
        .map(|(id, label)| Choice {
            id: (*id).to_owned(),
            label: (*label).to_owned(),
        })
        .collect()
}

/// Effort levels as choices: `xhigh` reads "Extra high", the rest as named.
fn effort_choices<S: AsRef<str>>(ids: &[S]) -> Vec<Choice> {
    ids.iter()
        .map(|id| {
            let id = id.as_ref();
            let label = match id {
                "xhigh" => "Extra high".to_owned(),
                _ => {
                    let mut chars = id.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect()
                    })
                }
            };
            Choice {
                id: id.to_owned(),
                label,
            }
        })
        .collect()
}

/// One model Codex's `/model` picker lists, and the effort levels it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexModel {
    id: String,
    efforts: Vec<String>,
}

/// The models Codex's own picker lists, in its order, from the cache it keeps
/// of what the signed-in account can use: `~/.codex/models_cache.json`.
fn parse_codex_models(bytes: &[u8]) -> Vec<CodexModel> {
    #[derive(serde::Deserialize)]
    struct Cache {
        #[serde(default)]
        models: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        slug: String,
        #[serde(default)]
        visibility: String,
        #[serde(default)]
        priority: i64,
        #[serde(default)]
        supported_reasoning_levels: Vec<Level>,
    }
    #[derive(serde::Deserialize)]
    struct Level {
        effort: String,
    }
    let Ok(cache) = serde_json::from_slice::<Cache>(bytes) else {
        return Vec::new();
    };
    let mut listed: Vec<Entry> = cache
        .models
        .into_iter()
        .filter(|entry| entry.visibility == "list")
        .collect();
    listed.sort_by_key(|entry| entry.priority);
    listed
        .into_iter()
        .map(|entry| CodexModel {
            id: entry.slug,
            efforts: entry
                .supported_reasoning_levels
                .into_iter()
                .map(|level| level.effort)
                .collect(),
        })
        .collect()
}

type CodexCache = Mutex<Option<((u64, Option<std::time::SystemTime>), Vec<CodexModel>)>>;

/// `parse_codex_models` of Codex's cache, read again only when the file
/// changes: a snapshot asks every second, and the file runs to hundreds of
/// kilobytes. Empty when there's none to read.
fn codex_models() -> Vec<CodexModel> {
    static CACHE: OnceLock<CodexCache> = OnceLock::new();
    let Ok(home) = crate::paths::home() else {
        return Vec::new();
    };
    let path = home.join(".codex").join("models_cache.json");
    let Ok(meta) = std::fs::metadata(&path) else {
        return Vec::new();
    };
    let stamp = (meta.len(), meta.modified().ok());
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some((seen, models)) = cache.as_ref()
        && *seen == stamp
    {
        return models.clone();
    }
    let models = std::fs::read(&path)
        .map(|bytes| parse_codex_models(&bytes))
        .unwrap_or_default();
    *cache = Some((stamp, models.clone()));
    models
}

/// The effort levels Codex takes on `model`: those its cache gives that
/// model, or every level any listed model takes while it's not one of them.
fn codex_efforts(listed: &[CodexModel], model: &str) -> Vec<String> {
    if listed.is_empty() {
        return CODEX_EFFORTS.iter().map(|&id| id.to_owned()).collect();
    }
    if let Some(found) = listed.iter().find(|found| found.id == model) {
        return found.efforts.clone();
    }
    let mut all: Vec<String> = Vec::new();
    for effort in listed.iter().flat_map(|found| &found.efforts) {
        if !all.contains(effort) {
            all.push(effort.clone());
        }
    }
    all
}

/// The alias of Claude model id `id` — `claude-opus-5-5` is `opus` — or the
/// id itself for one with none.
fn claude_alias(id: &str) -> String {
    CLAUDE_MODELS
        .iter()
        .find(|(alias, _)| id.contains(alias))
        .map_or_else(|| id.to_owned(), |(alias, _)| (*alias).to_owned())
}

/// What `agent` runs with in `session`, and what it can be switched to.
/// `None` for an agent ket can't switch, or a shell.
pub fn settings(agent: &str, session: Option<&str>) -> Option<Settings> {
    let harness = crate::agents::harness(agent).to_ascii_lowercase();
    let codex = if harness == "codex" {
        codex_models()
    } else {
        Vec::new()
    };
    let (mut models, saves_default, path) = match harness.as_str() {
        "claude" => (
            choices(&CLAUDE_MODELS),
            true,
            session.and_then(crate::conversation::claude_transcript),
        ),
        "codex" => (
            if codex.is_empty() {
                CODEX_MODELS.iter().map(|&id| id.to_owned()).collect()
            } else {
                codex
                    .iter()
                    .map(|found| found.id.clone())
                    .collect::<Vec<_>>()
            }
            .into_iter()
            .map(|id| Choice {
                label: id.clone(),
                id,
            })
            .collect(),
            false,
            session.and_then(crate::conversation::codex_transcript),
        ),
        _ => return None,
    };
    let read = path.as_deref().and_then(|path| current(&harness, path));
    let (model, effort) = match (session.and_then(pending), read) {
        // A change made here, not yet answered under.
        (Some((model, effort, at)), Some(read)) if read.at <= at => (model, effort),
        (Some((model, effort, _)), None) => (model, effort),
        (_, Some(read)) => {
            if let Some(session) = session {
                forget(session);
            }
            (read.model, read.effort)
        }
        (None, None) => (String::new(), String::new()),
    };
    // A model the list doesn't hold is still the one in use: offered too.
    if !model.is_empty() && !models.iter().any(|choice| choice.id == model) {
        models.push(Choice {
            id: model.clone(),
            label: model.clone(),
        });
    }
    let efforts = if harness == "codex" {
        effort_choices(&codex_efforts(&codex, &model))
    } else {
        effort_choices(&CLAUDE_EFFORTS)
    };
    Some(Settings {
        model,
        effort,
        models,
        efforts,
        saves_default,
    })
}

/// What a transcript says the agent last ran with, and where it said it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Read_ {
    model: String,
    effort: String,
    /// The byte offset the record ends at.
    at: u64,
}

/// How much of a transcript's end is read for its newest model record.
const TAIL: u64 = 512 * 1024;

type ReadCache = Mutex<HashMap<PathBuf, (u64, Option<Read_>)>>;

/// Reads of each transcript, kept while it doesn't grow: a snapshot asks
/// every second, and a transcript is idle far more often than not.
fn reads() -> &'static ReadCache {
    static READS: OnceLock<ReadCache> = OnceLock::new();
    READS.get_or_init(Mutex::default)
}

fn current(harness: &str, path: &Path) -> Option<Read_> {
    let length = std::fs::metadata(path).ok()?.len();
    let mut cache = reads().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((seen, read)) = cache.get(path)
        && *seen == length
    {
        return read.clone();
    }
    let read = read_tail(harness, path, length);
    cache.insert(path.to_owned(), (length, read.clone()));
    read
}

fn read_tail(harness: &str, path: &Path, length: u64) -> Option<Read_> {
    let mut file = std::fs::File::open(path).ok()?;
    let start = length.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let mut found = None;
    let mut offset = start;
    for line in bytes.split(|&b| b == b'\n') {
        offset += line.len() as u64 + 1;
        let read = match harness {
            "claude" => claude_record(line),
            _ => codex_record(line),
        };
        if let Some((model, effort)) = read {
            found = Some(Read_ {
                model,
                effort,
                at: offset,
            });
        }
    }
    found
}

/// The model and effort on one of Claude's replies.
fn claude_record(line: &[u8]) -> Option<(String, String)> {
    let marker = b"\"assistant\"";
    if !line.windows(marker.len()).any(|w| w == marker) {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(line).ok()?;
    if record.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let model = record.get("message")?.get("model")?.as_str()?;
    // Claude's own placeholder for a reply no model wrote.
    if model.starts_with('<') {
        return None;
    }
    let effort = record
        .get("effort")
        .and_then(|e| e.as_str())
        .unwrap_or_default();
    Some((claude_alias(model), effort.to_owned()))
}

/// The model and effort in one of Codex's turn contexts.
fn codex_record(line: &[u8]) -> Option<(String, String)> {
    let marker = b"\"turn_context\"";
    if !line.windows(marker.len()).any(|w| w == marker) {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(line).ok()?;
    if record.get("type")?.as_str()? != "turn_context" {
        return None;
    }
    let payload = record.get("payload")?;
    let model = payload.get("model")?.as_str()?;
    let effort = payload
        .get("effort")
        .and_then(|e| e.as_str())
        .unwrap_or_default();
    Some((model.to_owned(), effort.to_owned()))
}

type Pending = Mutex<HashMap<String, (String, String, u64)>>;

/// Changes made from a phone, by session: the model and effort, and how
/// long the transcript was then. Kept until the agent answers under them.
fn changes() -> &'static Pending {
    static CHANGES: OnceLock<Pending> = OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

fn pending(session: &str) -> Option<(String, String, u64)> {
    changes()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(session)
        .cloned()
}

fn forget(session: &str) {
    changes()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(session);
}

/// How long the agent is given to confirm a command on screen.
const CONFIRM_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);
/// Between a command's last character and Enter: its `/` menu settles.
const SETTLE: Duration = Duration::from_millis(300);

/// Switches `agent` in `terminal` to `model` and `effort` — either empty
/// to leave it as it is — by the agent's own commands, waiting for each to
/// be confirmed on screen. Blocks while it types, so it runs on the
/// terminal's typing worker. `Err` says why not.
pub fn set(
    terminal: &LocalTerminal,
    agent: &str,
    session: Option<&str>,
    model: &str,
    effort: &str,
) -> Result<(), String> {
    let harness = crate::agents::harness(agent).to_ascii_lowercase();
    let now = settings(agent, session).ok_or("this agent's model can't be changed from here")?;
    if !model.is_empty() && !now.models.iter().any(|choice| choice.id == model) {
        return Err(format!("{model} isn't a model this agent offers"));
    }
    if !effort.is_empty() && !now.efforts.iter().any(|choice| choice.id == effort) {
        return Err(format!("{effort} isn't an effort level this agent takes"));
    }
    let model = if model.is_empty() {
        now.model.as_str()
    } else {
        model
    };
    let effort = if effort.is_empty() {
        now.effort.as_str()
    } else {
        effort
    };
    match harness.as_str() {
        "claude" => {
            let transcript = session.and_then(crate::conversation::claude_transcript);
            if model != now.model && !model.is_empty() {
                command(
                    terminal,
                    &format!("/model {model}"),
                    &["Set model to", "Kept model as"],
                    model,
                    "Claude",
                    transcript.as_deref(),
                )?;
            }
            if effort != now.effort && !effort.is_empty() {
                // "Set effort level to high (saved as your default for new
                // sessions): …" since Claude Code 2.1; the others before.
                command(
                    terminal,
                    &format!("/effort {effort}"),
                    &[
                        "Set effort level to",
                        "Effort set to",
                        "Effort level set to",
                    ],
                    effort,
                    "Claude",
                    transcript.as_deref(),
                )?;
            }
        }
        "codex" => {
            // Codex's `/model` names the model with the effort.
            if model.is_empty() {
                return Err("Codex hasn't said which model it's on yet — pick a model".to_owned());
            }
            if model != now.model || effort != now.effort {
                let line = if effort.is_empty() {
                    format!("/model {model}")
                } else {
                    format!("/model {model} --reasoning={effort}")
                };
                command(terminal, &line, &["Model changed to"], model, "Codex", None)?;
            }
        }
        _ => return Err("this agent's model can't be changed from here".to_owned()),
    }
    if let Some(session) = session {
        let at = crate::conversation::Transcript::locate(agent, session)
            .and_then(|transcript| match transcript {
                crate::conversation::Transcript::Claude(path)
                | crate::conversation::Transcript::Codex(path) => std::fs::metadata(path).ok(),
                _ => None,
            })
            .map_or(0, |meta| meta.len());
        changes()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                session.to_owned(),
                (model.to_owned(), effort.to_owned(), at),
            );
    }
    Ok(())
}

/// Lower case, letters and digits only: `GPT-6 Luna` is `gpt6luna`.
fn plain(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether `text` confirms a change to `wanted`: a confirmation phrase with
/// the value straight after it. Straight after, as Claude's effort line goes
/// on to describe the level — "Set effort level to xhigh (…): Deeper
/// reasoning than high" is not a change to high.
fn says(text: &str, phrases: &[&str], wanted: &str) -> bool {
    let wanted = plain(wanted);
    phrases.iter().any(|phrase| {
        text.split(phrase)
            .skip(1)
            .any(|after| plain(after).starts_with(&wanted))
    })
}

/// The rows of `rows` that confirm a change to `wanted`.
fn confirmations(rows: &[String], phrases: &[&str], wanted: &str) -> usize {
    rows.iter().filter(|row| says(row, phrases, wanted)).count()
}

/// Whether Claude's transcript at `path` has, past byte `from`, a command's
/// output confirming a change to `wanted`. Claude records what each command
/// printed there, so this holds when the screen doesn't: an older
/// confirmation scrolling off the top as the new one arrives leaves the
/// count on screen where it was.
fn recorded(path: &Path, from: u64, phrases: &[&str], wanted: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    bytes
        .split(|&b| b == b'\n')
        .filter_map(printed)
        .any(|text| says(&text, phrases, wanted))
}

/// What a command printed, from one line of Claude's transcript: a person's
/// record or, in some versions, a `local_command` note.
fn printed(line: &[u8]) -> Option<String> {
    let marker = b"local-command-stdout";
    if !line.windows(marker.len()).any(|w| w == marker) {
        return None;
    }
    let record: serde_json::Value = serde_json::from_slice(line).ok()?;
    let text = record
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| record.get("content"))?
        .as_str()?;
    let said = crate::conversation::tagged(text, "local-command-stdout")?;
    Some(crate::conversation::strip_ansi(said))
}

/// What an agent prints when a command's argument is refused.
const REFUSALS: [&str; 4] = [
    "is unavailable",
    "Unexpected argument",
    "exceeds the cap",
    "not supported",
];

/// Types `line` and Enter into `terminal`, then waits for a new row saying
/// one of `phrases` with `wanted` after it, or for the agent's `transcript`
/// to record one. A refusal on screen is the `Err`.
fn command(
    terminal: &LocalTerminal,
    line: &str,
    phrases: &[&str],
    wanted: &str,
    name: &str,
    transcript: Option<&Path>,
) -> Result<(), String> {
    let screen = || terminal.with_term(|term| crate::screen::bottom_rows(term, usize::MAX));
    let gone = |_| "the terminal closed".to_owned();
    let before = screen();
    let confirmed_before = confirmations(&before, phrases, wanted);
    let transcript = transcript.and_then(|path| Some((path, std::fs::metadata(path).ok()?.len())));
    let refused_before = before
        .iter()
        .filter(|row| REFUSALS.iter().any(|refusal| row.contains(refusal)))
        .count();
    // A key at a time, as typed: a burst reads as a paste to some agents.
    for byte in line.bytes() {
        terminal.send_input(vec![byte]).map_err(gone)?;
        std::thread::sleep(Duration::from_millis(4));
    }
    std::thread::sleep(SETTLE);
    terminal.send_input(b"\r".to_vec()).map_err(gone)?;
    let since = Instant::now();
    while since.elapsed() < CONFIRM_WAIT {
        std::thread::sleep(POLL);
        let rows = screen();
        if confirmations(&rows, phrases, wanted) > confirmed_before
            || transcript.is_some_and(|(path, from)| recorded(path, from, phrases, wanted))
        {
            return Ok(());
        }
        let refusals: Vec<&String> = rows
            .iter()
            .filter(|row| REFUSALS.iter().any(|refusal| row.contains(refusal)))
            .collect();
        if refusals.len() > refused_before
            && let Some(said) = refusals.last()
        {
            return Err(said
                .trim()
                .trim_start_matches(['■', '⚠', '•', ' '])
                .to_owned());
        }
    }
    Err(format!(
        "{name} didn't confirm the change — look at the terminal"
    ))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn claudes_model_ids_are_read_as_the_aliases_model_takes() {
        assert_eq!(claude_alias("claude-opus-5-5"), "opus");
        assert_eq!(claude_alias("claude-fable-5-1"), "fable");
        assert_eq!(claude_alias("claude-sonnet-5-5"), "sonnet");
        assert_eq!(claude_alias("claude-haiku-4-5-20251001"), "haiku");
        assert_eq!(claude_alias("some-other-model"), "some-other-model");
    }

    #[test]
    fn a_claude_reply_says_the_model_and_effort_it_ran_with() {
        let reply = br#"{"type":"assistant","effort":"xhigh","message":{"model":"claude-opus-5-5","content":[]}}"#;
        assert_eq!(
            claude_record(reply),
            Some(("opus".to_owned(), "xhigh".to_owned()))
        );
        // A model with no effort: the model alone.
        let haiku = br#"{"type":"assistant","message":{"model":"claude-haiku-4-5","content":[]}}"#;
        assert_eq!(
            claude_record(haiku),
            Some(("haiku".to_owned(), String::new()))
        );
        // Claude's placeholder for a reply no model wrote, and the person's
        // own words with "assistant" in them, are not replies.
        let synthetic = br#"{"type":"assistant","message":{"model":"<synthetic>","content":[]}}"#;
        assert_eq!(claude_record(synthetic), None);
        let user = br#"{"type":"user","message":{"content":"ask the assistant"}}"#;
        assert_eq!(claude_record(user), None);
        assert_eq!(claude_record(b"not json \"assistant\""), None);
    }

    #[test]
    fn a_codex_turn_says_the_model_and_effort_it_ran_with() {
        let turn = br#"{"type":"turn_context","payload":{"model":"gpt-6.1-sol","effort":"medium","summary":"none"}}"#;
        assert_eq!(
            codex_record(turn),
            Some(("gpt-6.1-sol".to_owned(), "medium".to_owned()))
        );
        assert_eq!(
            codex_record(br#"{"type":"event_msg","payload":{"type":"turn_context"}}"#),
            None
        );
        assert_eq!(
            codex_record(br#"{"type":"turn_context","payload":{}}"#),
            None
        );
    }

    fn transcript(name: &str, lines: &[&str]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ket-agent-model-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    #[test]
    fn the_newest_record_in_a_transcript_is_the_one_in_use_and_where_it_ends() {
        let lines = [
            r#"{"type":"assistant","effort":"high","message":{"model":"claude-sonnet-5-5","content":[]}}"#,
            r#"{"type":"user","message":{"content":"next"}}"#,
            r#"{"type":"assistant","effort":"low","message":{"model":"claude-haiku-4-5","content":[]}}"#,
            r#"{"type":"user","message":{"content":"last"}}"#,
        ];
        let path = transcript("claude", &lines);
        let length = std::fs::metadata(&path).unwrap().len();
        let read = read_tail("claude", &path, length).unwrap();
        assert_eq!(
            (read.model.as_str(), read.effort.as_str()),
            ("haiku", "low")
        );
        let through_third = (lines[0].len() + lines[1].len() + lines[2].len() + 3) as u64;
        assert_eq!(read.at, through_third);
        // The cached read is the same until the file grows.
        assert_eq!(current("claude", &path), Some(read));

        let codex = transcript(
            "codex",
            &[
                r#"{"type":"turn_context","payload":{"model":"gpt-6-sol","effort":"low"}}"#,
                r#"{"type":"turn_context","payload":{"model":"gpt-6-luna","effort":"xhigh"}}"#,
            ],
        );
        let length = std::fs::metadata(&codex).unwrap().len();
        let read = read_tail("codex", &codex, length).unwrap();
        assert_eq!(
            (read.model.as_str(), read.effort.as_str()),
            ("gpt-6-luna", "xhigh")
        );
        let none = transcript("empty", &[r#"{"type":"user","message":{"content":"hi"}}"#]);
        let length = std::fs::metadata(&none).unwrap().len();
        assert_eq!(read_tail("claude", &none, length), None);
    }

    #[test]
    fn a_change_is_confirmed_by_a_new_row_naming_it() {
        let before = rows(&["  ⎿  Set model to Sonnet 5.5 and saved as your default"]);
        assert_eq!(confirmations(&before, &["Set model to"], "opus"), 0);
        let after = rows(&[
            "  ⎿  Set model to Sonnet 5.5 and saved as your default",
            "  ⎿  Set model to `Opus 5.5` and saved as your default for new sessions",
        ]);
        assert_eq!(confirmations(&after, &["Set model to"], "opus"), 1);
        let codex = rows(&["• Model changed to gpt-6-luna low for this session only"]);
        assert_eq!(
            confirmations(&codex, &["Model changed to"], "gpt-6-luna"),
            1
        );
        // gpt-6-sol is not gpt-6.1-sol.
        assert_eq!(confirmations(&codex, &["Model changed to"], "gpt-6-sol"), 0);
        // Claude Code 2.1's effort line, and the one before it.
        let effort_phrases = [
            "Set effort level to",
            "Effort set to",
            "Effort level set to",
        ];
        let effort = rows(&[
            "  ⎿  Set effort level to xhigh (saved as your default for new sessions): Deeper reasoning than high, just below maximum (on supported models)",
        ]);
        assert_eq!(confirmations(&effort, &effort_phrases, "xhigh"), 1);
        // Its description names high, which is not a change to high.
        assert_eq!(confirmations(&effort, &effort_phrases, "high"), 0);
        let older = rows(&["  ⎿  Effort set to medium (saved as your default for new sessions)"]);
        assert_eq!(confirmations(&older, &effort_phrases, "medium"), 1);
        // A row naming the value without the phrase is not a confirmation.
        assert_eq!(
            confirmations(&rows(&["Using opus today"]), &["Set model to"], "opus"),
            0
        );
        assert_eq!(plain("GPT-6.1 Sol"), "gpt61sol");
    }

    #[test]
    fn a_change_is_confirmed_by_claudes_transcript_recording_what_the_command_printed() {
        let phrases = [
            "Set effort level to",
            "Effort set to",
            "Effort level set to",
        ];
        let earlier = r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set effort level to high (saved as your default for new sessions): Comprehensive implementation</local-command-stdout>"}}"#;
        let command = r#"{"type":"user","message":{"role":"user","content":"<command-name>/effort</command-name>\n<command-args>low</command-args>"}}"#;
        let printed_low = r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set effort level to low (saved as your default for new sessions): Quick, straightforward implementation</local-command-stdout>"}}"#;
        let path = transcript("recorded", &[earlier, command, printed_low]);
        let from = (earlier.len() + 1) as u64;
        assert!(recorded(&path, from, &phrases, "low"));
        // Only what was written after `from` counts.
        assert!(!recorded(&path, from, &phrases, "high"));
        assert!(recorded(&path, 0, &phrases, "high"));
        assert!(!recorded(&path, from, &phrases, "xhigh"));
        // Older versions bold the model and file the output as a note.
        let model = transcript(
            "recorded-model",
            &[
                r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout>Set model to \u001b[1mOpus 5 (1M context)\u001b[22m and saved as your default for new sessions</local-command-stdout>"}"#,
            ],
        );
        assert!(recorded(&model, 0, &["Set model to"], "opus"));
        assert!(!recorded(&model, 0, &["Set model to"], "sonnet"));
        assert!(!recorded(Path::new("/nonexistent/ket"), 0, &phrases, "low"));
    }

    #[test]
    fn only_claude_and_codex_can_be_switched_and_say_whether_a_pick_is_kept() {
        let claude = settings("claude", None).unwrap();
        assert!(claude.saves_default);
        let ids: Vec<&str> = claude.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["fable", "opus", "sonnet", "haiku"]);
        let efforts: Vec<&str> = claude.efforts.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(efforts, ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(claude.efforts[3].label, "Extra high");
        assert_eq!(claude.efforts[4].label, "Max");
        assert!(claude.model.is_empty());
        // Codex's come from this Mac's cache of them, or a fallback.
        let codex = settings("codex", None).unwrap();
        assert!(!codex.saves_default);
        assert!(!codex.models.is_empty() && !codex.efforts.is_empty());
        assert_eq!(settings("opencode", None), None);
        assert_eq!(settings("grok", None), None);
        assert_eq!(settings("", None), None);
    }

    #[test]
    fn codex_offers_the_models_its_picker_lists_with_each_ones_efforts() {
        let cache = br#"{"fetched_at":"2026-10-29T03:27:00Z","models":[
            {"slug":"gpt-6-luna","visibility":"list","priority":5,"description":"x",
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"max"}]},
            {"slug":"gpt-reserve","visibility":"hide","priority":4,
             "supported_reasoning_levels":[{"effort":"low"}]},
            {"slug":"gpt-6.1-sol","visibility":"list","priority":1,
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}]}
        ]}"#;
        let listed = parse_codex_models(cache);
        let ids: Vec<&str> = listed.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6.1-sol", "gpt-6-luna"]);
        assert_eq!(codex_efforts(&listed, "gpt-6-luna"), ["low", "max"]);
        // No model known: every level a listed one takes.
        assert_eq!(codex_efforts(&listed, ""), ["low", "ultra", "max"]);
        assert_eq!(codex_efforts(&[], ""), CODEX_EFFORTS);
        assert!(parse_codex_models(b"not json").is_empty());
        let labels: Vec<String> = effort_choices(&["xhigh", "ultra"])
            .into_iter()
            .map(|choice| choice.label)
            .collect();
        assert_eq!(labels, ["Extra high", "Ultra"]);
    }

    #[test]
    fn a_change_made_here_shows_until_the_agent_answers_under_it() {
        let session = format!("ket-agent-model-pending-{}", std::process::id());
        changes()
            .lock()
            .unwrap()
            .insert(session.clone(), ("opus".to_owned(), "high".to_owned(), 100));
        assert_eq!(
            pending(&session),
            Some(("opus".to_owned(), "high".to_owned(), 100))
        );
        // No transcript to read: the change made here is what shows.
        let shown = settings("claude", Some(&session)).unwrap();
        assert_eq!(
            (shown.model.as_str(), shown.effort.as_str()),
            ("opus", "high")
        );
        forget(&session);
        assert_eq!(pending(&session), None);
    }
}
