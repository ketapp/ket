//! Grok's quota, read from its own agent, and what its sessions have spent.
//!
//! Grok publishes its plan's usage in one place: the `/usage` screen of its
//! TUI, which shows the weekly allowance — "Weekly limit (SuperGrok Lite),
//! 0%, resets October 7" — beside a session's own numbers. Its status line
//! leaves the rate limits out on purpose, and `grok usage <session>` only
//! prints what one session spent. What the screen draws from is an
//! undocumented ACP extension method, `x.ai/billing`, which the agent
//! answers by asking xAI with the login it already holds. So ket asks the
//! agent: `grok agent --no-leader stdio`, `initialize`, then
//! `_x.ai/billing` — the leading underscore is how ACP carries an extension
//! method on the wire — and nothing else. No session is created, no prompt
//! sent, no token spent, and `~/.grok/auth.json` is never opened: the same
//! arrangement as Codex's `app-server`, found against Grok 1.0.44 on
//! 2026-09-30.
//!
//! The answer, as it came back that morning:
//!
//! ```json
//! {"config": {"currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY",
//!   "start": "2026-09-30T13:51:59Z", "end": "2026-10-07T13:51:59Z"},
//!   "onDemandCap": {"val": 0}, "isUnifiedBillingUser": true, ...},
//!  "subscription_tier": "SuperGrok Lite"}
//! ```
//!
//! **The used share is not in it.** That week had begun two minutes before,
//! the TUI said 0%, and a protobuf-shaped answer leaves out a field at its
//! zero. The binary's field list names it `creditUsagePercent`, beside the
//! period; it is read as a percentage, and absent is read as the 0% the TUI
//! shows. Neither has been seen on a non-zero week yet — check it against
//! `/usage` the first time the bar moves.
//!
//! Beside that, two files Grok keeps for every session under
//! `~/.grok/sessions/<cwd>/<session>/`, readable by their owner and neither
//! a credential:
//!
//! - `usage.json`, the very document `grok usage` prints: tokens and cost for
//!   every turn, and when each turn ended. Cost is in integer ticks, 10¹⁰ to
//!   the dollar, and is left off traffic the server did not price —
//!   subscription traffic, often — so an unpriced turn makes the period's
//!   cost unknown rather than smaller.
//! - `updates.jsonl`, the session's event log. A request the API refuses with
//!   a 429 is recorded there as a `retry_state` whose `error_type` is
//!   `rate_limited`, and Grok retries it, up to fifteen times. Almost every
//!   one of those is over in seconds; what is worth saying is a refusal that
//!   nothing has been answered since.
//!
//! Those two are attached to whatever the agent said, and survive it saying
//! nothing: a Grok that cannot be started still shows what it spent.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::opencode::parse_rfc3339_ms;
use super::*;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// The periods spend is summed over, as the panel labels them.
const PERIODS: [(&str, u64); 2] = [("24h", DAY_MS), ("7d", 7 * DAY_MS)];

/// Dollars per cost tick, from Grok's own documentation of `usage.json`.
const USD_PER_TICK: f64 = 1e-10;

/// How long a refusal nothing has been answered since keeps saying so.
///
/// Long enough to outlast Grok's fifteen retries and the person reading it;
/// short enough that a limit hit before lunch is not still claimed after.
const LIMIT_HOLDS_MS: u64 = 30 * 60 * 1000;

/// How much of the end of a session's log is read for its latest refusal.
/// A long session's log runs to megabytes, and only its end is news.
const LOG_TAIL: u64 = 256 * 1024;

/// The JSON-RPC id of the `_x.ai/billing` request — one per fetch.
const BILLING_REQUEST_ID: i64 = 2;

/// Grok's usage: its allowance from its agent, its spend from its home.
#[derive(Debug, Clone)]
pub struct GrokRateLimitSource {
    home: PathBuf,
    harness: Arc<dyn ProcessHarness>,
}

impl Default for GrokRateLimitSource {
    fn default() -> Self {
        Self::new()
    }
}

impl GrokRateLimitSource {
    /// Grok's home — `GROK_HOME`, else `~/.grok`, as Grok itself resolves
    /// it — and the real `grok`.
    pub fn new() -> Self {
        let home = std::env::var_os("GROK_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".grok")))
            .unwrap_or_else(|| PathBuf::from(".grok"));
        Self::with(home, Arc::new(RealProcessHarness))
    }

    /// A source over another Grok home and an injected [`ProcessHarness`].
    pub fn with(home: PathBuf, harness: Arc<dyn ProcessHarness>) -> Self {
        Self { home, harness }
    }

    /// Only what Grok's home says as of `now_ms` — spend and refusals, no
    /// allowance — with the clock handed in.
    pub fn local_at(&self, now_ms: u64) -> ProviderSnapshot {
        let mut snapshot = ProviderSnapshot::unsupported(Provider::Grok);
        attach_local(&mut snapshot, &self.home, now_ms);
        snapshot
    }

    /// The `grok` to run: on `PATH` or through the login shell, as ket finds
    /// an agent, else where Grok's installer puts it — a window opened from
    /// Finder has neither on its `PATH`.
    fn binary(&self) -> OsString {
        crate::shell::probe("grok")
            .map(OsString::from)
            .unwrap_or_else(|| self.home.join("bin").join("grok").into_os_string())
    }

    fn command(&self, cwd: &Path) -> HarnessCommand {
        let stdin_lines = vec![
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": { "name": "ket", "version": crate::build_info::VERSION }
                }
            })
            .to_string(),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": BILLING_REQUEST_ID,
                "method": "_x.ai/billing",
                "params": {}
            })
            .to_string(),
        ];
        let mut env = Vec::new();
        // As for Claude: a window opened from Finder may have no `USER`, and
        // an agent that cannot name its user may not find its own login.
        if std::env::var_os("USER").is_none()
            && let Some(user) = super::claude::current_username()
        {
            env.push((OsString::from("USER"), OsString::from(user)));
        }
        HarnessCommand {
            program: self.binary(),
            // Its own agent, not the shared leader a running TUI may have
            // started: a fetch should neither wake one nor lean on one.
            args: ["agent", "--no-leader", "stdio"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            cwd: cwd.to_path_buf(),
            stdin_lines,
            env,
            // An ACP agent reads a closed stdin as the client gone.
            close_stdin: false,
        }
    }

    /// Asks the agent in `cwd` for the allowance.
    fn billing_in(&self, cwd: &Path, timeout: Duration) -> ProviderSnapshot {
        let mut child = match self.harness.spawn(&self.command(cwd)) {
            Ok(child) => child,
            Err(e) => {
                return ProviderSnapshot::unavailable(
                    Provider::Grok,
                    format!("could not start grok: {e}"),
                );
            }
        };
        let deadline = Instant::now() + timeout;
        let result = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break ProviderSnapshot::unavailable(
                    Provider::Grok,
                    "timed out waiting for grok's billing",
                );
            }
            match child.next_line(remaining) {
                Ok(Some(line)) => match interpret_billing_line(&line) {
                    Some(snapshot) => break snapshot,
                    None => continue,
                },
                Ok(None) => {
                    break ProviderSnapshot::unavailable(
                        Provider::Grok,
                        child
                            .outcome()
                            .describe("grok agent exited without answering"),
                    );
                }
                Err(e) => {
                    break ProviderSnapshot::unavailable(
                        Provider::Grok,
                        format!("reading grok's output failed: {e}"),
                    );
                }
            }
        };
        child.kill();
        result
    }
}

impl RateLimitSource for GrokRateLimitSource {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn fetch(&self, timeout: Duration) -> ProviderSnapshot {
        // Switched off in settings, or no Grok here at all: nothing is
        // started and nothing of Grok's is read. Settings are read per fetch,
        // as OpenCode's key is, so the switch takes effect without a restart.
        if !grok_enabled() || !self.home.is_dir() {
            return ProviderSnapshot::unsupported(Provider::Grok);
        }
        let mut snapshot = match new_scratch_dir() {
            Ok(scratch) => {
                let snapshot = self.billing_in(&scratch, timeout);
                let _ = std::fs::remove_dir_all(&scratch);
                snapshot
            }
            Err(e) => ProviderSnapshot::unavailable(
                Provider::Grok,
                format!("could not prepare a scratch directory: {e}"),
            ),
        };
        attach_local(&mut snapshot, &self.home, now_ms());
        snapshot
    }
}

/// Whether Grok is switched on in ket's settings — see
/// [`crate::config::AgentConfig::disabled`]. A settings file that will not
/// load switches nothing off, as it does for every other agent.
fn grok_enabled() -> bool {
    !Config::load()
        .unwrap_or_default()
        .agent
        .disabled
        .contains("grok")
}

/// Reads one line of the agent's output: `None` until it is the answer to
/// `_x.ai/billing`, then the snapshot it makes, good or bad.
fn interpret_billing_line(line: &str) -> Option<ProviderSnapshot> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("id").and_then(serde_json::Value::as_i64) != Some(BILLING_REQUEST_ID) {
        return None;
    }
    let unavailable = |why: String| Some(ProviderSnapshot::unavailable(Provider::Grok, why));
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return unavailable(format!("grok: {message}"));
    }
    let Some(config) = value.pointer("/result/config") else {
        return unavailable("grok's billing had no allowance in it".to_owned());
    };

    let time = |pointer: &str| {
        config
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .and_then(parse_rfc3339_ms)
    };
    let start = time("/currentPeriod/start").or_else(|| time("/billingPeriodStart"));
    let end = time("/currentPeriod/end").or_else(|| time("/billingPeriodEnd"));
    let minutes = start
        .zip(end)
        .and_then(|(start, end)| end.checked_sub(start))
        .and_then(|span| u32::try_from(span / 60_000).ok());
    let fallback = match config
        .pointer("/currentPeriod/type")
        .and_then(serde_json::Value::as_str)
    {
        Some("USAGE_PERIOD_TYPE_WEEKLY") => "Weekly",
        Some("USAGE_PERIOD_TYPE_MONTHLY") => "Monthly",
        _ => "Allowance",
    };
    // Left out at zero, as the TUI's 0% says — see the module docs.
    let used = match config.get("creditUsagePercent") {
        None => 0.0,
        Some(value) => match value.as_f64() {
            Some(used) => used as f32,
            None => return unavailable(format!("grok's usage share was {value}")),
        },
    };

    Some(ProviderSnapshot {
        provider: Provider::Grok,
        status: SnapshotStatus::Fresh,
        windows: vec![RateWindow {
            name: window_label(minutes, fallback),
            used_percent: used,
            resets_at_ms: end,
            window_minutes: minutes,
        }],
        plan: value
            .pointer("/result/subscription_tier")
            .and_then(serde_json::Value::as_str)
            .filter(|tier| !tier.is_empty())
            .map(str::to_owned),
        account: None,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    })
}

/// Adds what Grok's home says as of `now_ms` — spend, and a refusal nothing
/// has been answered since — to `snapshot`. Nothing, when Grok has never
/// run here.
fn attach_local(snapshot: &mut ProviderSnapshot, home: &Path, now_ms: u64) {
    let root = home.join("sessions");
    if !root.is_dir() {
        return;
    }
    let sessions = sessions(&root);
    snapshot.spend = spend(&sessions, now_ms);
    snapshot.limited_at_ms = limited_at(&sessions, now_ms);
}

/// Every session directory: `sessions/<cwd>/<session>/`.
fn sessions(root: &Path) -> Vec<PathBuf> {
    let dirs = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect()
    };
    dirs(root).iter().flat_map(|cwd| dirs(cwd)).collect()
}

/// Whether `path` was written within `span_ms` of `now_ms`.
fn touched_within(path: &Path, now_ms: u64, span_ms: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_millis()).ok())
        .is_some_and(|modified| now_ms.saturating_sub(modified) <= span_ms)
}

/// What every session spent over each of [`PERIODS`].
fn spend(sessions: &[PathBuf], now_ms: u64) -> Vec<Spend> {
    #[derive(Default)]
    struct Tally {
        tokens: u64,
        ticks: u64,
        priced: bool,
        sessions: u32,
    }
    let widest = PERIODS.iter().map(|(_, span)| *span).max().unwrap_or(0);
    let mut tallies: Vec<Tally> = PERIODS
        .iter()
        .map(|_| Tally {
            priced: true,
            ..Tally::default()
        })
        .collect();

    for dir in sessions {
        let path = dir.join("usage.json");
        // A file untouched for longer than the widest period has no turn in
        // any of them.
        if !touched_within(&path, now_ms, widest) {
            continue;
        }
        let Some(usage) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            continue;
        };
        let turns = usage
            .get("turns")
            .and_then(|turns| turns.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();

        for ((_, span), tally) in PERIODS.iter().zip(&mut tallies) {
            let mut counted = false;
            for turn in turns {
                let Some(ended) = turn
                    .get("endedAt")
                    .and_then(|ended| ended.as_str())
                    .and_then(parse_rfc3339_ms)
                else {
                    continue;
                };
                if now_ms.saturating_sub(ended) > *span {
                    continue;
                }
                counted = true;
                tally.tokens += turn
                    .get("totalTokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let partial = turn
                    .get("costIsPartial")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                match turn.get("costUsdTicks").and_then(serde_json::Value::as_u64) {
                    Some(ticks) if !partial => tally.ticks += ticks,
                    _ => tally.priced = false,
                }
            }
            if counted {
                tally.sessions += 1;
            }
        }
    }

    PERIODS
        .iter()
        .zip(tallies)
        .map(|((period, _), tally)| Spend {
            period: (*period).to_owned(),
            tokens: tally.tokens,
            // Nothing spent is nothing priced, which costs nothing.
            cost_usd: tally.priced.then_some(tally.ticks as f64 * USD_PER_TICK),
            sessions: tally.sessions,
        })
        .collect()
}

/// When Grok last had a request refused for going too fast with nothing
/// answered since, if that was within [`LIMIT_HOLDS_MS`].
///
/// A limit is the account's, not one session's, so an answer in any session
/// after the refusal means it has passed.
fn limited_at(sessions: &[PathBuf], now_ms: u64) -> Option<u64> {
    let mut refused = None::<u64>;
    let mut answered = 0u64;
    for dir in sessions {
        let log = dir.join("updates.jsonl");
        if !touched_within(&log, now_ms, LIMIT_HOLDS_MS) {
            continue;
        }
        for line in tail(&log).lines() {
            // Most lines are neither; parsing only the ones that might be
            // keeps a busy log cheap to scan.
            if !(line.contains("retry_state")
                || line.contains("_chunk")
                || line.contains("_completed"))
            {
                continue;
            }
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            // Seconds, on every line alike: the finer `agentTimestampMs` is
            // not on all of them, and mixing the two would order a refusal
            // and the answer in the same second the wrong way round.
            let Some(at) = event.get("timestamp").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let at = at.saturating_mul(1000);
            let update = &event["params"]["update"];
            match update["sessionUpdate"].as_str() {
                Some("retry_state") if is_rate_limit(update) => {
                    refused = Some(refused.map_or(at, |refused| refused.max(at)));
                }
                Some(
                    "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "turn_completed"
                    | "response_completed"
                    | "reasoning_completed",
                ) => answered = answered.max(at),
                _ => {}
            }
        }
    }
    refused
        .filter(|&refused| refused > answered && now_ms.saturating_sub(refused) <= LIMIT_HOLDS_MS)
}

/// Whether a `retry_state` is Grok being told to slow down, rather than a
/// retry for any other reason.
fn is_rate_limit(update: &serde_json::Value) -> bool {
    update["error_type"].as_str() == Some("rate_limited")
        || update["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("429"))
}

/// The last [`LOG_TAIL`] bytes of `path`, from the first whole line in them.
fn tail(path: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(LOG_TAIL);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.take(LOG_TAIL).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match (start > 0, text.find('\n')) {
        // Started mid-line: that fragment is not a line.
        (true, Some(newline)) => text[newline + 1..].to_owned(),
        (true, None) => String::new(),
        (false, _) => text,
    }
}

#[cfg(test)]
mod tests {
    //! The billing answer, the spend in `usage.json`, and the refusals in
    //! `updates.jsonl`. No real `grok` runs: the agent is the shared
    //! [`FakeHarness`], and Grok's home is a directory the test writes.

    use super::*;
    use crate::rate_limits::fakes::*;

    /// A Grok home under the temp dir, removed when dropped.
    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "ket-grok-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// A session directory, `sessions/<cwd>/<session>/`.
        fn session(&self, cwd: &str, session: &str) -> PathBuf {
            let dir = self.0.join("sessions").join(cwd).join(session);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `ms` since the epoch as Grok writes a time: `2026-10-14T09:30:00Z`.
    fn rfc3339(ms: u64) -> String {
        let secs = ms / 1000;
        let (days, rem) = (secs / 86_400, secs % 86_400);
        // Howard Hinnant's civil-from-days.
        let z = days as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }

    fn billing(result: serde_json::Value) -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": BILLING_REQUEST_ID, "result": result})
            .to_string()
    }

    fn fetch_with(lines: &[&str]) -> ProviderSnapshot {
        let home = Home::new("billing");
        let source =
            GrokRateLimitSource::with(home.0.clone(), Arc::new(FakeHarness::with_lines(lines)));
        source.billing_in(&home.0, Duration::from_secs(5))
    }

    #[test]
    fn the_weekly_allowance_is_read_from_the_billing_answer() {
        // The shape observed on 2026-09-30, with a used share filled in.
        let answer = billing(serde_json::json!({
            "config": {
                "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-09-30T13:51:59Z", "end": "2026-10-07T13:51:59Z"},
                "creditUsagePercent": 37.5
            },
            "subscription_tier": "SuperGrok Lite"
        }));
        let snapshot = fetch_with(&[
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#,
            r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#,
            &answer,
        ]);
        assert_eq!(snapshot.status, SnapshotStatus::Fresh);
        assert_eq!(snapshot.plan.as_deref(), Some("SuperGrok Lite"));
        let week = &snapshot.windows[0];
        assert_eq!(week.name, "Weekly");
        assert_eq!(week.window_minutes, Some(7 * 24 * 60));
        assert!((week.used_percent - 37.5).abs() < f32::EPSILON);
        assert_eq!(
            week.resets_at_ms,
            super::super::opencode::parse_rfc3339_ms("2026-10-07T13:51:59Z")
        );
    }

    #[test]
    fn a_used_share_left_out_is_the_zero_the_tui_shows() {
        let answer = billing(serde_json::json!({
            "config": {"currentPeriod": {"type": "USAGE_PERIOD_TYPE_MONTHLY"}}
        }));
        let snapshot = fetch_with(&[&answer]);
        let window = &snapshot.windows[0];
        assert_eq!(window.used_percent, 0.0);
        // No dates, so no length: named by its type alone.
        assert_eq!(window.name, "Monthly");
        assert_eq!(window.window_minutes, None);
        assert_eq!(snapshot.plan, None);
    }

    #[test]
    fn the_older_period_fields_are_read_when_the_newer_ones_are_absent() {
        let answer = billing(serde_json::json!({
            "config": {"billingPeriodStart": "2026-10-01T00:00:00Z",
                       "billingPeriodEnd": "2026-10-01T05:00:00Z"}
        }));
        let window = &fetch_with(&[&answer]).windows[0];
        assert_eq!(window.window_minutes, Some(300));
        assert_eq!(window.name, "Session");
    }

    #[test]
    fn an_error_or_an_answer_with_no_allowance_is_unavailable_with_a_reason() {
        let error = serde_json::json!({"jsonrpc": "2.0", "id": BILLING_REQUEST_ID,
            "error": {"code": -32601, "message": "Method not found"}})
        .to_string();
        assert_eq!(
            fetch_with(&[&error]).status,
            SnapshotStatus::Unavailable {
                reason: "grok: Method not found".to_owned()
            }
        );

        let empty = billing(serde_json::json!({}));
        assert!(matches!(
            fetch_with(&[&empty]).status,
            SnapshotStatus::Unavailable { reason } if reason.contains("no allowance")
        ));

        let odd = billing(serde_json::json!({"config": {"creditUsagePercent": "lots"}}));
        assert!(matches!(
            fetch_with(&[&odd]).status,
            SnapshotStatus::Unavailable { reason } if reason.contains("usage share")
        ));
    }

    #[test]
    fn an_agent_that_exits_without_answering_is_unavailable() {
        let snapshot = fetch_with(&[r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, "not json"]);
        assert!(
            matches!(snapshot.status, SnapshotStatus::Unavailable { .. }),
            "{:?}",
            snapshot.status
        );
    }

    #[test]
    fn an_agent_that_never_answers_times_out() {
        let home = Home::new("hang");
        let source = GrokRateLimitSource::with(home.0.clone(), Arc::new(FakeHarness::hanging()));
        let started = Instant::now();
        let snapshot = source.billing_in(&home.0, Duration::from_millis(50));
        // The shared fake ends its output once the wait runs out, so this
        // reads as an agent gone quiet rather than the deadline: either way
        // unavailable, never a 0% read off nothing.
        assert!(
            matches!(snapshot.status, SnapshotStatus::Unavailable { .. }),
            "{:?}",
            snapshot.status
        );
        assert!(snapshot.windows.is_empty());
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn the_request_asks_initialize_then_billing_and_nothing_else() {
        let home = Home::new("command");
        let harness = Arc::new(FakeHarness::with_lines(&[]));
        let source = GrokRateLimitSource::with(home.0.clone(), harness.clone());
        let _ = source.billing_in(&home.0, Duration::from_millis(50));
        let spawned = harness.spawned.lock().unwrap();
        let methods: Vec<String> = spawned[0]
            .stdin_lines
            .iter()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["method"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        assert_eq!(methods, ["initialize", "_x.ai/billing"]);
    }

    #[test]
    fn spend_is_summed_per_period_and_unpriced_turns_make_the_cost_unknown() {
        let home = Home::new("spend");
        let now = now_ms();
        let hour = 60 * 60 * 1000;
        std::fs::write(
            home.session("repo", "s1").join("usage.json"),
            serde_json::json!({"turns": [
                {"endedAt": rfc3339(now - hour), "totalTokens": 1000, "costUsdTicks": 20_000_000_000u64},
                {"endedAt": rfc3339(now - 3 * 24 * hour), "totalTokens": 500, "costUsdTicks": 10_000_000_000u64},
                {"endedAt": rfc3339(now - 30 * 24 * hour), "totalTokens": 9999, "costUsdTicks": 1u64},
                {"totalTokens": 7}
            ]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            home.session("other", "s2").join("usage.json"),
            serde_json::json!({"turns": [
                {"endedAt": rfc3339(now - 2 * 24 * hour), "totalTokens": 250, "costIsPartial": true, "costUsdTicks": 5u64}
            ]})
            .to_string(),
        )
        .unwrap();

        let snapshot =
            GrokRateLimitSource::with(home.0.clone(), Arc::new(FakeHarness::with_lines(&[])))
                .local_at(now);
        let [day, week] = snapshot.spend.as_slice() else {
            panic!("{:?}", snapshot.spend);
        };
        assert_eq!(day.period, "24h");
        assert_eq!(day.tokens, 1000);
        assert_eq!(day.sessions, 1);
        assert_eq!(day.cost_usd, Some(2.0));

        assert_eq!(week.period, "7d");
        assert_eq!(week.tokens, 1750);
        assert_eq!(week.sessions, 2);
        // The second session's turn was partially priced.
        assert_eq!(week.cost_usd, None);
    }

    #[test]
    fn a_home_where_grok_never_ran_has_no_spend_and_no_refusal() {
        let home = Home::new("empty");
        let snapshot =
            GrokRateLimitSource::with(home.0.clone(), Arc::new(FakeHarness::with_lines(&[])))
                .local_at(now_ms());
        assert!(snapshot.spend.is_empty());
        assert_eq!(snapshot.limited_at_ms, None);
    }

    fn update(at_secs: u64, update: serde_json::Value) -> String {
        serde_json::json!({"timestamp": at_secs, "params": {"update": update}}).to_string()
    }

    #[test]
    fn a_refusal_nothing_has_answered_since_is_reported() {
        let home = Home::new("limited");
        let now = now_ms();
        let secs = now / 1000;
        let log = [
            update(
                secs - 120,
                serde_json::json!({"sessionUpdate": "agent_message_chunk"}),
            ),
            update(
                secs - 60,
                serde_json::json!({"sessionUpdate": "retry_state",
                                                  "error_type": "rate_limited"}),
            ),
            // A retry for another reason is not a limit.
            update(
                secs - 30,
                serde_json::json!({"sessionUpdate": "retry_state",
                                                  "error_type": "overloaded"}),
            ),
            "{not json but mentions retry_state".to_owned(),
        ]
        .join("\n");
        std::fs::write(home.session("repo", "s1").join("updates.jsonl"), log).unwrap();

        let source =
            GrokRateLimitSource::with(home.0.clone(), Arc::new(FakeHarness::with_lines(&[])));
        assert_eq!(source.local_at(now).limited_at_ms, Some((secs - 60) * 1000));

        // An answer in any session afterwards means the limit has passed.
        std::fs::write(
            home.session("elsewhere", "s2").join("updates.jsonl"),
            update(
                secs - 10,
                serde_json::json!({"sessionUpdate": "turn_completed"}),
            ),
        )
        .unwrap();
        assert_eq!(source.local_at(now).limited_at_ms, None);
    }

    #[test]
    fn a_429_in_the_reason_counts_as_a_limit_and_an_old_one_does_not_hold() {
        assert!(is_rate_limit(
            &serde_json::json!({"reason": "HTTP 429 Too Many Requests"})
        ));
        assert!(!is_rate_limit(&serde_json::json!({"reason": "HTTP 500"})));

        let home = Home::new("old-limit");
        let now = now_ms();
        let long_ago = (now - LIMIT_HOLDS_MS - 60_000) / 1000;
        std::fs::write(
            home.session("repo", "s1").join("updates.jsonl"),
            update(
                long_ago,
                serde_json::json!({"sessionUpdate": "retry_state",
                                                 "reason": "429"}),
            ),
        )
        .unwrap();
        let source =
            GrokRateLimitSource::with(home.0.clone(), Arc::new(FakeHarness::with_lines(&[])));
        assert_eq!(source.local_at(now).limited_at_ms, None);
    }

    #[test]
    fn only_the_end_of_a_long_log_is_read_and_from_a_whole_line() {
        let home = Home::new("tail");
        let path = home.0.join("updates.jsonl");
        let filler = "x".repeat(100);
        let mut text = String::new();
        while text.len() < (LOG_TAIL as usize) + 10_000 {
            text.push_str(&filler);
            text.push('\n');
        }
        text.push_str("last line\n");
        std::fs::write(&path, &text).unwrap();
        let tailed = tail(&path);
        assert!(tailed.len() <= LOG_TAIL as usize);
        assert!(tailed.ends_with("last line\n"));
        assert!(tailed.starts_with(&filler), "starts on a whole line");
        assert_eq!(tail(&home.0.join("missing")), "");
    }

    #[test]
    fn the_test_clock_formats_as_grok_writes_it() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            super::super::opencode::parse_rfc3339_ms(&rfc3339(1_791_000_000_000)),
            Some(1_791_000_000_000)
        );
    }
}
