//! Claude Code's status line: ket's script, chained in front of whatever
//! status line the person already had.

use std::path::{Path, PathBuf};

use super::home;
use super::places::{Place, STATUSLINE_PLACE};
use super::settings::{read_settings, write_settings};
use crate::{KetError, Result};

// ---- the status line --------------------------------------------------------

/// Claude Code's status line, which is where its rate limits are published.
///
/// Quota is the one thing Claude Code will not hand over cheaply. There is no
/// file to read and no command to ask, so [`crate::rate_limits`] gets it by
/// starting a *real, billed* session in a scratch directory and reading one
/// frame — every five minutes, for as long as ket is open.
///
/// The status line carries the same numbers for nothing. Claude renders it by
/// running a command of the user's choosing and handing it a JSON payload on
/// stdin, and that payload has a `rate_limits` block in it. A session that is
/// already on screen pays for those numbers whether or not anybody reads them.
///
/// **This one is not a merge, and that is the whole difficulty.** `hooks` is a
/// map that ket can add its own entry to and leave everyone else's alone.
/// `statusLine` is a *single command*, so installing ket's replaces whatever
/// was there — and on this machine what is there belongs to another
/// application. So ket does not replace it: it captures it into
/// [`statusline_chain_path`] and its own script runs it, feeds it the same
/// payload, and passes its output through as the status line. Removing ket's
/// puts the captured command back exactly.
///
/// Two things this deliberately will not do:
///
/// - **Capture its own script as the chained command.** A ket that installed
///   over a ket would otherwise build a script that runs itself, forever.
/// - **Speak for a session it did not launch.** The payload is only posted
///   when [`PORT_ENV`](super::PORT_ENV) and [`TOKEN_ENV`](super::TOKEN_ENV) are in the environment, which is true
///   for panes ket opened and nothing else. Quota is per account, so one
///   reporting session answers for all of them.
pub struct ClaudeStatusLine {
    /// The settings file, overridable so this is exercisable without a home.
    settings: PathBuf,
    /// Where the command ket displaced is kept, so it can be given back.
    chain: PathBuf,
}

impl Default for ClaudeStatusLine {
    fn default() -> Self {
        Self {
            settings: crate::sessions::claude_config_dir().join("settings.json"),
            chain: statusline_chain_path(),
        }
    }
}

impl ClaudeStatusLine {
    /// A status line written into specific files.
    pub fn at(settings: PathBuf, chain: PathBuf) -> Self {
        Self { settings, chain }
    }

    /// Where this status line goes, as [`record_installed`](super::record_installed) notes it.
    pub fn place(&self) -> Place {
        Place::new(STATUSLINE_PLACE, &self.settings)
    }

    /// The command currently configured, if there is one.
    fn configured(&self) -> Result<Option<String>> {
        let document = read_settings(&self.settings)?;
        Ok(document
            .get("statusLine")
            .and_then(|line| line.get("command"))
            .and_then(|c| c.as_str())
            .map(str::to_owned))
    }

    /// Whether ket's own script is what Claude is running.
    pub fn is_installed(&self) -> Result<bool> {
        Ok(self
            .configured()?
            .is_some_and(|command| command.contains(STATUSLINE_MARK)))
    }

    /// Points Claude's status line at `script`, keeping whatever it displaced.
    pub fn install(&self, script: &Path) -> Result<()> {
        let existing = self.configured()?;

        // Written before the settings change, not after: if this fails, the
        // command it would have recorded is still the one Claude runs.
        match &existing {
            // Already ket's — the chain file behind it is the real previous
            // command and must not be overwritten with a pointer to ket.
            Some(command) if command.contains(STATUSLINE_MARK) => {}
            Some(command) => {
                if let Some(parent) = self.chain.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| KetError::io(parent, e))?;
                }
                std::fs::write(&self.chain, command).map_err(|e| KetError::io(&self.chain, e))?;
            }
            // Nothing displaced, so nothing to give back. An old chain file
            // from a previous install would be a command Claude no longer has.
            None => {
                let _ = std::fs::remove_file(&self.chain);
            }
        }

        let mut document = read_settings(&self.settings)?;
        let root = document
            .as_object_mut()
            .ok_or_else(|| KetError::Config("settings.json is not an object".to_owned()))?;
        root.insert(
            "statusLine".to_owned(),
            serde_json::json!({
                "type": "command",
                "command": format!("/bin/sh {} 2>/dev/null", script.display()),
            }),
        );

        write_settings(&self.settings, &document)
    }

    /// Gives the displaced command back, or takes the status line away
    /// entirely when ket displaced nothing.
    pub fn remove(&self) -> Result<()> {
        self.give_back()?;
        let _ = std::fs::remove_file(&self.chain);
        Ok(())
    }

    /// [`remove`](Self::remove), keeping the chain file for another
    /// directory's line to be given back from.
    pub(super) fn give_back(&self) -> Result<()> {
        if !self.is_installed()? {
            return Ok(());
        }

        let mut document = read_settings(&self.settings)?;
        let Some(root) = document.as_object_mut() else {
            return Ok(());
        };

        match std::fs::read_to_string(&self.chain) {
            Ok(command) if !command.trim().is_empty() => {
                root.insert(
                    "statusLine".to_owned(),
                    serde_json::json!({ "type": "command", "command": command }),
                );
            }
            _ => {
                root.remove("statusLine");
            }
        }

        write_settings(&self.settings, &document)
    }
}

/// How ket recognises its own status line command.
pub const STATUSLINE_MARK: &str = "ket-statusline";

/// Where ket keeps the status line script.
pub fn statusline_script_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("ket-statusline.sh")
}

/// Where the command ket displaced is kept.
pub fn statusline_chain_path() -> PathBuf {
    home()
        .join(".ket")
        .join("agent-hooks")
        .join("statusline-chain")
}

/// Writes the status line script, creating its directory.
pub fn install_statusline_script() -> Result<PathBuf> {
    let path = statusline_script_path();
    let dir = path
        .parent()
        .ok_or_else(|| KetError::Config("status line script has no directory".to_owned()))?;
    std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    std::fs::write(&path, STATUSLINE_SCRIPT).map_err(|e| KetError::io(&path, e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)
            .map_err(|e| KetError::io(&path, e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).map_err(|e| KetError::io(&path, e))?;
    }

    Ok(path)
}

/// The script Claude runs to draw its status line.
///
/// Its output *is* the status line, which makes the failure mode here worse
/// than the hook script's: a hook that misbehaves is a report ket does not
/// get, and this is a line the person is looking at. So the chained command's
/// stdout is passed through untouched and everything ket does happens off to
/// the side of it, after the payload has been read and before the chain runs.
///
/// There is no spool. A status line is drawn again a second later; a dropped
/// one costs nothing and a queue of stale quota readings is worse than none.
pub(super) const STATUSLINE_SCRIPT: &str = r#"#!/bin/sh
# ket-statusline — installed by ket. Reads Claude's status line payload for
# the rate limits in it, then runs whatever command ket displaced and prints
# its output, which is what you see.
#
# Removing ket's `statusLine` from the agent's settings uninstalls this; the
# file itself is inert without it.

payload=$(cat 2>/dev/null || printf '')
if [ -z "$payload" ]; then exit 0; fi

escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# Only for a session ket launched: without the port and the token there is
# nowhere to send this and nothing that would believe it.
if [ -n "${KET_HOOK_PORT:-}" ] && [ -n "${KET_LAUNCH_TOKEN:-}" ] && command -v curl >/dev/null 2>&1; then
  envelope=$(printf '{"agent":"%s","worktree":"%s","pane":"%s","token":"%s","version":"%s","payload":%s}' \
    "$(escape "${KET_AGENT_NAME:-claude}")" \
    "$(escape "${KET_WORKTREE_ID:-}")" \
    "$(escape "${KET_PANE_KEY:-}")" \
    "$(escape "${KET_LAUNCH_TOKEN:-}")" \
    "$(escape "${KET_HOOK_VERSION:-1}")" \
    "$payload")
  printf '%s' "$envelope" | curl -sS -o /dev/null \
    --connect-timeout 1 --max-time 1 \
    -H 'Content-Type: application/json' \
    --data-binary @- \
    "http://127.0.0.1:${KET_HOOK_PORT}/statusline" 2>/dev/null || :
fi

# Whatever was drawing this line before ket arrived still draws it.
chain="${HOME:-}/.ket/agent-hooks/statusline-chain"
if [ -n "${HOME:-}" ] && [ -s "$chain" ]; then
  command=$(cat "$chain" 2>/dev/null || printf '')
  if [ -n "$command" ]; then
    printf '%s' "$payload" | /bin/sh -c "$command" 2>/dev/null || :
  fi
fi

exit 0
"#;
