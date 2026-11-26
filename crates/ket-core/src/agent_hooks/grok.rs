//! Grok's hooks, and its payloads put into the shape Claude's have.

use std::path::{Path, PathBuf};

use super::places::Place;
use super::script::script_path;
use super::settings::OWNER_MARK;
use super::{HookEvent, HookInstaller, home};
use crate::{KetError, Result};

// ---- grok -------------------------------------------------------------------

/// Grok: a hooks file of ket's own, `~/.grok/hooks/ket.json`.
///
/// Grok merges every `*.json` in its hooks directory, so, as with
/// [`OpenCodeHooks`](super::OpenCodeHooks), ket owns a whole file and never edits one of the
/// person's. The document is Claude Code's shape. Checked against Grok 1.0.44
/// on 2026-09-29:
///
/// - Grok **expands `$VAR` in a hook's command itself**, and refuses to run a
///   hook whose variables are unset. The command ket writes has no `$` in it.
/// - The payload carries Claude's snake_case keys — `hook_event_name` (with
///   Claude's spelling of the event), `session_id`, `tool_name`,
///   `tool_input`, `cwd` — beside Grok's own camelCase ones, but tool names
///   and a few fields are Grok's. [`grok_payload`] translates them.
/// - Grok **also runs Claude Code's hooks** from `~/.claude/settings.json`
///   unless told otherwise, so ket's Claude entry fires inside Grok too. The
///   script tells the two apart by the `grok` argument this entry passes; see
///   [`SCRIPT`](super::script::SCRIPT).
/// - A `SessionStart` hook's `additionalContext` does not reach the model, so
///   Grok is not handed the Economy note.
///
/// Inert on a machine without Grok: nothing is written unless its home
/// directory already exists.
pub struct GrokHooks {
    /// Grok's home, overridable so this is exercisable without one.
    home: PathBuf,
}

impl Default for GrokHooks {
    fn default() -> Self {
        let home = std::env::var_os("GROK_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home().join(".grok"), PathBuf::from);
        Self { home }
    }
}

impl GrokHooks {
    /// Hooks for a Grok whose home is `home`.
    pub fn at(home: PathBuf) -> Self {
        Self { home }
    }

    fn file(&self) -> PathBuf {
        self.home.join("hooks").join("ket.json")
    }
}

/// ket's hooks file for Grok, calling `script`.
fn grok_document(script: &Path) -> String {
    let command = format!(
        "/bin/sh {} grok 2>/dev/null || printf '{{}}\\n'",
        script.display()
    );
    let hooks: serde_json::Map<String, serde_json::Value> = HookEvent::GROK_EVENTS
        .into_iter()
        .map(|event| {
            let entry = serde_json::json!([{
                "hooks": [{ "type": "command", "command": command }],
            }]);
            (event.to_owned(), entry)
        })
        .collect();
    let document = serde_json::json!({ "hooks": hooks });
    serde_json::to_string_pretty(&document).unwrap_or_default() + "\n"
}

impl HookInstaller for GrokHooks {
    fn agent(&self) -> &str {
        "grok"
    }

    /// Whether the file is exactly what this version writes — or Grok is not
    /// here at all, in which case there is nothing to install.
    fn is_installed(&self) -> Result<bool> {
        if !self.home.is_dir() {
            return Ok(true);
        }
        let file = self.file();
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(text == grok_document(&script_path())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KetError::io(&file, e)),
        }
    }

    fn install(&self, script: &Path) -> Result<()> {
        if !self.home.is_dir() {
            return Ok(());
        }
        let file = self.file();
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
        }
        crate::config::write_atomically(&file, &grok_document(script), false)
    }

    fn place(&self) -> Place {
        Place::new("grok", &self.home)
    }

    fn has_any(&self) -> Result<bool> {
        let file = self.file();
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(text.contains(OWNER_MARK)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KetError::io(&file, e)),
        }
    }

    /// Deletes the file — only if it is ket's.
    fn remove(&self) -> Result<()> {
        let file = self.file();
        match std::fs::read_to_string(&file) {
            Ok(text) if text.contains(OWNER_MARK) => {
                std::fs::remove_file(&file).map_err(|e| KetError::io(&file, e))
            }
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KetError::io(&file, e)),
        }
    }
}

/// Copies `from` to `to` in `object` unless `to` is already there.
fn alias(object: &mut serde_json::Map<String, serde_json::Value>, from: &str, to: &str) {
    if !object.contains_key(to)
        && let Some(value) = object.get(from).cloned()
    {
        object.insert(to.to_owned(), value);
    }
}

/// A Grok payload in the shape everything downstream reads, or `None` when
/// it should be dropped.
///
/// Grok sends Claude's snake_case keys for the common fields but its own tool
/// names and a few camelCase-only fields. Translating once here means
/// [`tool_phrase`], [`permission_question`], [`closing_line`] and the rest
/// read a Grok report exactly as they read a Claude one.
///
/// Dropped: anything that fired inside a subagent (it carries
/// `subagentType`). A subagent's tools are not the pane's.
pub(super) fn grok_payload(payload: &serde_json::Value) -> Option<serde_json::Value> {
    if payload.get("subagentType").is_some() {
        return None;
    }
    let mut payload = payload.clone();
    let object = payload.as_object_mut()?;

    alias(object, "lastAssistantMessage", "last_assistant_message");
    alias(object, "notificationType", "notification_type");

    if let Some(tasks) = object.get("backgroundTasks").and_then(|t| t.as_array()) {
        let tasks: Vec<serde_json::Value> = tasks
            .iter()
            .map(|task| {
                let mut task = task.clone();
                if let Some(task) = task.as_object_mut() {
                    alias(task, "agentType", "agent_type");
                }
                task
            })
            .collect();
        object.insert("background_tasks".to_owned(), tasks.into());
    }

    let claude = object
        .get("tool_name")
        .and_then(|t| t.as_str())
        .and_then(grok_tool);
    if let Some(claude) = claude {
        object.insert("tool_name".to_owned(), claude.into());
    }
    if let Some(input) = object.get_mut("tool_input").and_then(|i| i.as_object_mut()) {
        grok_input(input);
    }

    Some(payload)
}

/// A Grok tool by Claude's name, or `None` for one with no Claude twin.
/// Shared with [`crate::conversation`], which reads Grok's transcript.
pub(crate) fn grok_tool(name: &str) -> Option<&'static str> {
    Some(match name {
        "run_terminal_command" => "Bash",
        "read_file" => "Read",
        "write" => "Write",
        "search_replace" => "Edit",
        "grep" => "Grep",
        "list_dir" => "Glob",
        "spawn_subagent" => "Task",
        "web_fetch" => "WebFetch",
        "web_search" => "WebSearch",
        _ => return None,
    })
}

/// A Grok tool's input with Claude's keys added beside Grok's own.
pub(crate) fn grok_input(input: &mut serde_json::Map<String, serde_json::Value>) {
    alias(input, "target_file", "file_path");
    alias(input, "target_directory", "pattern");
}
