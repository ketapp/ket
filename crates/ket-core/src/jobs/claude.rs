//! What Claude's transcript says about the commands it ran in the background:
//! what each was for, and how it ended.
//!
//! Three records, each written as it happens:
//!
//! - the `Bash` call, with `run_in_background: true`, its `command` and the
//!   `description` Claude gave it;
//! - the call's result, naming the task Claude made of it in
//!   `toolUseResult.backgroundTaskId`;
//! - a `<task-notification>` naming that task with its `status` —
//!   `completed`, `failed`, `killed` — and a summary that carries the exit
//!   code, queued for Claude the moment the command ends.
//!
//! A `stopped` notification is Claude saying it lost track of a command when
//! its own process ended, not that the command did; it is read as no ending
//! at all, and the process tree says whether the command is still there.
//!
//! The `background_tasks` inventory Claude attaches to `Stop` lists the same
//! shells, but only when a turn ends, and adds nothing the transcript does not
//! already say sooner.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How much of a transcript's end the first read takes in: a command started
/// before that has its command line to go by, not its description.
const FIRST_READ: u64 = 4 * 1024 * 1024;

/// Most that one read takes in. A transcript grows by far less between reads;
/// one that grew more is read on from where this one stopped.
const MOST_READ: u64 = 16 * 1024 * 1024;

/// How a command Claude ran in the background ended, as Claude says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// It ran to the end and exited 0.
    Completed,
    /// It exited with anything else.
    Failed,
    /// Something stopped it: Claude, at the person's word or its own.
    Killed,
}

/// The end of a command, as Claude's notification says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    /// How it ended.
    pub status: Status,
    /// Its exit code, where the summary gives one.
    pub exit: Option<i32>,
    /// When Claude heard, in milliseconds since the epoch.
    pub at_ms: u64,
}

/// A command Claude ran in the background, as its transcript tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// Claude's own id for the task.
    pub id: String,
    /// The command, as [`super::asked`] gives a process's: without the
    /// redirections at its end, so the two can be matched.
    pub command: String,
    /// What Claude said it was for, when it said.
    pub description: Option<String>,
    /// When Claude started it, in milliseconds since the epoch.
    pub started_at_ms: u64,
    /// How it ended, once Claude has heard.
    pub ended: Option<Ending>,
}

/// Where Claude keeps session `session` of an agent working in `cwd`:
/// `<config>/projects/<cwd, each character but an ASCII letter or digit made
/// `-`>/<session>.jsonl`, under whichever of Claude's config directories has
/// it. A look in each, where finding it by its name alone means looking in
/// every project folder Claude has ever made. `None` where Claude filed it
/// elsewhere — a session resumed from another directory.
pub fn transcript_at(cwd: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains(['/', '\\']) || session.contains("..") {
        return None;
    }
    let folder: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let file = format!("{session}.jsonl");
    crate::sessions::claude_config_dirs()
        .into_iter()
        .map(|dir| dir.join("projects").join(&folder).join(&file))
        .find(|path| path.is_file())
}

/// A `Bash` call made to run in the background, waiting for the result that
/// names its task.
struct Asked {
    command: String,
    description: Option<String>,
    at_ms: u64,
}

/// Reads one Claude transcript as it grows, keeping its background commands.
pub struct Tail {
    path: PathBuf,
    /// Where the next read starts; `None` before the first.
    offset: Option<u64>,
    asked: HashMap<String, Asked>,
    told: Vec<Told>,
}

impl Tail {
    /// A reader for the transcript at `path`, which has read nothing yet.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: None,
            asked: HashMap::new(),
            told: Vec::new(),
        }
    }

    /// The transcript it reads.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The commands read so far, oldest first: at most [`super::MOST`],
    /// ended ones dropped first to make room.
    pub fn told(&self) -> &[Told] {
        &self.told
    }

    /// Reads what the transcript has gained since the last read, up to its
    /// last whole line. Whether anything it keeps changed.
    pub fn poll(&mut self) -> bool {
        // Its length by path first: a transcript that has not grown, which is
        // most of the time, is never opened.
        let Ok(length) = std::fs::metadata(&self.path).map(|m| m.len()) else {
            return false;
        };
        if self.offset == Some(length) {
            return false;
        }
        // Shorter than what was read: rewritten. Start over.
        if self.offset.is_some_and(|offset| offset > length) {
            *self = Self::new(std::mem::take(&mut self.path));
        }
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return false;
        };
        let first = self.offset.is_none();
        let start = self
            .offset
            .unwrap_or_else(|| length.saturating_sub(FIRST_READ));
        if start >= length || file.seek(SeekFrom::Start(start)).is_err() {
            self.offset = Some(start.min(length));
            return false;
        }
        let mut bytes = Vec::new();
        if file
            .take((length - start).min(MOST_READ))
            .read_to_end(&mut bytes)
            .is_err()
        {
            return false;
        }
        let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else {
            self.offset = Some(start);
            return false;
        };
        self.offset = Some(start + end as u64 + 1);
        let mut lines = bytes[..end].split(|&b| b == b'\n');
        // A first read that starts mid-transcript starts mid-record.
        if first && start > 0 {
            lines.next();
        }
        let mut changed = false;
        for line in lines {
            changed |= self.read(line);
        }
        changed
    }

    /// One record. Most are none of the three, and are told apart by a
    /// substring before anything is parsed.
    fn read(&mut self, line: &[u8]) -> bool {
        if has(line, b"<task-notification>") {
            self.ended(line)
        } else if has(line, b"\"backgroundTaskId\":\"") {
            self.started(line)
        } else {
            if has(line, b"\"run_in_background\":true") {
                self.asked(line);
            }
            false
        }
    }

    fn asked(&mut self, line: &[u8]) {
        let Some(record) = record_of(line, &["assistant"]) else {
            return;
        };
        let at_ms = dated(&record);
        let calls = record
            .pointer("/message/content")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| {
                block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                    && block.get("name").and_then(|n| n.as_str()) == Some("Bash")
                    && block.pointer("/input/run_in_background")
                        == Some(&serde_json::Value::Bool(true))
            });
        for call in calls {
            let text = |pointer: &str| {
                call.pointer(pointer)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            };
            let (Some(id), Some(command)) = (text("/id"), text("/input/command")) else {
                continue;
            };
            self.asked.insert(
                id.to_owned(),
                Asked {
                    command: super::unredirected(command).to_owned(),
                    description: text("/input/description").map(str::to_owned),
                    at_ms,
                },
            );
        }
        // Calls whose results never came: keep the newest.
        if self.asked.len() > super::MOST {
            let mut ages: Vec<u64> = self.asked.values().map(|a| a.at_ms).collect();
            ages.sort_unstable();
            let cut = ages[ages.len() - super::MOST];
            self.asked.retain(|_, asked| asked.at_ms >= cut);
        }
    }

    fn started(&mut self, line: &[u8]) -> bool {
        let Some(record) = record_of(line, &["user"]) else {
            return false;
        };
        let Some(task) = record
            .pointer("/toolUseResult/backgroundTaskId")
            .and_then(|t| t.as_str())
        else {
            return false;
        };
        let call = record
            .pointer("/message/content")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|block| block.get("tool_use_id").and_then(|id| id.as_str()));
        let Some(asked) = call.and_then(|call| self.asked.remove(call)) else {
            return false;
        };
        if self.told.iter().any(|told| told.id == task) {
            return false;
        }
        if self.told.len() >= super::MOST {
            let oldest = self
                .told
                .iter()
                .position(|told| told.ended.is_some())
                .unwrap_or(0);
            self.told.remove(oldest);
        }
        self.told.push(Told {
            id: task.to_owned(),
            command: asked.command,
            description: asked.description,
            started_at_ms: asked.at_ms,
            ended: None,
        });
        true
    }

    fn ended(&mut self, line: &[u8]) -> bool {
        // Queued for Claude, then handed to it as a message: either will do,
        // and the second finds the command already ended.
        let Some(record) = record_of(line, &["queue-operation", "user"]) else {
            return false;
        };
        let text = String::from_utf8_lossy(line);
        let (Some(task), Some(status)) = (tag(&text, "task-id"), tag(&text, "status")) else {
            return false;
        };
        let status = match status {
            "completed" => Status::Completed,
            "failed" => Status::Failed,
            "killed" => Status::Killed,
            _ => return false,
        };
        let Some(told) = self
            .told
            .iter_mut()
            .find(|told| told.id == task && told.ended.is_none())
        else {
            return false;
        };
        told.ended = Some(Ending {
            status,
            exit: tag(&text, "summary").and_then(exit_code),
            at_ms: dated(&record),
        });
        true
    }
}

/// `line` parsed, when it is a record of one of `kinds`. Claude also copies
/// what it sends the model into `api-request-blob` records, which say the same
/// things again, later, and undated.
fn record_of(line: &[u8], kinds: &[&str]) -> Option<serde_json::Value> {
    let record = serde_json::from_slice::<serde_json::Value>(line).ok()?;
    let kind = record.get("type").and_then(|t| t.as_str())?;
    kinds.contains(&kind).then_some(record)
}

fn has(line: &[u8], needle: &[u8]) -> bool {
    line.windows(needle.len()).any(|w| w == needle)
}

/// What `<name>…</name>` holds in `text`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&format!("</{name}>"))?;
    Some(text[start..end].trim())
}

/// The exit code in a summary: "… completed (exit code 0)", "… failed with
/// exit code 2".
fn exit_code(summary: &str) -> Option<i32> {
    let start = summary.find("exit code ")? + "exit code ".len();
    let digits: String = summary[start..]
        .chars()
        .enumerate()
        .take_while(|&(i, c)| c.is_ascii_digit() || (i == 0 && c == '-'))
        .map(|(_, c)| c)
        .collect();
    digits.parse().ok()
}

/// When `record` was written, from its `timestamp`; 0 when it does not say.
fn dated(record: &serde_json::Value) -> u64 {
    record
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(crate::rate_limits::parse_rfc3339_ms)
        .unwrap_or_default()
}
