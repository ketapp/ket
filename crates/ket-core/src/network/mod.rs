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
//! [`launch_env`]), so the window never needs to know where the proxy
//! listens, and a session already running keeps going as it started.
//!
//! **Per pane, by token.** Each inspected terminal gets its own address,
//! `http://127.0.0.1:<port>/<token>/`, and the token says which pane a request
//! came from. A request without a token the host issued is turned away: the
//! proxy is never a way out to anywhere for anything else on the machine.
//!
//! **Which agents.** Claude Code, by `ANTHROPIC_BASE_URL`. Others are left as
//! they are: their traffic goes straight to their provider, unseen.

mod proxy;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub use proxy::Proxy;

/// Set by a window on a terminal whose agent's traffic is to be inspected;
/// the host removes it and points the agent at the proxy instead.
pub const LOG_ENV: &str = "KET_NETWORK_LOG";

/// Where a request goes once it has been seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Anthropic's API, for Claude Code — subscription and API key alike.
    Anthropic,
}

impl Route {
    /// The provider's host, reached over HTTPS.
    pub fn upstream(self) -> &'static str {
        match self {
            Route::Anthropic => "api.anthropic.com",
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

    /// Stops accepting the tokens of panes that have closed.
    pub fn unbind(&self, panes: &[String]) {
        self.lock()
            .retain(|_, binding| !panes.contains(&binding.pane));
    }

    /// The pane `token` was issued for, if it was.
    pub fn lookup(&self, token: &str) -> Option<Binding> {
        self.lock().get(token).cloned()
    }
}

/// Which route `agent`'s traffic takes, for the agents ket can inspect.
pub fn route_for(agent: &str) -> Option<Route> {
    match crate::agents::harness(agent) {
        "claude" => Some(Route::Anthropic),
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
/// provider, by `env` or by the environment the terminal inherits.
pub fn sent_elsewhere(route: Route, env: &BTreeMap<String, String>) -> bool {
    match route {
        Route::Anthropic => CLAUDE_ELSEWHERE
            .iter()
            .any(|name| env.contains_key(*name) || std::env::var_os(name).is_some()),
    }
}

/// What points `route`'s agent at `base`, the pane's address on the proxy.
///
/// Only the address: never a key or a token of the provider's. Claude Code
/// sends whichever credential it has — a subscription's included — and the
/// proxy passes it on as it came.
pub fn launch_env(route: Route, base: &str) -> BTreeMap<String, String> {
    match route {
        Route::Anthropic => BTreeMap::from([("ANTHROPIC_BASE_URL".to_owned(), base.to_owned())]),
    }
}
