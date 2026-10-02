//! OpenCode Go's quota — the keyed usage API first, the console session as
//! the legacy fallback.
//!
//! The key pasted into ket's settings — or set on `OPENCODE_API_KEY` — is
//! spent against an endpoint that needs no console session, and the pasted
//! cookie stays only for the console-only accounts that endpoint cannot
//! see.

use super::*;

// ---------------------------------------------------------------------------
// OpenCode Go
// ---------------------------------------------------------------------------

/// The console API ket reads OpenCode Go's usage from — the JSON API
/// `opencode.ai/console` itself calls, signed in with the console's session
/// cookie.
///
/// It replaced the server-rendered dashboard this module used to scrape: by
/// 2026-09-26 that dashboard's endpoints answered a console session as signed
/// out, and the usage figures had moved behind `/go/status` here.
const OPENCODE_API: &str = "https://opencode.ai/console/api";

/// The keyed usage API: `GET /zen/go/v1/usage`, answered with each meter
/// already a percentage and needing no console session.
///
/// It takes an OpenCode Go API key rather than a session, so the quota can
/// be read without signing in to the console at all.
const OPENCODE_GO_USAGE_API: &str = "https://opencode.ai/zen/go/v1/usage";

/// The environment variable ket falls back to for a Go key when settings
/// hold none. models.dev declares it for both `opencode` and `opencode-go`,
/// so a key found on it might not be a Go key at all — which is why it never
/// outranks the one pasted into settings.
const OPENCODE_API_KEY_ENV: &str = "OPENCODE_API_KEY";

/// The most bytes of a refused answer worth reading for an error name. A
/// real error body is a few hundred; anything near this is not one.
const OPENCODE_MAX_ERROR_BYTES: usize = 4_000;

/// The header the console scopes a request to one organisation with. The
/// Go subscription belongs to an organisation, not to the signed-in user, so
/// `/go/status` without it has nothing to answer about.
const OPENCODE_ORG_HEADER: &str = "x-org-id";

/// Cookie names that actually carry the session on `opencode.ai`.
///
/// A pasted `Cookie` header carries whatever else the browser had for that
/// domain — analytics, Stripe's device ids, a consent record. Filtering to
/// these means ket sends the account's session and nothing else, so a
/// careless paste cannot forward unrelated state to a server on the user's
/// behalf.
///
/// `auth` is the Iron seal the dashboard first issued; by 2026-09-26 a
/// signed-in browser carried `__Host-console_session` (an `st_…` token)
/// instead, and no `auth` at all.
const OPENCODE_AUTH_COOKIES: [&str; 3] = ["auth", "__Host-auth", CONSOLE_SESSION_COOKIE];

/// The session cookie opencode.ai's console issues now — see
/// [`OPENCODE_AUTH_COOKIES`].
const CONSOLE_SESSION_COOKIE: &str = "__Host-console_session";

/// OpenCode Go's five-hour window, in minutes, which [`window_label`]
/// already names `Session`. It opens on first use rather than on a clock, so
/// an idle plan has no reset to report.
const OPENCODE_ROLLING_MINUTES: u32 = 300;

/// The weekly window, in minutes — a UTC calendar week.
const OPENCODE_WEEKLY_MINUTES: u32 = 10_080;

/// The monthly window, in minutes: the paid period, which resets when the
/// subscription renews.
const OPENCODE_MONTHLY_MINUTES: u32 = 43_200;

/// The largest response ket will parse. A real `/go/status` answer is a few
/// hundred bytes; anything near this is not that answer.
const OPENCODE_MAX_BODY_BYTES: usize = 1024 * 1024;

/// One HTTP GET, as much of it as this module needs to describe.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// The absolute URL to fetch.
    pub url: String,
    /// Header lines to send, `Name: value`.
    pub headers: Vec<String>,
    /// The `Cookie` header's value, sent separately so a client can put it
    /// somewhere safer than an argument vector if it ever needs to.
    pub cookie: String,
}

/// Fetches a URL and answers with its status and body together.
///
/// A seam for the same reason [`ProcessHarness`] is one: what this module
/// owns is the interpretation of a response, and that should be reachable
/// without a network or an account. The status travels beside the body
/// because the keyed usage path reads an error's name out of the *body* of a
/// refused request — a migrated account's answer is proxied to a console
/// that owns its own status codes, and the error name is the stable signal.
pub trait HttpGet: Send + Sync + std::fmt::Debug {
    /// Fetches `request`, waiting at most `timeout`.
    ///
    /// The pair is whatever the server answered, an error status included:
    /// classifying it is the caller's, because only the caller knows which
    /// request it was. An `Err` is a fetch that never completed — no curl, no
    /// network, a timeout — and never an HTTP status.
    fn get(&self, request: &HttpRequest, timeout: Duration) -> Result<(u16, String)>;
}

/// The real client: `curl`.
///
/// **Why a subprocess rather than an HTTP crate.** `ket-core` has no HTTP
/// client and no TLS stack, and this is the only thing in it that makes a
/// network request. Adding a client and a certificate store to the crate for
/// one dashboard read is a large dependency for a small feature, and every
/// other fetcher in this module is already a process spawn. `curl` ships with
/// macOS, is on every Linux ket would run on, and brings its own timeout.
///
/// The cookie goes to `curl` on stdin as a config file rather than in argv,
/// because argv is world-readable through `ps` for as long as the process
/// lives. That is the whole reason the value is not simply a `-H` argument.
#[derive(Debug, Clone, Copy, Default)]
pub struct CurlHttpGet;

impl HttpGet for CurlHttpGet {
    fn get(&self, request: &HttpRequest, timeout: Duration) -> Result<(u16, String)> {
        use std::io::Write as _;
        use std::process::Stdio;

        let fail = |why: String| KetError::Agent {
            agent: "opencode.ai".to_owned(),
            why,
        };

        // `--config -` reads options from stdin. `-w` appends the status to
        // the body after a marker, so one read gets both without needing
        // `-D` and a second stream to correlate.
        let mut child = std::process::Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--location",
                "--config",
                "-",
                "--max-time",
                &timeout.as_secs().max(1).to_string(),
                "--write-out",
                "\n%{http_code}",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| fail(format!("could not start curl: {e}")))?;

        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| fail("curl's stdin was not piped".to_owned()))?;
            let mut config = String::new();
            config.push_str(&curl_config_line("url", &request.url));
            for header in &request.headers {
                config.push_str(&curl_config_line("header", header));
            }
            if !request.cookie.is_empty() {
                config.push_str(&curl_config_line("cookie", &request.cookie));
            }
            stdin
                .write_all(config.as_bytes())
                .map_err(|e| fail(format!("could not write curl's options: {e}")))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| fail(format!("curl did not finish: {e}")))?;

        if !output.status.success() {
            return Err(fail(format!("curl exited with {}", output.status)));
        }

        let body = String::from_utf8_lossy(&output.stdout);
        let (body, status) = body
            .rsplit_once('\n')
            .ok_or_else(|| fail("curl wrote no status".to_owned()))?;
        let status: u16 = status
            .trim()
            .parse()
            .map_err(|_| fail("curl wrote no status".to_owned()))?;

        // The status and the body travel together, an error status included:
        // what it means is the caller's to decide, because only the caller
        // knows which request it was.
        Ok((status, body.to_owned()))
    }
}

/// One line of a `curl --config` file, quoted the way curl expects.
///
/// curl's config format takes a double-quoted argument with backslash
/// escapes. A cookie is a long opaque seal that may contain anything, so it
/// is escaped rather than trusted to be quote-free.
fn curl_config_line(key: &str, value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("--{key} \"{escaped}\"\n")
}

/// Turns whatever the user pasted into a `Cookie` header value.
///
/// People paste one of two things: the whole `Cookie` header copied out of a
/// request in DevTools, or just the value from the session row of the cookie
/// table — `__Host-console_session`, or `auth` on an older session. Both are reasonable readings of "paste your session cookie", and a
/// bare seal sent unwrapped would be a header that looks populated, is
/// non-empty, and authenticates nothing — a failure with no visible cause.
///
/// Anything that is neither is returned unchanged, so it fails against the
/// server with a status rather than being reshaped into something that hides
/// what was actually pasted.
pub fn normalize_cookie(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let already_a_header = trimmed.contains(';')
        || OPENCODE_AUTH_COOKIES
            .iter()
            .any(|name| trimmed.len() > name.len() && trimmed.starts_with(&format!("{name}=")));
    if already_a_header {
        return trimmed.to_owned();
    }

    // An `st_…` token is the console's session, and is named for it.
    if trimmed.starts_with("st_") {
        return format!("{CONSOLE_SESSION_COOKIE}={trimmed}");
    }

    // `Fe26.2**` is Iron's seal prefix, which is what opencode.ai first
    // issued. The looser second test catches a token format it might move to
    // without wrapping arbitrary text that was never a token.
    let looks_like_a_seal = trimmed.starts_with("Fe26.2**")
        || trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if looks_like_a_seal {
        return format!("auth={trimmed}");
    }

    trimmed.to_owned()
}

/// Keeps only the pairs that carry the session — see [`OPENCODE_AUTH_COOKIES`].
///
/// Returns an empty string when the paste contained none of them, which the
/// caller reports as a configuration problem rather than sending a header
/// that cannot possibly authenticate.
fn auth_cookies_only(header: &str) -> String {
    header
        .split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            let (name, value) = pair.split_once('=')?;
            let (name, value) = (name.trim(), value.trim());
            (OPENCODE_AUTH_COOKIES.contains(&name) && !value.is_empty())
                .then(|| format!("{name}={value}"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether `id` is shaped like an organisation id, as the console's own
/// schema defines one: `org_…`, or `wrk_…` for a workspace from before the
/// console.
///
/// Applied to the configured override before it is put in a header, so a
/// mistyped setting is refused here rather than sent.
fn is_org_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("org_").or_else(|| id.strip_prefix("wrk_")) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Where ket looks for the OpenCode Go key beyond its own settings.
///
/// A seam for the same reason [`OpenCodeCredentials`] is one: the answer
/// depends on the machine a fetch runs on, and a caller that wants "there is
/// no key" to be a settled answer needs to be able to say so without
/// depending on whichever machine that is.
pub trait OpenCodeKeyStore: Send + Sync + std::fmt::Debug {
    /// The OpenCode Go key, when one is set on the environment.
    ///
    /// `None` means "no key here", never an error: a machine without one
    /// exported is the common case, not a failure.
    fn go_key(&self) -> Option<String>;
}

/// Reads it from `OPENCODE_API_KEY`, and from nothing else. Not OpenCode's
/// `auth.json`, not its `credential` table: reading another application's
/// credential files is this repository's policy to refuse — see `AGENTS.md`
/// — and a credential ket was never given is a credential ket does not hold.
/// The variable never outranks a pasted key because it is shared with the
/// Zen provider, so a key found on it might not be a Go key at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoveredGoKey;

impl OpenCodeKeyStore for DiscoveredGoKey {
    fn go_key(&self) -> Option<String> {
        std::env::var(OPENCODE_API_KEY_ENV)
            .ok()
            .and_then(|key| trimmed_credential(&key))
    }
}

/// A credential as pasted, trimmed — the way every paste into settings is
/// normalised on the way in.
fn trimmed_credential(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// One organisation from `GET /orgs`.
#[derive(Debug, Deserialize)]
struct ConsoleOrg {
    id: String,
}

/// `GET /go/status`: `null` for an organisation with no Go subscription.
#[derive(Debug, Deserialize)]
struct GoStatus {
    /// `null` while a subscription exists but grants nothing — lapsed, or
    /// waiting on a renewal payment.
    access: Option<GoAccess>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoAccess {
    /// When the paid period ends, which is when the monthly meter resets.
    ends_at: Option<String>,
    meters: GoMeters,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoMeters {
    five_hour: GoMeter,
    week: GoMeter,
    month: Option<GoMeter>,
}

/// One meter: spend against a limit, in millionths of a cent. Plans are
/// priced in money rather than tokens, so this is what the percentage is of.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoMeter {
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(deserialize_with = "micro_cents")]
    limit_micro_cents: f64,
    #[serde(deserialize_with = "micro_cents")]
    used_micro_cents: f64,
}

/// `GET /zen/go/v1/usage`, as the keyed endpoint answers it: the meters
/// already percentages, the division already done server-side.
#[derive(Debug, Deserialize)]
struct GoUsage {
    usage: GoUsageWindows,
}

#[derive(Debug, Deserialize)]
struct GoUsageWindows {
    /// The five-hour rolling window.
    rolling: GoUsageMeter,
    /// The calendar-week window.
    weekly: GoUsageMeter,
    /// The paid period, on plans that have one.
    #[serde(default)]
    monthly: Option<GoUsageMeter>,
}

/// One keyed meter — `{ status: "ok" | "rate-limited", percent: 0-100,
/// resetsAt }` — of which only the number and the reset are ket's to use.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoUsageMeter {
    /// Percentage of this window used. A string or a `null` here fails the
    /// whole parse, which is what an answer that shape deserves.
    percent: f64,
    /// When the meter resets, RFC 3339.
    #[serde(default)]
    resets_at: Option<String>,
}

/// One refusal of the keyed usage request, as words rather than as a status.
///
/// A generic "could not parse" for what was really an entitlement answer
/// misleads exactly the person the message is for. Each refusal says what
/// the account's state is and what to do about it.
enum GoRefusal {
    /// The key is accepted and the account has no Go subscription behind it.
    NoSubscription,
    /// The key itself was not accepted — stale, replaced, or never a key.
    Unauthorized,
    /// opencode.ai answered something else, or nothing.
    Failed(String),
}

impl GoRefusal {
    /// What the usage panel should say about it.
    fn message(&self) -> String {
        match self {
            Self::NoSubscription => "this OpenCode account has no OpenCode Go subscription — \
                 subscribe at opencode.ai to see its usage"
                .to_owned(),
            Self::Unauthorized => "the OpenCode Go API key was rejected — paste a fresh one in \
                 settings, or fix OPENCODE_API_KEY"
                .to_owned(),
            Self::Failed(why) => why.clone(),
        }
    }
}

/// What a refused keyed request said, in the words the reader can act on.
///
/// The error name in the body decides before the status does: a migrated
/// account's request is proxied to the new console, which owns its own status
/// codes — the name is the stable signal.
fn classify_go_refusal(status: u16, body: &str) -> GoRefusal {
    let kind = error_type(body);
    if kind.as_deref() == Some("EntitlementError") || status == 403 {
        return GoRefusal::NoSubscription;
    }
    if kind.as_deref() == Some("AuthError") || status == 401 {
        return GoRefusal::Unauthorized;
    }
    GoRefusal::Failed(format!("opencode.ai answered {status}"))
}

/// The `error.type` of an error body, when the body is small enough to be
/// one. The names are OpenCode Console's — see its `routes/zen/go/v1/usage`.
fn error_type(body: &str) -> Option<String> {
    if body.is_empty() || body.len() > OPENCODE_MAX_ERROR_BYTES {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    parsed
        .get("error")?
        .get("type")?
        .as_str()
        .map(str::to_owned)
}

/// A micro-cent amount. The console encodes these as decimal strings —
/// they are bigints on its side — but a plain number is accepted too.
fn micro_cents<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<f64, D::Error> {
    use serde::de::Error as _;
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(text) => text.trim().parse().map_err(D::Error::custom),
        serde_json::Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| D::Error::custom("micro-cents out of range")),
        other => Err(D::Error::custom(format!(
            "expected micro-cents, got {other}"
        ))),
    }
}

/// An RFC 3339 timestamp — `2026-09-27T01:00:00.000Z`, or with an offset —
/// as milliseconds since the epoch. `None` for anything else, which leaves
/// the window without a reset rather than with a wrong one.
pub(super) fn parse_rfc3339_ms(text: &str) -> Option<u64> {
    let text = text.trim();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !matches!(text.as_bytes().get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }

    // Fractional seconds, then the zone.
    let mut rest = &text[19..];
    let mut millis = 0i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        let padded = format!("{:0<3}", &fraction[..digits.min(3)]);
        millis = padded.parse().ok()?;
        rest = &fraction[digits..];
    }
    let offset_secs = match rest {
        "Z" | "z" => 0,
        zone if zone.len() == 6 && matches!(zone.as_bytes()[0], b'+' | b'-') => {
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            sign * (hours * 3600 + minutes * 60)
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    u64::try_from(secs * 1000 + millis).ok()
}

/// Where the OpenCode Go fetcher reads its credentials.
///
/// A seam rather than a field, because the cookie can be changed in settings
/// while the window is open: reading it per fetch means Refresh picks up a
/// freshly pasted cookie, where a value captured when the cache was built
/// would need the app restarted.
pub trait OpenCodeCredentials: Send + Sync + std::fmt::Debug {
    /// The current settings.
    fn read(&self) -> OpenCodeGoConfig;
}

/// Reads them from ket's own config file.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConfiguredOpenCodeCredentials;

impl OpenCodeCredentials for ConfiguredOpenCodeCredentials {
    fn read(&self) -> OpenCodeGoConfig {
        // A config that will not load is a config with no cookie in it, which
        // is the unconfigured case — the settings screen is where a malformed
        // file gets reported, not a status bar.
        Config::load().unwrap_or_default().opencode_go
    }
}

/// OpenCode's quota, which belongs to OpenCode Go rather than to OpenCode.
///
/// **Read the plain OpenCode case first.** OpenCode the CLI is
/// bring-your-own-key: it has no plan of its own, and the limit that applies
/// to a session belongs to whichever backend provider the key is for.
/// `opencode stats` reports local history — tokens and cost across past runs,
/// the `UsageUpdate` kind of data the module docs warn against conflating
/// with quota — and `opencode providers list` reports which credentials are
/// configured, not how much of anything is left. Nothing in the CLI, its
/// local server, or its config exposes a plan quota.
///
/// **OpenCode Go is a different product.** It is OpenCode's own hosted
/// subscription, it does have a plan, and it publishes three concurrent
/// windows — a five-hour rolling one, a weekly one, and on some plans a
/// monthly one for the paid period. The key pasted into ket's settings —
/// or set on `OPENCODE_API_KEY`, which never outranks the pasted one — is
/// spent against `GET /zen/go/v1/usage`, an endpoint that needs no console
/// session. ket does not read the key out of OpenCode's own credential
/// stores (`auth.json`, its `credential` table): reading another
/// application's credential files is this repository's policy to refuse,
/// and a credential ket was never given is a credential ket does not hold.
/// The pasted cookie stays as the fallback for a console-only account whose
/// usage the keyed endpoint cannot see, and with nothing configured this
/// still reports [`SnapshotStatus::Unsupported`], exactly as before.
///
/// It remains the most fragile fetcher in this module by some distance, and
/// it is built so that its fragility surfaces as a reason a person can act
/// on — "the API key was rejected", "this account has no OpenCode Go
/// subscription", "opencode.ai answered 500" — rather than as a percentage
/// nobody should trust.
#[derive(Debug)]
pub struct OpenCodeRateLimitSource {
    http: Arc<dyn HttpGet>,
    credentials: Arc<dyn OpenCodeCredentials>,
    store: Arc<dyn OpenCodeKeyStore>,
}

impl Default for OpenCodeRateLimitSource {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeRateLimitSource {
    /// The real source: `curl` against `opencode.ai`, credentials and key
    /// from config, `OPENCODE_API_KEY` for the key when config holds none.
    pub fn new() -> Self {
        Self {
            http: Arc::new(CurlHttpGet),
            credentials: Arc::new(ConfiguredOpenCodeCredentials),
            store: Arc::new(DiscoveredGoKey),
        }
    }

    /// A source over an explicit client, credentials and key store.
    pub fn with(
        http: Arc<dyn HttpGet>,
        credentials: Arc<dyn OpenCodeCredentials>,
        store: Arc<dyn OpenCodeKeyStore>,
    ) -> Self {
        Self {
            http,
            credentials,
            store,
        }
    }

    /// One `GET` against the console API, as JSON.
    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        org: Option<&str>,
        cookie: &str,
        timeout: Duration,
    ) -> std::result::Result<T, String> {
        let mut headers = vec![
            "Accept: application/json".to_owned(),
            "Origin: https://opencode.ai".to_owned(),
            "Referer: https://opencode.ai/console/".to_owned(),
        ];
        if let Some(org) = org {
            headers.push(format!("{OPENCODE_ORG_HEADER}: {org}"));
        }
        let request = HttpRequest {
            url: format!("{OPENCODE_API}{path}"),
            headers,
            cookie: cookie.to_owned(),
        };

        let (status, body) = self
            .http
            .get(&request, timeout)
            .map_err(|e| e.to_string())?;
        if body.len() > OPENCODE_MAX_BODY_BYTES {
            return Err(format!(
                "opencode.ai answered {path} with {} bytes",
                body.len()
            ));
        }
        match status {
            200 => {}
            // A genuine refusal of the console session, not a page: the
            // cookie is what the console declined.
            401 | 403 => {
                return Err(
                    "opencode.ai rejected the session cookie — sign in again and re-paste it"
                        .to_owned(),
                );
            }
            other => return Err(format!("opencode.ai answered {other}")),
        }
        // A sign-in page where JSON was asked for: the session is gone.
        if body.trim_start().starts_with('<') {
            return Err(
                "opencode.ai does not recognise the session cookie — sign in again and paste \
                 a fresh one"
                    .to_owned(),
            );
        }
        serde_json::from_str(&body)
            .map_err(|e| format!("opencode.ai's {path} answer was not understood ({e})"))
    }

    /// One keyed `GET /zen/go/v1/usage`, its refusals classified by name
    /// before status.
    fn fetch_keyed(
        &self,
        key: &str,
        timeout: Duration,
    ) -> std::result::Result<ProviderSnapshot, GoRefusal> {
        let request = HttpRequest {
            url: OPENCODE_GO_USAGE_API.to_owned(),
            headers: vec![
                "Accept: application/json".to_owned(),
                // On the request, never in a message: curl takes it through
                // its stdin config, where no other process can read it.
                format!("Authorization: Bearer {key}"),
            ],
            cookie: String::new(),
        };
        let (status, body) = self
            .http
            .get(&request, timeout)
            .map_err(|e| GoRefusal::Failed(e.to_string()))?;

        if status != 200 {
            return Err(classify_go_refusal(status, &body));
        }
        if body.len() > OPENCODE_MAX_BODY_BYTES {
            return Err(GoRefusal::Failed(format!(
                "opencode.ai answered the usage request with {} bytes",
                body.len()
            )));
        }
        // A key bounced to the console's sign-in page arrives as a 200 page —
        // curl follows redirects — so the page is read as the refusal it is.
        if body.trim_start().starts_with('<') {
            return Err(GoRefusal::Unauthorized);
        }
        let usage: GoUsage = serde_json::from_str(&body).map_err(|_| {
            GoRefusal::Failed("opencode.ai's usage answer was not understood".to_owned())
        })?;
        Ok(snapshot_from_usage(&usage))
    }

    /// The whole fetch, with the reason for any refusal.
    fn fetch_inner(&self, timeout: Duration) -> std::result::Result<ProviderSnapshot, String> {
        let settings = self.credentials.read();
        let pasted = normalize_cookie(&settings.session_cookie);
        let cookie = auth_cookies_only(&pasted);

        // The key: the one pasted into settings, else `OPENCODE_API_KEY`.
        let key = trimmed_credential(&settings.api_key).or_else(|| self.store.go_key());

        // The whole fetch — the keyed request, and the cookie's two requests
        // behind it — shares one deadline. Sequential requests each given the
        // full timeout would take three times as long as the caller was
        // promised, which is invariant 2 lost to arithmetic.
        let deadline = Instant::now() + timeout;
        let remaining = || deadline.saturating_duration_since(Instant::now());

        match key {
            Some(key) => match self.fetch_keyed(&key, remaining()) {
                Ok(snapshot) => Ok(snapshot),
                // A key that is accepted but grants nothing is the
                // console-only account's — OpenCode Black, whose usage still
                // exists only behind the console session — so the cookie
                // path gets its turn before the keyed refusal is reported.
                Err(refusal) if !cookie.is_empty() => {
                    match self.fetch_with_cookie(&settings, &cookie, remaining()) {
                        Ok(snapshot) => Ok(snapshot),
                        Err(_) => Err(refusal.message()),
                    }
                }
                Err(refusal) => Err(refusal.message()),
            },
            // No key anywhere and nothing pasted: not an error — a BYO-key
            // OpenCode genuinely has no plan quota, and this is the state
            // that says so. Nothing is fetched to find that out.
            None if settings.session_cookie.trim().is_empty() => {
                Ok(ProviderSnapshot::unsupported(Provider::OpenCode))
            }
            // Something pasted that carried no session: report the paste,
            // not a guess at what was meant by it.
            None if cookie.is_empty() => Err(
                "no session cookie in what was pasted — copy the whole Cookie header from an \
                 opencode.ai request, or just the `__Host-console_session` value"
                    .to_owned(),
            ),
            None => self.fetch_with_cookie(&settings, &cookie, timeout),
        }
    }

    /// The console-cookie path: the console session lists the organisations
    /// the account can see, and the one holding the Go subscription answers
    /// `/go/status`.
    ///
    /// Kept whole rather than retired because a console-only account —
    /// OpenCode Black — still has its usage only here, and the keyed
    /// endpoint's refusals fall through to it.
    fn fetch_with_cookie(
        &self,
        settings: &OpenCodeGoConfig,
        cookie: &str,
        timeout: Duration,
    ) -> std::result::Result<ProviderSnapshot, String> {
        let deadline = Instant::now() + timeout;
        let remaining = || deadline.saturating_duration_since(Instant::now());

        let override_id = settings.workspace_id.trim();
        let orgs = if override_id.is_empty() {
            self.get_json::<Vec<ConsoleOrg>>("/orgs", None, cookie, remaining())?
                .into_iter()
                .map(|org| org.id)
                .filter(|id| is_org_id(id))
                .collect::<Vec<_>>()
        } else if is_org_id(override_id) {
            vec![override_id.to_owned()]
        } else {
            return Err(format!(
                "`{override_id}` is not an organisation id — it should look like `org_…`"
            ));
        };

        if orgs.is_empty() {
            return Err("no organisation found for this account — set one in settings".to_owned());
        }

        // Several organisations means several candidates, and only one of
        // them holds the subscription. Trying each in turn is why a wrong
        // guess is recoverable without the user having to find the id.
        let mut last = String::from("no organisation on this account has OpenCode Go");
        for org in &orgs {
            if remaining().is_zero() {
                break;
            }
            match self.get_json::<Option<GoStatus>>("/go/status", Some(org), cookie, remaining()) {
                Ok(Some(GoStatus {
                    access: Some(access),
                })) => return Ok(snapshot_from(access, org)),
                Ok(Some(GoStatus { access: None })) => {
                    last = "the OpenCode Go subscription grants no access right now — lapsed, \
                            or waiting on a renewal payment"
                        .to_owned();
                }
                Ok(None) => {}
                Err(e) => last = e,
            }
        }

        Err(last)
    }
}

/// Turns the console path's `/go/status` answer into a snapshot.
///
/// The console reports spend against a limit; the percentage is computed
/// here, once. A meter with no limit is reported at 0% rather than dividing
/// by zero, and never above 100%.
fn snapshot_from(access: GoAccess, org: &str) -> ProviderSnapshot {
    let window = |meter: &GoMeter, resets_at: Option<&str>, minutes: u32| RateWindow {
        name: window_label(Some(minutes), "Window"),
        used_percent: if meter.limit_micro_cents > 0.0 {
            (meter.used_micro_cents / meter.limit_micro_cents * 100.0).clamp(0.0, 100.0) as f32
        } else {
            0.0
        },
        resets_at_ms: resets_at.and_then(parse_rfc3339_ms),
        window_minutes: Some(minutes),
    };

    let meters = &access.meters;
    let mut windows = vec![
        window(
            &meters.five_hour,
            meters.five_hour.resets_at.as_deref(),
            OPENCODE_ROLLING_MINUTES,
        ),
        window(
            &meters.week,
            meters.week.resets_at.as_deref(),
            OPENCODE_WEEKLY_MINUTES,
        ),
    ];
    if let Some(month) = &meters.month {
        // The month has no reset of its own: it is the paid period.
        windows.push(window(
            month,
            month.resets_at.as_deref().or(access.ends_at.as_deref()),
            OPENCODE_MONTHLY_MINUTES,
        ));
    }

    ProviderSnapshot {
        provider: Provider::OpenCode,
        status: SnapshotStatus::Fresh,
        plan: Some("Go".to_owned()),
        account: Some(org.to_owned()),
        windows,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    }
}

/// Turns the keyed path's usage answer into a snapshot.
///
/// The percentages arrive already computed, so this is only the framing: the
/// same three windows the console path reports, so a card cannot tell which
/// path the numbers came from.
fn snapshot_from_usage(usage: &GoUsage) -> ProviderSnapshot {
    let window = |meter: &GoUsageMeter, minutes: u32| RateWindow {
        name: window_label(Some(minutes), "Window"),
        used_percent: meter.percent.clamp(0.0, 100.0) as f32,
        resets_at_ms: meter.resets_at.as_deref().and_then(parse_rfc3339_ms),
        window_minutes: Some(minutes),
    };

    let meters = &usage.usage;
    let mut windows = vec![
        window(&meters.rolling, OPENCODE_ROLLING_MINUTES),
        window(&meters.weekly, OPENCODE_WEEKLY_MINUTES),
    ];
    if let Some(monthly) = &meters.monthly {
        windows.push(window(monthly, OPENCODE_MONTHLY_MINUTES));
    }

    ProviderSnapshot {
        provider: Provider::OpenCode,
        status: SnapshotStatus::Fresh,
        plan: Some("Go".to_owned()),
        // The keyed answer names no account; `None` is the honest account.
        account: None,
        windows,
        fetched_at_ms: Some(now_ms()),
        spend: Vec::new(),
        limited_at_ms: None,
    }
}

impl RateLimitSource for OpenCodeRateLimitSource {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn fetch(&self, timeout: Duration) -> ProviderSnapshot {
        match self.fetch_inner(timeout) {
            Ok(snapshot) => snapshot,
            Err(why) => ProviderSnapshot::unavailable(Provider::OpenCode, why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- pure helpers ---------------------------------------------------------

    #[test]
    fn auth_cookies_only_keeps_only_the_recognised_names_with_a_value() {
        let header =
            format!("auth=abc; other=1; {CONSOLE_SESSION_COOKIE}=xyz; __Host-auth=; junk=z");
        assert_eq!(
            auth_cookies_only(&header),
            format!("auth=abc; {CONSOLE_SESSION_COOKIE}=xyz")
        );
    }

    #[test]
    fn auth_cookies_only_of_an_empty_header_is_empty() {
        assert_eq!(auth_cookies_only(""), "");
    }

    #[test]
    fn trimmed_credential_drops_surrounding_whitespace_and_blank_input() {
        assert_eq!(trimmed_credential("  abc123  "), Some("abc123".to_owned()));
        assert_eq!(trimmed_credential(""), None);
        assert_eq!(trimmed_credential("   "), None);
    }

    #[test]
    fn error_type_reads_the_nested_field_and_refuses_an_oversized_body() {
        assert_eq!(
            error_type(r#"{"error":{"type":"AuthError"}}"#),
            Some("AuthError".to_owned())
        );
        assert_eq!(error_type(""), None);
        assert_eq!(error_type("not json"), None);
        assert_eq!(error_type(r#"{"error":{}}"#), None);
        let oversized = "x".repeat(OPENCODE_MAX_ERROR_BYTES + 1);
        assert_eq!(error_type(&oversized), None);
    }

    #[test]
    fn classify_go_refusal_prefers_the_error_type_over_the_status() {
        assert!(matches!(
            classify_go_refusal(200, r#"{"error":{"type":"EntitlementError"}}"#),
            GoRefusal::NoSubscription
        ));
        assert!(matches!(
            classify_go_refusal(200, r#"{"error":{"type":"AuthError"}}"#),
            GoRefusal::Unauthorized
        ));
        assert!(matches!(
            classify_go_refusal(418, "no body"),
            GoRefusal::Failed(why) if why.contains("418")
        ));
    }

    // -- OpenCode -----------------------------------------------------------

    /// Settings with nothing pasted into them.
    #[derive(Debug, Default)]
    struct NoCredentials;

    impl OpenCodeCredentials for NoCredentials {
        fn read(&self) -> OpenCodeGoConfig {
            OpenCodeGoConfig::default()
        }
    }

    /// A key store with nothing in it, so the test says "unconfigured" for
    /// the machine it runs on rather than whichever machine that is.
    #[derive(Debug)]
    struct NoKey;

    impl OpenCodeKeyStore for NoKey {
        fn go_key(&self) -> Option<String> {
            None
        }
    }

    /// An HTTP client that fails the test if it is used at all.
    #[derive(Debug)]
    struct NeverFetches;

    impl HttpGet for NeverFetches {
        fn get(&self, request: &HttpRequest, _timeout: Duration) -> Result<(u16, String)> {
            panic!(
                "neither a key nor a cookie is configured, so nothing should be fetched: {request:?}"
            );
        }
    }

    #[test]
    fn an_unconfigured_opencode_reports_unsupported_without_a_request() {
        // BYO-key OpenCode has no plan quota, and saying so is not an error.
        // The client panics rather than returning, so this also pins the more
        // important half: an empty cookie must not become a request that
        // reaches opencode.ai and comes back as a sign-in page.
        let source = OpenCodeRateLimitSource::with(
            Arc::new(NeverFetches),
            Arc::new(NoCredentials),
            Arc::new(NoKey),
        );
        let snapshot = source.fetch(Duration::from_secs(1));

        assert_eq!(snapshot.status, SnapshotStatus::Unsupported);
        assert!(snapshot.windows.is_empty());
    }
}
