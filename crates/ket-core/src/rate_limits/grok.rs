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
