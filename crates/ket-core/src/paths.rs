//! Where ket keeps things on disk.
//!
//! XDG conventions are honoured on every platform, macOS included. That is a
//! deliberate departure from Apple's `~/Library/Application Support`: ket is a
//! developer tool driven from a terminal, and `~/.config/ket/config.toml` is
//! where its user will look for a config file.
//!
//! Resolution is written against an injected environment lookup rather than
//! reading globals directly. Tests can then cover the fallback rules without
//! mutating process-wide state — which under a parallel test runner is a race,
//! and in edition 2024 requires `unsafe`, which this workspace forbids.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::{KetError, Result};

/// Reads an environment variable. Injected so resolution stays testable.
type Lookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// The real environment.
fn from_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Returns a variable only when it is set *and* non-empty.
///
/// An empty `XDG_CONFIG_HOME` genuinely happens, and treating it as "set"
/// produces paths rooted at `/`.
fn non_empty(lookup: Lookup<'_>, key: &str) -> Option<PathBuf> {
    lookup(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_with(lookup: Lookup<'_>) -> Result<PathBuf> {
    non_empty(lookup, "HOME").ok_or_else(|| KetError::Path {
        what: "home directory",
        why: "$HOME is unset or empty".to_owned(),
    })
}

fn config_dir_with(lookup: Lookup<'_>) -> Result<PathBuf> {
    match non_empty(lookup, "XDG_CONFIG_HOME") {
        Some(base) => Ok(base.join("ket")),
        None => Ok(home_with(lookup)?.join(".config").join("ket")),
    }
}

fn cache_dir_with(lookup: Lookup<'_>) -> Result<PathBuf> {
    match non_empty(lookup, "XDG_CACHE_HOME") {
        Some(base) => Ok(base.join("ket")),
        None => Ok(home_with(lookup)?.join(".cache").join("ket")),
    }
}

fn data_dir_with(lookup: Lookup<'_>) -> Result<PathBuf> {
    match non_empty(lookup, "XDG_DATA_HOME") {
        Some(base) => Ok(base.join("ket")),
        None => Ok(home_with(lookup)?.join(".local").join("share").join("ket")),
    }
}

/// The user's home directory.
pub fn home() -> Result<PathBuf> {
    home_with(&from_env)
}

/// Directory holding ket's configuration — `$XDG_CONFIG_HOME/ket`, else `~/.config/ket`.
pub fn config_dir() -> Result<PathBuf> {
    config_dir_with(&from_env)
}

/// Path to `config.toml`.
pub fn config_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Path to `keybindings.toml`.
///
/// Lives beside `config.toml` under [`config_dir`] rather than under
/// [`data_dir`], for the same reason a theme does: a keybinding only needs
/// to exist to *override* one of [`crate::keybinding::Keymap::builtin`]'s
/// entries, which makes it something a person edits, not state ket writes.
pub fn keybindings_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("keybindings.toml"))
}

/// Path to `snippets.toml`, the saved prompt snippets.
///
/// Its own file rather than a table in `config.toml`, which denies unknown
/// fields: an older ket would refuse a config that carried snippets. Beside
/// it under [`config_dir`] because a snippet is something a person writes.
pub fn snippets_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("snippets.toml"))
}

/// Directory for things ket generates and can regenerate.
///
/// Separate from [`data_dir`] because the distinction is what protects the
/// latter. Everything under the data directory is the owner's — their projects,
/// their worktrees, their journal — and anything ket writes there is something
/// they may have to be told about. A generated shell wrapper is not that: it is
/// derived entirely from the build, deleting it costs nothing, and a test that
/// writes one must not have to be trusted with the owner's state to do it.
pub fn cache_dir() -> Result<PathBuf> {
    cache_dir_with(&from_env)
}

/// Directory holding ket's durable state — `$XDG_DATA_HOME/ket`, else `~/.local/share/ket`.
pub fn data_dir() -> Result<PathBuf> {
    data_dir_with(&from_env)
}

/// The default root under which per-project worktrees are created.
///
/// Settings can name another — see `Config::worktrees_dir`, which is the one
/// to ask where a new worktree goes.
///
/// Layout is `<data_dir>/worktrees/<project-slug>/<worktree-slug>/`, keeping each
/// project's worktrees separate.
///
/// The final component is a *slug*, never the raw branch name: branch names
/// contain `/`, so `feature/login` would silently nest a directory and collide
/// with a branch named `feature`. The real branch name lives in worktree
/// metadata instead.
pub fn worktrees_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("worktrees"))
}

/// Directory holding each project's backlog — see [`crate::backlog`].
pub fn backlog_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("backlog"))
}

/// Directory holding user-defined themes — `<config_dir>/themes`.
///
/// A theme only needs to exist to override the shipped dark or light default,
/// which is why it lives under config rather than data: it is something a
/// person edits, not state ket writes.
pub fn themes_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("themes"))
}

/// Path to a named theme file — `<themes_dir>/<name>.toml`.
pub fn theme_file(name: &str) -> Result<PathBuf> {
    Ok(themes_dir()?.join(format!("{name}.toml")))
}

/// The append-only event journal — `<data_dir>/events.jsonl`.
///
/// Shared by every `ket` process on the machine, which is what lets
/// `ket events --follow` watch a `ket run` happening in another terminal. See
/// [`crate::journal`].
pub fn events_file() -> Result<PathBuf> {
    Ok(data_dir()?.join("events.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a fake environment from `(key, value)` pairs.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| {
            owned
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn xdg_config_home_wins_when_set() {
        let e = env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/me")]);
        assert_eq!(config_dir_with(&e).unwrap(), PathBuf::from("/xdg/ket"));
    }

    #[test]
    fn empty_xdg_var_falls_back_to_home() {
        let e = env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/me")]);
        assert_eq!(
            config_dir_with(&e).unwrap(),
            PathBuf::from("/home/me/.config/ket")
        );
    }

    #[test]
    fn data_dir_falls_back_under_local_share() {
        let e = env(&[("HOME", "/home/me")]);
        assert_eq!(
            data_dir_with(&e).unwrap(),
            PathBuf::from("/home/me/.local/share/ket")
        );
    }

    #[test]
    fn missing_home_is_an_error_not_a_panic() {
        let e = env(&[]);
        assert!(matches!(data_dir_with(&e), Err(KetError::Path { .. })));
    }

    #[test]
    fn empty_home_is_treated_as_missing() {
        let e = env(&[("HOME", "")]);
        assert!(matches!(config_dir_with(&e), Err(KetError::Path { .. })));
    }
}
