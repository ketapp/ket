//! How much of a person's plan each agent has used, and when it resets.
//!
//! This is **provider quota** — "you have used 43% of your five-hour window" —
//! not token usage. ACP's `UsageUpdate` (tokens in the current context, per
//! session) is a different task, a different source, and must never be
//! conflated with this one: a context nearing its window and a plan nearing
//! its rate limit look similar in a status bar and mean nothing alike.
//!
//! ## Why this is expensive, and why that shapes everything here
//!
//! Neither provider hands quota over cheaply. Claude Code exposes it three
//! ways, none of them a struct field: a status line a running session already
//! draws, an OAuth endpoint that needs the token that session is holding, and
//! a `/usage` screen the CLI renders locally. Codex exposes it through its own
//! background process's JSON-RPC protocol. All of them mean a fetch is a
//! process spawn with real wall-clock cost. That is why [`RateLimitCache`]
//! exists at all: invariant "nothing may fetch per render" is not a nicety
//! here, it is the difference between a status bar and a fork bomb.
//!
//! **None of these rungs spends a token.** An earlier version of this module
//! read the numbers out of a `rate_limit_event` frame, which meant running a
//! real, billed turn every time the cache went stale — about nine cents an
//! hour to watch a status bar, on Opus. Anything added here should be held to
//! the same standard: a status bar that consumes the quota it reports is a
//! status bar that changes the number it exists to show.
//!
//! ## The hazard this module exists to avoid (read this before touching Claude)
//!
//! The CLI rung starts a real Claude Code process. A real session's first act
//! is project discovery: it walks upward from its working directory looking
//! for `CLAUDE.md`, version control roots, and anything else a normal session
//! would want context from. Start it in the wrong directory — the user's home
//! directory, say, because nothing set a working directory at all — and ket
//! has silently pointed a coding agent's project discovery at someone's entire
//! filesystem. Nothing about that would look wrong at the call site; it would
//! just quietly begin reading and indexing far more than a usage screen.
//!
//! [`new_scratch_dir`] is the mitigation: every hidden session runs in a
//! directory ket created for that purpose, verified by [`is_root_like`] to be
//! neither a filesystem root nor a shallow, shared location before anything is
//! spawned into it. See
//! `the_bounded_scratch_directory_check_rejects_a_root_like_path` below — of
//! everything in this file, that check is the one that matters most, because
//! nothing downstream of it can catch the mistake if it is wrong.
//!
//! ## The second hazard: what a spawned child inherits
//!
//! ket launches agents by typing at a real login shell, so an agent gets the
//! `PATH`, the aliases and the exported `CLAUDE_CONFIG_DIR` a person has. This
//! module spawns directly, and a directly spawned child inherits only what ket
//! itself was given — which, for an app opened from Finder rather than from a
//! terminal, is close to nothing. A `claude` with no `USER` in its environment
//! cannot find its own Keychain item, reports `Not logged in`, and exits
//! having emitted no usage at all.
//!
//! That failure was invisible for as long as it existed, because stderr went
//! to `/dev/null` and the exit code was never read: every distinct cause
//! arrived at the panel as the same sentence. [`ClaudeRateLimitSource::child_env`]
//! is the fix, [`ChildOutcome`] is how a failure now says which one it was.
//!
//! ## On credentials
//!
//! ket used to read no other application's credentials at all, and this
//! module said that changing it was a maintainer's call rather than
//! something to route around quietly here. That call has been made twice
//! now, once per provider:
//!
//! - Claude's OAuth rung reads the token Claude Code is already holding,
//!   from the Keychain first and a `.credentials.json` only where there is
//!   one.
//! - OpenCode Go's key is the one pasted into ket's settings, or
//!   `OPENCODE_API_KEY` in the environment — never lifted from OpenCode's
//!   own `auth.json` or `credential` table: reading another application's
//!   credential files is this repository's policy to refuse, and a
//!   credential ket was never given is a credential ket does not hold. See
//!   [`OpenCodeRateLimitSource`] for the full chain.
//!
//! Codex needs no credential either way: `codex app-server` resolves its own
//! ChatGPT session and hands ket nothing.
//!
//! The rules that came with those decisions, and that a change here must
//! keep:
//!
//! - A credential is read into memory, handed to exactly one child through
//!   its **stdin**, and never written to a file, a log, or an argument
//!   vector. [`ClaudeRateLimitSource::usage_request`] and the OpenCode Go
//!   key's `Authorization` header are shaped the way they are for that
//!   reason alone — a header in `argv` is readable by every process on the
//!   machine for as long as the request lasts.
//! - No credential is ever *stored* by ket, refreshed by ket, or copied
//!   anywhere. A stale token is reported as stale, not repaired.
//! - The rung below it needs no credential at all, so a person who would
//!   rather ket never opened their Keychain loses a reset timestamp, not
//!   the feature. OpenCode Go's floor is [`SnapshotStatus::Unsupported`]
//!   with no fetch at all.
//!
//! ## What was verified on this machine, and what the brief got wrong
//!
//! This module was written against Claude Code 2.1.260, Codex CLI 0.153.4 and
//! OpenCode 1.18.29, actually installed and actually run — not against
//! documentation, because there isn't any for these interfaces.
//!
//! - **Claude, the `rate_limit_event` frame — measured, then abandoned.** A
//!   `claude -p ... --output-format stream-json --verbose` session does emit
//!   `rate_limit_info.unifiedWindows`, keyed by window (`five_hour`,
//!   `seven_day`, and on this account `seven_day_overage_included`), each with
//!   a `utilization` **fraction** and a `resetsAt` unix timestamp. Two things
//!   the original note got wrong, both found by running it again: the frame
//!   arrives *after* the assistant's reply, not before, so the session cannot
//!   be killed early and every fetch is a complete billed turn — 3.3s and
//!   $0.085 on Opus, measured; and a session that cannot log in emits no frame
//!   at all, which is what the panel had been reporting for weeks. The parser
//!   is gone with the mechanism. [`snapshot_from_statusline`] reads the same
//!   `utilization` fraction from a live session, and is the reason that scale
//!   still appears in this file.
//! - **Claude, the OAuth endpoint**: `GET
//!   https://api.anthropic.com/api/oauth/usage`, with the bearer token from
//!   the Keychain, `anthropic-beta: oauth-2025-04-20`, and a `claude-code`
//!   user agent. Answers `five_hour` and `seven_day` objects plus a
//!   per-model weekly window, and — the trap — reports its percentages as
//!   **percentages**, not as the fraction the frame and the status line use.
//!   Same field name, two scales, which is why
//!   [`interpret_claude_usage`] and [`statusline_window`] are separate
//!   functions rather than one shared one.
//! - **Claude, `/usage`**: `claude -p "/usage" --output-format json` renders
//!   the same screen a person sees into `result`, and is answered locally —
//!   verified `num_turns: 0`, `total_cost_usd: 0`, ~2s. It needs no
//!   credential of ket's own, because the CLI resolves its own login. The
//!   screen is written for people, so [`interpret_claude_usage_text`] reads
//!   it the way one does and skips anything it cannot be sure of.
//! - **Codex — the brief's own mechanism was stale.** The brief describes
//!   `GET https://chatgpt.com/backend-api/wham/usage` with hand-assembled
//!   auth headers. That was not used here, and not because it was hard: this
//!   machine's `codex-cli` (0.153.4) has since grown a first-class, if
//!   still `[experimental]`, JSON-RPC method for exactly this —
//!   `account/rateLimits/read` on `codex app-server` — which was confirmed
//!   live and returns `primary`/`secondary` windows with `usedPercent`,
//!   `windowDurationMins`, `resetsAt`, plus `planType` and `accountId`. It is
//!   strictly better than the documented endpoint: `codex app-server` resolves
//!   its own ChatGPT session internally, so ket never touches
//!   `~/.codex/auth.json` or assembles a bearer header at all. Given the
//!   choice between reverse-engineering an HTTP endpoint that needs a
//!   credential file read and calling a method the vendor's own CLI already
//!   exposes over stdio, this module takes the second path.
//! - **OpenCode — the spike asked the wrong product.** Everything the spike
//!   found still holds for OpenCode the CLI: `opencode stats` aggregates its
//!   *own* local session history (tokens and cost across past runs) — the
//!   ACP-`UsageUpdate` kind of data this module must not conflate with quota
//!   — and `opencode providers list` shows which credentials are configured,
//!   not how much of anything is left. That is not an oversight in OpenCode;
//!   it is bring-your-own-key, so the plan limit belongs to whichever backend
//!   provider the key is for, and OpenCode has no view of it.
//!
//!   **OpenCode Go does have a plan**, and reports three concurrent windows:
//!   a five-hour rolling one, a weekly one, and on some plans a thirty-day
//!   one. This reads them through `GET opencode.ai/zen/go/v1/usage`, an
//!   endpoint that takes an OpenCode Go API key — the one pasted into
//!   settings, or `OPENCODE_API_KEY`, never a key lifted from OpenCode's own
//!   credential stores — and answers with each meter already a percentage.
//!   The console session cookie stays as the fallback for a legacy console
//!   account, and with neither configured this still reports
//!   [`SnapshotStatus::Unsupported`], which is the honest answer for a
//!   BYO-key OpenCode. It remains the most fragile fetcher here; see its
//!   own docs for what that costs and how it is contained.
//!
//! ## The normalised model
//!
//! [`ProviderSnapshot`] is one shape every provider maps onto: a
//! [`Provider`], an explicit [`SnapshotStatus`], a list of named
//! [`RateWindow`]s (never a single scalar — "Session", "Weekly" and a
//! per-model window are all windows, and the set genuinely differs per
//! provider and per plan), and whatever plan/account identity the provider
//! was willing to say. `fetched_at_ms` is the "Updated 1m ago" the usage
//! UI shows, and the reason it exists as its own field rather than being
//! implied by "now" is the whole point of [`SnapshotStatus::Stale`]:
//! presenting an old number as current is worse than presenting nothing.
//!
//! `SnapshotStatus` has four states on purpose, not two:
//!
//! - [`SnapshotStatus::Fresh`] — fetched within the cache's TTL.
//! - [`SnapshotStatus::Stale`] — real numbers, just older than the TTL. A
//!   background refresh that failed does not erase the last good snapshot
//!   (see [`RateLimitCache::store`]); it ages into this state instead, which
//!   is a far more honest thing to show than either a stuck "Fresh" badge or
//!   a scary error where a person is used to seeing a percentage.
//! - [`SnapshotStatus::Unavailable`] — no good snapshot exists. A timeout, a
//!   spawn failure, or a response that could not be parsed all land here,
//!   carrying a reason. **This is the state a failed fetch must produce.**
//!   Anything that would otherwise let a failure render as "0% used" is the
//!   bug this module exists to prevent.
//! - [`SnapshotStatus::Unsupported`] — the provider has no quota concept to
//!   report, which is a fact about the provider, not a failure of the fetch.
//!
//! ## Caching, scheduling, and the choice not to use the event bus
//!
//! [`RateLimitCache`] fetches nothing on a plain read: [`RateLimitCache::snapshot`]
//! and [`RateLimitCache::snapshots`] are synchronous map lookups, always. A
//! fetch happens only via [`RateLimitCache::refresh`] /
//! [`RateLimitCache::refresh_all`] (bypassing the TTL unconditionally — the
//! usage UI's manual refresh button needs exactly that) or via the
//! optional background loop started by
//! [`RateLimitCache::spawn_background_refresh`], which owns its own thread
//! and stops when its [`BackgroundRefresh`] handle is dropped, the same idiom
//! [`crate::surface::FileWatcher`] uses for the same reason: nothing here may
//! outlive the thing that asked for it.
//!
//! This module does **not** publish onto [`crate::event::EventBus`]. It was
//! considered — the module doc there frames exactly this kind of change as
//! the reason the bus exists — but [`crate::event::Event`] is deliberately
//! *not* `#[non_exhaustive]` so that adding a variant is a compile error at
//! every match site, and this task has no visibility into what those sites
//! are across a codebase several other agents are actively changing in
//! parallel. A poll-based read (`snapshot`/`snapshots`/`last_updated_ms`) is
//! sufficient for a status bar that redraws on its own timer, and wiring an
//! event is a small, mechanical addition for whoever builds that bar to make
//! once this lands, with full visibility into its own match arms. Static, not
//! load-bearing: the day this needs to be event-driven, nothing about the
//! cache's shape has to change to add it.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{Config, OpenCodeGoConfig};
use crate::{KetError, Result, now_ms};

// ---------------------------------------------------------------------------
// The normalised model
// ---------------------------------------------------------------------------

/// One of the agents ket reads plan usage for.
///
/// Named here rather than reusing the free-form `agent` string used
/// elsewhere (see [`crate::event::Event::AgentSessionStarted`]): quota
/// fetching is a fixed, closed set of mechanisms, and a typo in a
/// config-driven agent name should not silently produce another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    /// Claude Code.
    Claude,
    /// OpenAI's Codex CLI.
    Codex,
    /// OpenCode.
    OpenCode,
    /// xAI's Grok CLI. Publishes no quota at all — see [`GrokRateLimitSource`]
    /// for what it reports instead.
    Grok,
}

impl Provider {
    /// Every provider ket knows how to ask about, in a fixed order.
    pub const ALL: [Provider; 4] = [
        Provider::Claude,
        Provider::Codex,
        Provider::OpenCode,
        Provider::Grok,
    ];

    /// A short human-readable name, for error messages and logs.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
            Provider::OpenCode => "OpenCode",
            Provider::Grok => "Grok",
        }
    }
}

/// One concurrent limit window within a provider's plan.
///
/// A provider reports a list of these rather than a single scalar because the
/// set genuinely varies: a five-hour "Session" window, a rolling "Weekly"
/// window, and sometimes a per-model window, all reported independently and
/// none of them derivable from the others.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateWindow {
    /// A display name — `"Session"`, `"Weekly"`, or a provider-specific label
    /// when neither of those durations matches. See [`window_label`].
    pub name: String,
    /// Percentage of this window used, `0.0..=100.0`.
    ///
    /// Normalised to a percentage at the parse boundary even though Claude's
    /// wire format reports a `0.0..=1.0` fraction and Codex's reports an
    /// integer 0-100 already — a caller drawing a progress bar should never
    /// need to know which provider it came from.
    pub used_percent: f32,
    /// When this window resets, in milliseconds since the Unix epoch — the
    /// same clock as [`crate::now_ms`], converted from whatever unit the
    /// provider used (both observed providers report unix *seconds*).
    pub resets_at_ms: Option<u64>,
    /// The window's duration in minutes, where the provider says so.
    pub window_minutes: Option<u32>,
}

/// Freshness of a [`ProviderSnapshot`], reported explicitly rather than
/// inferred from a missing value.
///
/// See the module docs for why all four states exist and what collapses into
/// which — in particular, why a failed fetch must never be indistinguishable
/// from a provider genuinely at 0%.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SnapshotStatus {
    /// Fetched within the cache's TTL. The numbers in [`ProviderSnapshot::windows`]
    /// are current as of [`ProviderSnapshot::fetched_at_ms`].
    Fresh,
    /// Real numbers, just older than the cache's TTL — the last successful
    /// fetch, kept rather than discarded when a later refresh failed.
    Stale,
    /// No usable snapshot exists: never fetched, the fetch timed out, the
    /// process could not be started, or the response could not be parsed.
    Unavailable {
        /// What went wrong, for a tooltip or a log line — never shown as a
        /// percentage, because there is none to show.
        reason: String,
    },
    /// This provider has no quota concept to report. A fact about the
    /// provider, not a failure — see the OpenCode section of the module docs.
    Unsupported,
}

/// One provider's rate-limit state, normalised.
///
/// This is the read-side view a status bar or a detail popover renders
/// directly — see `status.rs` for the same shape of contract on worktree
/// status. Nothing in this type requires further computation to draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    /// Which agent this describes.
    pub provider: Provider,
    /// Whether the data below is current, stale, absent, or not applicable.
    pub status: SnapshotStatus,
    /// Concurrent limit windows, empty unless [`SnapshotStatus::Fresh`] or
    /// [`SnapshotStatus::Stale`].
    pub windows: Vec<RateWindow>,
    /// The plan or tier, as the provider reported it, unmodified — a plan
    /// name a provider adds tomorrow should show up verbatim rather than
    /// vanish behind a mapping this module would otherwise own and have to
    /// keep current.
    pub plan: Option<String>,
    /// Which account is signed in, where the provider says so. Codex reports
    /// an account id; Claude's frame carries none (verified absent on
    /// 2.1.260), so this is always `None` for [`Provider::Claude`] today.
    pub account: Option<String>,
    /// When this snapshot was captured, in milliseconds since the Unix
    /// epoch. `None` only when [`SnapshotStatus::Unavailable`] with no prior
    /// successful fetch to fall back on, or [`SnapshotStatus::Unsupported`].
    pub fetched_at_ms: Option<u64>,
    /// What the provider's sessions have spent, for a provider that says so
    /// and has no quota to report instead — Grok. Empty for everyone else,
    /// and left off the wire when empty, so a host from before this field
    /// reads every other provider exactly as it did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spend: Vec<Spend>,
    /// When the provider last turned a request away for going too fast and
    /// has not since answered one, in milliseconds since the Unix epoch.
    /// Only where that is all a provider will say about its limits — see
    /// [`GrokRateLimitSource`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited_at_ms: Option<u64>,
}

/// What a provider's sessions spent over one period.
///
/// Spend, not quota: a plan's limit is not a number these providers
/// publish, so this is the nearest honest reading — what went through, and
/// what it cost where the provider priced it. Never drawn as a percentage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spend {
    /// `"24h"`, `"7d"`: how far back this reaches from the reading.
    pub period: String,
    /// Tokens in and out, cached reads included, as the provider counts them.
    pub tokens: u64,
    /// What it cost in US dollars. `None` when any turn in the period went
    /// unpriced — Grok leaves cost off subscription traffic — because a sum
    /// of the priced turns alone would read as the whole bill.
    pub cost_usd: Option<f64>,
    /// How many sessions had a turn in the period.
    pub sessions: u32,
}

impl ProviderSnapshot {
    /// A snapshot for a provider whose fetch failed, or has never
    /// succeeded — never presented as a percentage.
    pub fn unavailable(provider: Provider, reason: impl Into<String>) -> Self {
        Self {
            provider,
            status: SnapshotStatus::Unavailable {
                reason: reason.into(),
            },
            windows: Vec::new(),
            plan: None,
            account: None,
            fetched_at_ms: None,
            spend: Vec::new(),
            limited_at_ms: None,
        }
    }

    /// A snapshot for a provider with no quota concept at all.
    pub fn unsupported(provider: Provider) -> Self {
        Self {
            provider,
            status: SnapshotStatus::Unsupported,
            windows: Vec::new(),
            plan: None,
            account: None,
            fetched_at_ms: None,
            spend: Vec::new(),
            limited_at_ms: None,
        }
    }
}

/// Turns a window's duration into the usage UI's names where they match a
/// duration this module has actually observed, and a duration-qualified
/// fallback otherwise.
///
/// Both providers were observed reporting a five-hour rolling window and a
/// seven-day rolling window under different field names (`five_hour`/`primary`
/// and `seven_day`/`secondary`); this is the one place that correspondence is
/// asserted, so a future provider or plan with a genuinely different window
/// still gets a readable label instead of nothing.
fn window_label(minutes: Option<u32>, fallback: &str) -> String {
    match minutes {
        Some(300) => "Session".to_owned(),
        Some(10_080) => "Weekly".to_owned(),
        Some(43_200) => "Monthly".to_owned(),
        Some(m) => format!("{fallback} ({m}m)"),
        None => fallback.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// The bounded scratch directory
// ---------------------------------------------------------------------------

/// Where hidden harvesting sessions are created — always under the OS temp
/// directory, never a caller-supplied path.
fn scratch_root() -> PathBuf {
    std::env::temp_dir()
}

/// Whether `dir` is unsafe to start a hidden agent session in.
///
/// This is the check described at length in the module docs. A directory is
/// rejected if any of the following hold:
///
/// - it is not absolute, so it cannot be reasoned about without knowing the
///   caller's own working directory;
/// - it is shallower than three path components (`/`, `/tmp`), which rejects
///   the root itself along with shared top-level directories that are not
///   *this session's own* scratch space even though software often runs
///   there;
/// - it falls outside `temp_root` entirely — the one condition that would
///   catch a future refactor accidentally passing a caller-supplied or
///   default-constructed path instead of one this module built;
/// - it *is*, or is an ancestor of, `home` — defence against a misconfigured
///   `$TMPDIR` pointed at or above the user's home directory, which would
///   otherwise slip past the `temp_root` check by construction.
///
/// [`new_scratch_dir`] always builds a path that passes this; the check
/// exists for when that construction is wrong, which is exactly the failure
/// mode with no other safety net.
pub fn is_root_like(dir: &Path, temp_root: &Path, home: Option<&Path>) -> bool {
    if !dir.is_absolute() || dir.components().count() < 3 {
        return true;
    }
    if !dir.starts_with(temp_root) {
        return true;
    }
    if let Some(home) = home
        && (dir == home || home.starts_with(dir))
    {
        return true;
    }
    false
}

/// Creates a fresh, bounded, empty directory for one hidden harvesting
/// session, validated by [`is_root_like`] before anything is spawned into it.
fn new_scratch_dir() -> Result<PathBuf> {
    let temp_root = scratch_root();
    let home = crate::paths::home().ok();
    let dir = temp_root.join(format!("ket-telemetry-{}-{}", std::process::id(), now_ms()));

    if is_root_like(&dir, &temp_root, home.as_deref()) {
        return Err(KetError::Config(format!(
            "refusing to start a hidden telemetry session in {}: it is not a \
             bounded scratch directory",
            dir.display()
        )));
    }

    std::fs::create_dir_all(&dir).map_err(|e| KetError::io(&dir, e))?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// The process seam — see `crate::surface` for the same pattern
// ---------------------------------------------------------------------------

/// A process to spawn for one fetch attempt, and what to hand its stdin.
///
/// `stdin_lines` is written immediately after spawn, one line per entry.
#[derive(Debug, Clone)]
pub struct HarnessCommand {
    /// The program to run — never resolved through a shell.
    pub program: OsString,
    /// Arguments, in order.
    pub args: Vec<OsString>,
    /// Working directory. Always a value from [`new_scratch_dir`] in real
    /// use — see the module docs for why that matters.
    pub cwd: PathBuf,
    /// Lines to write to the child's stdin right after spawn.
    pub stdin_lines: Vec<String>,
    /// Variables to set in the child, on top of what it inherits.
    ///
    /// A directly spawned child inherits ket's own environment, and ket's own
    /// environment is whatever started it — for an app opened from Finder,
    /// almost nothing. See [`ClaudeRateLimitSource::child_env`] for what that
    /// cost before this field existed.
    pub env: Vec<(OsString, OsString)>,
    /// Whether to close stdin once [`Self::stdin_lines`] have been written.
    ///
    /// Load-bearing in both directions. `curl --config -` reads its
    /// configuration until end-of-stream and waits forever without this;
    /// Codex's `app-server` treats a closed stdin as the client hanging up and
    /// answers nothing once it has.
    pub close_stdin: bool,
}

/// A spawned process' stdout, read one line at a time, bounded by a timeout.
///
/// Object-safe on purpose, so a fetcher can hold `Box<dyn ChildLines>`
/// without knowing whether it is a real child process or a test double.
pub trait ChildLines: Send {
    /// The next full line of stdout, waiting at most `timeout`.
    ///
    /// `Ok(None)` means either end-of-stream or the timeout elapsed — the
    /// caller cannot tell which, and does not need to: both mean "nothing
    /// more is coming right now," and a caller tracking its own overall
    /// deadline (as every fetcher here does) treats them identically.
    fn next_line(&mut self, timeout: Duration) -> Result<Option<String>>;

    /// Ends the process. Must be safe to call more than once, and must not
    /// block indefinitely — invariant 2 applies to cleanup too.
    fn kill(&mut self);

    /// What the process said on the way out.
    ///
    /// Defaulted to nothing so a test double can ignore it, because a scripted
    /// transcript has no exit code to report. The real implementation is where
    /// this earns its keep: a fetcher that got no usable stdout can say *why*
    /// instead of guessing, which is the whole difference between "claude
    /// exited without reporting rate limits" and "claude is not logged in".
    fn outcome(&mut self) -> ChildOutcome {
        ChildOutcome::default()
    }
}

/// A finished child's exit code and whatever it wrote to stderr.
#[derive(Debug, Clone, Default)]
pub struct ChildOutcome {
    /// The exit status, where the process had exited by the time it was asked.
    pub code: Option<i32>,
    /// Everything the process wrote to stderr, trimmed.
    pub stderr: String,
}

impl ChildOutcome {
    /// `fallback`, with whatever the process actually said appended.
    ///
    /// Used for the failure messages a person reads in the usage panel, so
    /// they carry the child's own words rather than ket's guess at them.
    fn describe(&self, fallback: &str) -> String {
        let stderr = self.stderr.trim();
        match (self.code, stderr.is_empty()) {
            (_, false) => format!("{fallback}: {}", first_line_of(stderr)),
            (Some(code), true) if code != 0 => format!("{fallback} and exited {code}"),
            _ => fallback.to_owned(),
        }
    }
}

/// The first line of `text`, shortened to something a tooltip can hold.
fn first_line_of(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= 160 {
        return line.to_owned();
    }

    let short: String = line.chars().take(157).collect();
    format!("{short}...")
}

/// Spawns a process for a fetch attempt.
///
/// This is a seam rather than a direct `Command::spawn`, exactly as
/// [`crate::surface::Spawner`] is: a test asserts on the argv and the
/// scripted transcript a fetcher would react to, never on a real `claude` or
/// `codex` binary, a real login session, or a real network call.
pub trait ProcessHarness: Send + Sync + std::fmt::Debug {
    /// Starts `command` and returns a handle to its stdout.
    fn spawn(&self, command: &HarnessCommand) -> Result<Box<dyn ChildLines>>;
}

/// The real harness: spawns an actual child process.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealProcessHarness;

impl ProcessHarness for RealProcessHarness {
    fn spawn(&self, command: &HarnessCommand) -> Result<Box<dyn ChildLines>> {
        use std::io::Write;
        use std::process::Stdio;

        let mut cmd = std::process::Command::new(&command.program);
        cmd.args(&command.args)
            .current_dir(&command.cwd)
            .envs(command.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Kept rather than discarded. A child that fails says why here and
            // nowhere else, and throwing it away is what made every distinct
            // Claude failure read as the same sentence.
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| KetError::Agent {
            agent: command.program.to_string_lossy().into_owned(),
            why: format!("could not start: {e}"),
        })?;

        let stdout = child.stdout.take().expect("stdout was requested as piped");
        let lines = spawn_line_reader(stdout);
        let stderr = child.stderr.take().expect("stderr was requested as piped");
        let errors = spawn_error_reader(stderr);

        let mut stdin = child.stdin.take();
        if command.stdin_lines.is_empty() {
            // Nothing to send; close it so the child never blocks reading a
            // stdin no one is going to write to.
            stdin.take();
        } else if let Some(sink) = stdin.as_mut() {
            for line in &command.stdin_lines {
                // Best-effort: if the child has already exited, the next
                // `next_line` call reports EOF, which is the right outcome
                // either way.
                let _ = writeln!(sink, "{line}");
            }
            let _ = sink.flush();
        }
        if command.close_stdin {
            stdin.take();
        }

        Ok(Box::new(RealChildLines {
            child,
            _stdin: stdin,
            lines,
            errors: Some(errors),
        }))
    }
}

/// Reads `stdout` line by line on a background thread, forwarding each line
/// to a channel so [`RealChildLines::next_line`] can honour a timeout without
/// blocking on a read syscall that may never return — the same shape as
/// [`crate::surface::FileWatcher`]'s notify callback.
fn spawn_line_reader(stdout: std::process::ChildStdout) -> Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // `tx` drops here; a subsequent `recv_timeout` reports `Disconnected`,
        // which `RealChildLines::next_line` treats the same as a timeout.
    });
    rx
}

/// How long [`ChildLines::outcome`] will wait for a child that has closed its
/// stdout to finish exiting. Long enough for a reap, short enough that a
/// wedged process cannot turn a failure report into a hang.
const CHILD_EXIT_GRACE: Duration = Duration::from_millis(250);

/// Reads `stderr` to the end on a background thread.
///
/// A thread rather than a read at exit time, for the reason the pipe exists at
/// all: a child that fills its stderr pipe while nobody is draining it blocks
/// on the write, and a fetcher waiting on stdout would then wait out its whole
/// timeout for a process that had already finished saying what went wrong.
fn spawn_error_reader(stderr: std::process::ChildStderr) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        use std::io::Read;
        let mut text = String::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_string(&mut text);
        text
    })
}

/// A real child process' stdout, exposed one line at a time.
struct RealChildLines {
    child: std::process::Child,
    /// Held only to keep the pipe open until [`RealChildLines::kill`] drops
    /// it; never written to after the constructor.
    _stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    /// Taken by the first [`ChildLines::outcome`] call, which joins it.
    errors: Option<std::thread::JoinHandle<String>>,
}

impl ChildLines for RealChildLines {
    fn next_line(&mut self, timeout: Duration) -> Result<Option<String>> {
        match self.lines.recv_timeout(timeout) {
            Ok(line) => Ok(Some(line)),
            Err(_) => Ok(None),
        }
    }

    fn kill(&mut self) {
        self._stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// The exit code and stderr, with a short grace period and no more.
    ///
    /// `try_wait` in a bounded loop rather than `wait`: this is called on the
    /// failure path, where the child may be wedged rather than exiting, and
    /// invariant 2 forbids blocking there. The grace exists because stdout
    /// reaching end-of-stream and the process actually reaping are separate
    /// events microseconds apart — without it, the common case would lose the
    /// message it is here to collect. A child still running after the grace
    /// reports no code and whatever it has written so far.
    fn outcome(&mut self) -> ChildOutcome {
        let deadline = Instant::now() + CHILD_EXIT_GRACE;
        let mut code = None;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    code = status.code();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }

        let stderr = match self.errors.take() {
            Some(handle) => {
                while !handle.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if handle.is_finished() {
                    handle.join().unwrap_or_default()
                } else {
                    // Still open, which means the child is still running and
                    // holding the pipe. Leaving the thread detached is fine —
                    // it ends when `kill` closes the pipe.
                    String::new()
                }
            }
            None => String::new(),
        };

        ChildOutcome { code, stderr }
    }
}

// ---------------------------------------------------------------------------
// The fetcher trait
// ---------------------------------------------------------------------------

/// Something that can report one provider's rate-limit state.
///
/// `fetch` must never panic and must never block past `timeout` (invariant
/// 2) — a provider that cannot answer in time reports
/// [`SnapshotStatus::Unavailable`], never an `Err` a caller would have to
/// unwrap and never silence into a `0%`.
pub trait RateLimitSource: Send + Sync + std::fmt::Debug {
    /// Which provider this fetches for.
    fn provider(&self) -> Provider;

    /// Fetches a fresh snapshot, bounded by `timeout`.
    fn fetch(&self, timeout: Duration) -> ProviderSnapshot;
}

mod claude;
mod codex;
mod grok;
mod opencode;

pub use claude::*;
pub use codex::*;
pub use grok::*;
pub use opencode::*;

// ---------------------------------------------------------------------------
// Caching and scheduling
// ---------------------------------------------------------------------------

/// Default cache lifetime before a snapshot is old enough to refetch.
///
/// Provider quota moves on the order of hours or days, not seconds, so this
/// favours not spawning a hidden agent session over shaving staleness down
/// further — the explicit refresh path exists for when a person wants the
/// current number right now.
pub const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);

/// Default bound on a single fetch attempt.
///
/// Generous enough for a real `claude` process to spin up and emit its first
/// frame on a slow connection (observed: 2-3 seconds on this machine);
/// short enough that invariant 2 is felt as "a few seconds," not as a status
/// bar stuck for a whole session.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// One provider's last-known state, plus when it was captured — the
/// bookkeeping [`RateLimitCache::snapshot`] uses to decide whether to report
/// it as [`SnapshotStatus::Stale`].
struct CacheEntry {
    snapshot: ProviderSnapshot,
    fetched_at: Instant,
}

/// Caches every provider's [`ProviderSnapshot`] so a render never fetches.
///
/// Reads ([`RateLimitCache::snapshot`], [`RateLimitCache::snapshots`],
/// [`RateLimitCache::last_updated_ms`]) are plain, synchronous map lookups —
/// no I/O, no locking longer than a `HashMap` access. Every fetch goes
/// through [`RateLimitCache::refresh`], [`RateLimitCache::refresh_all`], or
/// the background loop from [`RateLimitCache::spawn_background_refresh`].
#[derive(Debug)]
pub struct RateLimitCache {
    sources: Vec<Arc<dyn RateLimitSource>>,
    ttl: Duration,
    timeout: Duration,
    entries: Mutex<HashMap<Provider, CacheEntry>>,
}

impl std::fmt::Debug for CacheEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheEntry")
            .field("snapshot", &self.snapshot)
            .field("age", &self.fetched_at.elapsed())
            .finish()
    }
}

impl RateLimitCache {
    /// Builds an empty cache over `sources`, with the given TTL and
    /// per-fetch timeout.
    pub fn new(sources: Vec<Arc<dyn RateLimitSource>>, ttl: Duration, timeout: Duration) -> Self {
        Self {
            sources,
            ttl,
            timeout,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// A cache wired to the real Claude, Codex, OpenCode and Grok fetchers,
    /// with [`DEFAULT_TTL`] and [`DEFAULT_TIMEOUT`].
    pub fn with_real_sources() -> Self {
        let sources: Vec<Arc<dyn RateLimitSource>> = vec![
            Arc::new(ClaudeRateLimitSource::new()),
            Arc::new(CodexRateLimitSource::new()),
            Arc::new(OpenCodeRateLimitSource::new()),
            Arc::new(GrokRateLimitSource::new()),
        ];
        Self::new(sources, DEFAULT_TTL, DEFAULT_TIMEOUT)
    }

    /// The cached snapshot for `provider`. Never fetches.
    ///
    /// A provider never fetched at all reports [`SnapshotStatus::Unavailable`]
    /// — not a default-constructed 0%, and not a provider absent from the
    /// result, either of which would be indistinguishable from "nothing to
    /// worry about" at the call site.
    pub fn snapshot(&self, provider: Provider) -> ProviderSnapshot {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match entries.get(&provider) {
            None => ProviderSnapshot::unavailable(provider, "not fetched yet"),
            Some(entry) => {
                let mut snapshot = entry.snapshot.clone();
                if matches!(snapshot.status, SnapshotStatus::Fresh)
                    && entry.fetched_at.elapsed() > self.ttl
                {
                    snapshot.status = SnapshotStatus::Stale;
                }
                snapshot
            }
        }
    }

    /// Every configured provider's cached snapshot, in the order sources
    /// were given to [`RateLimitCache::new`].
    pub fn snapshots(&self) -> Vec<ProviderSnapshot> {
        self.sources
            .iter()
            .map(|source| self.snapshot(source.provider()))
            .collect()
    }

    /// When the most recently successful fetch across all providers
    /// happened, in milliseconds since the Unix epoch — the "Updated 1m ago"
    /// the usage UI shows for the whole bar, not per provider.
    pub fn last_updated_ms(&self) -> Option<u64> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter_map(|entry| entry.snapshot.fetched_at_ms)
            .max()
    }

    /// Takes a snapshot somebody else obtained, on the same terms as a fetch.
    ///
    /// The push half of this module. Everything else here asks a provider a
    /// question and waits; a status line report arrives unbidden from a
    /// session that was running anyway, and lands in the same cache so that
    /// nothing downstream has to know which way a number came in.
    pub fn accept(&self, snapshot: ProviderSnapshot) {
        self.store(snapshot);
    }

    /// Whether `provider` has a snapshot still inside the TTL.
    fn is_fresh(&self, provider: Provider) -> bool {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.get(&provider).is_some_and(|entry| {
            matches!(entry.snapshot.status, SnapshotStatus::Fresh)
                && entry.fetched_at.elapsed() <= self.ttl
        })
    }

    /// Fetches only the providers whose numbers have gone stale.
    ///
    /// What the background loop wants, and the reason [`Self::accept`] earns
    /// its keep: a provider that has been pushing fresh numbers is not asked
    /// again. For Claude that means a person who is working never has a fetch
    /// run on their behalf at all — the status line they are already drawing
    /// answers the question first.
    pub fn refresh_stale(&self) {
        std::thread::scope(|scope| {
            for source in &self.sources {
                if self.is_fresh(source.provider()) {
                    continue;
                }
                let source = Arc::clone(source);
                let timeout = self.timeout;
                scope.spawn(move || {
                    let snapshot = source.fetch(timeout);
                    self.store(snapshot);
                });
            }
        });
    }

    /// Fetches `provider` right now, ignoring the TTL entirely.
    ///
    /// The usage UI's manual refresh control needs exactly this: a
    /// person clicking "refresh" should not be told "still within its TTL."
    pub fn refresh(&self, provider: Provider) {
        if let Some(source) = self.sources.iter().find(|s| s.provider() == provider) {
            let snapshot = source.fetch(self.timeout);
            self.store(snapshot);
        }
    }

    /// Fetches every configured provider right now, concurrently, ignoring
    /// the TTL.
    ///
    /// Concurrent rather than sequential so the wall-clock cost is bounded by
    /// the slowest single fetch (at most `timeout`) rather than their sum —
    /// three providers at up to [`DEFAULT_TIMEOUT`] each would otherwise mean
    /// a person's "refresh" button taking up to a minute.
    pub fn refresh_all(&self) {
        std::thread::scope(|scope| {
            for source in &self.sources {
                let source = Arc::clone(source);
                let timeout = self.timeout;
                scope.spawn(move || {
                    let snapshot = source.fetch(timeout);
                    self.store(snapshot);
                });
            }
        });
    }

    /// Records a fresh fetch result.
    ///
    /// A fetch that came back [`SnapshotStatus::Unavailable`] does **not**
    /// overwrite a previously good ([`SnapshotStatus::Fresh`] or
    /// [`SnapshotStatus::Stale`]) entry — a transient failure ages the old
    /// numbers toward `Stale` via [`RateLimitCache::snapshot`]'s own TTL
    /// check rather than blanking them to `Unavailable`, which is the
    /// difference between "the last good number, marked old" and "no number
    /// at all" for a person watching the bar during a flaky network blip.
    /// Only a provider with no prior successful fetch reports `Unavailable`
    /// outright.
    fn store(&self, snapshot: ProviderSnapshot) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let keep_previous = matches!(snapshot.status, SnapshotStatus::Unavailable { .. })
            && entries.get(&snapshot.provider).is_some_and(|e| {
                matches!(
                    e.snapshot.status,
                    SnapshotStatus::Fresh | SnapshotStatus::Stale
                )
            });

        if keep_previous {
            return;
        }

        entries.insert(
            snapshot.provider,
            CacheEntry {
                snapshot,
                fetched_at: Instant::now(),
            },
        );
    }

    /// Starts a background thread that calls [`RateLimitCache::refresh_stale`]
    /// once immediately, then again every TTL, until the returned handle is
    /// dropped.
    ///
    /// Stale rather than all: a provider pushing its own numbers in — Claude,
    /// through its status line — should not also be interrogated for them.
    pub fn spawn_background_refresh(self: &Arc<Self>) -> BackgroundRefresh {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let cache = Arc::clone(self);

        let handle = std::thread::spawn(move || {
            cache.refresh_stale();
            'outer: while !stop_flag.load(Ordering::Relaxed) {
                let step = Duration::from_millis(200);
                let mut waited = Duration::ZERO;
                while waited < cache.ttl {
                    if stop_flag.load(Ordering::Relaxed) {
                        break 'outer;
                    }
                    let sleep_for = step.min(cache.ttl - waited);
                    std::thread::sleep(sleep_for);
                    waited += sleep_for;
                }
                cache.refresh_stale();
            }
        });

        BackgroundRefresh {
            stop,
            handle: Some(handle),
        }
    }
}

/// Handle for a background refresh loop started by
/// [`RateLimitCache::spawn_background_refresh`].
///
/// Dropping it stops the loop and joins the thread — the same
/// drop-stops-the-work idiom as [`crate::surface::FileWatcher`], so nothing
/// here can outlive whatever created it.
pub struct BackgroundRefresh {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for BackgroundRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundRefresh").finish_non_exhaustive()
    }
}

impl Drop for BackgroundRefresh {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::collections::VecDeque;

    // -- fakes for the process seam ------------------------------------------

    #[derive(Debug, Default)]
    pub(crate) struct FakeHarness {
        pub(crate) responses: Mutex<VecDeque<Vec<String>>>,
        pub(crate) hang: bool,
        pub(crate) spawned: Mutex<Vec<HarnessCommand>>,
    }

    impl FakeHarness {
        pub(crate) fn with_lines(lines: &[&str]) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from([lines
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect()])),
                hang: false,
                spawned: Mutex::new(Vec::new()),
            }
        }

        /// One scripted response per spawn, in order.
        ///
        /// The Claude fetcher walks a ladder — keychain, then the usage
        /// endpoint, then the CLI — so a test of it has to say what each rung
        /// finds, including the rungs that find nothing.
        pub(crate) fn with_responses(responses: &[&[&str]]) -> Self {
            Self {
                responses: Mutex::new(
                    responses
                        .iter()
                        .map(|lines| lines.iter().map(|s| (*s).to_owned()).collect())
                        .collect(),
                ),
                hang: false,
                spawned: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn hanging() -> Self {
            Self {
                responses: Mutex::new(VecDeque::new()),
                hang: true,
                spawned: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProcessHarness for FakeHarness {
        fn spawn(&self, command: &HarnessCommand) -> Result<Box<dyn ChildLines>> {
            self.spawned.lock().unwrap().push(command.clone());
            let lines = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            Ok(Box::new(FakeChild {
                lines: VecDeque::from(lines),
                hang: self.hang,
            }))
        }
    }

    struct FakeChild {
        lines: VecDeque<String>,
        hang: bool,
    }

    impl ChildLines for FakeChild {
        fn next_line(&mut self, timeout: Duration) -> Result<Option<String>> {
            if self.hang {
                std::thread::sleep(timeout);
                return Ok(None);
            }
            Ok(self.lines.pop_front())
        }

        fn kill(&mut self) {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    // -- the bounded scratch directory --------------------------------------

    #[test]
    fn the_bounded_scratch_directory_check_rejects_a_root_like_path() {
        let temp_root = Path::new("/tmp");

        assert!(
            is_root_like(Path::new("/"), temp_root, None),
            "the root itself"
        );
        assert!(
            is_root_like(Path::new("/tmp"), temp_root, None),
            "the temp root itself is too shallow to be a dedicated scratch dir"
        );
        assert!(
            is_root_like(Path::new("relative/dir"), temp_root, None),
            "a relative path"
        );
        assert!(
            is_root_like(Path::new("/Users/me/dev/project"), temp_root, None),
            "a real path that just happens to be outside the temp root"
        );

        let scratch = Path::new("/tmp/ket-telemetry-123-456");
        assert!(
            !is_root_like(scratch, temp_root, None),
            "a properly nested scratch path should be accepted"
        );
        assert!(
            is_root_like(scratch, temp_root, Some(scratch)),
            "a scratch path that happens to equal $HOME must still be rejected"
        );
    }

    #[test]
    fn a_freshly_created_scratch_directory_passes_its_own_safety_check() {
        let dir = new_scratch_dir().expect("scratch dir creation should succeed in a test env");
        let home = crate::paths::home().ok();
        assert!(!is_root_like(&dir, &scratch_root(), home.as_deref()));
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- the cache ------------------------------------------------------------

    #[derive(Debug)]
    struct FakeSource {
        provider: Provider,
        calls: Arc<AtomicUsize>,
        result: ProviderSnapshot,
    }

    impl RateLimitSource for FakeSource {
        fn provider(&self) -> Provider {
            self.provider
        }

        fn fetch(&self, _timeout: Duration) -> ProviderSnapshot {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    fn fresh(provider: Provider, percent: f32) -> ProviderSnapshot {
        ProviderSnapshot {
            provider,
            status: SnapshotStatus::Fresh,
            windows: vec![RateWindow {
                name: "Session".to_owned(),
                used_percent: percent,
                resets_at_ms: None,
                window_minutes: None,
            }],
            plan: None,
            account: None,
            fetched_at_ms: Some(now_ms()),
            spend: Vec::new(),
            limited_at_ms: None,
        }
    }

    #[test]
    fn the_cache_serves_from_memory_and_does_not_refetch_inside_its_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn RateLimitSource> = Arc::new(FakeSource {
            provider: Provider::Claude,
            calls: Arc::clone(&calls),
            result: fresh(Provider::Claude, 10.0),
        });
        let cache = RateLimitCache::new(
            vec![source],
            Duration::from_secs(60),
            Duration::from_secs(1),
        );

        cache.refresh(Provider::Claude);
        cache.snapshot(Provider::Claude);
        cache.snapshot(Provider::Claude);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_explicit_refresh_bypasses_the_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn RateLimitSource> = Arc::new(FakeSource {
            provider: Provider::Claude,
            calls: Arc::clone(&calls),
            result: fresh(Provider::Claude, 10.0),
        });
        let cache = RateLimitCache::new(
            vec![source],
            Duration::from_secs(3600),
            Duration::from_secs(1),
        );

        cache.refresh(Provider::Claude);
        cache.refresh(Provider::Claude);

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_snapshot_older_than_the_ttl_is_reported_stale_rather_than_silently_fresh() {
        let source: Arc<dyn RateLimitSource> = Arc::new(FakeSource {
            provider: Provider::Claude,
            calls: Arc::new(AtomicUsize::new(0)),
            result: fresh(Provider::Claude, 10.0),
        });
        let cache = RateLimitCache::new(
            vec![source],
            Duration::from_millis(10),
            Duration::from_secs(1),
        );

        cache.refresh(Provider::Claude);
        std::thread::sleep(Duration::from_millis(40));

        let snapshot = cache.snapshot(Provider::Claude);
        assert_eq!(snapshot.status, SnapshotStatus::Stale);
        // The numbers survive into the stale state — this is not the same as
        // losing them to `Unavailable`.
        assert_eq!(snapshot.windows.len(), 1);
    }

    #[derive(Debug)]
    struct FlakySource {
        calls: AtomicUsize,
    }

    impl RateLimitSource for FlakySource {
        fn provider(&self) -> Provider {
            Provider::Claude
        }

        fn fetch(&self, _timeout: Duration) -> ProviderSnapshot {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                fresh(Provider::Claude, 10.0)
            } else {
                ProviderSnapshot::unavailable(Provider::Claude, "network blip")
            }
        }
    }

    #[test]
    fn a_failed_refresh_does_not_erase_a_previously_good_snapshot() {
        let source: Arc<dyn RateLimitSource> = Arc::new(FlakySource {
            calls: AtomicUsize::new(0),
        });
        let cache = RateLimitCache::new(
            vec![source],
            Duration::from_secs(3600),
            Duration::from_secs(1),
        );

        cache.refresh(Provider::Claude); // succeeds
        cache.refresh(Provider::Claude); // fails internally

        let snapshot = cache.snapshot(Provider::Claude);
        assert_eq!(snapshot.status, SnapshotStatus::Fresh);
        assert_eq!(snapshot.windows.len(), 1);
    }

    #[test]
    fn a_provider_never_fetched_reports_unavailable_not_zero() {
        let cache = RateLimitCache::new(
            Vec::<Arc<dyn RateLimitSource>>::new(),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );

        let snapshot = cache.snapshot(Provider::Codex);
        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn refresh_all_fetches_every_configured_provider() {
        let claude_calls = Arc::new(AtomicUsize::new(0));
        let codex_calls = Arc::new(AtomicUsize::new(0));
        let sources: Vec<Arc<dyn RateLimitSource>> = vec![
            Arc::new(FakeSource {
                provider: Provider::Claude,
                calls: Arc::clone(&claude_calls),
                result: fresh(Provider::Claude, 1.0),
            }),
            Arc::new(FakeSource {
                provider: Provider::Codex,
                calls: Arc::clone(&codex_calls),
                result: fresh(Provider::Codex, 2.0),
            }),
        ];
        let cache = RateLimitCache::new(sources, Duration::from_secs(60), Duration::from_secs(1));

        cache.refresh_all();

        assert_eq!(claude_calls.load(Ordering::SeqCst), 1);
        assert_eq!(codex_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.snapshots().len(), 2);
    }

    #[test]
    fn a_background_refresh_fetches_immediately_and_stops_cleanly_on_drop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn RateLimitSource> = Arc::new(FakeSource {
            provider: Provider::Claude,
            calls: Arc::clone(&calls),
            result: fresh(Provider::Claude, 5.0),
        });
        let cache = Arc::new(RateLimitCache::new(
            vec![source],
            Duration::from_secs(3600),
            Duration::from_secs(1),
        ));

        let handle = cache.spawn_background_refresh();
        // The loop fetches once immediately on its own thread; give it a
        // moment to actually run before asserting.
        for _ in 0..50 {
            if calls.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(handle);

        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    // -- wire format ------------------------------------------------------------

    #[test]
    fn the_wire_format_is_flat_camel_case_with_a_tagged_status() {
        let snapshot = ProviderSnapshot {
            provider: Provider::Codex,
            status: SnapshotStatus::Unavailable {
                reason: "timed out".to_owned(),
            },
            windows: vec![RateWindow {
                name: "Session".to_owned(),
                used_percent: 43.0,
                resets_at_ms: Some(1_788_719_198_000),
                window_minutes: Some(300),
            }],
            plan: Some("plus".to_owned()),
            account: Some("acc-123".to_owned()),
            fetched_at_ms: None,
            spend: Vec::new(),
            limited_at_ms: None,
        };

        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            serde_json::json!({
                "provider": "codex",
                "status": { "kind": "unavailable", "reason": "timed out" },
                "windows": [{
                    "name": "Session",
                    "usedPercent": 43.0,
                    "resetsAtMs": 1_788_719_198_000_u64,
                    "windowMinutes": 300,
                }],
                "plan": "plus",
                "account": "acc-123",
                "fetchedAtMs": null,
            })
        );
    }

    #[test]
    fn provider_names_are_camel_case_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&Provider::Claude).unwrap(),
            r#""claude""#
        );
        assert_eq!(
            serde_json::to_string(&Provider::Codex).unwrap(),
            r#""codex""#
        );
        assert_eq!(
            serde_json::to_string(&Provider::OpenCode).unwrap(),
            r#""openCode""#
        );
    }

    #[test]
    fn provider_label_is_a_short_human_name() {
        assert_eq!(Provider::Claude.label(), "Claude");
        assert_eq!(Provider::Codex.label(), "Codex");
        assert_eq!(Provider::OpenCode.label(), "OpenCode");
    }

    #[test]
    fn window_label_names_the_two_observed_durations_and_falls_back_otherwise() {
        assert_eq!(window_label(Some(300), "X"), "Session");
        assert_eq!(window_label(Some(10_080), "X"), "Weekly");
        assert_eq!(window_label(Some(43_200), "X"), "Monthly");
        assert_eq!(window_label(Some(60), "Overage"), "Overage (60m)");
        assert_eq!(window_label(None, "Overage"), "Overage");
    }

    #[test]
    fn provider_snapshot_unavailable_carries_the_reason_and_nothing_else() {
        let snapshot = ProviderSnapshot::unavailable(Provider::Codex, "timed out");
        assert_eq!(
            snapshot.status,
            SnapshotStatus::Unavailable {
                reason: "timed out".to_owned()
            }
        );
        assert!(snapshot.windows.is_empty());
        assert!(snapshot.fetched_at_ms.is_none());
    }

    #[test]
    fn provider_snapshot_unsupported_carries_no_data() {
        let snapshot = ProviderSnapshot::unsupported(Provider::OpenCode);
        assert_eq!(snapshot.status, SnapshotStatus::Unsupported);
        assert!(snapshot.windows.is_empty());
    }

    #[test]
    fn child_outcome_describe_prefers_the_processs_own_stderr() {
        let outcome = ChildOutcome {
            code: Some(1),
            stderr: "boom\nsecond line".to_owned(),
        };
        assert_eq!(outcome.describe("fallback"), "fallback: boom");
    }

    #[test]
    fn child_outcome_describe_names_the_exit_code_with_no_stderr() {
        let outcome = ChildOutcome {
            code: Some(7),
            stderr: String::new(),
        };
        assert_eq!(outcome.describe("fallback"), "fallback and exited 7");
    }

    #[test]
    fn child_outcome_describe_is_just_the_fallback_for_a_clean_exit_with_nothing_said() {
        let outcome = ChildOutcome {
            code: Some(0),
            stderr: String::new(),
        };
        assert_eq!(outcome.describe("fallback"), "fallback");
        let no_code = ChildOutcome {
            code: None,
            stderr: String::new(),
        };
        assert_eq!(no_code.describe("fallback"), "fallback");
    }

    #[test]
    fn first_line_of_takes_only_the_first_line() {
        assert_eq!(first_line_of("one\ntwo\nthree"), "one");
        assert_eq!(first_line_of("  padded  \nrest"), "padded");
        assert_eq!(first_line_of(""), "");
    }

    #[test]
    fn first_line_of_truncates_a_very_long_line() {
        let long = "x".repeat(200);
        let shortened = first_line_of(&long);
        assert!(shortened.ends_with("..."));
        assert_eq!(shortened.chars().count(), 160);
    }

    #[test]
    fn last_updated_ms_is_none_until_something_has_ever_fetched() {
        let cache = RateLimitCache::new(
            vec![Arc::new(FakeSource {
                provider: Provider::Claude,
                calls: Arc::new(AtomicUsize::new(0)),
                result: ProviderSnapshot::unavailable(Provider::Claude, "nope"),
            })],
            Duration::from_secs(60),
            Duration::from_secs(5),
        );
        assert_eq!(cache.last_updated_ms(), None);
    }

    #[test]
    fn last_updated_ms_is_the_most_recent_fetch_across_every_provider() {
        let cache =
            RateLimitCache::new(Vec::new(), Duration::from_secs(60), Duration::from_secs(5));

        let mut older = fresh(Provider::Claude, 10.0);
        older.fetched_at_ms = Some(1_000);
        let mut newer = fresh(Provider::Codex, 20.0);
        newer.fetched_at_ms = Some(2_000);

        cache.accept(older);
        cache.accept(newer);

        assert_eq!(cache.last_updated_ms(), Some(2_000));
    }

    #[test]
    fn refresh_stale_refetches_a_provider_whose_snapshot_has_gone_stale() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn RateLimitSource> = Arc::new(FakeSource {
            provider: Provider::Claude,
            calls: Arc::clone(&calls),
            result: fresh(Provider::Claude, 10.0),
        });
        let cache = RateLimitCache::new(
            vec![source],
            Duration::from_millis(10),
            Duration::from_secs(1),
        );

        cache.refresh(Provider::Claude);
        std::thread::sleep(Duration::from_millis(40));
        cache.refresh_stale();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the stale entry was refetched"
        );
    }

    #[test]
    fn accept_does_not_overwrite_a_good_snapshot_with_an_unavailable_one() {
        let cache =
            RateLimitCache::new(Vec::new(), Duration::from_secs(60), Duration::from_secs(5));

        cache.accept(fresh(Provider::Claude, 42.0));
        cache.accept(ProviderSnapshot::unavailable(
            Provider::Claude,
            "network blip",
        ));

        let snapshot = cache.snapshot(Provider::Claude);
        assert_eq!(snapshot.status, SnapshotStatus::Fresh);
        assert_eq!(snapshot.windows[0].used_percent, 42.0);
    }

    #[test]
    fn accept_stores_an_unavailable_snapshot_when_there_is_no_prior_good_one() {
        let cache =
            RateLimitCache::new(Vec::new(), Duration::from_secs(60), Duration::from_secs(5));

        cache.accept(ProviderSnapshot::unavailable(
            Provider::Claude,
            "never fetched",
        ));

        assert!(matches!(
            cache.snapshot(Provider::Claude).status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn refresh_stale_does_not_refetch_a_provider_pushed_in_by_accept() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn RateLimitSource> = Arc::new(FakeSource {
            provider: Provider::Claude,
            calls: calls.clone(),
            result: fresh(Provider::Claude, 5.0),
        });
        let cache = RateLimitCache::new(
            vec![source],
            Duration::from_secs(60),
            Duration::from_secs(5),
        );

        cache.accept(fresh(Provider::Claude, 42.0));
        cache.refresh_stale();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "already fresh, so no fetch"
        );
        assert_eq!(
            cache.snapshot(Provider::Claude).windows[0].used_percent,
            42.0
        );
    }
}
