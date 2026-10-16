//! What a local Ollama has, and the few things ket asks it to do.
//!
//! Not a [`crate::agents::Prober`]: that trait answers "is this binary on
//! `PATH`", and a local model is a different kind of question — Ollama the
//! binary can be installed with its daemon not running, or running with
//! nothing pulled, and a settings screen needs to tell those apart. So this
//! is a second, parallel detection step [`crate::agents::Catalogue::detect`]
//! runs after the ordinary candidate table, feeding in the same
//! [`crate::agents::Prober`] it was given rather than inventing its own way
//! to find the `ollama` binary.
//!
//! One [`probe`] answers everything — the models with what each can do, the
//! loaded ones and their memory — and is cached for the life of the process,
//! because the catalogue runs on hot paths. [`clear_cache`] is what Settings'
//! Refresh calls. Beyond reading, ket asks Ollama for three things: to
//! [`pull`] a model, to [`prepare`] an agent profile of one (a copy pinned to
//! a context length — see [`crate::local_models`]), and to [`delete`] one.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

use crate::agents::Prober;
use crate::{KetError, Result};

/// How long a request to the daemon may take before it is treated as not
/// running — mirrors [`crate::agents::PROBE_TIMEOUT`], since this is the same
/// kind of "don't hang the window on a dead local service" guard.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// How long creating a profile may take. A copy shares the original's blobs,
/// so it is quick, but it is a write and Ollama checks the layers.
const CREATE_TIMEOUT: Duration = Duration::from_secs(60);

/// Ollama's own default address.
const DEFAULT_BASE: &str = "http://127.0.0.1:11434";

/// Where Ollama is, from `OLLAMA_HOST` the way Ollama's own CLI reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The base URL, without a trailing slash.
    pub base: String,
}

impl Endpoint {
    /// `OLLAMA_HOST` read as `host`, `host:port`, `:port` or a URL; unset or
    /// empty is the default. A server bound to `0.0.0.0` is reached on
    /// loopback, as Ollama's CLI does.
    pub fn from_env() -> Self {
        Self::parse(std::env::var("OLLAMA_HOST").ok().as_deref())
    }

    fn parse(value: Option<&str>) -> Self {
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        let Some(value) = value else {
            return Self {
                base: DEFAULT_BASE.to_owned(),
            };
        };
        let (scheme, rest) = match value.split_once("://") {
            Some((scheme, rest)) => (scheme, rest),
            None => ("http", value),
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            // A bare IPv6 address has colons of its own; only `]:port` or a
            // single colon is a port.
            Some((host, port))
                if port.chars().all(|c| c.is_ascii_digit())
                    && (!host.contains(':') || host.ends_with(']')) =>
            {
                (host, port)
            }
            _ => (rest, "11434"),
        };
        let host = match host {
            "" | "0.0.0.0" => "127.0.0.1",
            "[::]" => "[::1]",
            other => other,
        };
        Self {
            base: format!("{scheme}://{host}:{port}"),
        }
    }

    /// Whether the daemon is on this machine. A model served from another
    /// machine is not "local", whatever it is called.
    pub fn is_loopback(&self) -> bool {
        let host = self
            .base
            .split_once("://")
            .map_or(self.base.as_str(), |(_, rest)| rest);
        let host = host
            .rsplit_once(':')
            .map_or(host, |(host, _)| host)
            .trim_start_matches('[')
            .trim_end_matches(']');
        // An address, read as one: `127.example.com` is a name, and could be
        // anywhere.
        match host.parse::<std::net::IpAddr>() {
            Ok(address) => address.is_loopback(),
            Err(_) => host.eq_ignore_ascii_case("localhost"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

/// One model the daemon lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// The full name, as `ollama list` shows it: `qwen3-coder:30b`.
    pub tag: String,
    /// Bytes on disk — the weights, roughly what loading it takes before
    /// any context. A cloud model's few hundred bytes are its manifest.
    pub size_bytes: u64,
    /// `qwen3`, `gemma3`…
    pub family: Option<String>,
    /// `30.5B`, `751.63M`…
    pub parameters: Option<String>,
    /// `Q4_K_M`, `MXFP4`…
    pub quantization: Option<String>,
    /// Whether the daemon passes it on to Ollama's cloud: prompts leave this
    /// machine, so it is never offered as a local agent.
    pub cloud: bool,
    /// Whether it can call tools, which a coding agent cannot work without.
    pub tools: bool,
    /// The longest context the model itself supports.
    pub context_max: Option<u32>,
    /// The model this one was made from, for a copy such as an agent profile.
    pub parent: Option<String>,
    /// The context length its Modelfile pins, when it pins one.
    pub num_ctx: Option<u32>,
    /// Bytes of KV cache per token of context, from the model's shape, when
    /// the daemon said — see [`crate::local_models::kv_bytes`].
    pub kv_per_token: Option<u64>,
}

/// A model the daemon has in memory right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    /// Its full name.
    pub tag: String,
    /// Bytes it holds in all.
    pub size_bytes: u64,
    /// Of those, bytes in GPU memory — on Apple silicon, the same memory.
    pub size_vram: u64,
    /// The context it was loaded with.
    pub context_length: Option<u32>,
}

/// What a running daemon has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Where it was asked.
    pub endpoint: Endpoint,
    /// Its version, when it said.
    pub version: Option<String>,
    /// Every model it lists, agent profiles included.
    pub models: Vec<ModelInfo>,
    /// The ones loaded now.
    pub loaded: Vec<LoadedModel>,
}

impl Snapshot {
    /// The models a person picked, without ket's agent profiles of them.
    pub fn listed(&self) -> impl Iterator<Item = &ModelInfo> {
        self.models.iter().filter(|model| !is_profile(model))
    }

    /// The agent profile ket made of `tag`, when there is one.
    pub fn profile_of(&self, tag: &str) -> Option<&ModelInfo> {
        let wanted = crate::local_models::profile_tag(tag);
        self.models
            .iter()
            .find(|model| model.tag == wanted && model.parent.as_deref() == Some(tag))
    }
}

/// Whether `model` is an agent profile ket made: named after its parent, and
/// made from it.
fn is_profile(model: &ModelInfo) -> bool {
    model
        .parent
        .as_deref()
        .is_some_and(|parent| model.tag == crate::local_models::profile_tag(parent))
}

/// What asking a local Ollama found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The `ollama` binary is not on `PATH`, and nothing was asked of a
    /// daemon on this machine.
    NotInstalled,
    /// Nothing answered.
    NotRunning,
    /// A daemon answered.
    Running(Snapshot),
}

/// The short form a settings screen and the catalogue used before [`Probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The `ollama` binary is not on `PATH`.
    NotInstalled,
    /// Installed, but nothing answered on its usual port.
    NotRunning,
    /// Installed and running, with every model it has pulled — possibly
    /// none. Agent profiles are left out.
    Models(Vec<ModelInfo>),
}

/// Asks what a local Ollama has, in full.
///
/// Cached for the life of the process — the catalogue runs on hot paths, so
/// this must not re-ask the daemon on every call. [`clear_cache`] is what
/// Settings' Refresh calls.
pub fn probe(prober: &dyn Prober) -> Probe {
    if let Some(cached) = cache().lock().ok().and_then(|cache| cache.clone()) {
        return cached;
    }
    let endpoint = Endpoint::from_env();
    // Without the binary there is no local daemon to ask. A daemon on another
    // machine is still worth asking: its models are shown, though never
    // offered as local agents.
    let probe = if prober.find("ollama").is_none() && endpoint.is_loopback() {
        Probe::NotInstalled
    } else {
        fetch(endpoint)
    };
    if let Ok(mut cache) = cache().lock() {
        *cache = Some(probe.clone());
    }
    probe
}

/// The short form of [`probe`].
pub fn status(prober: &dyn Prober) -> Status {
    match probe(prober) {
        Probe::NotInstalled => Status::NotInstalled,
        Probe::NotRunning => Status::NotRunning,
        Probe::Running(snapshot) => Status::Models(snapshot.listed().cloned().collect()),
    }
}

/// Every model a local Ollama has pulled, agent profiles left out, or empty
/// if it is not installed, not running, or has nothing pulled.
pub fn installed_models(prober: &dyn Prober) -> Vec<ModelInfo> {
    match status(prober) {
        Status::Models(models) => models,
        Status::NotInstalled | Status::NotRunning => Vec::new(),
    }
}

/// Forgets the cached answer, so the next call re-asks the daemon.
pub fn clear_cache() {
    if let Ok(mut cache) = cache().lock() {
        *cache = None;
    }
}

fn cache() -> &'static Mutex<Option<Probe>> {
    static CACHE: OnceLock<Mutex<Option<Probe>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(timeout)
        .timeout_connect(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    remote_model: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    details: TagsDetails,
}

#[derive(Deserialize, Default)]
struct TagsDetails {
    #[serde(default)]
    parent_model: String,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    parameter_size: Option<String>,
    #[serde(default)]
    quantization_level: Option<String>,
    #[serde(default)]
    context_length: Option<u32>,
}

#[derive(Deserialize)]
struct ShowResponse {
    #[serde(default)]
    parameters: String,
    #[serde(default)]
    details: TagsDetails,
    #[serde(default)]
    model_info: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
struct PsResponse {
    #[serde(default)]
    models: Vec<PsModel>,
}

#[derive(Deserialize)]
struct PsModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    size_vram: u64,
    #[serde(default)]
    context_length: Option<u32>,
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

/// The most models whose shape is asked for, one request each: past this
/// the rest show without a memory estimate rather than stall the probe.
const MAX_SHOWN: usize = 32;

/// Asks the daemon, over its own HTTP API rather than `ollama list`'s table.
/// A connection refused is an unambiguous "not running".
fn fetch(endpoint: Endpoint) -> Probe {
    let http = agent(Some(REQUEST_TIMEOUT));
    let tags = match http.get(endpoint.url("/api/tags")).call() {
        Ok(response) if response.status().is_success() => response,
        _ => return Probe::NotRunning,
    };
    let mut body = tags.into_body();
    let listed = body
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_json::<TagsResponse>()
        .map(|tags| tags.models)
        .unwrap_or_default();

    let version = http
        .get(endpoint.url("/api/version"))
        .call()
        .ok()
        .and_then(|response| response.into_body().read_json::<VersionResponse>().ok())
        .map(|version| version.version);

    let mut models: Vec<ModelInfo> = listed.into_iter().map(model_info).collect();
    // The shape, and any pinned context, only for what could become an
    // agent: a local model that calls tools.
    for model in models
        .iter_mut()
        .filter(|model| !model.cloud && model.tools)
        .take(MAX_SHOWN)
    {
        if let Some(shown) = show(&http, &endpoint, &model.tag) {
            model.num_ctx = num_ctx(&shown.parameters);
            model.kv_per_token = kv_per_token(&shown.model_info);
            if model.context_max.is_none() {
                model.context_max = context_length(&shown.model_info);
            }
            if !shown.capabilities.is_empty() {
                model.tools = shown.capabilities.iter().any(|c| c == "tools");
            }
        }
    }

    let loaded = http
        .get(endpoint.url("/api/ps"))
        .call()
        .ok()
        .and_then(|response| response.into_body().read_json::<PsResponse>().ok())
        .map(|ps| {
            ps.models
                .into_iter()
                .map(|model| LoadedModel {
                    tag: model.name,
                    size_bytes: model.size,
                    size_vram: model.size_vram,
                    context_length: model.context_length,
                })
                .collect()
        })
        .unwrap_or_default();

    Probe::Running(Snapshot {
        endpoint,
        version,
        models,
        loaded,
    })
}

fn model_info(model: TagsModel) -> ModelInfo {
    let cloud = model.remote_host.is_some()
        || model.remote_model.is_some()
        || crate::local_models::is_cloud_tag(&model.name);
    ModelInfo {
        cloud,
        tools: model.capabilities.iter().any(|c| c == "tools"),
        size_bytes: model.size,
        family: model.details.family,
        parameters: model.details.parameter_size,
        quantization: model.details.quantization_level,
        context_max: model.details.context_length,
        parent: Some(model.details.parent_model).filter(|parent| !parent.is_empty()),
        num_ctx: None,
        kv_per_token: None,
        tag: model.name,
    }
}

fn show(http: &ureq::Agent, endpoint: &Endpoint, tag: &str) -> Option<ShowResponse> {
    let response = http
        .post(endpoint.url("/api/show"))
        .send_json(serde_json::json!({ "model": tag }))
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .into_body()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_json()
        .ok()
}

/// The `num_ctx` a Modelfile's parameters pin, from `/api/show`'s text form.
fn num_ctx(parameters: &str) -> Option<u32> {
    parameters.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some("num_ctx"))
            .then(|| words.next()?.parse().ok())
            .flatten()
    })
}

/// A `model_info` number under any architecture's prefix: `qwen3.block_count`.
fn info_number(info: &serde_json::Map<String, serde_json::Value>, suffix: &str) -> Option<u64> {
    info.iter()
        .find(|(key, _)| key.ends_with(suffix))
        .and_then(|(_, value)| value.as_u64())
}

fn context_length(info: &serde_json::Map<String, serde_json::Value>) -> Option<u32> {
    info_number(info, ".context_length").and_then(|n| u32::try_from(n).ok())
}

/// Bytes of KV cache per token: keys and values, every layer, every KV head,
/// at two bytes each.
fn kv_per_token(info: &serde_json::Map<String, serde_json::Value>) -> Option<u64> {
    let layers = info_number(info, ".block_count")?;
    let heads = info_number(info, ".attention.head_count_kv")?;
    let key = info_number(info, ".attention.key_length")?;
    let value = info_number(info, ".attention.value_length").unwrap_or(key);
    Some(layers * heads * (key + value) * 2)
}

/// Makes `tag`'s agent profile: a copy pinned to `num_ctx`, named
/// [`crate::local_models::profile_tag`]. Shares the original's blobs, so it
/// costs no disk. Replaces an older profile of the same name. Returns the
/// profile's name.
pub fn prepare(tag: &str, num_ctx: u32) -> Result<String> {
    let profile = crate::local_models::profile_tag(tag);
    let endpoint = Endpoint::from_env();
    // Creating over a name replaces whatever had it. Only a profile ket made
    // from `tag` is ket's to replace; any other model by that name is the
    // person's own.
    if let Some(parent) = made_from(&endpoint, &profile)?
        && parent != tag
    {
        return Err(KetError::Config(format!(
            "{profile} is already a model of yours, so ket will not make {tag}'s agent profile under that name"
        )));
    }
    let response = agent(Some(CREATE_TIMEOUT))
        .post(endpoint.url("/api/create"))
        .send_json(serde_json::json!({
            "model": profile,
            "from": tag,
            "parameters": { "num_ctx": num_ctx },
            "stream": false,
        }))
        .map_err(|e| KetError::Config(format!("Ollama: {e}")))?;
    if !response.status().is_success() {
        let why = response.into_body().read_to_string().unwrap_or_default();
        return Err(KetError::Config(format!(
            "Ollama would not prepare {tag}: {why}"
        )));
    }
    clear_cache();
    Ok(profile)
}

/// Takes `tag` out of memory now, rather than after Ollama's idle time: a
/// request with `keep_alive` 0 and no prompt only unloads.
pub fn unload(tag: &str) -> Result<()> {
    let endpoint = Endpoint::from_env();
    let response = agent(Some(REQUEST_TIMEOUT))
        .post(endpoint.url("/api/generate"))
        .send_json(serde_json::json!({ "model": tag, "keep_alive": 0 }))
        .map_err(|e| KetError::Config(format!("Ollama: {e}")))?;
    if !response.status().is_success() {
        let why = response.into_body().read_to_string().unwrap_or_default();
        return Err(KetError::Config(format!(
            "Ollama would not unload {tag}: {}",
            error_text(&why)
        )));
    }
    clear_cache();
    Ok(())
}

/// Deletes the agent profile ket made of `tag`, if there is one. A model of
/// the person's own that only shares the profile's name is left alone.
pub fn delete_profile(tag: &str) -> Result<()> {
    let profile = crate::local_models::profile_tag(tag);
    let endpoint = Endpoint::from_env();
    match made_from(&endpoint, &profile)? {
        Some(parent) if parent == tag => delete(&profile),
        _ => Ok(()),
    }
}

/// What `name` was made from: `None` when no model has that name, an empty
/// string for one made from nothing. Anything short of Ollama's own answer —
/// a timeout, an error — is an error rather than "no such model", so a
/// caller deciding whether a name is free to write over never guesses yes.
fn made_from(endpoint: &Endpoint, name: &str) -> Result<Option<String>> {
    let response = agent(Some(REQUEST_TIMEOUT))
        .post(endpoint.url("/api/show"))
        .send_json(serde_json::json!({ "model": name }))
        .map_err(|e| KetError::Config(format!("Ollama: {e}")))?;
    match response.status().as_u16() {
        404 => Ok(None),
        status if (200..300).contains(&status) => {
            let shown: ShowResponse = response
                .into_body()
                .with_config()
                .limit(16 * 1024 * 1024)
                .read_json()
                .map_err(|e| KetError::Config(format!("Ollama: {e}")))?;
            Ok(Some(shown.details.parent_model))
        }
        _ => {
            let why = response.into_body().read_to_string().unwrap_or_default();
            Err(KetError::Config(format!(
                "Ollama could not say what {name} is: {}",
                error_text(&why)
            )))
        }
    }
}

/// Deletes `tag` from the daemon.
pub fn delete(tag: &str) -> Result<()> {
    let endpoint = Endpoint::from_env();
    let response = agent(Some(REQUEST_TIMEOUT))
        .delete(endpoint.url("/api/delete"))
        .force_send_body()
        .send_json(serde_json::json!({ "model": tag }))
        .map_err(|e| KetError::Config(format!("Ollama: {e}")))?;
    // Already gone is what was asked for.
    if !response.status().is_success() && response.status().as_u16() != 404 {
        let why = response.into_body().read_to_string().unwrap_or_default();
        return Err(KetError::Config(format!(
            "Ollama would not delete {tag}: {why}"
        )));
    }
    clear_cache();
    Ok(())
}

/// How far a pull has got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullProgress {
    /// Ollama's own word for the step: `pulling manifest`, `verifying…`.
    pub status: String,
    /// Bytes of the current layer received.
    pub completed: u64,
    /// Bytes of the current layer in all; `0` until a layer starts.
    pub total: u64,
    /// Finished, one way or another.
    pub done: bool,
    /// Why it failed, when it did. A cancelled pull has none.
    pub error: Option<String>,
    /// Whether it was cancelled.
    pub cancelled: bool,
}

/// A pull running on its own thread. Outlives whatever started it: closing
/// Settings does not stop a download.
#[derive(Debug)]
pub struct Pull {
    tag: String,
    progress: Mutex<PullProgress>,
    cancel: AtomicBool,
}

impl Pull {
    /// The model being pulled.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Where it is now.
    pub fn progress(&self) -> PullProgress {
        self.progress
            .lock()
            .map(|progress| progress.clone())
            .unwrap_or_default()
    }

    /// Stops it. Closing the connection is what stops Ollama's download; the
    /// part already fetched stays, and a later pull carries on from it.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn update(&self, change: impl FnOnce(&mut PullProgress)) {
        if let Ok(mut progress) = self.progress.lock() {
            change(&mut progress);
        }
    }
}

fn pulls_registry() -> &'static Mutex<Vec<Arc<Pull>>> {
    static PULLS: OnceLock<Mutex<Vec<Arc<Pull>>>> = OnceLock::new();
    PULLS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Every pull this process started that has not been forgotten.
pub fn pulls() -> Vec<Arc<Pull>> {
    pulls_registry()
        .lock()
        .map(|pulls| pulls.clone())
        .unwrap_or_default()
}

/// Drops a finished pull from [`pulls`], once whatever shows it has said so.
pub fn forget_pull(tag: &str) {
    if let Ok(mut pulls) = pulls_registry().lock() {
        pulls.retain(|pull| !(pull.tag == tag && pull.progress().done));
    }
}

#[derive(Deserialize)]
struct PullLine {
    #[serde(default)]
    status: String,
    #[serde(default)]
    total: u64,
    #[serde(default)]
    completed: u64,
    #[serde(default)]
    error: Option<String>,
}

/// Starts pulling `tag`, or returns the pull already running for it.
pub fn pull(tag: &str) -> Arc<Pull> {
    let mut registry = match pulls_registry().lock() {
        Ok(registry) => registry,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(running) = registry
        .iter()
        .find(|pull| pull.tag == tag && !pull.progress().done)
    {
        return running.clone();
    }
    registry.retain(|pull| pull.tag != tag);
    let pull = Arc::new(Pull {
        tag: tag.to_owned(),
        progress: Mutex::new(PullProgress {
            status: "starting".to_owned(),
            ..PullProgress::default()
        }),
        cancel: AtomicBool::new(false),
    });
    registry.push(pull.clone());
    drop(registry);

    let running = pull.clone();
    let spawned = std::thread::Builder::new()
        .name("ket-ollama-pull".to_owned())
        .spawn(move || {
            let outcome = stream_pull(&running);
            running.update(|progress| {
                progress.done = true;
                match outcome {
                    Ok(()) => {}
                    Err(_) if running.cancel.load(Ordering::Relaxed) => progress.cancelled = true,
                    Err(why) => progress.error = Some(why),
                }
            });
            clear_cache();
        });
    if let Err(e) = spawned {
        pull.update(|progress| {
            progress.done = true;
            progress.error = Some(e.to_string());
        });
    }
    pull
}

fn stream_pull(pull: &Pull) -> std::result::Result<(), String> {
    let endpoint = Endpoint::from_env();
    // No overall timeout: a large model takes as long as it takes.
    let response = agent(None)
        .post(endpoint.url("/api/pull"))
        .send_json(serde_json::json!({ "model": pull.tag, "stream": true }))
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let why = response.into_body().read_to_string().unwrap_or_default();
        return Err(error_text(&why));
    }
    read_pull(
        pull,
        std::io::BufReader::new(response.into_body().into_reader()),
    )
}

/// Follows a pull's stream, one JSON object per line, into `pull`'s progress.
fn read_pull(pull: &Pull, reader: impl BufRead) -> std::result::Result<(), String> {
    for line in reader.lines() {
        if pull.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }
        let line = line.map_err(|e| e.to_string())?;
        let Ok(parsed) = serde_json::from_str::<PullLine>(&line) else {
            continue;
        };
        if let Some(error) = parsed.error {
            return Err(error);
        }
        let finished = parsed.status == "success";
        pull.update(|progress| {
            progress.status = parsed.status;
            if parsed.total > 0 {
                progress.total = parsed.total;
                progress.completed = parsed.completed;
            }
        });
        if finished {
            return Ok(());
        }
    }
    if pull.cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    Err("Ollama stopped answering before the download finished".to_owned())
}

/// Ollama's `{"error": "…"}`, or the body as it came.
fn error_text(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| body.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_host_is_read_the_way_ollamas_cli_reads_it() {
        let base = |v: Option<&str>| Endpoint::parse(v).base;
        assert_eq!(base(None), "http://127.0.0.1:11434");
        assert_eq!(base(Some("")), "http://127.0.0.1:11434");
        assert_eq!(base(Some("0.0.0.0")), "http://127.0.0.1:11434");
        assert_eq!(base(Some(":8080")), "http://127.0.0.1:8080");
        assert_eq!(base(Some("gpu-box.local")), "http://gpu-box.local:11434");
        assert_eq!(
            base(Some("gpu-box.local:9000")),
            "http://gpu-box.local:9000"
        );
        assert_eq!(
            base(Some("https://ollama.example.com/")),
            "https://ollama.example.com:11434"
        );
        assert_eq!(base(Some("[::1]:11434")), "http://[::1]:11434");
        assert!(Endpoint::parse(None).is_loopback());
        assert!(Endpoint::parse(Some("localhost:1")).is_loopback());
        assert!(Endpoint::parse(Some("[::1]:11434")).is_loopback());
        assert!(!Endpoint::parse(Some("gpu-box.local")).is_loopback());
        assert!(!Endpoint::parse(Some("192.168.1.9")).is_loopback());
        assert!(!Endpoint::parse(Some("127.example.com")).is_loopback());
        assert!(Endpoint::parse(Some("127.0.0.2:11434")).is_loopback());
    }

    #[test]
    fn tags_mark_cloud_models_tools_and_ket_profiles() {
        let json = r#"{"models":[
            {"name":"gpt-oss:20b-cloud","size":306,"remote_host":"https://ollama.com","remote_model":"gpt-oss:20b","capabilities":["completion","tools"],"details":{}},
            {"name":"qwen3:0.6b","size":522653767,"capabilities":["completion","tools","thinking"],"details":{"parent_model":"","family":"qwen3","parameter_size":"751.63M","quantization_level":"Q4_K_M","context_length":40960}},
            {"name":"qwen3:0.6b-ket","size":522653411,"capabilities":["completion","tools"],"details":{"parent_model":"qwen3:0.6b","context_length":40960}},
            {"name":"mine:latest","size":1,"capabilities":["completion"],"details":{"parent_model":"qwen3:0.6b"}},
            {"name":"llama3.2:3b","size":2019393189,"capabilities":["completion"],"details":{}}
        ]}"#;
        let tags: TagsResponse = serde_json::from_str(json).unwrap();
        let models: Vec<ModelInfo> = tags.models.into_iter().map(model_info).collect();
        assert!(models[0].cloud && models[0].tools);
        assert!(!models[1].cloud && models[1].tools);
        assert_eq!(models[1].context_max, Some(40960));
        assert_eq!(models[1].parameters.as_deref(), Some("751.63M"));
        assert!(!models[4].tools);
        let snapshot = Snapshot {
            endpoint: Endpoint::parse(None),
            version: None,
            models,
            loaded: Vec::new(),
        };
        // The person's own copy stays listed; only ket's profile folds away.
        let listed: Vec<&str> = snapshot.listed().map(|m| m.tag.as_str()).collect();
        assert_eq!(
            listed,
            [
                "gpt-oss:20b-cloud",
                "qwen3:0.6b",
                "mine:latest",
                "llama3.2:3b"
            ]
        );
        assert_eq!(
            snapshot.profile_of("qwen3:0.6b").map(|m| m.tag.as_str()),
            Some("qwen3:0.6b-ket")
        );
        assert!(snapshot.profile_of("llama3.2:3b").is_none());
    }

    #[test]
    fn show_gives_the_pinned_context_and_the_kv_shape() {
        let parameters = "temperature                    0.6\nnum_ctx                        16384\nstop \"<|im_end|>\"";
        assert_eq!(num_ctx(parameters), Some(16384));
        assert_eq!(num_ctx("temperature 0.6"), None);
        let info: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{"qwen3.attention.head_count_kv":8,"qwen3.attention.key_length":128,"qwen3.attention.value_length":128,"qwen3.block_count":28,"qwen3.context_length":40960}"#,
        )
        .unwrap();
        // 28 layers × 8 heads × (128 + 128) × 2 bytes.
        assert_eq!(kv_per_token(&info), Some(114_688));
        assert_eq!(context_length(&info), Some(40960));
        let partial: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"x.block_count":28}"#).unwrap();
        assert_eq!(kv_per_token(&partial), None);
    }

    fn pull(tag: &str) -> Pull {
        Pull {
            tag: tag.to_owned(),
            progress: Mutex::new(PullProgress::default()),
            cancel: AtomicBool::new(false),
        }
    }

    #[test]
    fn a_pull_stream_reads_as_progress_and_ends_at_success() {
        let stream = concat!(
            "{\"status\":\"pulling manifest\"}\n",
            "{\"status\":\"pulling 7f40\",\"digest\":\"sha256:7f40\",\"total\":522640096}\n",
            "{\"status\":\"pulling 7f40\",\"digest\":\"sha256:7f40\",\"total\":522640096,\"completed\":261320048}\n",
            "not json\n",
            "{\"status\":\"verifying sha256 digest\"}\n",
            "{\"status\":\"success\"}\n",
        );
        let running = pull("qwen3:0.6b");
        assert_eq!(read_pull(&running, stream.as_bytes()), Ok(()));
        let progress = running.progress();
        assert_eq!(progress.status, "success");
        assert_eq!(progress.total, 522640096);
        assert_eq!(progress.completed, 261320048);
    }

    #[test]
    fn a_pull_stops_on_an_error_line_on_cancel_and_on_a_cut_stream() {
        let failed = pull("nope");
        assert_eq!(
            read_pull(
                &failed,
                "{\"error\":\"pull model manifest: file does not exist\"}\n".as_bytes()
            ),
            Err("pull model manifest: file does not exist".to_owned())
        );
        let cancelled = pull("qwen3:0.6b");
        cancelled.cancel();
        assert_eq!(
            read_pull(&cancelled, "{\"status\":\"pulling manifest\"}\n".as_bytes()),
            Err("cancelled".to_owned())
        );
        let cut = pull("qwen3:0.6b");
        assert!(read_pull(&cut, "{\"status\":\"pulling manifest\"}\n".as_bytes()).is_err());
    }

    #[test]
    fn an_error_body_reads_as_its_message() {
        assert_eq!(
            error_text(r#"{"error":"pull model manifest: file does not exist"}"#),
            "pull model manifest: file does not exist"
        );
        assert_eq!(error_text("  plain  "), "plain");
    }
}
