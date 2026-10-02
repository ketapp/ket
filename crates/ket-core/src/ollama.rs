//! Which models a local Ollama install has pulled.
//!
//! Not a [`crate::agents::Prober`]: that trait answers "is this binary on
//! `PATH`", and a local model is a different kind of question — Ollama the
//! binary can be installed with its daemon not running, or running with
//! nothing pulled, and a settings screen needs to tell those two apart. So
//! this is a second, parallel detection step [`crate::agents::Catalogue::detect`]
//! runs after the ordinary candidate table, feeding in the same
//! [`crate::agents::Prober`] it was given rather than inventing its own way
//! to find the `ollama` binary.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

use crate::agents::Prober;

/// How long the request to Ollama's own daemon may take before it is
/// treated as not running — mirrors [`crate::agents::PROBE_TIMEOUT`], since
/// this is the same kind of "don't hang the window on a dead local service"
/// guard.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// One model `ollama list` would show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModel {
    /// The tag `ollama run` takes, e.g. `qwen2.5-coder:7b`.
    pub tag: String,
}

/// What asking a local Ollama found, in the detail a settings screen needs
/// to explain itself: "not installed" and "installed but not running" read
/// very differently to someone deciding what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The `ollama` binary is not on `PATH`.
    NotInstalled,
    /// Installed, but nothing answered on its usual port.
    NotRunning,
    /// Installed and running, with every model it has pulled — possibly
    /// none.
    Models(Vec<OllamaModel>),
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
}

/// Asks what a local Ollama has, in full detail.
///
/// Cached for the life of the process — [`crate::agents::Catalogue::detect`]
/// runs on hot paths (every pane open, every worktree dialog), so this must
/// not re-hit the network on every call. [`clear_cache`] is what Settings'
/// Refresh button calls, the same way [`crate::shell::clear_probe_cache`] is.
pub fn status(prober: &dyn Prober) -> Status {
    if let Some(cached) = cache().lock().ok().and_then(|cache| cache.clone()) {
        return cached;
    }

    // Gate 1: is Ollama even installed? Cheap, and the one check every other
    // candidate in `agents.rs` already does.
    let status = if prober.find("ollama").is_some() {
        fetch_tags()
    } else {
        Status::NotInstalled
    };

    if let Ok(mut cache) = cache().lock() {
        *cache = Some(status.clone());
    }
    status
}

/// Every model a local Ollama has pulled, or empty if it is not installed,
/// not running, or has nothing pulled.
///
/// What [`crate::agents::Catalogue::detect`] wants — it only ever turns a
/// model into a runnable agent, and has no separate "why none" to show.
pub fn installed_models(prober: &dyn Prober) -> Vec<OllamaModel> {
    match status(prober) {
        Status::Models(models) => models,
        Status::NotInstalled | Status::NotRunning => Vec::new(),
    }
}

/// Forgets the cached status, so the next call re-asks the daemon.
pub fn clear_cache() {
    if let Ok(mut cache) = cache().lock() {
        *cache = None;
    }
}

fn cache() -> &'static Mutex<Option<Status>> {
    static CACHE: OnceLock<Mutex<Option<Status>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Gate 2: ask the daemon what it has, over its own HTTP API.
///
/// A `GET` rather than shelling `ollama list`: it returns real JSON instead
/// of a column-aligned table meant for a terminal, and a connection refused
/// is an unambiguous "daemon not running" rather than a shell exit code to
/// interpret.
fn fetch_tags() -> Status {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .new_agent();

    let response = match agent.get("http://127.0.0.1:11434/api/tags").call() {
        Ok(response) => response,
        // Connection refused, timed out, or anything else: no usable daemon.
        Err(_) => return Status::NotRunning,
    };

    let mut body = response.into_body();
    let models = match body.read_json::<TagsResponse>() {
        Ok(tags) => tags
            .models
            .into_iter()
            .map(|model| OllamaModel { tag: model.name })
            .collect(),
        // Answered, but not with anything recognisable — treat as reachable
        // with nothing pulled rather than guessing further.
        Err(_) => Vec::new(),
    };
    Status::Models(models)
}
