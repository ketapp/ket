//! The organisation's own numbers, from Anthropic's Admin API.
//!
//! Everything else in this crate measures what happens on *this* machine. That
//! is the right unit for a developer and the wrong one for a team: asking what
//! a change saved wants the whole organisation's before and after, across every developer, including the ones who never ran ket. Two
//! endpoints answer that, and reading them costs no tokens at all — they are
//! reports about spending, not requests to a model.
//!
//! - `/v1/organizations/usage_report/messages` — token counts per bucket,
//!   split into exactly the five classes [`crate::rates`] prices, the
//!   five-minute and one-hour cache writes included.
//! - `/v1/organizations/cost_report` — what Anthropic actually charged, which
//!   is the only figure in this crate that is a bill rather than an
//!   estimate.
//!
//! Those two together are what makes a contracted-rate recompute checkable:
//! `cost_report` grouped by description splits by `token_type`, whose values
//! are the same five classes, so a rate table can be validated against real
//! money instead of asserted.
//!
//! ## API organisations only
//!
//! This is not available on a Pro or Max subscription — there is no
//! organisation behind one, and no admin key to read it with. A subscriber's
//! before-and-after is the headroom this crate already measures. Nothing here
//! is a fallback for them, and nothing should pretend to be.
//!
//! ## The key is never in the config file
//!
//! [`AdminConfig::key_env`] names an environment variable; the key itself is
//! read from the environment at the moment of the call. An admin key can read
//! an entire organisation's spending, and config files get committed, synced
//! and pasted into issues. Naming the variable is as much as a file should
//! know.
//!
//! It does not reach the command line either. `curl` arguments are visible in
//! `ps` to every user on the machine, so the URL and the headers go in on
//! **stdin** as a curl config, and the process table sees `curl --config -`.

use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{KetError, Result};

/// The API version header every request carries.
const API_VERSION: &str = "2023-06-01";

/// Where the reports live.
const BASE: &str = "https://api.anthropic.com/v1/organizations";

/// How long to wait before giving up on a report.
///
/// Longer than [`crate::pack`]'s, because this is a report over a date range
/// rather than one small document, and because nothing renders while it runs —
/// it is a deliberate act by somebody preparing a number, not a launch path.
const TIMEOUT: Duration = Duration::from_secs(60);

/// How ket reaches an organisation's reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    /// The environment variable holding the admin key.
    ///
    /// The variable's *name*, never the key — see this module's header. The
    /// default is Anthropic's own conventional spelling, so an organisation
    /// that already exports it needs no configuration at all.
    pub key_env: String,
    /// Restrict every report to these workspaces, when the organisation
    /// separated its Claude Code spending into one.
    ///
    /// Empty means the whole organisation, which is the honest default: a
    /// organisation that did not separate its workspaces has no per-team number to
    /// give, and quietly reporting a subset would be worse than reporting the
    /// total and saying so.
    pub workspace_ids: Vec<String>,
    /// Restrict every report to these API keys. See
    /// [`AdminConfig::workspace_ids`].
    pub api_key_ids: Vec<String>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            key_env: "ANTHROPIC_ADMIN_KEY".to_owned(),
            workspace_ids: Vec::new(),
            api_key_ids: Vec::new(),
        }
    }
}

impl AdminConfig {
    /// The admin key, from the environment.
    ///
    /// `None` when the variable is unset or empty, which is every install that
    /// has not been given one — and the state in which nothing here runs.
    pub fn key(&self) -> Option<String> {
        std::env::var(&self.key_env)
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }
}

/// One bucket's worth of token counts, for one grouping.
///
/// Field names match the wire exactly, so this type can be read beside the API
/// reference without a translation step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageRow {
    /// The model, when grouped by it. `null` on the wire otherwise.
    pub model: Option<String>,
    /// Fresh input tokens.
    pub uncached_input_tokens: u64,
    /// Input tokens served from cache.
    pub cache_read_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cache writes, split by the lifetime written.
    pub cache_creation: CacheCreation,
    /// The workspace, when grouped by it.
    pub workspace_id: Option<String>,
    /// The API key, when grouped by it.
    pub api_key_id: Option<String>,
}

/// Cache-write tokens, split by the lifetime they bought.
///
/// The split matters because the two are priced differently, and the whole
/// point of the TTL lever is that the dearer write is cheaper overall when the
/// session outlives five minutes. A total would hide exactly the thing being
/// argued about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheCreation {
    /// Tokens written into a five-minute cache.
    pub ephemeral_5m_input_tokens: u64,
    /// Tokens written into a one-hour cache.
    pub ephemeral_1h_input_tokens: u64,
}

/// One bucket's worth of cost, for one grouping.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostRow {
    /// **In the lowest currency unit, as a decimal string** — `"123.45"` in
    /// USD is one dollar and twenty-three cents, not a hundred and twenty-three
    /// dollars.
    ///
    /// Kept as the string the wire sent and converted by
    /// [`CostRow::micros`], deliberately: a hundredfold error in a figure put
    /// in front of a team is the single worst thing this module could do, and
    /// the conversion is worth having in exactly one place with the unit
    /// written above it.
    pub amount: String,
    /// Always `"USD"` today, and read rather than assumed.
    pub currency: String,
    /// What kind of cost this is — `tokens`, `web_search`, `code_execution`,
    /// `session_usage` — when grouped by description.
    pub cost_type: Option<String>,
    /// Which token class, when this is a token cost grouped by description:
    /// `uncached_input_tokens`, `output_tokens`, `cache_read_input_tokens`,
    /// `cache_creation.ephemeral_5m_input_tokens` or
    /// `cache_creation.ephemeral_1h_input_tokens`.
    ///
    /// The same five classes [`UsageRow`] counts and [`crate::rates`] prices,
    /// which is what lets a contracted figure be checked against a real bill.
    pub token_type: Option<String>,
    /// The model, when grouped by description.
    pub model: Option<String>,
    /// A human description, e.g. `"Claude Opus 5 Usage - Input Tokens"`.
    pub description: Option<String>,
    /// The workspace, when grouped by it.
    pub workspace_id: Option<String>,
}

impl CostRow {
    /// The amount in micros, from the cents-as-a-decimal-string on the wire.
    ///
    /// One cent is ten thousand micros. `None` when the string will not parse,
    /// which is a row to drop rather than a total to quietly understate — see
    /// [`CostReport::total_micros`], which refuses rather than guesses.
    pub fn micros(&self) -> Option<i64> {
        let cents: f64 = self.amount.trim().parse().ok()?;
        if !cents.is_finite() {
            return None;
        }
        Some((cents * 10_000.0).round() as i64)
    }
}

/// One time bucket of a report.
///
/// `Default` is spelled out rather than derived: the derive would demand
/// `Row: Default` on the whole type, which is a bound the rows do not need and
/// the reports never use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, bound = "Row: serde::de::DeserializeOwned + Serialize")]
pub struct Bucket<Row> {
    /// Start of the bucket, inclusive, RFC 3339.
    pub starting_at: String,
    /// End of the bucket, exclusive, RFC 3339.
    pub ending_at: String,
    /// The rows in it. Empty for an interval with no usage, which the API
    /// returns rather than omitting.
    pub results: Vec<Row>,
}

impl<Row> Default for Bucket<Row> {
    fn default() -> Self {
        Self {
            starting_at: String::new(),
            ending_at: String::new(),
            results: Vec::new(),
        }
    }
}

/// A whole report page.
///
/// See [`Bucket`] on why `Default` is written out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, bound = "Row: serde::de::DeserializeOwned + Serialize")]
pub struct Report<Row> {
    /// Buckets, oldest first.
    pub data: Vec<Bucket<Row>>,
    /// Whether another page follows.
    pub has_more: bool,
    /// The cursor for it, or `null`.
    pub next_page: Option<String>,
}

impl<Row> Default for Report<Row> {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            has_more: false,
            next_page: None,
        }
    }
}

/// Token counts across an organisation.
pub type UsageReport = Report<UsageRow>;

/// What an organisation was charged.
pub type CostReport = Report<CostRow>;

impl UsageReport {
    /// Every row, across every bucket.
    pub fn rows(&self) -> impl Iterator<Item = &UsageRow> {
        self.data.iter().flat_map(|bucket| bucket.results.iter())
    }

    /// Tokens per class, summed over the whole report, keyed by model.
    ///
    /// Rows not grouped by model land under `None`, rather than being dropped
    /// or attributed to a model that was never named.
    pub fn by_model(&self) -> std::collections::BTreeMap<Option<String>, UsageRow> {
        let mut totals: std::collections::BTreeMap<Option<String>, UsageRow> =
            std::collections::BTreeMap::new();
        for row in self.rows() {
            let total = totals.entry(row.model.clone()).or_default();
            total.model = row.model.clone();
            total.uncached_input_tokens += row.uncached_input_tokens;
            total.cache_read_input_tokens += row.cache_read_input_tokens;
            total.output_tokens += row.output_tokens;
            total.cache_creation.ephemeral_5m_input_tokens +=
                row.cache_creation.ephemeral_5m_input_tokens;
            total.cache_creation.ephemeral_1h_input_tokens +=
                row.cache_creation.ephemeral_1h_input_tokens;
        }
        totals
    }
}

impl CostReport {
    /// Every row, across every bucket.
    pub fn rows(&self) -> impl Iterator<Item = &CostRow> {
        self.data.iter().flat_map(|bucket| bucket.results.iter())
    }

    /// The whole report's cost in micros.
    ///
    /// `Err` rather than a silent undercount when any row's amount will not
    /// parse: this is the one number here that is a bill, and a total
    /// quietly missing a row is worse than no total at all.
    pub fn total_micros(&self) -> Result<i64> {
        let mut total: i64 = 0;
        for row in self.rows() {
            let micros = row.micros().ok_or_else(|| {
                KetError::Config(format!(
                    "cost report has an unreadable amount: {}",
                    row.amount
                ))
            })?;
            total = total.saturating_add(micros);
        }
        Ok(total)
    }
}

/// A `GET` against the Admin API, with the key kept off the command line.
///
/// `query` is passed as already-encoded `key=value` pairs. Everything sensitive
/// — the URL and both headers — goes to `curl` on stdin as a config file, so
/// the process table shows only `curl --config -`. Anthropic's own examples use
/// `curl` with an `Authorization` bearer, which is exactly what this builds.
///
/// This does not reuse [`crate::pack::fetch`]: that one caps the response at a
/// pack's size, sends no headers and has a launch-path timeout, none of which
/// suits a report. What they share is the rule that matters — arguments as
/// argv, never through a shell.
fn get(path: &str, query: &[(&str, String)], key: &str) -> Result<serde_json::Value> {
    let mut url = format!("{BASE}/{path}");
    if !query.is_empty() {
        let pairs: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect();
        url.push('?');
        url.push_str(&pairs.join("&"));
    }

    // curl's own config format. Quoted values, so a cursor containing an
    // awkward character cannot end the line early.
    let config = format!(
        "url = \"{url}\"\nheader = \"Authorization: Bearer {key}\"\nheader = \
         \"anthropic-version: {API_VERSION}\"\n"
    );

    let mut child = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--max-redirs")
        .arg("3")
        .arg("--max-time")
        .arg(TIMEOUT.as_secs().to_string())
        // The body is wanted even on a failure: the API says why in JSON, and
        // "401 unauthorized" is far more use than "curl exited 22".
        .arg("--write-out")
        .arg("\n%{http_code}")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| KetError::Config(format!("could not run curl: {e}")))?;

    {
        use std::io::Write;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| KetError::Config("curl took no stdin".to_owned()))?;
        stdin
            .write_all(config.as_bytes())
            .map_err(|e| KetError::Config(format!("could not configure curl: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| KetError::Config(format!("curl did not finish: {e}")))?;

    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(KetError::Config(format!(
            "the admin API could not be reached: {}",
            reason.trim()
        )));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let (body, status) = body
        .rsplit_once('\n')
        .ok_or_else(|| KetError::Config("curl reported no status".to_owned()))?;

    if status.trim() != "200" {
        // The API's own message, when it gave one. Far more actionable than the
        // number: an expired key and a key belonging to no organisation are
        // both 401 and want different answers.
        let detail = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|body| {
                body.get("error")?
                    .get("message")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| body.trim().chars().take(200).collect());
        return Err(KetError::Config(format!(
            "the admin API answered {status}: {detail}"
        )));
    }

    serde_json::from_str(body)
        .map_err(|e| KetError::Config(format!("the admin API's answer could not be read: {e}")))
}

/// Percent-encodes everything that is not unreserved.
///
/// Small on purpose: the only values that reach it are RFC 3339 timestamps,
/// opaque cursors and identifiers, so a full URL library would be a dependency
/// bought for four characters. Conservative rather than clever — anything not
/// explicitly safe is escaped.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The scoping every report shares, as query pairs.
fn scope(config: &AdminConfig) -> Vec<(&'static str, String)> {
    let mut query = Vec::new();
    // Repeated rather than comma-joined: these are array parameters, and the
    // API's own reference spells them `workspace_ids[]`.
    for id in &config.workspace_ids {
        query.push(("workspace_ids[]", id.clone()));
    }
    for id in &config.api_key_ids {
        query.push(("api_key_ids[]", id.clone()));
    }
    query
}

/// Token counts for the organisation, from `starting_at` onwards.
///
/// `starting_at` is RFC 3339 and required by the API. Grouped by model, because
/// a token count that does not say which model produced it cannot be priced —
/// see [`crate::rates`].
pub fn messages(config: &AdminConfig, starting_at: &str, days: u32) -> Result<UsageReport> {
    let key = config
        .key()
        .ok_or_else(|| KetError::Config(format!("{} is not set", config.key_env)))?;

    let mut query = vec![
        ("starting_at", starting_at.to_owned()),
        ("bucket_width", "1d".to_owned()),
        ("limit", days.clamp(1, 31).to_string()),
        ("group_by[]", "model".to_owned()),
    ];
    query.extend(scope(config));

    let body = get("usage_report/messages", &query, &key)?;
    serde_json::from_value(body)
        .map_err(|e| KetError::Config(format!("the usage report could not be read: {e}")))
}

/// What the organisation was charged, from `starting_at` onwards.
///
/// Grouped by description, which is what splits the answer by `token_type` —
/// the whole reason to call this rather than only the usage report.
pub fn costs(config: &AdminConfig, starting_at: &str, days: u32) -> Result<CostReport> {
    let key = config
        .key()
        .ok_or_else(|| KetError::Config(format!("{} is not set", config.key_env)))?;

    let mut query = vec![
        ("starting_at", starting_at.to_owned()),
        ("bucket_width", "1d".to_owned()),
        ("limit", days.clamp(1, 31).to_string()),
        ("group_by[]", "description".to_owned()),
    ];
    // The cost report groups by description and workspace only — an
    // `api_key_ids` filter is not one of its parameters, so it is not sent.
    for id in &config.workspace_ids {
        query.push(("workspace_ids[]", id.clone()));
    }

    let body = get("cost_report", &query, &key)?;
    serde_json::from_value(body)
        .map_err(|e| KetError::Config(format!("the cost report could not be read: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_leaves_unreserved_characters_alone() {
        assert_eq!(encode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    }

    #[test]
    fn encode_percent_encodes_everything_else_in_uppercase_hex() {
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("2026-01-01T00:00:00Z"), "2026-01-01T00%3A00%3A00Z");
        assert_eq!(encode("a/b?c=d"), "a%2Fb%3Fc%3Dd");
    }

    #[test]
    fn encode_of_an_empty_string_is_empty() {
        assert_eq!(encode(""), "");
    }

    #[test]
    fn scope_of_a_default_config_is_empty() {
        assert!(scope(&AdminConfig::default()).is_empty());
    }

    #[test]
    fn scope_repeats_each_workspace_and_api_key_id_as_its_own_pair() {
        let config = AdminConfig {
            workspace_ids: vec!["ws1".to_owned(), "ws2".to_owned()],
            api_key_ids: vec!["key1".to_owned()],
            ..AdminConfig::default()
        };

        assert_eq!(
            scope(&config),
            vec![
                ("workspace_ids[]", "ws1".to_owned()),
                ("workspace_ids[]", "ws2".to_owned()),
                ("api_key_ids[]", "key1".to_owned()),
            ]
        );
    }
}
