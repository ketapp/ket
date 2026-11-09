//! The network inspector: what agents send to their model providers.
//!
//! An agent ket launches can be pointed at a proxy on this Mac instead of at
//! its provider. The proxy passes every request on unchanged and every reply
//! back unchanged, and on the way it sees what left the machine: the prompt,
//! the files the agent read, the output of the commands it ran. Outgoing
//! only — what comes back is the provider's, not the person's.
//!
//! **Opt in, per launch.** A window that wants a terminal's agent inspected
//! puts [`LOG_ENV`] in the terminal's environment. The host takes it out and
//! puts in its place what points that agent at the proxy (see
//! [`point`]), so the window never needs to know where the proxy
//! listens, and a session already running keeps going as it started.
//!
//! **Per pane, by token.** Each inspected terminal gets its own address,
//! `http://127.0.0.1:<port>/<token>/`, and the token says which pane a request
//! came from. A request without a token the host issued is turned away: the
//! proxy is never a way out to anywhere for anything else on the machine.
//!
//! **Which agents.** Claude Code, by `ANTHROPIC_BASE_URL`, and Codex, by the
//! `openai_base_url` its command line sets — over HTTPS, and over the
//! WebSocket it tries first (see [`socket`]). Others are left as they are:
//! their traffic goes straight to their provider, unseen.
//!
//! **Flagged, never changed.** What looks like an identifier or a secret in
//! what a request added is noted beside it — see [`flags`] — and the request
//! goes on as it was.
//!
//! **Kept per worktree, after the fact.** A request is written down once its
//! reply has ended, on a thread of its own, so logging never holds up the
//! agent and a failure to log never fails a request — see [`store`].

mod delta;
pub mod flags;
mod proxy;
mod socket;
pub mod store;
pub mod usage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use hyper::HeaderMap;
use hyper::body::Bytes;

use crate::terminal::TerminalSpec;

pub use proxy::Proxy;

/// Set by a window on a terminal whose agent's traffic is to be inspected;
/// the host removes it and points the agent at the proxy instead.
pub const LOG_ENV: &str = "KET_NETWORK_LOG";

/// Where a request goes once it has been seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Anthropic's API, for Claude Code — subscription and API key alike.
    Anthropic,
    /// OpenAI's, for Codex: the API for a key, ChatGPT's own backend for a
    /// ChatGPT sign-in.
    OpenAi,
}

/// Where one request goes: the provider's host, reached over HTTPS, and the
/// path its API sits under there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// `api.openai.com`, say.
    pub host: &'static str,
    /// `/v1`, say: what the agent's own paths go on from.
    pub base: &'static str,
}

impl Route {
    /// Where a request carrying `headers` goes.
    pub fn target(self, headers: &HeaderMap) -> Target {
        match self {
            Route::Anthropic => Target {
                host: "api.anthropic.com",
                base: "",
            },
            // Codex signed in with ChatGPT names the account on every
            // request, and is answered by ChatGPT's backend rather than the
            // API; with a key it has no account to name.
            Route::OpenAi if headers.contains_key("chatgpt-account-id") => Target {
                host: "chatgpt.com",
                base: "/backend-api/codex",
            },
            Route::OpenAi => Target {
                host: "api.openai.com",
                base: "/v1",
            },
        }
    }
}

/// The pane an inspected agent runs in, as the host files it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The pane's key, `worktree|agent|nonce`.
    pub pane: String,
    /// Which agent, as the key names it.
    pub agent: String,
    /// Where its requests go.
    pub route: Route,
}

impl Binding {
    /// The worktree the pane belongs to.
    pub fn worktree(&self) -> &str {
        self.pane.split('|').next().unwrap_or_default()
    }
}

/// The tokens the host has issued, each for one pane.
#[derive(Clone, Default)]
pub struct NetworkBindings {
    tokens: Arc<Mutex<HashMap<String, Binding>>>,
}

impl NetworkBindings {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Binding>> {
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Issues a token for `binding`'s pane.
    pub fn bind(&self, binding: Binding) -> String {
        let token = crate::agent_hooks::new_token();
        self.lock().insert(token.clone(), binding);
        token
    }

    /// Stops accepting `token`: what it was issued for never used it.
    pub fn forget(&self, token: &str) {
        self.lock().remove(token);
    }

    /// Stops accepting the tokens of panes that have closed.
    pub fn unbind(&self, panes: &[String]) {
        self.lock()
            .retain(|_, binding| !panes.contains(&binding.pane));
    }

    /// The pane `token` was issued for, if it was.
    pub fn lookup(&self, token: &str) -> Option<Binding> {
        self.lock().get(token).cloned()
    }

    /// Whether `pane` still holds a token: whether its terminal is still
    /// one of the host's.
    pub fn bound(&self, pane: &str) -> bool {
        self.lock().values().any(|binding| binding.pane == pane)
    }
}

/// Which route `agent`'s traffic takes, for the agents ket can inspect.
pub fn route_for(agent: &str) -> Option<Route> {
    match crate::agents::harness(agent) {
        "claude" => Some(Route::Anthropic),
        "codex" => Some(Route::OpenAi),
        _ => None,
    }
}

/// The variables that already send Claude Code somewhere of its own: a
/// gateway, Bedrock, Vertex. Inspecting it would mean overriding them, so a
/// terminal that has any of them is left alone.
const CLAUDE_ELSEWHERE: [&str; 4] = [
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// Whether `route`'s agent is already sent somewhere other than its
/// provider, by `spec` or by the environment the terminal inherits.
pub fn sent_elsewhere(route: Route, spec: &TerminalSpec) -> bool {
    let set = |name: &str| spec.env.contains_key(name) || std::env::var_os(name).is_some();
    match route {
        Route::Anthropic => CLAUDE_ELSEWHERE.iter().any(|name| set(name)),
        Route::OpenAi => {
            set("OPENAI_BASE_URL")
                || spec.startup().is_some_and(|line| {
                    line.contains("openai_base_url") || line.contains("model_provider")
                })
                || codex_config_elsewhere(spec)
        }
    }
}

/// Whether Codex's config gives OpenAI an address of its own, or picks
/// another provider, itself or in the profile it runs: overriding the one
/// would send it somewhere it was not meant to go, and the other means it
/// never asks OpenAI at all.
fn codex_config_elsewhere(spec: &TerminalSpec) -> bool {
    let home = spec
        .env
        .get("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .unwrap_or_else(|| crate::sessions::home().join(".codex"));
    let Some(config) = std::fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
    else {
        return false;
    };
    let elsewhere = |table: &toml::Table| {
        table.contains_key("openai_base_url")
            || table
                .get("model_provider")
                .and_then(toml::Value::as_str)
                .is_some_and(|provider| provider != "openai")
    };
    elsewhere(&config)
        || config
            .get("profile")
            .and_then(toml::Value::as_str)
            .and_then(|profile| config.get("profiles")?.get(profile)?.as_table())
            .is_some_and(elsewhere)
}

/// Points `route`'s agent in `spec` at `base`, the pane's address on the
/// proxy. `false` when there was nowhere in `spec` to say it.
///
/// Only the address: never a key or a token of the provider's. The agent
/// sends whichever credential it has — a subscription's included — and the
/// proxy passes it on as it came.
pub fn point(route: Route, base: &str, spec: &mut TerminalSpec) -> bool {
    match route {
        Route::Anthropic => {
            spec.env
                .insert("ANTHROPIC_BASE_URL".to_owned(), base.to_owned());
            true
        }
        // Signed in with ChatGPT, Codex takes no address from its
        // environment, only from its config, which its command line can set.
        // Its HTTPS bodies go uncompressed as well: compressed, they could
        // not be read here. What it sends over its WebSocket is compressed
        // either way, and is read — see [`socket`].
        Route::OpenAi => spec.append_to_startup(&[
            "-c".to_owned(),
            format!("openai_base_url={base}"),
            "-c".to_owned(),
            "features.enable_request_compression=false".to_owned(),
        ]),
    }
}

/// One request the proxy passed on, once its reply has ended.
pub(crate) struct Observed {
    pub binding: Binding,
    /// The provider's host.
    pub upstream: &'static str,
    /// The provider's path, without the query.
    pub path: String,
    /// What was sent, as it was sent.
    pub request: Bytes,
    /// When it was sent, in milliseconds since the Unix epoch.
    pub at: u64,
    pub status: u16,
    /// Bytes of reply passed back.
    pub received: usize,
    /// The reply as received, up to what its usage is read from.
    pub reply: Vec<u8>,
    /// The reply's `Content-Encoding`.
    pub encoding: Option<String>,
    pub ms: u64,
    pub complete: bool,
}

/// Told of each request once it is written down: the worktree it was filed
/// under, and its summary. The host sends it on to the windows.
pub type Told = Arc<dyn Fn(&str, &store::RequestSummary) + Send + Sync>;

/// Starts the thread that writes requests down, and returns where to send
/// them.
pub(crate) fn sink(
    bindings: NetworkBindings,
    told: Told,
) -> std::io::Result<mpsc::Sender<Observed>> {
    let (send, receive) = mpsc::channel::<Observed>();
    std::thread::Builder::new()
        .name("ket-host-network-log".into())
        .spawn(move || {
            let mut delta = delta::Delta::default();
            let mut last = (0u64, 0u32);
            for observed in receive {
                // A reply that ended as its terminal was closed — the agent
                // killed with its worktree, say — reaches here after the
                // host has let the pane go, and after the worktree's logs
                // were deleted with it. Writing it down would start them
                // again.
                if !bindings.bound(&observed.binding.pane) {
                    tracing::debug!(pane = %observed.binding.pane, "network inspector: pane gone, request not logged");
                    continue;
                }
                // Ids sort as the requests are written down: the
                // millisecond, then a count within it. Written down as their
                // replies end, which is not the order they were sent in, so
                // one that started before the last written keeps the last's
                // millisecond and takes the next count rather than its own
                // time, which could repeat an id already given out.
                last = if observed.at > last.0 {
                    (observed.at, 0)
                } else {
                    (last.0, last.1 + 1)
                };
                let id = format!("{}-{:03}", last.0, last.1);
                record(&mut delta, id, observed, &told);
            }
        })?;
    Ok(send)
}

/// Writes one request down: its summary, and what it added.
fn record(delta: &mut delta::Delta, id: String, observed: Observed, told: &Told) {
    let json = serde_json::from_slice::<serde_json::Value>(&observed.request).ok();
    let model = json
        .as_ref()
        .and_then(|request| request.get("model")?.as_str())
        .unwrap_or_default()
        .to_owned();
    let split = json
        .as_ref()
        .and_then(|request| delta.split(&observed.binding.pane, request));
    // Before the body is readied: what is flagged is read from the
    // messages as sent, images and all, not the copy cut to fit.
    // A scan that fails costs this request its flags, never the thread
    // every later request is logged on.
    let flags = split
        .as_ref()
        .and_then(|split| std::panic::catch_unwind(|| flags::scan(&split.kept)).ok())
        .unwrap_or_default();
    let (body, truncated) = match (&split, &json) {
        (Some(split), _) => {
            let (body, truncated) = store::ready_body(split.kept.clone());
            (Some(body), truncated)
        }
        (None, Some(request)) => {
            let (body, truncated) = store::ready_body(request.clone());
            (Some(body), truncated)
        }
        (None, None) if !observed.request.is_empty() => {
            let (body, truncated) = store::ready_raw(&observed.request);
            (Some(body), truncated)
        }
        (None, None) => (None, false),
    };
    // A request that is not a conversation is new from end to end.
    let whole = split.is_none();
    let split = split.unwrap_or_default();
    let summary = store::RequestSummary {
        id,
        at: observed.at,
        pane: observed.binding.pane.clone(),
        agent: observed.binding.agent.clone(),
        upstream: observed.upstream.to_owned(),
        path: observed.path,
        model,
        status: observed.status,
        complete: observed.complete,
        ms: observed.ms,
        sent: observed.request.len(),
        received: observed.received,
        system_bytes: split.system_bytes,
        tools_bytes: split.tools_bytes,
        resent_bytes: split.resent_bytes,
        new_bytes: if whole {
            observed.request.len()
        } else {
            split.new_bytes
        },
        new_messages: split.new_messages,
        continued: split.continued,
        body: body.is_some(),
        truncated,
        usage: usage::parse(&observed.reply, observed.encoding.as_deref()),
        flags,
    };
    tracing::info!(
        pane = %summary.pane,
        path = %summary.path,
        model = %summary.model,
        status = summary.status,
        sent = summary.sent,
        new = summary.new_bytes,
        received = summary.received,
        ms = summary.ms,
        complete = summary.complete,
        flagged = summary.flags.len(),
        "network inspector: observed"
    );
    match store::append(observed.binding.worktree(), &summary, body.as_deref()) {
        Ok(()) => told(observed.binding.worktree(), &summary),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(pane = %summary.pane, "network inspector: worktree gone, request not logged");
        }
        Err(error) => {
            tracing::warn!(%error, pane = %summary.pane, "network inspector: could not log a request");
        }
    }
}
