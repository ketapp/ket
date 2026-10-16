//! The feedback receiver: where the reports people send from inside ket go.
//!
//! ket's relay runs on each person's own Mac, so nothing on the desktop side
//! can receive reports centrally. This is the one piece that runs elsewhere:
//! a plain binary that takes a report over HTTP and emails it to one
//! address. Delivery is SMTP, the standard every mail host speaks, so it runs
//! on any machine — a container, a VPS, a spare box — and depends on no
//! provider.
//!
//! # Endpoint
//!
//! `POST /v1/feedback` takes one report as JSON, at most 32 KiB:
//!
//! ```json
//! {
//!   "kind": "bug",
//!   "message": "The sidebar forgets its width after a restart.",
//!   "reply_to": "someone@example.com",
//!   "app_version": "0.1.0",
//!   "os": "macOS 27.0",
//!   "install": "0123456789abcdef0123456789abcdef"
//! }
//! ```
//!
//! `kind` is `feedback`, `bug` or `idea`; `reply_to` and `os` may be `null`;
//! `install` is an anonymous random id per install, 32 lowercase hex
//! characters. Unknown fields are ignored. It answers `202 {"ok": true}` once
//! the report is sent; `400`, `413` and `429` with `{"error": "..."}` — a
//! sentence the app can show a person as it is — and `429` also with
//! `retry_after_secs` and a `Retry-After` header; and `502` when the mail
//! server could not take it. `GET /healthz` answers `200 ok` for container
//! health checks.
//!
//! # Limits
//!
//! One accepted report per install per 7 days, and at most 10 per client
//! address per 24 hours; a report counts once it has been sent, not when it
//! arrives. Both are kept in memory and reset when the service restarts —
//! acceptable because the app enforces the 7 days itself, and these are for
//! whatever does not go through the app.
//!
//! # Configuration
//!
//! All of it is environment variables, so a container needs no file:
//!
//! - `KET_FEEDBACK_LISTEN` — the address to listen on; default
//!   `127.0.0.1:7980`. The binary's first argument, when there is one, wins.
//! - `KET_FEEDBACK_TO` — the address reports go to. Unset, the service runs
//!   in *log mode*: each report is printed as the email it would have been,
//!   and nothing is sent, which is how to try it before there is a mailbox.
//! - `KET_FEEDBACK_FROM` — the From address, `feedback@example.com` or
//!   `ket <feedback@example.com>`. Required with `KET_FEEDBACK_TO`.
//! - `KET_FEEDBACK_SMTP` — the mail server, as a URL:
//!   `smtps://user:pass@smtp.example.com:465` for TLS from the first byte,
//!   `smtp://user:pass@smtp.example.com:587?tls=required` for STARTTLS.
//!   Credentials in it have to be percent-encoded; a path,
//!   `smtps://…/mail.example.com`, sets the name sent in EHLO, which is
//!   otherwise `[127.0.0.1]`. Required with `KET_FEEDBACK_TO`.
//! - `KET_FEEDBACK_TRUST_PROXY` — `1` when the service sits behind a reverse
//!   proxy, so the client's address is the first hop of `X-Forwarded-For`
//!   rather than the proxy's. Only set it when that proxy overwrites the
//!   header: otherwise anyone can write their own and walk around the
//!   per-address limit.
//!
//! The mail server's certificate is checked against the operating system's
//! root certificates. On Linux that is the CA bundle — Debian's
//! `ca-certificates`, or whatever `SSL_CERT_FILE` names — so an image built
//! `FROM scratch` needs one copied in.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message};
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// Where the service listens when neither the command line nor
/// `KET_FEEDBACK_LISTEN` says otherwise. Loopback, so a first run is not
/// open to the network before anyone has decided it should be.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:7980";

/// The largest body accepted. A report is a few paragraphs; anything near
/// this is not one.
const MAX_BODY: usize = 32 * 1024;

/// How long a client has to send its headers, and then its body. Anything
/// slower is someone holding a socket open, not the app.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the mail server has to take a report before the person is told
/// it could not be sent.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Connections open at once. Past this, new ones are closed on accept, so a
/// flood of slow clients runs out of their sockets before the process runs
/// out of file descriptors.
const MAX_CONNECTIONS: usize = 512;

/// Limits on the report itself, in characters.
const MAX_MESSAGE: usize = 10_000;
const MAX_REPLY_TO: usize = 254;
const MAX_APP_VERSION: usize = 64;
const MAX_OS: usize = 128;

/// How much of the message's first line the subject carries.
const SUBJECT_CHARS: usize = 60;

/// One accepted report per install per this long. The app holds itself to
/// the same week; this is the check that does not trust it to.
const INSTALL_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Accepted reports per client address per window. Generous enough for an
/// office behind one address, and still a ceiling on what anyone can push
/// into the mailbox in a day by minting install ids.
const ADDRESS_REPORTS: u32 = 10;
const ADDRESS_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Entries per limit table. Distinct installs and addresses cost memory, so
/// the tables are bounded and cannot become a sink themselves; when one is
/// full of live entries, new reports are refused until some expire.
const MAX_ENTRIES: usize = 100_000;

/// What a refusal for a full table asks the client to wait.
const FULL_RETRY: Duration = Duration::from_secs(60 * 60);

/// The one sentence a person sees when the mail server fails. What actually
/// went wrong is logged, never returned: it can name the server, the account
/// or the address reports go to.
const SEND_FAILED: &str = "The message could not be sent. Try again later.";

/// Why the service could not be configured, in a sentence for whoever is
/// running it.
#[derive(Debug)]
pub struct ConfigError(String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// How the service runs, read from the environment by [`Config::from_env`].
pub struct Config {
    /// What happens to an accepted report.
    pub delivery: Delivery,
    /// Whether the client's address is taken from `X-Forwarded-For`'s first
    /// hop rather than the socket — see `KET_FEEDBACK_TRUST_PROXY` in the
    /// crate documentation.
    pub trust_proxy: bool,
}

/// What happens to an accepted report.
pub enum Delivery {
    /// Printed to the log as the email it would have been, and not sent:
    /// how the service runs before there is an address to send to.
    Log,
    /// Emailed.
    Smtp(Box<Mailer>),
}

/// The mail side: who reports go to, who they come from, and the server
/// that carries them.
pub struct Mailer {
    to: Mailbox,
    from: Mailbox,
    transport: AsyncSmtpTransport<Tokio1Executor>,
    /// The server URL without its credentials, for the startup line.
    server: String,
}

impl Mailer {
    /// A mailer for `to`, from `from`, through the SMTP server at `url` —
    /// see `KET_FEEDBACK_SMTP` in the crate documentation for its form.
    ///
    /// Nothing is dialled yet: a malformed URL or address fails here, a
    /// wrong password on the first send.
    pub fn new(to: &str, from: &str, url: &str) -> Result<Self, ConfigError> {
        let to = to.parse::<Mailbox>().map_err(|_| {
            ConfigError(format!(
                "KET_FEEDBACK_TO is not an email address like feedback@example.com: {to:?}"
            ))
        })?;
        let from = from.parse::<Mailbox>().map_err(|_| {
            ConfigError(format!(
                "KET_FEEDBACK_FROM is not an email address like feedback@example.com: {from:?}"
            ))
        })?;
        // The URL carries the password, so neither it nor lettre's message
        // about it — which can quote it — goes into the error.
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|_| {
                ConfigError(
                    "KET_FEEDBACK_SMTP is not an SMTP URL like \
                     smtps://user:pass@smtp.example.com:465 or \
                     smtp://user:pass@smtp.example.com:587?tls=required"
                        .to_owned(),
                )
            })?
            .timeout(Some(SEND_TIMEOUT))
            .build();
        Ok(Self {
            to,
            from,
            transport,
            server: without_credentials(url),
        })
    }

    /// Sends one email, or says why it did not go — for the log only.
    async fn send(&self, email: &Email) -> Result<(), String> {
        let mut builder = Message::builder()
            .from(self.from.clone())
            .to(self.to.clone())
            .subject(email.subject.as_str())
            .message_id(Some(message_id(self.from.email.domain())))
            .header(ContentType::TEXT_PLAIN);
        if let Some(address) = &email.reply_to {
            builder = builder.reply_to(Mailbox::new(None, address.clone()));
        }
        // Every part was checked or built here, so a refusal from the
        // builder is a bug in this file rather than in the report; it is
        // still a failed send, not a panic.
        let message = builder
            .body(email.body.clone())
            .map_err(|error| format!("could not build the email: {error}"))?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

impl Config {
    /// Reads the configuration from the environment — see the crate
    /// documentation for each variable.
    ///
    /// With `KET_FEEDBACK_TO` unset, the service runs in log mode and the
    /// mail variables are not read. With it set, `KET_FEEDBACK_FROM` and
    /// `KET_FEEDBACK_SMTP` are required, and all three have to parse:
    /// starting with half a mail setup would accept reports and lose them.
    pub fn from_env() -> Result<Self, ConfigError> {
        let trust_proxy = match var("KET_FEEDBACK_TRUST_PROXY").as_deref() {
            None | Some("0") => false,
            Some("1") => true,
            Some(other) => {
                return Err(ConfigError(format!(
                    "KET_FEEDBACK_TRUST_PROXY has to be 1 or 0, not {other:?}"
                )));
            }
        };
        let delivery = match var("KET_FEEDBACK_TO") {
            None => Delivery::Log,
            Some(to) => {
                let from = var("KET_FEEDBACK_FROM").ok_or_else(|| {
                    ConfigError("KET_FEEDBACK_TO is set, so KET_FEEDBACK_FROM is needed too: the address reports are sent from".to_owned())
                })?;
                let url = var("KET_FEEDBACK_SMTP").ok_or_else(|| {
                    ConfigError("KET_FEEDBACK_TO is set, so KET_FEEDBACK_SMTP is needed too: the mail server, like smtps://user:pass@smtp.example.com:465".to_owned())
                })?;
                Delivery::Smtp(Box::new(Mailer::new(&to, &from, &url)?))
            }
        };
        Ok(Self {
            delivery,
            trust_proxy,
        })
    }

    /// One line on what the service will do with a report, for the log at
    /// startup. Never includes the SMTP credentials.
    pub fn summary(&self) -> String {
        let delivery = match &self.delivery {
            Delivery::Log => {
                "log mode (KET_FEEDBACK_TO is unset): reports are printed, not sent".to_owned()
            }
            Delivery::Smtp(mailer) => format!(
                "emailing reports to {} from {} through {}",
                mailer.to, mailer.from, mailer.server
            ),
        };
        let proxy = if self.trust_proxy {
            "client address from X-Forwarded-For"
        } else {
            "client address from the socket"
        };
        format!("{delivery}; {proxy}")
    }
}

/// An environment variable, with unset and blank treated alike.
fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// An SMTP URL with the user and password taken out: the scheme, the host
/// and port, and the `tls` setting if there is one.
fn without_credentials(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("smtp", url));
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let tls = rest[authority_end..]
        .split_once('?')
        .and_then(|(_, query)| {
            query
                .split(['&', '#'])
                .find_map(|pair| pair.strip_prefix("tls="))
        })
        .map(|tls| format!(" (tls={tls})"))
        .unwrap_or_default();
    format!("{scheme}://{host}{tls}")
}

/// A Message-ID under the From domain. Mail servers usually add one, but a
/// message that arrives without one scores as spam with some filters.
fn message_id(domain: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("<ket-feedback.{nanos}.{sequence}@{domain}>")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A report as it arrives, before anything about it is trusted. Every field
/// is optional here so that a missing one gets a sentence of its own rather
/// than a parser's message.
#[derive(Deserialize)]
struct Submission {
    kind: Option<String>,
    message: Option<String>,
    reply_to: Option<String>,
    app_version: Option<String>,
    os: Option<String>,
    install: Option<String>,
}

#[derive(Clone, Copy)]
enum Kind {
    Feedback,
    Bug,
    Idea,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Feedback => "Feedback",
            Self::Bug => "Bug",
            Self::Idea => "Idea",
        }
    }
}

/// A report that has been checked field by field.
struct Report {
    kind: Kind,
    message: String,
    reply_to: Option<Address>,
    app_version: String,
    os: Option<String>,
    install: String,
}

impl Report {
    /// Checks a submission against the contract, or says — in a sentence
    /// the app can show as it is — what is wrong with it.
    fn check(submission: Submission) -> Result<Self, &'static str> {
        let kind = match submission.kind.as_deref() {
            Some("feedback") => Kind::Feedback,
            Some("bug") => Kind::Bug,
            Some("idea") => Kind::Idea,
            _ => return Err("The report's kind has to be feedback, bug or idea."),
        };

        let message = submission.message.unwrap_or_default();
        let message = message.trim();
        if message.is_empty() {
            return Err("The message is empty.");
        }
        if message.chars().count() > MAX_MESSAGE {
            return Err("The message is longer than 10,000 characters.");
        }

        let reply_to = match submission.reply_to.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(address) if address.chars().count() > MAX_REPLY_TO => {
                return Err("The reply address is longer than 254 characters.");
            }
            Some(address) => Some(
                reply_address(address)
                    .ok_or("The reply address does not look like an email address.")?,
            ),
        };

        let Some(app_version) = submission.app_version else {
            return Err("The report does not say which version of ket sent it.");
        };
        let app_version = app_version.trim();
        if app_version.chars().count() > MAX_APP_VERSION {
            return Err("The app version is longer than 64 characters.");
        }

        let os = submission
            .os
            .as_deref()
            .map(str::trim)
            .filter(|os| !os.is_empty());
        if os.is_some_and(|os| os.chars().count() > MAX_OS) {
            return Err("The operating system is longer than 128 characters.");
        }

        let install = submission.install.unwrap_or_default();
        let is_hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
        if install.len() != 32 || !install.bytes().all(is_hex) {
            return Err("The install id has to be 32 lowercase hexadecimal characters.");
        }

        Ok(Self {
            kind,
            message: clean_text(message),
            reply_to,
            app_version: clean_line(app_version),
            os: os.map(clean_line),
            install,
        })
    }

    /// The install id as a number, which is what the limit table keys on:
    /// half the memory of the text, for the same thing.
    fn install_key(&self) -> u128 {
        u128::from_str_radix(&self.install, 16).unwrap_or_default()
    }

    /// The email this report becomes.
    fn email(&self, received: SystemTime) -> Email {
        let first_line = self.message.lines().next().unwrap_or_default().trim();
        let summary = if first_line.chars().count() > SUBJECT_CHARS {
            let cut: String = first_line.chars().take(SUBJECT_CHARS).collect();
            format!("{}…", cut.trim_end())
        } else {
            first_line.to_owned()
        };
        let subject = format!("[ket] {}: {summary}", self.kind.label());

        let app_version = if self.app_version.is_empty() {
            "not given"
        } else {
            &self.app_version
        };
        // "-- " is the signature separator, so mail clients set the footer
        // apart and leave it out of a quoted reply to the person.
        let body = format!(
            "{message}\n\n-- \nKind: {kind}\nApp version: {app_version}\nOS: {os}\nInstall: {install}\nReceived: {received}\n",
            message = self.message,
            kind = self.kind.label(),
            os = self.os.as_deref().unwrap_or("not shared"),
            install = self.install,
            received = rfc3339(received),
        );
        Email {
            subject,
            reply_to: self.reply_to.clone(),
            body,
        }
    }
}

/// A reply address, if `text` is one: a single `@` with something either
/// side, no whitespace or control characters — CR and LF above all, which
/// would end the header it goes into — and an address lettre accepts.
fn reply_address(text: &str) -> Option<Address> {
    let (local, domain) = text.split_once('@')?;
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || text.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    text.parse().ok()
}

/// Message text with line endings made `\n` and any other control
/// character — a terminal escape, say, which log mode would print as it is
/// — replaced by U+FFFD. Tabs stay.
fn clean_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| match c {
            '\r' => '\n',
            '\n' | '\t' => c,
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .collect()
}

/// A one-line field with every control character replaced by U+FFFD.
fn clean_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// `time` as RFC 3339 in UTC, to the second: `2026-10-01T09:30:00Z`.
///
/// By hand rather than through a date crate: it is one calendar conversion,
/// Howard Hinnant's `civil_from_days`, for one line in a footer.
fn rfc3339(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let (hour, minute, second) = (of_day / 3600, of_day % 3600 / 60, of_day % 60);

    // Days since 0000-03-01, counted in 400-year eras, so that the leap day
    // falls at the end of each year of the cycle.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// One report as an email, before it has a sender or a recipient.
struct Email {
    subject: String,
    reply_to: Option<Address>,
    body: String,
}

impl Email {
    /// The email as log mode prints it: the headers that come from the
    /// report, then the body.
    fn preview(&self) -> String {
        let reply_to = self
            .reply_to
            .as_ref()
            .map(|address| format!("Reply-To: {address}\n"))
            .unwrap_or_default();
        format!("Subject: {}\n{reply_to}\n{}", self.subject, self.body)
    }
}

/// The rate limits. In memory, so they reset whenever the service restarts;
/// the app holds each install to one report a week itself, and these exist
/// for whatever does not go through the app.
#[derive(Default)]
struct Limits {
    /// When each install last had a report accepted.
    installs: HashMap<u128, Instant>,
    /// Reports accepted per client address in its current window.
    addresses: HashMap<IpAddr, Window>,
}

/// A fixed window of [`ADDRESS_WINDOW`], opened by an address's first
/// accepted report.
struct Window {
    opened: Instant,
    count: u32,
}

/// A report's place against the limits, taken before it is sent so that two
/// sent at once cannot both get through, and handed back if sending fails:
/// only a report that went out counts.
struct Reservation {
    install: u128,
    address: IpAddr,
    at: Instant,
    window: Instant,
}

/// Why a report was refused by the limits, and for how long.
enum Refusal {
    Install(Duration),
    Address(Duration),
    Full,
}

impl Limits {
    fn reserve(
        &mut self,
        install: u128,
        address: IpAddr,
        now: Instant,
    ) -> Result<Reservation, Refusal> {
        if let Some(&last) = self.installs.get(&install) {
            let since = now.duration_since(last);
            if since < INSTALL_INTERVAL {
                return Err(Refusal::Install(INSTALL_INTERVAL - since));
            }
        }
        if let Some(window) = self.addresses.get(&address) {
            let since = now.duration_since(window.opened);
            if since < ADDRESS_WINDOW && window.count >= ADDRESS_REPORTS {
                return Err(Refusal::Address(ADDRESS_WINDOW - since));
            }
        }

        // Room for a new entry: expired ones go first, and if the table is
        // still full of live ones the report waits rather than the table
        // growing.
        if !self.installs.contains_key(&install) && self.installs.len() >= MAX_ENTRIES {
            self.installs
                .retain(|_, last| now.duration_since(*last) < INSTALL_INTERVAL);
            if self.installs.len() >= MAX_ENTRIES {
                return Err(Refusal::Full);
            }
        }
        if !self.addresses.contains_key(&address) && self.addresses.len() >= MAX_ENTRIES {
            self.addresses
                .retain(|_, window| now.duration_since(window.opened) < ADDRESS_WINDOW);
            if self.addresses.len() >= MAX_ENTRIES {
                return Err(Refusal::Full);
            }
        }

        self.installs.insert(install, now);
        let window = self.addresses.entry(address).or_insert(Window {
            opened: now,
            count: 0,
        });
        if now.duration_since(window.opened) >= ADDRESS_WINDOW {
            *window = Window {
                opened: now,
                count: 0,
            };
        }
        window.count += 1;
        Ok(Reservation {
            install,
            address,
            at: now,
            window: window.opened,
        })
    }

    /// Hands back a reservation whose report was not sent.
    fn release(&mut self, reservation: &Reservation) {
        if self.installs.get(&reservation.install) == Some(&reservation.at) {
            self.installs.remove(&reservation.install);
        }
        if let Some(window) = self.addresses.get_mut(&reservation.address)
            && window.opened == reservation.window
        {
            window.count = window.count.saturating_sub(1);
            if window.count == 0 {
                self.addresses.remove(&reservation.address);
            }
        }
    }
}

/// What the address limit keys on. IPv4 addresses as they are; IPv6 by
/// their /64, because one connection is routinely given a whole /64 and
/// could otherwise take a fresh address for every report.
fn address_key(address: IpAddr) -> IpAddr {
    match address.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(
            u128::from(v6) & 0xffff_ffff_ffff_ffff_0000_0000_0000_0000,
        )),
        v4 => v4,
    }
}

struct State {
    config: Config,
    limits: Mutex<Limits>,
}

/// Accepts connections on `listener` and handles reports until the future
/// is dropped.
pub async fn serve(listener: TcpListener, config: Config) {
    let state = Arc::new(State {
        config,
        limits: Mutex::default(),
    });
    // The mail server is dialled once at startup, so a wrong password or
    // host shows in the log now rather than with the first person's report.
    // Not fatal: a server that is down for a minute should not stop this.
    let checking = state.clone();
    tokio::spawn(async move {
        let Delivery::Smtp(mailer) = &checking.config.delivery else {
            return;
        };
        match mailer.transport.test_connection().await {
            Ok(true) => tracing::info!("mail server reachable"),
            Ok(false) => tracing::warn!("mail server connected but did not answer"),
            Err(error) => tracing::warn!(%error, "mail server not reachable"),
        }
    });

    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // Out of file descriptors, most likely; accepting again at
                // once would only spin.
                tracing::warn!(%error, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let service = service_fn(move |request| {
                let state = state.clone();
                async move { Ok::<_, Infallible>(route(state, request, peer.ip()).await) }
            });
            // One request per connection: the app sends one report and goes,
            // and an idle keep-alive socket is one more held open.
            let connection = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(READ_TIMEOUT)
                .keep_alive(false)
                .max_buf_size(64 * 1024)
                .serve_connection(TokioIo::new(stream), service);
            if let Err(error) = connection.await {
                tracing::debug!(%error, "connection ended");
            }
        });
    }
}

async fn route(
    state: Arc<State>,
    request: Request<Incoming>,
    peer: IpAddr,
) -> Response<Full<Bytes>> {
    let method = request.method();
    match request.uri().path() {
        "/v1/feedback" if method == Method::POST => receive(state, request, peer).await,
        "/v1/feedback" => {
            let mut response = refuse(
                StatusCode::METHOD_NOT_ALLOWED,
                "Reports are sent with POST.",
            );
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("POST"));
            response
        }
        "/healthz" if method == Method::GET || method == Method::HEAD => {
            let mut response = Response::new(Full::new(Bytes::from_static(b"ok")));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
        }
        "/healthz" => {
            let mut response = refuse(
                StatusCode::METHOD_NOT_ALLOWED,
                "The health check is read with GET.",
            );
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            response
        }
        _ => refuse(StatusCode::NOT_FOUND, "There is nothing at this address."),
    }
}

async fn receive(
    state: Arc<State>,
    request: Request<Incoming>,
    peer: IpAddr,
) -> Response<Full<Bytes>> {
    // Turned away on what it says it is, before a byte of it is read.
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok())
        .and_then(|length| length.parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_BODY as u64) {
        return too_large();
    }
    // JSON only, and said so. Besides being the contract, it keeps a web
    // page from posting reports through its visitors' browsers: a JSON
    // content type needs a CORS preflight, which this never answers.
    let is_json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return refuse(
            StatusCode::BAD_REQUEST,
            "The report has to be sent as JSON.",
        );
    }
    let address = client_address(&request, peer, state.config.trust_proxy);

    let body = Limited::new(request.into_body(), MAX_BODY).collect();
    let body = match tokio::time::timeout(READ_TIMEOUT, body).await {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(error)) if error.is::<LengthLimitError>() => return too_large(),
        Ok(Err(_)) => return refuse(StatusCode::BAD_REQUEST, "The report could not be read."),
        Err(_) => {
            return refuse(
                StatusCode::REQUEST_TIMEOUT,
                "The report took too long to arrive.",
            );
        }
    };
    let Ok(submission) = serde_json::from_slice::<Submission>(&body) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "The report could not be read as JSON.",
        );
    };
    let report = match Report::check(submission) {
        Ok(report) => report,
        Err(sentence) => return refuse(StatusCode::BAD_REQUEST, sentence),
    };

    let reserved =
        lock(&state.limits).reserve(report.install_key(), address_key(address), Instant::now());
    let reservation = match reserved {
        Ok(reservation) => reservation,
        Err(Refusal::Install(wait)) => {
            return rate_limited(
                "This install has already sent a report in the last 7 days.",
                wait,
            );
        }
        Err(Refusal::Address(wait)) => {
            return rate_limited("Too many reports have come from this network today.", wait);
        }
        Err(Refusal::Full) => {
            return rate_limited("The service is busy. Try again later.", FULL_RETRY);
        }
    };

    // Sent from a task of its own, so a client that hangs up mid-send does
    // not cancel it halfway: the report either goes and counts, or fails
    // and its reservation is handed back, whether anyone is waiting or not.
    let kind = report.kind;
    let email = report.email(SystemTime::now());
    let sending = state.clone();
    let sent = tokio::spawn(async move {
        let sent = deliver(&sending.config.delivery, kind, &email).await;
        if !sent {
            lock(&sending.limits).release(&reservation);
        }
        sent
    })
    .await
    .unwrap_or(false);

    if sent {
        json(StatusCode::ACCEPTED, &serde_json::json!({ "ok": true }))
    } else {
        refuse(StatusCode::BAD_GATEWAY, SEND_FAILED)
    }
}

/// Sends — or in log mode, prints — one report. `false` when it did not go.
async fn deliver(delivery: &Delivery, kind: Kind, email: &Email) -> bool {
    match delivery {
        Delivery::Log => {
            tracing::info!(
                kind = kind.label(),
                "report received; log mode, so not sent:\n{}",
                email.preview()
            );
            true
        }
        Delivery::Smtp(mailer) => match mailer.send(email).await {
            Ok(()) => {
                tracing::info!(kind = kind.label(), "report sent");
                true
            }
            Err(error) => {
                tracing::error!(%error, kind = kind.label(), "report could not be sent");
                false
            }
        },
    }
}

/// The address a report is counted against: the socket's peer, or behind a
/// trusted proxy the first hop of `X-Forwarded-For` — the client as the
/// proxy saw it. A header that does not parse falls back to the peer.
fn client_address(request: &Request<Incoming>, peer: IpAddr, trust_proxy: bool) -> IpAddr {
    if !trust_proxy {
        return peer;
    }
    request
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|hop| hop.trim().parse().ok())
        .unwrap_or(peer)
}

fn json(status: StatusCode, value: &serde_json::Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(value.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// An error response: `{"error": sentence}`.
fn refuse(status: StatusCode, sentence: &str) -> Response<Full<Bytes>> {
    json(status, &serde_json::json!({ "error": sentence }))
}

fn too_large() -> Response<Full<Bytes>> {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        "The report is larger than 32 KiB.",
    )
}

/// A 429, with how long to wait in both the body and `Retry-After`, in whole
/// seconds rounded up so that waiting exactly that long is always enough.
fn rate_limited(sentence: &str, wait: Duration) -> Response<Full<Bytes>> {
    let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    let mut response = json(
        StatusCode::TOO_MANY_REQUESTS,
        &serde_json::json!({ "error": sentence, "retry_after_secs": seconds }),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    response
}

#[cfg(test)]
mod tests {
    //! The checks a report passes, the email it becomes, and the limits it
    //! counts against. `tests/serve.rs` runs the same contract over HTTP.

    use std::net::Ipv4Addr;

    use super::*;

    const INSTALL: &str = "0123456789abcdef0123456789abcdef";

    fn submission(message: &str) -> Submission {
        Submission {
            kind: Some("bug".to_owned()),
            message: Some(message.to_owned()),
            reply_to: Some("someone@example.com".to_owned()),
            app_version: Some("0.1.0".to_owned()),
            os: Some("macOS 27.0".to_owned()),
            install: Some(INSTALL.to_owned()),
        }
    }

    fn refused(submission: Submission) -> &'static str {
        match Report::check(submission) {
            Ok(_) => panic!("should have been refused"),
            Err(sentence) => sentence,
        }
    }

    #[test]
    fn a_whole_report_passes_with_its_fields_tidied() {
        let mut sent = submission("  It crashed.\r\nTwice.\u{1b}[31m  ");
        sent.app_version = Some(" 0.1.0\u{7} ".to_owned());
        let report = Report::check(sent).unwrap();
        assert_eq!(report.message, "It crashed.\nTwice.\u{FFFD}[31m");
        assert_eq!(report.app_version, "0.1.0\u{FFFD}");
        assert_eq!(report.os.as_deref(), Some("macOS 27.0"));
        assert_eq!(
            report.reply_to.as_ref().map(|a| a.to_string()).as_deref(),
            Some("someone@example.com")
        );
        assert_eq!(
            report.install_key(),
            0x0123_4567_89ab_cdef_0123_4567_89ab_cdef
        );
    }

    #[test]
    fn each_field_out_of_contract_gets_its_own_sentence() {
        let mut kind = submission("x");
        kind.kind = Some("rant".to_owned());
        assert!(refused(kind).contains("feedback, bug or idea"));

        assert_eq!(refused(submission("   ")), "The message is empty.");
        assert!(refused(submission(&"x".repeat(MAX_MESSAGE + 1))).contains("10,000"));

        for bad in [
            "not-an-address",
            "a@b@c",
            "a b@c.d",
            "@c.d",
            "a@",
            "a@b.c\r\nBcc: x",
        ] {
            let mut reply = submission("x");
            reply.reply_to = Some(bad.to_owned());
            assert!(
                refused(reply).contains("reply address"),
                "{bad:?} should not be a reply address"
            );
        }
        let mut long_reply = submission("x");
        long_reply.reply_to = Some(format!("{}@example.com", "a".repeat(250)));
        assert!(refused(long_reply).contains("254"));

        let mut no_version = submission("x");
        no_version.app_version = None;
        assert!(refused(no_version).contains("which version"));
        let mut long_version = submission("x");
        long_version.app_version = Some("9".repeat(65));
        assert!(refused(long_version).contains("64"));

        let mut long_os = submission("x");
        long_os.os = Some("m".repeat(129));
        assert!(refused(long_os).contains("128"));

        for bad in [
            "",
            "0123",
            "0123456789ABCDEF0123456789ABCDEF",
            &"g".repeat(32),
        ] {
            let mut install = submission("x");
            install.install = Some(bad.to_owned());
            assert!(refused(install).contains("32 lowercase"), "{bad:?}");
        }
    }

    #[test]
    fn optional_fields_may_be_absent_or_blank() {
        let mut sent = submission("Fine");
        sent.reply_to = Some("   ".to_owned());
        sent.os = None;
        let report = Report::check(sent).unwrap();
        assert!(report.reply_to.is_none());
        assert!(report.os.is_none());
    }

    #[test]
    fn the_email_carries_a_short_subject_and_a_footer_after_the_signature_mark() {
        let long_first_line = "a".repeat(80);
        let mut sent = submission(&format!("{long_first_line}\nsecond line"));
        sent.os = None;
        let email = Report::check(sent)
            .unwrap()
            .email(UNIX_EPOCH + Duration::from_secs(86_400));
        assert_eq!(
            email.subject,
            format!("[ket] Bug: {}…", "a".repeat(SUBJECT_CHARS))
        );
        assert!(
            email
                .body
                .starts_with(&format!("{long_first_line}\nsecond line\n\n-- \n"))
        );
        assert!(email.body.contains("Kind: Bug\n"));
        assert!(email.body.contains("App version: 0.1.0\n"));
        assert!(email.body.contains("OS: not shared\n"));
        assert!(email.body.contains(&format!("Install: {INSTALL}\n")));
        assert!(email.body.ends_with("Received: 1970-01-02T00:00:00Z\n"));

        let preview = email.preview();
        assert!(preview.starts_with("Subject: [ket] Bug: "));
        assert!(preview.contains("\nReply-To: someone@example.com\n"));
    }

    #[test]
    fn an_empty_app_version_reads_as_not_given() {
        let mut sent = submission("Short");
        sent.app_version = Some("  ".to_owned());
        let email = Report::check(sent).unwrap().email(UNIX_EPOCH);
        assert_eq!(email.subject, "[ket] Bug: Short");
        assert!(email.body.contains("App version: not given\n"));
    }

    #[test]
    fn times_are_written_as_rfc_3339_in_utc() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // A leap day, and the turn of a century.
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_723)),
            "2000-02-29T01:02:03Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_791_936_000)),
            "2026-10-14T00:00:00Z"
        );
    }

    #[test]
    fn a_server_url_is_shown_without_its_credentials() {
        assert_eq!(
            without_credentials("smtps://user:pa%40ss@smtp.example.com:465"),
            "smtps://smtp.example.com:465"
        );
        assert_eq!(
            without_credentials("smtp://u:p@mail.example.com:587/helo.example.com?tls=required"),
            "smtp://mail.example.com:587 (tls=required)"
        );
        assert_eq!(
            without_credentials("mail.example.com"),
            "smtp://mail.example.com"
        );
    }

    #[test]
    fn message_ids_are_unique_and_under_the_from_domain() {
        let one = message_id("example.com");
        let two = message_id("example.com");
        assert_ne!(one, two);
        assert!(one.starts_with("<ket-feedback.") && one.ends_with("@example.com>"));
    }

    #[test]
    fn a_mailer_refuses_a_bad_address_or_server_before_dialling() {
        assert!(
            Mailer::new("not an address", "ket@example.com", "smtp://localhost")
                .err()
                .unwrap()
                .to_string()
                .contains("KET_FEEDBACK_TO")
        );
        assert!(
            Mailer::new("to@example.com", "nope", "smtp://localhost")
                .err()
                .unwrap()
                .to_string()
                .contains("KET_FEEDBACK_FROM")
        );
        assert!(Mailer::new("to@example.com", "ket@example.com", "ftp://x").is_err());
    }

    #[test]
    fn the_summary_names_the_delivery_and_never_the_password() {
        let log = Config {
            delivery: Delivery::Log,
            trust_proxy: false,
        };
        assert_eq!(
            log.summary(),
            "log mode (KET_FEEDBACK_TO is unset): reports are printed, not sent; \
             client address from the socket"
        );

        let mailer = Mailer::new(
            "to@example.com",
            "ket <from@example.com>",
            "smtps://user:secret@smtp.example.com:465",
        )
        .unwrap();
        let smtp = Config {
            delivery: Delivery::Smtp(Box::new(mailer)),
            trust_proxy: true,
        };
        let summary = smtp.summary();
        assert!(
            summary.contains("emailing reports to to@example.com"),
            "{summary}"
        );
        assert!(
            summary.contains("smtps://smtp.example.com:465"),
            "{summary}"
        );
        assert!(summary.contains("X-Forwarded-For"), "{summary}");
        assert!(!summary.contains("secret"), "{summary}");
    }

    // ---- limits ----------------------------------------------------------------

    fn v4(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
    }

    #[test]
    fn an_install_gets_one_report_a_week() {
        let mut limits = Limits::default();
        let now = Instant::now();
        limits.reserve(1, v4(1), now).ok().unwrap();
        match limits.reserve(1, v4(2), now + Duration::from_secs(60)) {
            Err(Refusal::Install(wait)) => {
                assert_eq!(wait, INSTALL_INTERVAL - Duration::from_secs(60));
            }
            _ => panic!("a second report in the week is refused"),
        }
        assert!(limits.reserve(1, v4(3), now + INSTALL_INTERVAL).is_ok());
    }

    #[test]
    fn an_address_gets_ten_reports_a_day_then_a_new_window() {
        let mut limits = Limits::default();
        let now = Instant::now();
        for install in 0..u128::from(ADDRESS_REPORTS) {
            limits.reserve(install, v4(1), now).ok().unwrap();
        }
        assert!(matches!(
            limits.reserve(99, v4(1), now + Duration::from_secs(1)),
            Err(Refusal::Address(_))
        ));
        // Another address is not affected, and the next day starts over.
        assert!(limits.reserve(100, v4(2), now).is_ok());
        assert!(limits.reserve(101, v4(1), now + ADDRESS_WINDOW).is_ok());
        assert_eq!(limits.addresses[&v4(1)].count, 1);
    }

    #[test]
    fn a_report_that_was_not_sent_hands_its_place_back() {
        let mut limits = Limits::default();
        let now = Instant::now();
        let reservation = limits.reserve(7, v4(1), now).ok().unwrap();
        limits.release(&reservation);
        assert!(limits.installs.is_empty());
        assert!(limits.addresses.is_empty());
        // So the same install can try again at once.
        assert!(limits.reserve(7, v4(1), now).is_ok());
    }

    #[test]
    fn handing_back_an_old_reservation_leaves_a_newer_one_alone() {
        let mut limits = Limits::default();
        let now = Instant::now();
        let old = limits.reserve(7, v4(1), now).ok().unwrap();
        let later = now + INSTALL_INTERVAL;
        limits.reserve(7, v4(1), later).ok().unwrap();
        limits.release(&old);
        assert_eq!(limits.installs.get(&7), Some(&later));
    }

    #[test]
    fn ipv6_addresses_count_by_their_slash_64_and_mapped_v4_as_v4() {
        let one: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let two: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(address_key(one), address_key(two));
        assert_ne!(address_key(one), address_key(other));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(address_key(mapped), v4(7));
    }

    #[test]
    fn a_wait_is_rounded_up_to_whole_seconds() {
        let response = rate_limited("Slow down.", Duration::from_millis(1_500));
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "2");
        let exact = rate_limited("Slow down.", Duration::from_secs(3));
        assert_eq!(exact.headers()[header::RETRY_AFTER], "3");
    }
}
