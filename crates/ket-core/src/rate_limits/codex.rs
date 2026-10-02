//! Codex's quota, read from its app server.

use super::*;

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

/// The JSON-RPC id used for the `account/rateLimits/read` request — fixed,
/// since exactly one request of each kind is sent per fetch.
const CODEX_RATE_LIMITS_REQUEST_ID: i64 = 2;

/// Fetches Codex's plan usage over `codex app-server`'s own JSON-RPC
/// protocol on stdio. See the module docs for why this replaces the raw HTTP
/// endpoint the original brief described.
///
/// **There is a second source for this same number, and it is deliberately not
/// used here.** Every `token_count` record Codex writes into its rollout file
/// carries a `rate_limits` block beside the token counts — `primary` and
/// `secondary` with `used_percent`, `window_minutes` and `resets_at`, plus
/// `plan_type` — which [`crate::sessions::CodexRollout`] is already tailing for
/// [`crate::usage::cache_from_codex`]. It is free, in the sense that the file
/// is being read anyway, and it costs no process spawn at all.
///
/// It stays unused because it answers a narrower question than this fetcher
/// does. A rollout only says what the quota was *the last time that session
/// made a request*: a worktree nobody has touched since this morning reports
/// this morning's percentage, and a person with no Codex session running at all
/// gets nothing. This fetcher answers "what is the quota now", for the account,
/// whether or not anything is running — which is what a status bar is asking.
///
/// If the rollout's copy is ever wired up as well, the two must not both write
/// into the cache without deciding which wins: the fresher `fetched_at_ms`
/// alone is the wrong rule, because a rollout read a second ago can carry a
/// reading from hours before. Prefer this fetcher, and treat the rollout's as a
/// fallback for when it fails.
#[derive(Debug, Clone)]
pub struct CodexRateLimitSource {
    harness: Arc<dyn ProcessHarness>,
    /// The `codex` binary to run.
    binary: OsString,
}

impl CodexRateLimitSource {
    /// A fetcher that spawns the real `codex` binary.
    pub fn new() -> Self {
        Self::with_harness(Arc::new(RealProcessHarness))
    }

    /// A fetcher using an injected [`ProcessHarness`] — for tests.
    pub fn with_harness(harness: Arc<dyn ProcessHarness>) -> Self {
        Self {
            harness,
            binary: OsString::from("codex"),
        }
    }

    fn command(&self, cwd: &Path) -> HarnessCommand {
        let stdin_lines = vec![
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": { "name": "ket", "version": crate::build_info::VERSION }
                }
            })
            .to_string(),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            })
            .to_string(),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": CODEX_RATE_LIMITS_REQUEST_ID,
                "method": "account/rateLimits/read",
                "params": {}
            })
            .to_string(),
        ];

        HarnessCommand {
            program: self.binary.clone(),
            args: vec![OsString::from("app-server")],
            cwd: cwd.to_path_buf(),
            stdin_lines,
            env: Vec::new(),
            // Left open deliberately: `app-server` reads a closed stdin as the
            // client hanging up and stops answering.
            close_stdin: false,
        }
    }

    fn fetch_in(&self, cwd: &Path, timeout: Duration) -> ProviderSnapshot {
        let mut child = match self.harness.spawn(&self.command(cwd)) {
            Ok(child) => child,
            Err(e) => {
                return ProviderSnapshot::unavailable(
                    Provider::Codex,
                    format!("could not start codex: {e}"),
                );
            }
        };

        let deadline = Instant::now() + timeout;
        let result = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break ProviderSnapshot::unavailable(
                    Provider::Codex,
                    "timed out waiting for account/rateLimits/read".to_owned(),
                );
            }
            match child.next_line(remaining) {
                Ok(Some(line)) => match interpret_codex_line(&line) {
                    Some(snapshot) => break snapshot,
                    None => continue,
                },
                Ok(None) => {
                    break ProviderSnapshot::unavailable(
                        Provider::Codex,
                        "codex app-server exited without answering".to_owned(),
                    );
                }
                Err(e) => {
                    break ProviderSnapshot::unavailable(
                        Provider::Codex,
                        format!("reading codex's output failed: {e}"),
                    );
                }
            }
        };

        child.kill();
        result
    }
}

impl Default for CodexRateLimitSource {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitSource for CodexRateLimitSource {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn fetch(&self, timeout: Duration) -> ProviderSnapshot {
        let scratch = match new_scratch_dir() {
            Ok(dir) => dir,
            Err(e) => {
                return ProviderSnapshot::unavailable(
                    Provider::Codex,
                    format!("could not prepare a scratch directory: {e}"),
                );
            }
        };
        let result = self.fetch_in(&scratch, timeout);
        let _ = std::fs::remove_dir_all(&scratch);
        result
    }
}

/// Interprets one line of `codex app-server`'s JSON-RPC output.
///
/// `None` means "not the response to our request, keep reading" — the
/// server's own `initialize` response and unsolicited notifications
/// (`remoteControl/status/changed` was observed) both land here. `Some`
/// means the response with the matching id arrived, whether it parsed
/// cleanly or not: a definitively bad answer to *our* request is reported
/// immediately rather than waiting out the timeout.
fn interpret_codex_line(line: &str) -> Option<ProviderSnapshot> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = value.get("id").and_then(serde_json::Value::as_i64);
    if id != Some(CODEX_RATE_LIMITS_REQUEST_ID) {
        return None;
    }

    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Some(ProviderSnapshot::unavailable(
            Provider::Codex,
            format!("codex app-server: {message}"),
        ));
    }

    let Some(result) = value.get("result") else {
        return Some(ProviderSnapshot::unavailable(
            Provider::Codex,
            "codex app-server response had neither a result nor an error".to_owned(),
        ));
    };

    let Some(rate_limits) = result.get("rateLimits") else {
        return Some(ProviderSnapshot::unavailable(
            Provider::Codex,
            "codex app-server response had no rateLimits field".to_owned(),
        ));
    };

    let mut windows = Vec::new();
    if let Some(window) = rate_limits
        .get("primary")
        .and_then(|v| codex_window(v, "Primary"))
    {
        windows.push(window);
    }
    if let Some(window) = rate_limits
        .get("secondary")
        .and_then(|v| codex_window(v, "Secondary"))
    {
        windows.push(window);
    }

    if windows.is_empty() {
        return Some(ProviderSnapshot::unavailable(
            Provider::Codex,
            "codex reported no rate limit windows".to_owned(),
        ));
    }

    let plan = rate_limits
        .get("planType")
        .and_then(serde_json::Value::as_str)
        .filter(|s| *s != "unknown")
        .map(str::to_owned);
    let account = result
        .get("accountId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);

    Some(ProviderSnapshot {
        provider: Provider::Codex,
        status: SnapshotStatus::Fresh,
        windows,
        plan,
        account,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    })
}

/// Builds one [`RateWindow`] from a `RateLimitWindow` object (`primary` or
/// `secondary`), or `None` if it is `null` or missing `usedPercent`.
fn codex_window(value: &serde_json::Value, fallback: &str) -> Option<RateWindow> {
    if value.is_null() {
        return None;
    }
    let used_percent = value.get("usedPercent")?.as_f64()?;
    let minutes = value
        .get("windowDurationMins")
        .and_then(serde_json::Value::as_u64)
        .and_then(|m| u32::try_from(m).ok());
    let resets_at_ms = value
        .get("resetsAt")
        .and_then(serde_json::Value::as_i64)
        .filter(|secs| *secs >= 0)
        .map(|secs| (secs as u64).saturating_mul(1000));

    Some(RateWindow {
        name: window_label(minutes, fallback),
        used_percent: used_percent.clamp(0.0, 100.0) as f32,
        resets_at_ms,
        window_minutes: minutes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_limits::fakes::*;

    // -- Codex ------------------------------------------------------------------

    #[test]
    fn codex_parses_a_real_account_rate_limits_response() {
        // Redacted copy of a real `account/rateLimits/read` response
        // observed live over `codex app-server` on codex-cli 0.153.4.
        let harness = Arc::new(FakeHarness::with_lines(&[
            r#"{"id":1,"result":{"userAgent":"ket/0.1.0","codexHome":"/x","platformFamily":"unix","platformOs":"macos"}}"#,
            r#"{"method":"remoteControl/status/changed","params":{"status":"disabled"}}"#,
            r#"{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":43,"windowDurationMins":300,"resetsAt":1788719198},"secondary":{"usedPercent":17,"windowDurationMins":10080,"resetsAt":1789251760},"planType":"plus"},"accountId":"acc-123"}}"#,
        ]));
        let source = CodexRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert_eq!(snapshot.status, SnapshotStatus::Fresh);
        assert_eq!(snapshot.plan.as_deref(), Some("plus"));
        assert_eq!(snapshot.account.as_deref(), Some("acc-123"));
        let session = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Session")
            .unwrap();
        assert_eq!(session.used_percent, 43.0);
        assert_eq!(session.window_minutes, Some(300));
        let weekly = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Weekly")
            .unwrap();
        assert_eq!(weekly.used_percent, 17.0);
    }

    #[test]
    fn codex_error_response_is_unavailable_not_a_panic() {
        let harness = Arc::new(FakeHarness::with_lines(&[
            r#"{"id":1,"result":{}}"#,
            r#"{"id":2,"error":{"code":-1,"message":"not logged in"}}"#,
        ]));
        let source = CodexRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn codex_response_missing_rate_limits_degrades_rather_than_panicking() {
        let harness = Arc::new(FakeHarness::with_lines(&[
            r#"{"id":1,"result":{}}"#,
            r#"{"id":2,"result":{"somethingElse":true}}"#,
        ]));
        let source = CodexRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn codex_that_never_answers_reports_unavailable_rather_than_zero() {
        let harness = Arc::new(FakeHarness::hanging());
        let source = CodexRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_millis(30));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }
}
