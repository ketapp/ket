//! Claude's quota: the OAuth usage endpoint, `/usage`, and the statusline frame.

use super::*;

// ---------------------------------------------------------------------------
// Claude
// ---------------------------------------------------------------------------

/// The OAuth usage endpoint — the same one Claude Code's own `/usage` screen
/// is backed by, and the cheapest honest answer to "how much is left".
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta header the endpoint requires alongside an OAuth bearer token.
const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";

/// Sent so the endpoint sees the same client its own CLI presents as.
const CLAUDE_USAGE_USER_AGENT: &str = "claude-code/2.1.0";

/// The Keychain service Claude Code stores its OAuth credentials under.
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The longest a Keychain lookup may take before the ladder moves on.
///
/// The lookup itself is instant; this bounds the case where it is not, which
/// on macOS means an unlocked-keychain prompt or a login item that has gone
/// unresponsive. Neither is worth a status bar's whole budget.
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Marks the HTTP status curl appends after the response body, so one stdout
/// stream carries both without a second request or a temporary file.
const HTTP_STATUS_MARK: &str = "ket-http-status:";

/// How a usable OAuth token was obtained, for the failure log.
///
/// Which store answered is the single most useful thing to know when usage
/// stops reporting, because each one fails for a different reason: a scoped
/// Keychain item goes stale when a session rotates it, the legacy item is
/// absent for anyone who has only ever used a custom `CLAUDE_CONFIG_DIR`, and
/// the file exists only where Claude Code could not reach a Keychain at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    /// A Keychain item scoped to a non-default `CLAUDE_CONFIG_DIR`.
    ScopedKeychain,
    /// The unscoped Keychain item, which is what a default install writes.
    LegacyKeychain,
    /// `<config dir>/.credentials.json`, where Claude Code fell back to a file.
    CredentialsFile,
}

impl CredentialSource {
    fn label(self) -> &'static str {
        match self {
            Self::ScopedKeychain => "scoped keychain",
            Self::LegacyKeychain => "keychain",
            Self::CredentialsFile => "credentials file",
        }
    }
}

/// An OAuth bearer token and where it came from.
#[derive(Debug, Clone)]
struct ClaudeCredential {
    token: String,
    source: CredentialSource,
}

/// Why one rung of the ladder gave up, in the terms the next rung needs.
///
/// The distinction that matters is [`FetchFailure::Auth`] and
/// [`FetchFailure::RateLimited`]: both mean the *next* rung would fail the
/// same way for the same reason, so trying it wastes a process spawn and,
/// worse, buries the real cause under a second, vaguer message. Everything
/// else is worth another attempt through a different door.
#[derive(Debug, Clone)]
enum FetchFailure {
    /// The token was rejected. Another source of the same credential will not
    /// do better.
    Auth(String),
    /// HTTP 429, with the server's own `Retry-After` where it sent one.
    RateLimited {
        reason: String,
        retry_after: Option<Duration>,
    },
    /// Anything else — no token, no network, a shape that would not parse.
    Other(String),
}

impl FetchFailure {
    fn reason(&self) -> &str {
        match self {
            Self::Auth(reason) | Self::Other(reason) => reason,
            Self::RateLimited { reason, .. } => reason,
        }
    }

    /// Whether a different rung is worth trying after this.
    fn worth_a_fallback(&self) -> bool {
        matches!(self, Self::Other(_))
    }
}

/// Fetches Claude Code's plan usage without spending any of it.
///
/// Three rungs, cheapest first, none of which costs a token:
///
/// 1. **Nothing at all**, when a live session is already pushing its status
///    line into the cache — [`RateLimitCache::refresh_stale`] is what skips
///    this fetcher entirely, and [`snapshot_from_statusline`] is what reads
///    those numbers.
/// 2. **The OAuth usage endpoint**, with the token Claude Code already holds.
///    One HTTPS GET, no model, no session: `five_hour` and `seven_day` come
///    back as percentages with real reset timestamps.
/// 3. **`claude -p "/usage"`**, whose output is the same screen the CLI draws
///    for a person. Verified free — `num_turns: 0`, `total_cost_usd: 0` — and
///    it needs no credential of ket's own, because the CLI resolves its own
///    login exactly as it does for a real session.
///
/// The rung this replaced ran a real, billed turn and read the
/// `rate_limit_event` frame out of the stream. It worked, but it cost roughly
/// nine cents an hour to watch a status bar, and it failed silently whenever
/// the hidden session could not log in — which is what
/// [`Self::child_env`] now exists to prevent.
///
/// ## The one thing to reconsider before shipping this widely
///
/// Rung two reads a Keychain item that belongs to another application, and
/// macOS may put a consent dialog in front of that the first time — from a
/// background refresh, with no obvious connection to anything the person just
/// did. Denying it costs nothing: the lookup fails, rung three answers, and
/// the panel fills in anyway. If that prompt is judged worse than losing exact
/// reset timestamps, the fix is to try [`Self::fetch_cli`] first and treat the
/// endpoint as the fallback — the two rungs are independent and the order is
/// one `match` in [`RateLimitSource::fetch`].
#[derive(Debug, Clone)]
pub struct ClaudeRateLimitSource {
    harness: Arc<dyn ProcessHarness>,
    /// The `claude` binary to run — a name resolved against `PATH`, matching
    /// how every other agent invocation in this codebase names its binary.
    binary: OsString,
    /// The `CLAUDE_CONFIG_DIR` in force, or `None` for a default install.
    ///
    /// Resolved once and never re-read: a fetch that consults ambient state is
    /// a fetch whose behaviour depends on which shell happened to start ket,
    /// which is the class of bug this module has just spent a rewrite getting
    /// out of. Resolved on first use rather than at construction because the
    /// answer is the login shell's to give — see
    /// [`crate::sessions::configured_claude_config_dir`] — and construction
    /// happens on the thread that builds the window. Pre-filled by
    /// [`Self::with_harness`] so a test asks no shell.
    config_dir: OnceLock<Option<PathBuf>>,
    /// Where a credentials file would be, when there is one to look for.
    ///
    /// Follows [`Self::config_dir`], and is pre-filled with `None` in
    /// [`Self::with_harness`] so that a test reaches no real filesystem — the
    /// process seam covers the Keychain, and this covers the only other
    /// real-world read in the ladder.
    credentials_file: OnceLock<Option<PathBuf>>,
    /// Set by a `429` and honoured until it passes, so a rate-limited account
    /// is not asked again every TTL for numbers the server has already said
    /// it will not give.
    retry_at: Arc<Mutex<Option<Instant>>>,
}

impl ClaudeRateLimitSource {
    /// A fetcher that spawns the real `claude` binary.
    pub fn new() -> Self {
        Self {
            config_dir: OnceLock::new(),
            credentials_file: OnceLock::new(),
            ..Self::with_harness(Arc::new(RealProcessHarness))
        }
    }

    /// A fetcher using an injected [`ProcessHarness`] — for tests.
    pub fn with_harness(harness: Arc<dyn ProcessHarness>) -> Self {
        Self {
            harness,
            binary: OsString::from("claude"),
            config_dir: OnceLock::from(None),
            credentials_file: OnceLock::from(None),
            retry_at: Arc::new(Mutex::new(None)),
        }
    }

    /// The config directory the agent runs with, resolved on the first call.
    fn config_dir(&self) -> Option<&Path> {
        self.config_dir
            .get_or_init(crate::sessions::configured_claude_config_dir)
            .as_deref()
    }

    /// The credentials file in that directory, or in the default install's.
    fn credentials_file(&self) -> Option<&Path> {
        self.credentials_file
            .get_or_init(|| {
                self.config_dir()
                    .map(Path::to_path_buf)
                    .or_else(|| crate::paths::home().ok().map(|home| home.join(".claude")))
                    .map(|dir| dir.join(".credentials.json"))
            })
            .as_deref()
    }

    /// The environment every child of this fetcher runs with.
    ///
    /// **This is the bug fix that matters most in this file.** ket launches
    /// agents by typing at a real login shell (see [`crate::shell`]), so an
    /// agent inherits a person's `PATH`, their aliases, and their exported
    /// `CLAUDE_CONFIG_DIR`. This fetcher spawns directly, and a directly
    /// spawned child inherits only what ket itself was given — which, for an
    /// app launched from Finder rather than a terminal, is close to nothing.
    ///
    /// `USER` is the one that bites: without it Claude Code cannot find its
    /// own Keychain item, reports `Not logged in`, and exits having emitted no
    /// usage at all. That was indistinguishable, in the old code, from a
    /// crash. `PATH` matters for the same reason one rung down — `claude`
    /// lives in `/opt/homebrew/bin`, which a GUI process's `PATH` does not
    /// contain — and is why the binary is resolved through
    /// [`crate::shell::probe`] rather than left to the child's own lookup.
    fn child_env(&self) -> Vec<(OsString, OsString)> {
        let mut env = Vec::new();

        if std::env::var_os("USER").is_none()
            && let Some(user) = current_username()
        {
            env.push((OsString::from("USER"), OsString::from(user)));
        }

        // Set rather than inherited, so the child reads the same config
        // directory this fetcher looked its credentials up in even if ket's
        // own environment is later scrubbed.
        if let Some(dir) = self.config_dir() {
            env.push((
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from(dir.as_os_str()),
            ));
        }

        env
    }

    /// The `claude` binary, resolved the same way ket resolves it for an
    /// agent — `PATH` first, then the user's own login shell.
    fn resolved_binary(&self) -> OsString {
        crate::shell::probe(&self.binary.to_string_lossy())
            .map(OsString::from)
            .unwrap_or_else(|| self.binary.clone())
    }

    /// Reads the OAuth token Claude Code is already holding.
    ///
    /// Tried in the order Claude Code itself writes them: an item scoped to a
    /// non-default `CLAUDE_CONFIG_DIR`, then the unscoped item a default
    /// install uses, then the file it falls back to where no Keychain was
    /// reachable. `None` means no token was found, which is a fallback, not
    /// an error — rung three needs no token.
    ///
    /// ## On reading another application's credentials
    ///
    /// The module docs used to say ket does not do this, and that changing it
    /// was a maintainer's call rather than something to route around quietly
    /// here. That call has since been made. The token is read into memory,
    /// handed to one child process through its stdin, and never written to
    /// disk, a log, or an argument vector — [`Self::usage_request`] exists in
    /// the shape it does precisely so the token never appears in `ps`.
    fn credential(&self, timeout: Duration) -> Option<ClaudeCredential> {
        if let Some(dir) = self.config_dir() {
            let scoped = format!("{CLAUDE_KEYCHAIN_SERVICE}-{}", dir.display());
            if let Some(token) = self.keychain_token(&scoped, timeout) {
                return Some(ClaudeCredential {
                    token,
                    source: CredentialSource::ScopedKeychain,
                });
            }
        }

        if let Some(token) = self.keychain_token(CLAUDE_KEYCHAIN_SERVICE, timeout) {
            return Some(ClaudeCredential {
                token,
                source: CredentialSource::LegacyKeychain,
            });
        }

        let raw = std::fs::read_to_string(self.credentials_file()?).ok()?;
        let token = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()?
            .get("claudeAiOauth")?
            .get("accessToken")?
            .as_str()?
            .to_owned();

        (!token.trim().is_empty()).then_some(ClaudeCredential {
            token,
            source: CredentialSource::CredentialsFile,
        })
    }

    /// One Keychain lookup, through the same process seam as everything else
    /// here so a test never touches a real Keychain.
    ///
    /// `security` writes the secret to stdout as a single line and nothing
    /// else; a missing item is an exit code, which surfaces as no line at all.
    /// Bounded by the caller's own budget rather than a constant of its own,
    /// because three rungs sharing one deadline is the only way `fetch` can
    /// promise to return within it.
    fn keychain_token(&self, service: &str, timeout: Duration) -> Option<String> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let account = current_username()?;
        let command = HarnessCommand {
            program: OsString::from("security"),
            args: ["find-generic-password", "-w", "-s", service, "-a", &account]
                .into_iter()
                .map(OsString::from)
                .collect(),
            cwd: crate::paths::home().unwrap_or_else(|_| PathBuf::from("/")),
            stdin_lines: Vec::new(),
            env: Vec::new(),
            close_stdin: true,
        };

        let mut child = self.harness.spawn(&command).ok()?;
        let line = child
            .next_line(timeout.min(KEYCHAIN_TIMEOUT))
            .ok()
            .flatten();
        child.kill();
        line.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty())
    }

    /// The curl invocation for one usage request.
    ///
    /// Everything sensitive goes in on stdin as a curl config file rather than
    /// on the command line: an `Authorization` header in `argv` is readable by
    /// every process on the machine for as long as the request lasts. The
    /// status code is appended to the body behind [`HTTP_STATUS_MARK`] so a
    /// single stdout stream carries both.
    fn usage_request(&self, token: &str, timeout: Duration) -> HarnessCommand {
        let seconds = timeout.as_secs().max(1);
        HarnessCommand {
            program: OsString::from("curl"),
            args: ["--silent", "--show-error", "--config", "-"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            cwd: crate::paths::home().unwrap_or_else(|_| PathBuf::from("/")),
            stdin_lines: vec![
                format!("url = \"{CLAUDE_USAGE_URL}\""),
                format!("max-time = {seconds}"),
                format!("header = \"Authorization: Bearer {token}\""),
                format!("header = \"anthropic-beta: {CLAUDE_OAUTH_BETA}\""),
                format!("header = \"User-Agent: {CLAUDE_USAGE_USER_AGENT}\""),
                format!("write-out = \"\\n{HTTP_STATUS_MARK}%{{http_code}}\\n\""),
            ],
            env: Vec::new(),
            close_stdin: true,
        }
    }

    /// Rung two: the OAuth endpoint.
    fn fetch_oauth(
        &self,
        timeout: Duration,
    ) -> std::result::Result<ProviderSnapshot, FetchFailure> {
        let Some(credential) = self.credential(timeout) else {
            return Err(FetchFailure::Other(
                "no Claude OAuth token in the keychain or credentials file".to_owned(),
            ));
        };

        // A token with a quote or a newline in it would break out of the curl
        // config file's quoting. No real token looks like that; one that does
        // is a reason to stop, not to escape it and hope.
        if credential.token.contains(['"', '\n', '\r', '\\']) {
            return Err(FetchFailure::Other(format!(
                "the {} token is not in a form that can be sent safely",
                credential.source.label()
            )));
        }

        let command = self.usage_request(&credential.token, timeout);
        let mut child = self
            .harness
            .spawn(&command)
            .map_err(|e| FetchFailure::Other(format!("could not run curl: {e}")))?;

        let mut body = String::new();
        let mut status: Option<u16> = None;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                child.kill();
                return Err(FetchFailure::Other(
                    "timed out asking the usage endpoint".to_owned(),
                ));
            }
            match child.next_line(remaining) {
                Ok(Some(line)) => match line.strip_prefix(HTTP_STATUS_MARK) {
                    Some(code) => status = code.trim().parse().ok(),
                    None => body.push_str(&line),
                },
                Ok(None) => break,
                Err(e) => {
                    child.kill();
                    return Err(FetchFailure::Other(format!("reading curl failed: {e}")));
                }
            }
        }
        let outcome = child.outcome();
        child.kill();

        match status {
            Some(200) => interpret_claude_usage(&body).ok_or_else(|| {
                FetchFailure::Other(
                    "the usage endpoint sent no windows this module knows".to_owned(),
                )
            }),
            Some(401 | 403) => Err(FetchFailure::Auth(format!(
                "the {} token was rejected — sign in again",
                credential.source.label()
            ))),
            Some(429) => Err(FetchFailure::RateLimited {
                reason: "the usage endpoint is rate limiting this account".to_owned(),
                retry_after: retry_after_of(&body),
            }),
            Some(code) => Err(FetchFailure::Other(format!(
                "the usage endpoint answered {code}"
            ))),
            None => Err(FetchFailure::Other(
                outcome.describe("curl said nothing about the usage endpoint"),
            )),
        }
    }

    /// Rung three: the CLI's own `/usage` screen.
    ///
    /// Free, and verified so — a `/usage` run reports `num_turns: 0` and
    /// `total_cost_usd: 0`, because the command is answered locally and never
    /// reaches a model. The text it prints is the same one a person sees.
    fn fetch_cli(&self, timeout: Duration) -> std::result::Result<ProviderSnapshot, FetchFailure> {
        let command = HarnessCommand {
            program: self.resolved_binary(),
            args: ["-p", "/usage", "--output-format", "json"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            cwd: new_scratch_dir()
                .map_err(|e| FetchFailure::Other(format!("no scratch directory: {e}")))?,
            stdin_lines: Vec::new(),
            env: self.child_env(),
            close_stdin: true,
        };
        let scratch = command.cwd.clone();

        let result = self.run_cli(&command, timeout);
        let _ = std::fs::remove_dir_all(&scratch);
        result
    }

    fn run_cli(
        &self,
        command: &HarnessCommand,
        timeout: Duration,
    ) -> std::result::Result<ProviderSnapshot, FetchFailure> {
        let mut child = self
            .harness
            .spawn(command)
            .map_err(|e| FetchFailure::Other(format!("could not start claude: {e}")))?;

        let mut stdout = String::new();
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                timed_out = true;
                break;
            }
            match child.next_line(remaining) {
                Ok(Some(line)) => stdout.push_str(&line),
                Ok(None) => break,
                Err(e) => {
                    child.kill();
                    return Err(FetchFailure::Other(format!(
                        "reading claude's output failed: {e}"
                    )));
                }
            }
        }
        let outcome = child.outcome();
        child.kill();

        // A child still holding the pipe when the budget ran out is a timeout;
        // one that closed it early is not, however little it said. The old
        // code could not tell these apart and called both the same thing.
        if timed_out || (stdout.is_empty() && Instant::now() >= deadline) {
            return Err(FetchFailure::Other(
                "claude did not print its usage in time".to_owned(),
            ));
        }

        match claude_cli_reply(&stdout) {
            Some(CliReply::Usage(text)) => interpret_claude_usage_text(&text).ok_or_else(|| {
                FetchFailure::Other("claude's usage screen did not parse".to_owned())
            }),
            // The CLI reports being signed out as a *successful* run carrying
            // an error reply, so this is the message a person actually gets,
            // and it arrives on stdout rather than stderr.
            Some(CliReply::Error(message)) => Err(classify_cli_message(&message)),
            // The old code said "claude exited without reporting rate limits"
            // for every one of these at once, having thrown away both the exit
            // code and stderr on the way. Whatever happened is what a person
            // needs to read.
            None => Err(classify_cli_failure(&outcome)),
        }
    }

    /// How long, if at all, this fetcher is still standing down after a 429.
    fn backoff_remaining(&self) -> Option<Duration> {
        let retry_at = self
            .retry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retry_at.and_then(|at| at.checked_duration_since(Instant::now()))
    }

    fn stand_down_for(&self, wait: Duration) {
        let mut retry_at = self
            .retry_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *retry_at = Instant::now().checked_add(wait);
    }
}

impl Default for ClaudeRateLimitSource {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitSource for ClaudeRateLimitSource {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn fetch(&self, timeout: Duration) -> ProviderSnapshot {
        if let Some(remaining) = self.backoff_remaining() {
            return ProviderSnapshot::unavailable(
                Provider::Claude,
                format!(
                    "rate limited — not asking again for {}",
                    approx_duration(remaining)
                ),
            );
        }

        let oauth = match self.fetch_oauth(timeout) {
            Ok(snapshot) => return snapshot,
            Err(failure) => self.note(failure),
        };

        if !oauth.worth_a_fallback() {
            return ProviderSnapshot::unavailable(Provider::Claude, oauth.reason().to_owned());
        }

        match self.fetch_cli(timeout) {
            Ok(snapshot) => snapshot,
            // Both rungs failed, and each failed its own way. Reporting only
            // the second would hide the first, which is usually the real one.
            Err(cli) => ProviderSnapshot::unavailable(
                Provider::Claude,
                format!("{} (and {})", self.note(cli).reason(), oauth.reason()),
            ),
        }
    }
}

impl ClaudeRateLimitSource {
    /// Records anything a failure implies for the next fetch, and hands it
    /// back unchanged.
    ///
    /// Only one thing qualifies today: a rate limit means stop asking. It is a
    /// method rather than a branch at each call site so that no rung can
    /// report one and forget to honour it.
    fn note(&self, failure: FetchFailure) -> FetchFailure {
        if let FetchFailure::RateLimited { retry_after, .. } = &failure {
            self.stand_down_for(retry_after.unwrap_or(CLAUDE_DEFAULT_BACKOFF));
        }
        failure
    }
}

/// How long to stand down after a `429` that carried no `Retry-After`.
const CLAUDE_DEFAULT_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// The `Retry-After` a 429 body may carry, as a duration.
///
/// curl is asked for headers nowhere here, so this reads the JSON error body
/// Claude's API sends, which repeats the value. A header-only 429 falls back
/// to [`CLAUDE_DEFAULT_BACKOFF`], and either way the wait is capped at a day —
/// a server asking for longer than that is a server ket should re-ask
/// tomorrow rather than trust indefinitely.
fn retry_after_of(body: &str) -> Option<Duration> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let seconds = value
        .get("retry_after")
        .or_else(|| value.get("retryAfter"))
        .or_else(|| value.get("error").and_then(|e| e.get("retry_after")))
        .and_then(serde_json::Value::as_u64)?;

    Some(Duration::from_secs(seconds.min(24 * 60 * 60)))
}

/// A rough, human duration — "12 minutes", not "12m 3.4s".
fn approx_duration(d: Duration) -> String {
    let minutes = d.as_secs().div_ceil(60);
    match minutes {
        0..=1 => "a minute".to_owned(),
        2..=59 => format!("{minutes} minutes"),
        60..=119 => "an hour".to_owned(),
        _ => format!("{} hours", minutes / 60),
    }
}

/// What a CLI rung's failure actually was.
///
/// The three cases worth separating, because each has a different fix: not
/// signed in (sign in), not installed where ket looked (a `PATH` problem),
/// and everything else (which at least now carries the child's own words).
fn classify_cli_failure(outcome: &ChildOutcome) -> FetchFailure {
    let stderr = outcome.stderr.trim();
    let haystack = stderr.to_lowercase();

    if haystack.contains("not logged in") || haystack.contains("please run /login") {
        return FetchFailure::Auth(
            "claude is not logged in — run `claude` once in a terminal and sign in".to_owned(),
        );
    }
    if haystack.contains("command not found") || haystack.contains("no such file") {
        return FetchFailure::Other("the claude binary could not be found".to_owned());
    }

    FetchFailure::Other(outcome.describe("claude printed no usage"))
}

/// What a `claude -p "/usage" --output-format json` run said.
enum CliReply {
    /// The usage screen, as text.
    Usage(String),
    /// The CLI's own error message, which it reports as an `is_error` reply
    /// rather than on stderr — "Not logged in · Please run /login" arrives
    /// this way, and it is the single most common reason for this rung to
    /// fail.
    Error(String),
}

/// Reads one `--output-format json` reply.
///
/// The reply is a single JSON object whose `result` holds either the screen or
/// the error, distinguished by `is_error`. A successful reply that carries no
/// percentage is treated as an error rather than as empty usage: the CLI
/// prints a sentence explaining itself in that case, and the sentence is worth
/// more to a person than "did not parse".
fn claude_cli_reply(stdout: &str) -> Option<CliReply> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let text = value.get("result")?.as_str()?.to_owned();
    let failed = value.get("is_error").and_then(serde_json::Value::as_bool) == Some(true);

    Some(if failed || !text.contains('%') {
        CliReply::Error(text)
    } else {
        CliReply::Usage(text)
    })
}

/// Turns the CLI's own error sentence into a failure the ladder understands.
fn classify_cli_message(message: &str) -> FetchFailure {
    let haystack = message.to_lowercase();

    if haystack.contains("not logged in") || haystack.contains("/login") {
        return FetchFailure::Auth(
            "claude is not logged in — run `claude` once in a terminal and sign in".to_owned(),
        );
    }
    if haystack.contains("rate limit") {
        return FetchFailure::RateLimited {
            reason: "claude is rate limiting this account".to_owned(),
            retry_after: None,
        };
    }

    FetchFailure::Other(format!("claude said: {}", first_line_of(message)))
}

/// Interprets the OAuth endpoint's JSON.
///
/// **Read against what the endpoint actually returns**, which is the only
/// specification there is: `five_hour` and `seven_day` objects carrying a
/// `utilization` (or `used_percentage`) and a `resets_at`, plus a per-model
/// weekly window that arrives either as its own key or inside `limits` tagged
/// `weekly_scoped`.
///
/// The percentage here is **already a percentage**, unlike the `utilization`
/// fraction in the status line block — the same field name, two scales, and
/// the reason both parsers are separate functions rather than one shared one.
fn interpret_claude_usage(body: &str) -> Option<ProviderSnapshot> {
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;

    let mut windows = Vec::new();
    if let Some(window) = usage_window(value.get("five_hour"), "Session", Some(300)) {
        windows.push(window);
    }
    if let Some(window) = usage_window(value.get("seven_day"), "Weekly", Some(10_080)) {
        windows.push(window);
    }
    if let Some(window) = scoped_weekly_window(&value) {
        windows.push(window);
    }

    (!windows.is_empty()).then(|| ProviderSnapshot {
        provider: Provider::Claude,
        status: SnapshotStatus::Fresh,
        windows,
        plan: value
            .get("plan")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        account: None,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    })
}

/// One window from the OAuth endpoint.
fn usage_window(
    value: Option<&serde_json::Value>,
    name: &str,
    minutes: Option<u32>,
) -> Option<RateWindow> {
    let value = value?;
    let percent = value
        .get("utilization")
        .or_else(|| value.get("used_percentage"))
        .or_else(|| value.get("percent"))
        .and_then(serde_json::Value::as_f64)?;

    Some(RateWindow {
        name: name.to_owned(),
        used_percent: percent.clamp(0.0, 100.0) as f32,
        resets_at_ms: value.get("resets_at").and_then(epoch_ms_of),
        window_minutes: minutes,
    })
}

/// The per-model weekly window, which the endpoint reports in whichever of
/// several shapes the account happens to produce.
fn scoped_weekly_window(value: &serde_json::Value) -> Option<RateWindow> {
    let scoped = value
        .get("limits")
        .and_then(serde_json::Value::as_array)
        .and_then(|limits| {
            limits.iter().find(|limit| {
                limit.get("kind").and_then(serde_json::Value::as_str) == Some("weekly_scoped")
            })
        });

    if let Some(limit) = scoped {
        let name = limit
            .get("scope")
            .and_then(|s| s.get("model"))
            .and_then(|m| m.get("display_name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("model");
        return usage_window(Some(limit), &format!("Weekly ({name})"), Some(10_080));
    }

    ["fable_weekly", "fable_seven_day", "seven_day_fable"]
        .into_iter()
        .find_map(|key| usage_window(value.get(key), "Weekly (Fable)", Some(10_080)))
}

/// A unix time from the endpoint, which reports seconds, milliseconds or an
/// ISO string depending on the field.
fn epoch_ms_of(value: &serde_json::Value) -> Option<u64> {
    if let Some(number) = value.as_u64() {
        // Anything below this is far too small to be milliseconds and far too
        // recent to be anything but seconds.
        return Some(if number > 10_000_000_000 {
            number
        } else {
            number.saturating_mul(1000)
        });
    }
    epoch_ms_of_text(value.as_str()?)
}

/// Interprets the CLI's `/usage` screen.
///
/// The screen is written for a person, so this reads it the way a person does
/// — find the line naming a window, take the percentage on it — rather than
/// pretending it is a format with a contract. Every value is optional and a
/// line that does not parse is skipped, because a missing window is a smaller
/// lie than a guessed one.
///
/// The lines, as of 2.1.260:
///
/// ```text
/// Current session: 13% used · resets Sep 10 at 6:09pm (America/New_York)
/// Current week (all models): 69% used · resets Sep 11 at 12:59pm (America/New_York)
/// Current week (Fable): 29% used · resets Sep 11 at 12:59pm (America/New_York)
/// ```
fn interpret_claude_usage_text(text: &str) -> Option<ProviderSnapshot> {
    let mut windows = Vec::new();

    for line in text.lines() {
        let lower = line.to_lowercase();
        let is_session = lower.contains("current session");
        let is_weekly = lower.contains("current week")
            || lower.contains("weekly limit")
            || lower.contains("weekly usage")
            || lower.contains("7-day")
            || lower.contains("7 day");

        if !is_session && !is_weekly {
            continue;
        }
        let Some(used_percent) = percent_used_in(&lower) else {
            continue;
        };

        let minutes = if is_session { 300 } else { 10_080 };
        let name = if is_session {
            "Session".to_owned()
        } else {
            match model_scope_in(line) {
                Some(model) => format!("Weekly ({model})"),
                None => "Weekly".to_owned(),
            }
        };

        windows.push(RateWindow {
            name,
            used_percent,
            resets_at_ms: line
                .split_once("resets ")
                .and_then(|(_, rest)| epoch_ms_of_text(rest)),
            window_minutes: Some(minutes),
        });
    }

    (!windows.is_empty()).then(|| ProviderSnapshot {
        provider: Provider::Claude,
        status: SnapshotStatus::Fresh,
        windows,
        plan: None,
        account: None,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    })
}

/// The percentage on a `/usage` line, normalised to "used".
///
/// Both polarities appear on that screen depending on the window and the
/// plan — "13% used" and "87% left" are the same fact — and reporting the
/// second as the first would be exactly the "failure rendering as a
/// percentage" this module exists to prevent, only inverted.
fn percent_used_in(lower: &str) -> Option<f32> {
    let (before, after) = lower.split_once('%')?;
    let digits: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f32 = digits.chars().rev().collect::<String>().parse().ok()?;

    let remaining = after.trim_start();
    let is_left = remaining.starts_with("left")
        || remaining.starts_with("remaining")
        || remaining.starts_with("available");

    Some(if is_left {
        (100.0 - value).clamp(0.0, 100.0)
    } else {
        value.clamp(0.0, 100.0)
    })
}

/// The model a weekly line is scoped to, where it names one.
///
/// `Current week (all models)` is the unscoped window and keeps the plain
/// "Weekly" label; `Current week (Fable)` is a second, separate window and
/// has to be labelled as one or the two overwrite each other in the panel.
fn model_scope_in(line: &str) -> Option<String> {
    let (_, rest) = line.split_once('(')?;
    let (scope, _) = rest.split_once(')')?;
    let scope = scope.trim();

    (!scope.eq_ignore_ascii_case("all models")).then(|| scope.to_owned())
}

/// A reset time written for a person — `Sep 10 at 6:09pm (America/New_York)`
/// — as milliseconds since the epoch.
///
/// The screen prints wall-clock time in the machine's own zone with no year,
/// so both have to be recovered: the offset from the machine (a reset is
/// always in the future, and never more than a week out, which is enough to
/// pin the year), and the year by choosing the one that puts the result
/// ahead of now. `None` whenever any part of that is uncertain — a window
/// with no reset time still shows its percentage, which is the part that
/// matters.
fn epoch_ms_of_text(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<u64>() {
        return Some(if seconds > 10_000_000_000 {
            seconds
        } else {
            seconds.saturating_mul(1000)
        });
    }

    // The zone name goes first, and not only for tidiness: `America/New_York`
    // contains "am", which would read 12:59pm back as five past midnight.
    let text = text.split('(').next().unwrap_or(text).trim();

    let (date, time) = text.split_once(" at ")?;
    let mut date_parts = date.split_whitespace();
    let month = month_of(date_parts.next()?)?;
    let day: u32 = date_parts.next()?.trim_end_matches(',').parse().ok()?;
    let (hour, minute) = clock_of(time)?;

    let now_secs = now_ms() / 1000;
    let this_year = year_of(now_secs);

    // A reset is ahead of now. Where the printed date has already passed this
    // year, it belongs to the next one — the December-to-January case, which
    // would otherwise report a reset eleven months in the past.
    [this_year, this_year + 1]
        .into_iter()
        .map(|year| civil_to_epoch_secs(year, month, day, hour, minute))
        .find(|candidate| *candidate + 60 * 60 > now_secs)
        .map(|secs| secs.saturating_mul(1000))
}

/// `6:09pm`, `18:09`, `6:09 PM` — as 24-hour hours and minutes.
fn clock_of(text: &str) -> Option<(u32, u32)> {
    let text = text.trim().to_lowercase();
    let is_pm = text.contains("pm");
    let is_am = text.contains("am");
    let digits = text
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .trim_start_matches(|c: char| !c.is_ascii_digit());

    // The `/usage` screen prints minutes only when there are any: `6:40pm`,
    // but `1pm` on the hour. A weekly window nearly always resets on the
    // hour, so without this every "Weekly" row lost its reset while the
    // session kept its own.
    let (hour, minute) = digits.split_once(':').unwrap_or((digits, "0"));
    let mut hour: u32 = hour.trim().parse().ok()?;
    let minute: u32 = minute.trim().parse().ok()?;

    if is_pm && hour < 12 {
        hour += 12;
    }
    if is_am && hour == 12 {
        hour = 0;
    }

    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// A three-letter month name as a 1-12 number.
fn month_of(name: &str) -> Option<u32> {
    let name = name.trim().to_lowercase();
    [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| name.starts_with(m))
    .map(|index| index as u32 + 1)
}

/// Local wall-clock time to a unix timestamp.
///
/// The offset is the machine's own, read once — the same machine drew the
/// screen being parsed, so its zone is by definition the one the time is
/// written in. A reading taken on one side of a daylight-saving change and
/// applied to a time on the other is an hour out for a few hours a year, in a
/// field that is displayed as "resets in about 5 hours".
fn civil_to_epoch_secs(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
    let days = days_from_civil(year, month as i64, day as i64);
    let utc = days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60;

    (utc - local_utc_offset_secs()).max(0) as u64
}

/// Days since 1970-01-01 for a civil date — Howard Hinnant's algorithm, which
/// is the standard one and correct for every proleptic Gregorian date.
pub(super) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

/// The civil year a unix timestamp falls in.
fn year_of(seconds: u64) -> i64 {
    let days = (seconds as i64 + local_utc_offset_secs()).div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month = (5 * day_of_year + 2) / 153;

    if month >= 10 { year + 1 } else { year }
}

/// This machine's current offset from UTC, in seconds, read once.
///
/// `date` rather than a crate: the alternative is a time library carried for
/// one number, or `libc` bindings for `localtime_r`, and this is a status bar
/// asking what hour it is locally. A machine whose `date` cannot answer is
/// treated as UTC, which puts a reset time out by the offset rather than
/// removing it.
fn local_utc_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();

    *OFFSET.get_or_init(|| {
        let Ok(output) = std::process::Command::new("date").arg("+%z").output() else {
            return 0;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let text = text.trim();
        if text.len() < 5 {
            return 0;
        }

        let sign = if text.starts_with('-') { -1 } else { 1 };
        let digits = &text[1..];
        let hours: i64 = digits[..2].parse().unwrap_or(0);
        let minutes: i64 = digits[2..4].parse().unwrap_or(0);

        sign * (hours * 3600 + minutes * 60)
    })
}

/// The name of the user ket is running as.
///
/// `USER` where the environment has it, and the home directory's own name
/// where it does not — which is the case this whole function exists for, a
/// GUI process started by launchd rather than by a shell.
pub(super) fn current_username() -> Option<String> {
    if let Some(user) = std::env::var_os("USER")
        && !user.is_empty()
    {
        return Some(user.to_string_lossy().into_owned());
    }

    crate::paths::home()
        .ok()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Interprets the payload Claude hands its status line command.
///
/// The first and best rung of the Claude ladder: the same numbers as
/// [`interpret_claude_usage`], from a session that is already running and has
/// already paid for them — see [`crate::agent_hooks::ClaudeStatusLine`] for
/// why that matters and what it costs to arrange.
///
/// `None` means "no quota in this payload", which is the ordinary case: the
/// status line is drawn constantly and the `rate_limits` block is not always
/// in it. That is deliberately *not* [`SnapshotStatus::Unavailable`] — a
/// payload without quota is not a failed fetch, and reporting it as one would
/// throw away the last good numbers on the very next repaint.
///
/// **Read against the shape Claude Code 2.1.260 publishes**, which is a
/// `rate_limits` object keyed by window with a `utilization` and a
/// `resets_at`. Both spellings of the reset field are accepted because the
/// sibling `stream-json` frame spells the same field `resetsAt`, and a window
/// missing its utilization is skipped rather than shown as zero.
pub fn snapshot_from_statusline(payload: &serde_json::Value) -> Option<ProviderSnapshot> {
    let limits = payload.get("rate_limits")?.as_object()?;

    let mut windows: Vec<RateWindow> = limits
        .iter()
        .filter_map(|(key, value)| statusline_window(key, value))
        .collect();

    if windows.is_empty() {
        return None;
    }

    windows.sort_by(|a, b| a.name.cmp(&b.name));

    Some(ProviderSnapshot {
        provider: Provider::Claude,
        status: SnapshotStatus::Fresh,
        windows,
        plan: None,
        account: None,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    })
}

/// One window from a status line `rate_limits` block.
///
/// The block carries scalars alongside the windows — `overage` and
/// `spend_limit` are not windows and have no utilization — so anything
/// without one is skipped rather than coerced.
fn statusline_window(key: &str, value: &serde_json::Value) -> Option<RateWindow> {
    let utilization = value.get("utilization")?.as_f64()?;
    let resets_at_ms = value
        .get("resets_at")
        .or_else(|| value.get("resetsAt"))
        .and_then(serde_json::Value::as_u64)
        .map(|secs| secs.saturating_mul(1000));
    let minutes = claude_window_minutes(key);

    Some(RateWindow {
        name: claude_window_label(key, minutes),
        // The frame reports a fraction and so, on the evidence of the two
        // sharing a field name, does this. A percentage arriving here instead
        // would clamp to 100 rather than read as 6000%.
        used_percent: (utilization * 100.0).clamp(0.0, 100.0) as f32,
        resets_at_ms,
        window_minutes: minutes,
    })
}

/// The known duration, in minutes, behind a `unifiedWindows` key — observed
/// directly on this account, not documented anywhere.
fn claude_window_minutes(key: &str) -> Option<u32> {
    match key {
        "five_hour" => Some(300),
        "seven_day" | "seven_day_overage_included" => Some(10_080),
        _ => None,
    }
}

/// A display name for a `unifiedWindows` key.
fn claude_window_label(key: &str, minutes: Option<u32>) -> String {
    if key == "seven_day_overage_included" {
        return "Weekly (overage)".to_owned();
    }
    window_label(minutes, &key.replace('_', " "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_limits::fakes::*;

    // -- Claude ---------------------------------------------------------------

    #[test]
    fn claude_reads_the_usage_endpoint_without_starting_a_session() {
        // Rung two: a keychain token, then one HTTP GET. The body is the
        // shape the endpoint returns.
        let harness = Arc::new(FakeHarness::with_responses(&[
            &["oauth-token"],
            &[
                r#"{"five_hour":{"utilization":19,"resets_at":1788718800},"seven_day":{"utilization":14,"resets_at":1789146000}}"#,
                "ket-http-status:200",
            ],
        ]));
        let source =
            ClaudeRateLimitSource::with_harness(harness.clone() as Arc<dyn ProcessHarness>);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert_eq!(snapshot.status, SnapshotStatus::Fresh);
        assert_eq!(snapshot.windows.len(), 2);
        let session = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Session")
            .unwrap();
        assert!((session.used_percent - 19.0).abs() < 0.01);
        assert_eq!(session.resets_at_ms, Some(1_788_718_800_000));
        assert_eq!(session.window_minutes, Some(300));
        let weekly = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Weekly")
            .unwrap();
        assert!((weekly.used_percent - 14.0).abs() < 0.01);

        // Nothing that costs a turn was started, and the token never reached
        // an argument vector.
        let spawned = harness.spawned.lock().unwrap();
        assert!(spawned.iter().all(|c| c.program != "claude"));
        assert!(spawned.iter().all(|c| {
            !c.args
                .iter()
                .any(|a| a.to_string_lossy().contains("oauth-token"))
        }));
    }

    #[test]
    fn claude_falls_back_to_the_free_usage_screen_when_no_token_is_available() {
        // Rung three: no keychain item, so the CLI is asked for the same
        // numbers. `/usage` costs nothing — verified on 2.1.260 as
        // `num_turns: 0`, `total_cost_usd: 0`.
        let usage = r#"{"is_error":false,"result":"Current session: 13% used · resets Sep 10 at 6:09pm (America/New_York)\nCurrent week (all models): 69% used · resets Sep 11 at 12:59pm (America/New_York)\nCurrent week (Fable): 29% used · resets Sep 11 at 12:59pm (America/New_York)"}"#;
        let harness = Arc::new(FakeHarness::with_responses(&[&[], &[usage]]));
        let source =
            ClaudeRateLimitSource::with_harness(harness.clone() as Arc<dyn ProcessHarness>);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert_eq!(snapshot.status, SnapshotStatus::Fresh);
        let session = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Session")
            .unwrap();
        assert!((session.used_percent - 13.0).abs() < 0.01);
        let weekly = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Weekly")
            .unwrap();
        assert!((weekly.used_percent - 69.0).abs() < 0.01);
        // The per-model window is a window of its own, not a second "Weekly"
        // overwriting the first.
        let fable = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Weekly (Fable)")
            .unwrap();
        assert!((fable.used_percent - 29.0).abs() < 0.01);

        // The CLI was asked for `/usage`, not for a turn.
        let spawned = harness.spawned.lock().unwrap();
        let cli = spawned.last().unwrap();
        assert!(cli.args.iter().any(|a| a == "/usage"));
    }

    #[test]
    fn a_rejected_token_is_not_retried_through_the_cli() {
        // A 401 means the credential is bad, and the CLI would present the
        // same one. Reporting it once beats spawning to be told twice.
        let harness = Arc::new(FakeHarness::with_responses(&[
            &["stale-token"],
            &["ket-http-status:401"],
        ]));
        let source =
            ClaudeRateLimitSource::with_harness(harness.clone() as Arc<dyn ProcessHarness>);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
        assert_eq!(harness.spawned.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_rate_limited_account_is_not_asked_again_while_it_is_rate_limited() {
        let harness = Arc::new(FakeHarness::with_responses(&[
            &["token"],
            &[r#"{"retry_after":600}"#, "ket-http-status:429"],
        ]));
        let source =
            ClaudeRateLimitSource::with_harness(harness.clone() as Arc<dyn ProcessHarness>);

        let first = source.fetch(Duration::from_secs(5));
        assert!(matches!(first.status, SnapshotStatus::Unavailable { .. }));
        let spawns = harness.spawned.lock().unwrap().len();

        // The second fetch must spend nothing at all: no keychain read, no
        // request, no session.
        let second = source.fetch(Duration::from_secs(5));
        assert!(matches!(second.status, SnapshotStatus::Unavailable { .. }));
        assert_eq!(harness.spawned.lock().unwrap().len(), spawns);
    }

    #[test]
    fn a_provider_that_never_answers_reports_unknown_rather_than_zero() {
        let harness = Arc::new(FakeHarness::hanging());
        let source = ClaudeRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_millis(30));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
        assert!(snapshot.windows.is_empty());
        assert!(snapshot.fetched_at_ms.is_none());
    }

    #[test]
    fn a_malformed_usage_response_degrades_rather_than_panicking() {
        let harness = Arc::new(FakeHarness::with_responses(&[
            &["token"],
            &["{}", "ket-http-status:200"],
            &[r#"{"is_error":false,"result":"nothing here"}"#],
        ]));
        let source = ClaudeRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn garbage_stdout_does_not_panic_and_resolves_to_unavailable() {
        let harness = Arc::new(FakeHarness::with_responses(&[
            &["token"],
            &["not json at all", "{", "ket-http-status:200"],
            &["also not json", "{"],
        ]));
        let source = ClaudeRateLimitSource::with_harness(harness);

        let snapshot = source.fetch(Duration::from_secs(5));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    #[test]
    fn a_failure_says_what_the_child_said_rather_than_one_sentence_for_everything() {
        // The whole point of keeping stderr: "not logged in" and "no such
        // file" used to arrive at the panel as the same words.
        #[derive(Debug)]
        struct NotLoggedIn;
        impl ChildLines for NotLoggedIn {
            fn next_line(&mut self, _timeout: Duration) -> Result<Option<String>> {
                Ok(None)
            }
            fn kill(&mut self) {}
            fn outcome(&mut self) -> ChildOutcome {
                ChildOutcome {
                    code: Some(1),
                    stderr: "Not logged in · Please run /login".to_owned(),
                }
            }
        }
        #[derive(Debug)]
        struct Harness;
        impl ProcessHarness for Harness {
            fn spawn(&self, _command: &HarnessCommand) -> Result<Box<dyn ChildLines>> {
                Ok(Box::new(NotLoggedIn))
            }
        }

        let source = ClaudeRateLimitSource::with_harness(Arc::new(Harness));
        let snapshot = source.fetch(Duration::from_secs(5));

        let SnapshotStatus::Unavailable { reason } = snapshot.status else {
            panic!("a child that reported nothing must be unavailable");
        };
        assert!(
            reason.contains("not logged in"),
            "the reason should carry the child's own words, got: {reason}"
        );
    }

    #[test]
    fn a_spawn_failure_is_unavailable_not_a_panic() {
        #[derive(Debug)]
        struct AlwaysFails;
        impl ProcessHarness for AlwaysFails {
            fn spawn(&self, _command: &HarnessCommand) -> Result<Box<dyn ChildLines>> {
                Err(KetError::Agent {
                    agent: "claude".to_owned(),
                    why: "not on PATH".to_owned(),
                })
            }
        }

        let source = ClaudeRateLimitSource::with_harness(Arc::new(AlwaysFails));
        let snapshot = source.fetch(Duration::from_secs(5));

        assert!(matches!(
            snapshot.status,
            SnapshotStatus::Unavailable { .. }
        ));
    }

    // -- parsing helpers --------------------------------------------------

    #[test]
    fn retry_after_of_reads_every_shape_the_api_sends() {
        assert_eq!(
            retry_after_of(r#"{"retry_after":30}"#),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_after_of(r#"{"retryAfter":30}"#),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_after_of(r#"{"error":{"retry_after":30}}"#),
            Some(Duration::from_secs(30))
        );
        assert_eq!(retry_after_of("not json"), None);
        assert_eq!(retry_after_of("{}"), None);
    }

    #[test]
    fn retry_after_of_is_capped_at_a_day() {
        assert_eq!(
            retry_after_of(r#"{"retry_after":999999}"#),
            Some(Duration::from_secs(24 * 60 * 60))
        );
    }

    #[test]
    fn approx_duration_rounds_up_into_the_next_named_bucket() {
        assert_eq!(approx_duration(Duration::from_secs(0)), "a minute");
        assert_eq!(approx_duration(Duration::from_secs(61)), "2 minutes");
        assert_eq!(approx_duration(Duration::from_secs(59 * 60)), "59 minutes");
        assert_eq!(approx_duration(Duration::from_secs(60 * 60)), "an hour");
        assert_eq!(approx_duration(Duration::from_secs(119 * 60)), "an hour");
        assert_eq!(approx_duration(Duration::from_secs(180 * 60)), "3 hours");
    }

    #[test]
    fn classify_cli_failure_recognises_not_logged_in() {
        let outcome = ChildOutcome {
            code: Some(1),
            stderr: "Error: not logged in".to_owned(),
        };
        assert!(matches!(
            classify_cli_failure(&outcome),
            FetchFailure::Auth(_)
        ));
    }

    #[test]
    fn classify_cli_failure_recognises_a_missing_binary() {
        let outcome = ChildOutcome {
            code: Some(127),
            stderr: "sh: claude: command not found".to_owned(),
        };
        assert!(matches!(
            classify_cli_failure(&outcome),
            FetchFailure::Other(reason) if reason.contains("could not be found")
        ));
    }

    #[test]
    fn classify_cli_failure_falls_back_to_the_childs_own_words() {
        let outcome = ChildOutcome {
            code: Some(1),
            stderr: "".to_owned(),
        };
        assert!(matches!(
            classify_cli_failure(&outcome),
            FetchFailure::Other(_)
        ));
    }

    #[test]
    fn claude_cli_reply_reads_a_usage_reply() {
        let reply = claude_cli_reply(r#"{"is_error":false,"result":"13% used"}"#).unwrap();
        assert!(matches!(reply, CliReply::Usage(text) if text == "13% used"));
    }

    #[test]
    fn claude_cli_reply_treats_is_error_as_an_error_reply() {
        let reply = claude_cli_reply(r#"{"is_error":true,"result":"not logged in"}"#).unwrap();
        assert!(matches!(reply, CliReply::Error(text) if text == "not logged in"));
    }

    #[test]
    fn claude_cli_reply_treats_a_percentless_success_as_an_error() {
        let reply = claude_cli_reply(r#"{"is_error":false,"result":"no usage here"}"#).unwrap();
        assert!(matches!(reply, CliReply::Error(_)));
    }

    #[test]
    fn claude_cli_reply_is_none_for_unparseable_stdout() {
        assert!(claude_cli_reply("not json").is_none());
        assert!(claude_cli_reply(r#"{"result": 5}"#).is_none());
    }

    #[test]
    fn classify_cli_message_recognises_auth_and_rate_limit_phrases() {
        assert!(matches!(
            classify_cli_message("Not logged in · Please run /login"),
            FetchFailure::Auth(_)
        ));
        assert!(matches!(
            classify_cli_message("you are being rate limited"),
            FetchFailure::RateLimited { .. }
        ));
        assert!(matches!(
            classify_cli_message("something else entirely"),
            FetchFailure::Other(_)
        ));
    }

    #[test]
    fn interpret_claude_usage_reads_session_and_weekly_windows() {
        let snapshot = interpret_claude_usage(
            r#"{"five_hour":{"utilization":19,"resets_at":1788718800},"seven_day":{"used_percentage":14},"plan":"Max"}"#,
        )
        .unwrap();
        assert_eq!(snapshot.plan.as_deref(), Some("Max"));
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(
            snapshot
                .windows
                .iter()
                .find(|w| w.name == "Session")
                .unwrap()
                .resets_at_ms,
            Some(1_788_718_800_000)
        );
    }

    #[test]
    fn interpret_claude_usage_is_none_without_any_window() {
        assert!(interpret_claude_usage(r#"{"plan":"Max"}"#).is_none());
        assert!(interpret_claude_usage("not json").is_none());
    }

    #[test]
    fn usage_window_tries_every_percentage_field_name_and_clamps() {
        assert_eq!(
            usage_window(Some(&serde_json::json!({"utilization": 200})), "S", None)
                .unwrap()
                .used_percent,
            100.0
        );
        assert_eq!(
            usage_window(Some(&serde_json::json!({"used_percentage": -5})), "S", None)
                .unwrap()
                .used_percent,
            0.0
        );
        assert_eq!(
            usage_window(Some(&serde_json::json!({"percent": 42})), "S", None)
                .unwrap()
                .used_percent,
            42.0
        );
        assert!(usage_window(Some(&serde_json::json!({})), "S", None).is_none());
        assert!(usage_window(None, "S", None).is_none());
    }

    #[test]
    fn scoped_weekly_window_reads_the_model_scoped_limit() {
        let value = serde_json::json!({
            "limits": [
                {"kind": "other"},
                {
                    "kind": "weekly_scoped",
                    "utilization": 29,
                    "scope": {"model": {"display_name": "Fable"}}
                }
            ]
        });
        let window = scoped_weekly_window(&value).unwrap();
        assert_eq!(window.name, "Weekly (Fable)");
        assert_eq!(window.used_percent, 29.0);
    }

    #[test]
    fn scoped_weekly_window_falls_back_to_a_named_fable_key() {
        let value = serde_json::json!({"fable_weekly": {"utilization": 5}});
        let window = scoped_weekly_window(&value).unwrap();
        assert_eq!(window.name, "Weekly (Fable)");
    }

    #[test]
    fn scoped_weekly_window_is_none_without_either_shape() {
        assert!(scoped_weekly_window(&serde_json::json!({})).is_none());
    }

    #[test]
    fn epoch_ms_of_scales_seconds_but_not_milliseconds() {
        assert_eq!(
            epoch_ms_of(&serde_json::json!(1_700_000_000)),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            epoch_ms_of(&serde_json::json!(1_700_000_000_000_u64)),
            Some(1_700_000_000_000)
        );
    }

    #[test]
    fn epoch_ms_of_falls_back_to_text_parsing() {
        assert_eq!(
            epoch_ms_of(&serde_json::json!("1700000000")),
            Some(1_700_000_000_000)
        );
        assert!(epoch_ms_of(&serde_json::json!(null)).is_none());
    }

    #[test]
    fn interpret_claude_usage_text_reads_the_usage_screen() {
        let text = "Current session: 13% used · resets Sep 10 at 6:09pm (America/New_York)\n\
                     Current week (all models): 69% used\n\
                     Current week (Fable): 29% used";
        let snapshot = interpret_claude_usage_text(text).unwrap();
        assert_eq!(snapshot.windows.len(), 3);
        let session = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Session")
            .unwrap();
        assert_eq!(session.window_minutes, Some(300));
        assert!(session.resets_at_ms.is_some());
        let weekly = snapshot
            .windows
            .iter()
            .find(|w| w.name == "Weekly")
            .unwrap();
        assert_eq!(weekly.window_minutes, Some(10_080));
        assert!(snapshot.windows.iter().any(|w| w.name == "Weekly (Fable)"));
    }

    #[test]
    fn interpret_claude_usage_text_ignores_unrelated_lines() {
        assert!(interpret_claude_usage_text("hello\nworld").is_none());
    }

    #[test]
    fn percent_used_in_normalises_used_and_left_to_the_same_polarity() {
        assert_eq!(percent_used_in("13% used"), Some(13.0));
        assert_eq!(percent_used_in("87% left"), Some(13.0));
        assert_eq!(percent_used_in("87% remaining"), Some(13.0));
        assert_eq!(percent_used_in("87% available"), Some(13.0));
        assert_eq!(percent_used_in("no percent here"), None);
    }

    #[test]
    fn percent_used_in_clamps_to_the_valid_range() {
        assert_eq!(percent_used_in("150% used"), Some(100.0));
    }

    #[test]
    fn model_scope_in_ignores_the_unscoped_all_models_label() {
        assert_eq!(model_scope_in("Current week (all models): 69%"), None);
        assert_eq!(
            model_scope_in("Current week (Fable): 29%"),
            Some("Fable".to_owned())
        );
        assert_eq!(model_scope_in("no parens here"), None);
    }

    #[test]
    fn epoch_ms_of_text_parses_a_raw_epoch() {
        assert_eq!(epoch_ms_of_text("1700000000"), Some(1_700_000_000_000));
    }

    #[test]
    fn epoch_ms_of_text_is_none_for_an_unparseable_string() {
        assert!(epoch_ms_of_text("whenever it feels like it").is_none());
    }

    #[test]
    fn clock_of_reads_twelve_and_twenty_four_hour_times() {
        assert_eq!(clock_of("6:09pm"), Some((18, 9)));
        assert_eq!(clock_of("6:09 PM"), Some((18, 9)));
        assert_eq!(clock_of("18:09"), Some((18, 9)));
        assert_eq!(clock_of("12am"), Some((0, 0)));
        assert_eq!(clock_of("12pm"), Some((12, 0)));
        assert_eq!(clock_of("1pm"), Some((13, 0)));
    }

    #[test]
    fn clock_of_rejects_an_out_of_range_time() {
        assert!(clock_of("25:00").is_none());
        assert!(clock_of("not a time").is_none());
    }

    #[test]
    fn month_of_matches_a_three_letter_prefix() {
        assert_eq!(month_of("Sep"), Some(9));
        assert_eq!(month_of("september"), Some(9));
        assert_eq!(month_of("Jan"), Some(1));
        assert_eq!(month_of("Dec"), Some(12));
        assert_eq!(month_of("nope"), None);
    }

    #[test]
    fn current_username_prefers_the_user_env_var() {
        // Reading it only when set avoids racing other tests that might
        // clear `USER`; this machine's test process always has one.
        if std::env::var_os("USER").is_some() {
            assert!(current_username().is_some());
        }
    }

    #[test]
    fn snapshot_from_statusline_reads_the_rate_limits_block() {
        let payload = serde_json::json!({
            "rate_limits": {
                "five_hour": {"utilization": 0.19, "resets_at": 1788718800},
                "seven_day": {"utilization": 0.14, "resetsAt": 1789146000}
            }
        });
        let snapshot = snapshot_from_statusline(&payload).unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        let session = snapshot
            .windows
            .iter()
            .find(|w| w.window_minutes == Some(300))
            .unwrap();
        assert!((session.used_percent - 19.0).abs() < 0.01);
    }

    #[test]
    fn snapshot_from_statusline_is_none_without_a_rate_limits_block() {
        assert!(snapshot_from_statusline(&serde_json::json!({})).is_none());
        assert!(
            snapshot_from_statusline(&serde_json::json!({"rate_limits": {"overage": {}}}))
                .is_none()
        );
    }

    #[test]
    fn statusline_window_skips_scalars_with_no_utilization() {
        assert!(statusline_window("overage", &serde_json::json!(5)).is_none());
        assert!(statusline_window("spend_limit", &serde_json::json!({})).is_none());
    }

    #[test]
    fn claude_window_minutes_knows_the_two_named_windows() {
        assert_eq!(claude_window_minutes("five_hour"), Some(300));
        assert_eq!(claude_window_minutes("seven_day"), Some(10_080));
        assert_eq!(
            claude_window_minutes("seven_day_overage_included"),
            Some(10_080)
        );
        assert_eq!(claude_window_minutes("unknown_key"), None);
    }

    #[test]
    fn claude_window_label_names_the_overage_window_specially() {
        assert_eq!(
            claude_window_label("seven_day_overage_included", Some(10_080)),
            "Weekly (overage)"
        );
    }
}
