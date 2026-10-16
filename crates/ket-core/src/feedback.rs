//! Feedback and bug reports, sent from inside ket.
//!
//! A report goes to the feedback service — `crates/ket-feedback`, a plain
//! binary anyone can host — which emails it on. The address it is emailed to
//! lives in that service's configuration, never here, so choosing or changing
//! it touches no copy of ket that is already installed.
//!
//! One report a week: [`INTERVAL`] is checked here, before anything is sent,
//! and again by the service, which also keys its limit on [`install`] — an
//! anonymous random id made for nothing else and sent with nothing else.
//!
//! The request goes through `curl`, the way [`crate::jev`] reaches its API:
//! ket links no TLS stack of its own, and every Mac has a curl that trusts
//! the system's certificates.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{KetError, Result, build_info, paths};

/// How long after one report another may be sent.
pub const INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Where reports are sent until the service has a home of its own.
///
/// `None` until it is deployed. `KET_FEEDBACK_URL` overrides it, which is how
/// a service run locally is tried: `KET_FEEDBACK_URL=http://127.0.0.1:7980`.
const ENDPOINT: Option<&str> = None;

/// The service's route, under the endpoint.
const ROUTE: &str = "/v1/feedback";

/// The longest message the service accepts, in characters.
pub const MAX_MESSAGE: usize = 10_000;

/// Where curl puts the response body, which nothing reads: the status is
/// the answer.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// How long a send may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What a report is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// What is working and what is not.
    Feedback,
    /// Something that went wrong.
    Bug,
    /// Something ket could do.
    Idea,
}

impl Kind {
    /// Every kind, in the order the form offers them.
    pub const ALL: [Kind; 3] = [Kind::Feedback, Kind::Bug, Kind::Idea];

    /// What the form calls it.
    pub const fn label(self) -> &'static str {
        match self {
            Kind::Feedback => "Feedback",
            Kind::Bug => "Bug",
            Kind::Idea => "Idea",
        }
    }
}

/// One report, as a person wrote it.
#[derive(Debug, Clone)]
pub struct Report {
    /// What it is about.
    pub kind: Kind,
    /// What they wrote.
    pub message: String,
    /// Where to reply, when they would like one. Empty means no reply.
    pub reply_to: String,
    /// Whether the macOS version goes with it. ket's version always does: a
    /// bug is unreadable without it, and it says nothing about the person.
    pub share_system: bool,
}

/// What this install remembers about reports, in `feedback.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Ledger {
    /// The anonymous id — see [`install`].
    #[serde(default)]
    install: String,
    /// When the last report was accepted, in Unix milliseconds.
    #[serde(default)]
    last_sent_ms: Option<u64>,
}

fn ledger_file() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join("feedback.json"))
}

fn load() -> Ledger {
    ledger_file()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(ledger: &Ledger) -> Result<()> {
    let text = serde_json::to_string_pretty(ledger)
        .map_err(|e| KetError::Config(format!("could not record the report: {e}")))?;
    crate::config::write_atomically(&ledger_file()?, &text, true)
}

/// Whether `id` is one [`install`] could have made: 32 lowercase hex digits.
fn is_install_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// This install's anonymous id, made the first time it is asked for.
///
/// Random rather than derived from anything about the machine or the
/// person, and used only so the service can hold one install to one report
/// a week.
fn install(ledger: &mut Ledger) -> Result<String> {
    if !is_install_id(&ledger.install) {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|e| KetError::Config(format!("could not make an install id: {e}")))?;
        ledger.install = bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    Ok(ledger.install.clone())
}

/// When another report may be sent, in Unix milliseconds — `None` when one
/// may be sent now.
pub fn next_allowed_ms() -> Option<u64> {
    let last = load().last_sent_ms?;
    let next = last.saturating_add(INTERVAL.as_millis() as u64);
    (next > crate::now_ms()).then_some(next)
}

/// The service's URL, or `None` while there is nowhere to send to.
pub fn endpoint() -> Option<String> {
    let base = std::env::var("KET_FEEDBACK_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| ENDPOINT.map(str::to_owned))?;
    Some(format!("{}{ROUTE}", base.trim().trim_end_matches('/')))
}

/// The OS and its version, for a report that shares them: `macOS 26.0
/// (aarch64)`.
fn system() -> String {
    let arch = std::env::consts::ARCH;
    #[cfg(target_os = "macos")]
    {
        let version = Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        if !version.is_empty() {
            return format!("macOS {version} ({arch})");
        }
    }
    format!("{} ({arch})", std::env::consts::OS)
}

/// Sends `report`, and starts the week before the next one may go.
///
/// Refused before anything is sent when a report went less than [`INTERVAL`]
/// ago, when the message is empty or too long, or when there is no service
/// to send to. Blocks for as long as the request takes, up to twenty
/// seconds, so call it off the UI thread.
pub fn send(report: &Report) -> Result<()> {
    let message = report.message.trim();
    if message.is_empty() {
        return Err(KetError::Conflict(
            "Write something to send first.".to_owned(),
        ));
    }
    if message.chars().count() > MAX_MESSAGE {
        return Err(KetError::Conflict(format!(
            "That is longer than {MAX_MESSAGE} characters."
        )));
    }
    if next_allowed_ms().is_some() {
        return Err(KetError::Conflict(
            "You can send one report a week.".to_owned(),
        ));
    }
    let Some(url) = endpoint() else {
        return Err(KetError::Conflict(
            "Feedback isn't connected yet.".to_owned(),
        ));
    };

    let mut ledger = load();
    let install = install(&mut ledger)?;
    let reply_to = report.reply_to.trim();
    let body = json!({
        "kind": report.kind,
        "message": message,
        "reply_to": (!reply_to.is_empty()).then_some(reply_to),
        "app_version": build_info::display(),
        "os": report.share_system.then(system),
        "install": install,
    })
    .to_string();

    let status = post(&url, &body)?;
    match refusal(status) {
        None => {
            ledger.last_sent_ms = Some(crate::now_ms());
            save(&ledger)
        }
        Some(sentence) => Err(KetError::Conflict(sentence.to_owned())),
    }
}

/// What to tell the person when the service answered `status`, or `None`
/// when it took the report.
fn refusal(status: u16) -> Option<&'static str> {
    match status {
        200..=299 => None,
        400 => Some("The report was refused — check the reply address."),
        413 => Some("That report is too long."),
        // Not recorded as sent: the service limits a network as well as an
        // install, and a refusal for the network is no report from this one.
        429 => Some("A report went from here recently. Try again later."),
        _ => Some("The report could not be sent. Try again later."),
    }
}

/// The config curl reads on stdin to POST `body` to `url`.
///
/// curl's config format: quoted values, with `\` and `"` escaped, so
/// nothing in a message can end a line early.
fn request_config(url: &str, body: &str) -> String {
    let quote = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "url = \"{}\"\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\n\
         data-binary = \"{}\"\noutput = \"{NULL_DEVICE}\"\n",
        quote(url),
        quote(body),
    )
}

/// POSTs `body` as JSON to `url`; the HTTP status back.
fn post(url: &str, body: &str) -> Result<u16> {
    let config_text = request_config(url, body);

    let mut child = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg(format!("{:.1}", TIMEOUT.as_secs_f32()))
        .arg("--write-out")
        .arg("%{http_code}")
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
            .write_all(config_text.as_bytes())
            .map_err(|e| KetError::Config(format!("could not configure curl: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| KetError::Config(format!("curl did not finish: {e}")))?;
    if !output.status.success() {
        tracing::info!(
            "feedback: not sent: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(KetError::Conflict(
            "The report could not be sent — check your connection.".to_owned(),
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| KetError::Config("curl reported no status".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_id_is_thirty_two_lowercase_hex_characters() {
        assert!(is_install_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_install_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!is_install_id("0123"));
        assert!(!is_install_id(&"g".repeat(32)));
        assert!(!is_install_id(""));
    }

    #[test]
    fn an_install_id_is_made_once_and_then_kept() {
        let mut ledger = Ledger::default();
        let made = install(&mut ledger).unwrap();
        assert!(is_install_id(&made), "{made}");
        assert_eq!(install(&mut ledger).unwrap(), made, "kept");

        let mut other = Ledger::default();
        assert_ne!(install(&mut other).unwrap(), made, "random per install");

        // A ledger with a damaged id gets a fresh one rather than sending it.
        let mut damaged = Ledger {
            install: "not-an-id".to_owned(),
            last_sent_ms: None,
        };
        assert!(is_install_id(&install(&mut damaged).unwrap()));
    }

    #[test]
    fn a_ledger_reads_back_what_was_written_and_tolerates_missing_fields() {
        let ledger: Ledger = serde_json::from_str("{}").unwrap();
        assert!(ledger.install.is_empty());
        assert_eq!(ledger.last_sent_ms, None);
        let written = serde_json::to_string(&Ledger {
            install: "a".repeat(32),
            last_sent_ms: Some(7),
        })
        .unwrap();
        let read: Ledger = serde_json::from_str(&written).unwrap();
        assert_eq!(read.last_sent_ms, Some(7));
    }

    #[test]
    fn each_refusal_says_what_to_do() {
        for ok in [200, 202, 299] {
            assert_eq!(refusal(ok), None, "{ok}");
        }
        assert!(refusal(400).unwrap().contains("reply address"));
        assert!(refusal(413).unwrap().contains("too long"));
        assert!(refusal(429).unwrap().contains("recently"));
        assert!(refusal(502).unwrap().contains("could not be sent"));
        assert!(refusal(301).unwrap().contains("could not be sent"));
    }

    #[test]
    fn a_message_cannot_break_out_of_the_curl_config() {
        let body = serde_json::json!({
            "message": "ends here\"\nurl = \"https://elsewhere.example\"\n\\"
        })
        .to_string();
        let config = request_config("https://feedback.example/v1/feedback", &body);
        // Exactly the five lines ket writes, and only ket's url.
        let lines: Vec<&str> = config.lines().collect();
        assert_eq!(lines.len(), 5, "{config}");
        assert_eq!(lines[0], "url = \"https://feedback.example/v1/feedback\"");
        assert!(lines[3].starts_with("data-binary = \""));
        assert_eq!(config.matches("\nurl = ").count(), 0);
        assert!(lines[4].starts_with("output = "));
    }

    #[test]
    fn report_kinds_are_labelled_and_sent_in_lowercase() {
        assert_eq!(Kind::ALL.map(Kind::label), ["Feedback", "Bug", "Idea"]);
        assert_eq!(serde_json::to_string(&Kind::Bug).unwrap(), "\"bug\"");
    }
}
