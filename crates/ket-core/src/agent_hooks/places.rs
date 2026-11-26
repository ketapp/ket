//! Where ket installed its hooks, kept on record so every one of them can
//! be taken out again.

use std::path::{Path, PathBuf};

use super::claude::ClaudeHooks;
use super::codex::CodexHooks;
use super::grok::GrokHooks;
use super::opencode::OpenCodeHooks;
use super::statusline::{ClaudeStatusLine, statusline_chain_path};
use super::{HookInstaller, home};
use crate::{KetError, Result};

/// What taking ket out of one agent did — see [`remove_everywhere`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// Ket's entries were there, and are gone.
    Removed,
    /// There were none to take out.
    NotThere,
    /// The agent's settings could not be read or parsed, so whether ket is
    /// in them is not known, and say why.
    Unreadable(String),
    /// Ket's entries were there and could not be taken out, and say why.
    Failed(String),
}

impl Removal {
    /// Whether this place may still have ket in it, for a second try.
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Unreadable(_) | Self::Failed(_))
    }
}

/// One place ket was taken out of, and how that went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// The agent, as [`HookInstaller::agent`] names it, or `claude` for its
    /// status line.
    pub agent: String,
    /// Which of its settings: a Claude config directory other than the usual
    /// one, or the status line. `None` for the agent's only one.
    pub detail: Option<String>,
    /// What happened there.
    pub outcome: Removal,
}

/// Takes ket out of every agent it puts itself into, for uninstalling it:
/// its hooks from each Claude config directory, Codex, OpenCode and Grok,
/// and its script from in front of Claude's status line, which gets back
/// whatever it displaced. Only ket's own entries go — everything else in
/// those files stays as it was.
///
/// Looked for where this process's environment says each agent keeps its
/// settings, and wherever [`record_installed`] noted the window put them:
/// Homebrew runs the uninstall with `CLAUDE_CONFIG_DIR`, `XDG_CONFIG_HOME`
/// and `GROK_HOME` filtered out, and the window may have had any of them.
///
/// A ket still running puts them back the next time it starts; this is for
/// the last thing done before it is deleted.
pub fn remove_everywhere() -> Vec<Removed> {
    let recorded = read_places(&installed_record_path());
    let usual = crate::sessions::claude_config_dir();
    let dirs = crate::sessions::claude_config_dirs();
    let mut installers: Vec<(Box<dyn HookInstaller>, Option<String>)> = Vec::new();
    for dir in &dirs {
        let detail = (*dir != usual).then(|| dir.display().to_string());
        installers.push((Box::new(ClaudeHooks::at(dir.join("settings.json"))), detail));
    }
    installers.push((Box::new(CodexHooks::default()), None));
    installers.push((Box::new(OpenCodeHooks::default()), None));
    installers.push((Box::new(GrokHooks::default()), None));
    for place in &recorded {
        if installers.iter().any(|(known, _)| known.place() == *place) {
            continue;
        }
        if let Some(installer) = place.installer() {
            installers.push((installer, Some(place.at.display().to_string())));
        }
    }

    let mut removed: Vec<Removed> = installers
        .iter()
        .map(|(installer, detail)| Removed {
            agent: installer.agent().to_owned(),
            detail: detail.clone(),
            outcome: match installer.has_any() {
                Ok(false) => Removal::NotThere,
                Ok(true) => match installer.remove() {
                    Ok(()) => Removal::Removed,
                    Err(why) => Removal::Failed(why.to_string()),
                },
                Err(why) => Removal::Unreadable(why.to_string()),
            },
        })
        .collect();

    // Every directory is looked in, and one other than the usual is only
    // mentioned when ket's line was there. There is one chain file for all
    // of them — a window that moved directories left its line in both — so
    // it goes once they are all done, and each gets the command back rather
    // than the second none.
    let mut status_settings = vec![usual.join("settings.json")];
    let recorded_lines = recorded
        .iter()
        .filter(|place| place.kind == STATUSLINE_PLACE)
        .map(|place| place.at.clone());
    for settings in dirs
        .iter()
        .map(|dir| dir.join("settings.json"))
        .chain(recorded_lines)
    {
        if !status_settings.contains(&settings) {
            status_settings.push(settings);
        }
    }
    for (index, settings) in status_settings.into_iter().enumerate() {
        let status_line = ClaudeStatusLine::at(settings.clone(), statusline_chain_path());
        let outcome = match status_line.is_installed() {
            Ok(false) => Removal::NotThere,
            Ok(true) => match status_line.give_back() {
                Ok(()) => Removal::Removed,
                Err(why) => Removal::Failed(why.to_string()),
            },
            Err(why) => Removal::Unreadable(why.to_string()),
        };
        if index > 0 && outcome == Removal::NotThere {
            continue;
        }
        let dir = settings.parent().unwrap_or(&settings);
        removed.push(Removed {
            agent: "claude".to_owned(),
            detail: Some(if index == 0 {
                "status line".to_owned()
            } else {
                format!("status line in {}", dir.display())
            }),
            outcome,
        });
    }

    // Everything recorded has been looked at: the record goes with it, unless
    // something could not be changed and a second try needs to find it.
    if !removed.iter().any(|entry| entry.outcome.is_failure()) {
        let _ = std::fs::remove_file(statusline_chain_path());
        let _ = std::fs::remove_file(installed_record_path());
    }
    removed
}

/// One place ket puts itself, as the records in `~/.ket/agent-hooks` name
/// it: what kind of install, and the file or directory it went into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// `claude`, `codex`, `opencode` or `grok` for that agent's hooks, or
    /// [`STATUSLINE_PLACE`] for Claude's status line.
    pub kind: String,
    /// The settings file, OpenCode's plugin, or Grok's home.
    pub at: PathBuf,
}

/// The [`Place::kind`] of Claude's status line.
pub const STATUSLINE_PLACE: &str = "statusline";

impl Place {
    pub(super) fn new(kind: &str, at: &Path) -> Self {
        Self {
            kind: kind.to_owned(),
            at: at.to_path_buf(),
        }
    }

    /// The installer for these hooks, or `None` for the status line and for
    /// a kind this version does not know.
    fn installer(&self) -> Option<Box<dyn HookInstaller>> {
        let at = self.at.clone();
        Some(match self.kind.as_str() {
            "claude" => Box::new(ClaudeHooks::at(at)),
            "codex" => Box::new(CodexHooks::at(at)),
            "opencode" => Box::new(OpenCodeHooks::at(at)),
            "grok" => Box::new(GrokHooks::at(at)),
            _ => return None,
        })
    }
}

/// Where [`record_installed`] keeps the places the window put ket.
pub fn installed_record_path() -> PathBuf {
    home().join(".ket").join("agent-hooks").join("installed")
}

/// The places in a record: one per line, kind and path apart by a tab.
fn read_places(path: &Path) -> Vec<Place> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (kind, at) = line.split_once('\t')?;
            (!kind.is_empty() && !at.is_empty()).then(|| Place::new(kind, Path::new(at)))
        })
        .collect()
}

fn write_places(path: &Path, places: &[Place]) -> Result<()> {
    let text: String = places
        .iter()
        .filter_map(|place| {
            let at = place.at.to_str()?;
            (!at.contains(['\t', '\n'])).then(|| format!("{}\t{at}\n", place.kind))
        })
        .collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| KetError::io(dir, e))?;
    }
    std::fs::write(path, text).map_err(|e| KetError::io(path, e))
}

/// Notes that ket is in `place`, so [`remove_everywhere`] finds it from an
/// environment that would look elsewhere. The window calls this for each
/// place it installs into or finds itself already in.
pub fn record_installed(place: Place) -> Result<()> {
    let path = installed_record_path();
    let mut places = read_places(&path);
    if places.contains(&place) {
        return Ok(());
    }
    places.push(place);
    write_places(&path, &places)
}
