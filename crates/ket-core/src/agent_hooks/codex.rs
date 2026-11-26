//! Codex's hooks.

use std::path::{Path, PathBuf};

use super::places::Place;
use super::settings::{has_kets_entry, is_kets, read_settings, write_settings};
use super::{HookEvent, HookInstaller, home};
use crate::{KetError, Result};

// ---- codex ------------------------------------------------------------------

/// Codex: hook commands in `~/.codex/hooks.json`.
///
/// A separate file from Claude's `settings.json`, holding only hooks, but the
/// same document shape inside it — `hooks` → event name → a list of groups,
/// each with its own `hooks` array of `{type, command}`. So this differs from
/// [`ClaudeHooks`](super::ClaudeHooks) in three things and no more: which file, which event names
/// (see [`HookEvent::CODEX_EVENTS`]), and no `matcher` key.
///
/// **No matcher on any event.** Claude Code validates matchers and rejects an
/// entry carrying one on an event that does not take it — the reason
/// `MATCHER_EVENTS` exists. Codex's documented shape allows a matcher, but
/// the working install already on this machine, written by another tool, omits
/// it on all eight events it registers, `PreToolUse` included. Following what
/// is demonstrably accepted beats following what is documented as allowed, and
/// a matcher ket does not need is a way for an entry to be thrown out silently.
///
/// **This file belongs to whoever else is in it.** On the machine this was
/// written against, another tool already had all eight of its own hooks
/// installed here. Everything in [`ClaudeHooks`](super::ClaudeHooks)'s merge — keep what is there,
/// replace only ket's own entries, remove ket's entries from events ket no
/// longer asks for, drop an event only once it is empty — matters more here,
/// not less.
pub struct CodexHooks {
    /// The hooks file, overridable so this is exercisable without a home.
    settings: PathBuf,
}

impl Default for CodexHooks {
    fn default() -> Self {
        Self {
            settings: home().join(".codex").join("hooks.json"),
        }
    }
}

impl CodexHooks {
    /// Hooks written into a specific file.
    pub fn at(settings: PathBuf) -> Self {
        Self { settings }
    }
}

impl HookInstaller for CodexHooks {
    fn agent(&self) -> &str {
        "codex"
    }

    fn is_installed(&self) -> Result<bool> {
        let document = read_settings(&self.settings)?;
        let Some(hooks) = document.get("hooks").and_then(|hooks| hooks.as_object()) else {
            return Ok(false);
        };

        let ours: std::collections::BTreeSet<&str> = hooks
            .iter()
            .filter(|(_, entries)| {
                entries.as_array().is_some_and(|list| {
                    list.iter().any(|entry| {
                        is_kets(entry)
                            && entry
                                .get("hooks")
                                .and_then(|hooks| hooks.as_array())
                                .is_some_and(|hooks| {
                                    hooks.iter().any(|hook| {
                                        hook.get("command")
                                            .and_then(|command| command.as_str())
                                            .is_some_and(|command| {
                                                command.contains(" codex 2>/dev/null")
                                            })
                                    })
                                })
                    })
                })
            })
            .map(|(event, _)| event.as_str())
            .collect();

        Ok(ours == HookEvent::CODEX_EVENTS.into_iter().collect())
    }

    fn install(&self, script: &Path) -> Result<()> {
        let mut document = read_settings(&self.settings)?;

        let root = document
            .as_object_mut()
            .ok_or_else(|| KetError::Config("hooks.json is not an object".to_owned()))?;
        let hooks = root
            .entry("hooks")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| KetError::Config("hooks.json: hooks is not an object".to_owned()))?;

        for event in HookEvent::CODEX_EVENTS {
            let entries = hooks
                .entry(event)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| {
                    KetError::Config(format!("hooks.json: hooks.{event} is not an array"))
                })?;

            entries.retain(|entry| !is_kets(entry));
            entries.push(serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": format!("/bin/sh {} codex 2>/dev/null || printf '{{}}\\n'", script.display()),
                }]
            }));
        }

        for (event, entries) in hooks.iter_mut() {
            if HookEvent::CODEX_EVENTS.contains(&event.as_str()) {
                continue;
            }
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|entry| !is_kets(entry));
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        write_settings(&self.settings, &document)
    }

    fn has_any(&self) -> Result<bool> {
        Ok(has_kets_entry(&read_settings(&self.settings)?))
    }

    fn place(&self) -> Place {
        Place::new("codex", &self.settings)
    }

    fn remove(&self) -> Result<()> {
        let mut document = read_settings(&self.settings)?;
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
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));

        write_settings(&self.settings, &document)
    }
}
