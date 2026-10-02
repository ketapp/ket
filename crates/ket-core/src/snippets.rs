//! Prompt snippets: named blocks of text saved to drop into any prompt.
//!
//! They live in `snippets.toml` under the config directory (see
//! [`paths::snippets_file`]), one `[[snippet]]` table each:
//!
//! ```toml
//! [[snippet]]
//! name = "Review"
//! body = """
//! Review the diff for correctness.
//! """
//! ```
//!
//! Until that file exists, [`Snippets::load`] offers [`STARTERS`].

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::write_atomically;
use crate::{KetError, Result, paths};

/// What a fresh install offers before anything is saved: phrases the Claude
/// Code team and heavy users send their agents over and over, none of them a
/// built-in command. There to keep, change or delete; any change saves the
/// whole list, so they never come back once one is made.
pub const STARTERS: &[(&str, &str)] = &[
    ("Commit and merge", "Commit and merge to main."),
    ("Open a PR", "Commit, push and open a pull request."),
    ("Prove it works", "Prove to me this works."),
    (
        "Grill me",
        "Grill me on these changes and don't make a PR until I pass your test.",
    ),
    (
        "Ask me first",
        "Before you start, ask me about anything unclear.",
    ),
    ("Use subagents", "Use subagents."),
    (
        "Ultrathink",
        "Ultrathink and take your time. Use parallel subagents freely and look online if you need to.",
    ),
];

/// One saved snippet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Snippet {
    /// What menus call it. Unique, ignoring case.
    pub name: String,
    /// The text it puts into a prompt.
    pub body: String,
}

/// Every saved snippet, in the order the person arranged them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Snippets {
    /// The snippets themselves.
    #[serde(rename = "snippet")]
    pub items: Vec<Snippet>,
}

impl Snippets {
    /// Loads the snippets from the default location, or [`STARTERS`] when
    /// nothing has been saved there yet.
    pub fn load() -> Result<Self> {
        let path = paths::snippets_file()?;
        if !path.exists() {
            return Ok(Self::starters());
        }
        Self::load_from(&path)
    }

    /// [`STARTERS`] as snippets.
    pub fn starters() -> Self {
        Self {
            items: STARTERS
                .iter()
                .map(|(name, body)| Snippet {
                    name: (*name).to_owned(),
                    body: (*body).to_owned(),
                })
                .collect(),
        }
    }

    /// Loads snippets from a specific path.
    ///
    /// A missing file is no snippets; a malformed one is an error, for the
    /// same reason as [`crate::config::Config::load_from`].
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str::<Self>(&text)
                .map_err(|e| KetError::Config(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(KetError::io(path, e)),
        }
    }

    /// Persists the snippets at the path [`Snippets::load`] reads.
    pub fn save(&self) -> Result<()> {
        self.save_to(&paths::snippets_file()?)
    }

    /// Persists the snippets at an explicit path, atomically.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|e| KetError::Config(e.to_string()))?;
        write_atomically(path, &text, false)
    }

    /// Checks every snippet has a name no other shares, and a body.
    pub fn validate(&self) -> Result<()> {
        for (i, snippet) in self.items.iter().enumerate() {
            Self::check(&self.items, Some(i), &snippet.name, &snippet.body)
                .map_err(KetError::Config)?;
        }
        Ok(())
    }

    /// Whether a snippet called `name` with `body` could take slot `at` in
    /// `items` (or join them, when `at` is `None`), and why not, in a sentence
    /// fit to show beside the field, if it could not.
    pub fn check(
        items: &[Snippet],
        at: Option<usize>,
        name: &str,
        body: &str,
    ) -> std::result::Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A snippet needs a name.".to_owned());
        }
        if body.trim().is_empty() {
            return Err(format!("{name} has nothing in it to send."));
        }
        let taken = items
            .iter()
            .enumerate()
            .any(|(i, other)| Some(i) != at && other.name.trim().eq_ignore_ascii_case(name));
        if taken {
            return Err(format!("There is already a snippet called {name}."));
        }
        Ok(())
    }
}
