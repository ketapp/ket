//! Claude Code's hooks: ket's entries in its `settings.json`.

use std::path::{Path, PathBuf};

use super::places::Place;
use super::settings::{has_kets_entry, is_kets, read_settings, write_settings};
use super::{HookEvent, HookInstaller};
use crate::{KetError, Result};

// ---- claude -----------------------------------------------------------------

/// Claude Code: hook commands in `~/.claude/settings.json`.
pub struct ClaudeHooks {
    /// The settings file, overridable so this is exercisable without a home.
    settings: PathBuf,
}

impl Default for ClaudeHooks {
    fn default() -> Self {
        Self {
            settings: crate::sessions::claude_config_dir().join("settings.json"),
        }
    }
}

impl ClaudeHooks {
    /// Hooks written into a specific settings file.
    pub fn at(settings: PathBuf) -> Self {
        Self { settings }
    }

    /// The settings document, or an empty one when there is no file yet.
    fn read(&self) -> Result<serde_json::Value> {
        read_settings(&self.settings)
    }

    /// Writes the document back, keeping a copy of what was there before.
    fn write(&self, document: &serde_json::Value) -> Result<()> {
        write_settings(&self.settings, document)
    }
}

impl HookInstaller for ClaudeHooks {
    fn agent(&self) -> &str {
        "claude"
    }

    /// Whether ket's entries are there *and* are the ones this version asks
    /// for.
    ///
    /// Not "is there a ket entry anywhere": that answer made the event list a
    /// one-time write. Someone who had ever run ket kept whatever set the ket
    /// that ran first installed, so a later version could add an event — a
    /// permission prompt, a session ending — and never hear about it on the
    /// only machines that mattered. The script is rewritten on every install
    /// for exactly this reason; the entries that point at it were the half
    /// that got left behind.
    fn is_installed(&self) -> Result<bool> {
        let document = self.read()?;
        let Some(hooks) = document.get("hooks").and_then(|h| h.as_object()) else {
            return Ok(false);
        };

        let ours: std::collections::BTreeSet<&str> = hooks
            .iter()
            .filter(|(event, entries)| {
                entries.as_array().is_some_and(|list| {
                    list.iter().any(|entry| {
                        // An entry from before prompts were held open has
                        // Claude's own timeout, which would cut the wait short.
                        is_kets(entry)
                            && (event.as_str() != "PermissionRequest"
                                || entry
                                    .pointer("/hooks/0/timeout")
                                    .and_then(serde_json::Value::as_u64)
                                    == Some(PROMPT_WAIT_SECS))
                    })
                })
            })
            .map(|(event, _)| event.as_str())
            .collect();

        Ok(ours == HookEvent::CLAUDE_EVENTS.into_iter().collect())
    }

    fn has_any(&self) -> Result<bool> {
        Ok(has_kets_entry(&self.read()?))
    }

    fn place(&self) -> Place {
        Place::new("claude", &self.settings)
    }

    fn install(&self, script: &Path) -> Result<()> {
        let mut document = self.read()?;

        // Anything already there stays: a settings file with a user's own
        // hooks in it must come out of this with those hooks still in it.
        let root = document
            .as_object_mut()
            .ok_or_else(|| KetError::Config("settings.json is not an object".to_owned()))?;
        let hooks = root
            .entry("hooks")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| KetError::Config("settings.json: hooks is not an object".to_owned()))?;

        for event in HookEvent::CLAUDE_EVENTS {
            let entries = hooks
                .entry(event)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| {
                    KetError::Config(format!("settings.json: hooks.{event} is not an array"))
                })?;

            // Idempotent: installing twice leaves one entry, not two.
            entries.retain(|entry| !is_kets(entry));
            entries.push(kets_entry(event, script));
        }

        // An event an earlier ket asked for and this one does not. Left alone
        // it would keep firing the script forever, which is a subprocess per
        // event for a report nothing reads.
        for (event, entries) in hooks.iter_mut() {
            if HookEvent::CLAUDE_EVENTS.contains(&event.as_str()) {
                continue;
            }
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        self.write(&document)
    }

    fn remove(&self) -> Result<()> {
        let mut document = self.read()?;
        let Some(hooks) = document
            .as_object_mut()
            .and_then(|root| root.get_mut("hooks"))
            .and_then(|hooks| hooks.as_object_mut())
        else {
            return Ok(());
        };

        for entries in hooks.values_mut() {
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        // An event ket emptied goes with it, so removing leaves no trace.
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        self.write(&document)
    }
}

/// How long Claude lets a `PermissionRequest` hook run, in seconds: the
/// longest a question or a plan can wait on a phone. The script's own wait is
/// a little shorter, so it answers `{}` before Claude gives up on it.
pub const PROMPT_WAIT_SECS: u64 = 3600;

/// The events that take a `matcher`.
///
/// Only the tool-shaped ones. An entry carrying a matcher on any of the others
/// is rejected by Claude Code's settings validation, and a rejected entry is a
/// hook that silently never runs — which is exactly what happened when ket
/// wrote one onto every event it asked for.
const MATCHER_EVENTS: [&str; 5] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
];

/// One hook entry, in the shape Claude Code expects.
///
/// Deliberately no ownership key: an entry is `hooks`, plus `matcher` where
/// the event allows one, and nothing else. ket marks its entries by the script
/// they call rather than by an extra field, because an unknown field is
/// another way to have the entry thrown out.
fn kets_entry(event: &str, script: &Path) -> serde_json::Value {
    let mut hook = serde_json::json!({
        "type": "command",
        "command": format!("/bin/sh {} 2>/dev/null || printf '{{}}\\n'", script.display()),
    });
    // A question or a plan holds its hook open until a phone answers it —
    // see `Waiters` — and Claude's own ten minutes is shorter than a walk.
    if event == "PermissionRequest" {
        hook["timeout"] = serde_json::json!(PROMPT_WAIT_SECS);
    }
    let hooks = serde_json::json!([hook]);

    if MATCHER_EVENTS.contains(&event) {
        serde_json::json!({ "matcher": "*", "hooks": hooks })
    } else {
        serde_json::json!({ "hooks": hooks })
    }
}
