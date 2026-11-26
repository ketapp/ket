//! The JSON settings files ket writes its hooks into, and how its own
//! entries in them are told from everyone else's.

use std::path::Path;

use crate::{KetError, Result};

/// One agent's settings document, or an empty one when there is no file yet.
pub(super) fn read_settings(path: &Path) -> Result<serde_json::Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| KetError::Config(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Value::Object(serde_json::Map::new()))
        }
        Err(e) => Err(KetError::io(path, e)),
    }
}

/// Writes a settings document back, keeping a copy of what was there before.
///
/// The backup is the whole point of the ceremony: this file is the user's, it
/// is large, and ket is a guest in it.
pub(super) fn write_settings(path: &Path, document: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
    }

    if path.exists() {
        let backup = path.with_extension("json.ket-backup");
        std::fs::copy(path, &backup).map_err(|e| KetError::io(&backup, e))?;
    }

    let text = serde_json::to_string_pretty(document)
        .map_err(|e| KetError::Config(format!("could not serialise settings: {e}")))?;
    std::fs::write(path, text).map_err(|e| KetError::io(path, e))
}

/// How ket recognises the hook entries it owns.
///
/// Matched against the *command*, not against a field of its own: the entry
/// has to be exactly the shape Claude Code expects, and the script's name is
/// already unique enough to identify. Kept as a key check too, so an entry
/// written by an earlier ket that did add a field is still removable.
pub const OWNER_MARK: &str = "ket-agent-hook";

/// Whether a hooks document — Claude's settings or Codex's `hooks.json` —
/// holds any entry of ket's, under any event.
pub(super) fn has_kets_entry(document: &serde_json::Value) -> bool {
    document
        .get("hooks")
        .and_then(|hooks| hooks.as_object())
        .is_some_and(|hooks| {
            hooks
                .values()
                .filter_map(|entries| entries.as_array())
                .flatten()
                .any(is_kets)
        })
}

/// Whether an entry is one ket added.
pub(super) fn is_kets(entry: &serde_json::Value) -> bool {
    entry.get(OWNER_MARK).is_some()
        || entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|command| command.contains("ket-agent-hook"))
                })
            })
}
