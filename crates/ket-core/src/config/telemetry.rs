//! Where per-request usage telemetry goes: `[telemetry]` in ket's config —
//! see [`TelemetryConfig`].

use serde::{Deserialize, Serialize};

/// The OTLP protocol used when a pack does not say otherwise.
///
/// `http/protobuf` rather than `grpc` because it is the one a collector behind
/// an ordinary HTTPS load balancer accepts without special configuration, and a
/// team's collector is far more often behind one of those than reachable on a
/// bare gRPC port.
pub const DEFAULT_OTLP_PROTOCOL: &str = "http/protobuf";

/// The OTLP protocols Claude Code accepts.
const OTLP_PROTOCOLS: [&str; 3] = ["grpc", "http/json", "http/protobuf"];

/// Where per-request usage telemetry goes.
///
/// Claude Code exports `claude_code.token.usage` and `claude_code.cost.usage`
/// itself, split by model, by `type` (`input`, `output`, `cacheRead`,
/// `cacheCreation`) and by `query_source` (`main`, `subagent`, `auxiliary`) —
/// which is per-request attribution ket would otherwise have to reconstruct
/// from a transcript. Turning it on is four environment variables on the launch
/// line and no parsing at all, because the collector on the other end does the
/// work.
///
/// Unset — the default, and what every install starts as — sets no variables,
/// opens no connection and exports nothing.
///
/// ## What this deliberately cannot turn on
///
/// Claude Code also has `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_ASSISTANT_RESPONSES`,
/// `OTEL_LOG_TOOL_CONTENT` and `OTEL_LOG_RAW_API_BODIES`, which put prompt text,
/// replies, tool input and whole API bodies into the exported stream. There is
/// no field here for any of them, and ket never sets them.
///
/// The reason is not that they are useless — they would make attribution
/// exact — but that this is a *cost* tool: nobody agreeing to have their token
/// counts measured has agreed to have their prompts shipped to a collector, and
/// a checkbox in a config file is not that consent. Anyone who genuinely wants
/// it can set the variable in their own environment, where the decision is
/// visibly theirs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    /// The collector's OTLP endpoint — `http://` or `https://`.
    ///
    /// Plaintext is allowed here, unlike [`PackConfig::url`], and the asymmetry
    /// is deliberate: a pack is an instruction ket hands its own agents and has
    /// to be authenticated, while a collector is very often reached on
    /// `localhost:4318` or across a private network where TLS terminates
    /// somewhere else. Refusing `http` would refuse the ordinary case.
    ///
    /// Unset turns the whole feature off.
    pub endpoint: Option<String>,
    /// One of `grpc`, `http/json` or `http/protobuf`.
    ///
    /// See [`DEFAULT_OTLP_PROTOCOL`]. An unrecognised value is refused at load
    /// rather than passed through, because Claude Code's own failure for one is
    /// to export nothing — silently, into a collector nobody is watching yet.
    pub protocol: String,
    /// Headers for the collector, in OTLP's own `key=value,key=value` form.
    ///
    /// Usually an `Authorization` bearer. Kept as one opaque string rather than
    /// a map because that is the shape the variable takes and re-encoding a map
    /// into it would only add a way to get the escaping wrong.
    pub headers: Option<String>,
    /// Whether to export events as well as metrics.
    ///
    /// Off by default. The events carry per-request cache-read and
    /// cache-creation counts, which is more detail than the metrics — but they
    /// are also the stream the content variables above would pour text into, so
    /// the safe default is the one that cannot carry any.
    pub logs: bool,
    /// How often metrics are flushed, in milliseconds.
    ///
    /// `None` leaves Claude Code's own default of 60,000. Worth lowering while
    /// measuring: a session shorter than one interval can end having
    /// exported nothing at all, and short sessions are common.
    pub export_interval_ms: Option<u64>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            protocol: DEFAULT_OTLP_PROTOCOL.to_owned(),
            headers: None,
            logs: false,
            export_interval_ms: None,
        }
    }
}

impl TelemetryConfig {
    /// The environment an agent is launched with, or nothing when unconfigured.
    ///
    /// Empty rather than an error for every unusable configuration, and the
    /// reasons are logged at the point they are found: telemetry is
    /// instrumentation, and a mistyped endpoint must never be the thing that
    /// stops a worktree opening. Same bargain [`crate::pack`] makes.
    pub fn env(&self) -> std::collections::BTreeMap<String, String> {
        let mut env = std::collections::BTreeMap::new();

        let Some(endpoint) = self
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        else {
            return env;
        };

        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            tracing::warn!(
                endpoint,
                "telemetry endpoint is not an http(s) URL; not exporting"
            );
            return env;
        }

        if !OTLP_PROTOCOLS.contains(&self.protocol.as_str()) {
            tracing::warn!(
                protocol = %self.protocol,
                "telemetry protocol is not one Claude Code accepts; not exporting"
            );
            return env;
        }

        env.insert("CLAUDE_CODE_ENABLE_TELEMETRY".to_owned(), "1".to_owned());
        env.insert("OTEL_METRICS_EXPORTER".to_owned(), "otlp".to_owned());
        env.insert(
            "OTEL_EXPORTER_OTLP_ENDPOINT".to_owned(),
            endpoint.to_owned(),
        );
        env.insert(
            "OTEL_EXPORTER_OTLP_PROTOCOL".to_owned(),
            self.protocol.clone(),
        );

        if let Some(headers) = self.headers.as_deref().filter(|h| !h.trim().is_empty()) {
            env.insert(
                "OTEL_EXPORTER_OTLP_HEADERS".to_owned(),
                headers.trim().to_owned(),
            );
        }

        // Set either way rather than only when on: the variable's absence and
        // `none` mean the same thing to Claude Code, but a person reading the
        // launch environment of a pane should be able to see that the decision
        // was made rather than wonder whether it was forgotten.
        env.insert(
            "OTEL_LOGS_EXPORTER".to_owned(),
            if self.logs { "otlp" } else { "none" }.to_owned(),
        );

        if let Some(interval) = self.export_interval_ms {
            env.insert(
                "OTEL_METRIC_EXPORT_INTERVAL".to_owned(),
                interval.to_string(),
            );
        }

        env
    }
}
