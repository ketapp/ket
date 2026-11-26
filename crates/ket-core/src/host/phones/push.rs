//! Sending a notice to a phone that asked for it and isn't connected: sealed
//! to the phone's own push key here, on the Mac, then posted to ket's push
//! service (`deploy/push`), which forwards it to Apple under a generic alert.
//! Neither the service nor Apple can read it; the phone's notification
//! extension opens it and shows the real words.
//!
//! What the extension opens is JSON:
//!
//! ```json
//! { "v": 1, "host": "<this Mac's id, base64url, as the phone names it>",
//!   "kind": "waiting" | "finished" | "failed" | "checks",
//!   "title": "…", "body": "…", "worktree": "<id>",
//!   "opens": "chat" | "worktree" | "pull_request",
//!   "terminal": "<id, for a chat>" }
//! ```
//!
//! or, for the test a person sends from Settings › Devices, `"kind": "test"`
//! with a title and a body and nothing to open.
//!
//! sealed with HPKE (RFC 9180) in base mode — DHKEM(X25519, HKDF-SHA256),
//! HKDF-SHA256 and ChaCha20-Poly1305, info [`INFO`], no associated data —
//! and sent as the 32-byte encapsulated key followed by the ciphertext, in
//! standard base64. CryptoKit's `HPKE.Recipient` opens it on the phone.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64};
use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem, OpModeS, Serializable};
use serde_json::json;

use super::notices::{Kind, Notice, Opens};
use super::pushes::{Environment, Registration};
use crate::config::PhonesConfig;
use crate::curl::{PostError, post_json_answer};

/// Binds a sealed notification to this use, so a ciphertext made for push
/// can't be passed off as anything else sealed to the same key.
const INFO: &[u8] = b"ket push v1";

/// How long one post may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long before the one retry of a post that didn't get through.
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// Push as the config has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::host) struct Settings {
    pub(in crate::host) enabled: bool,
    /// The push service, without its route.
    pub(in crate::host) url: String,
    /// How long a notice waits before it goes.
    pub(in crate::host) after: Duration,
}

impl Settings {
    pub(in crate::host) fn from_config(config: &PhonesConfig) -> Self {
        Self {
            enabled: config.push && !config.push_url.trim().is_empty(),
            url: config.push_url.trim().trim_end_matches('/').to_owned(),
            after: Duration::from_secs(config.push_after_seconds),
        }
    }
}

impl Registration {
    /// Whether the phone asked to be told about `kind`.
    pub(in crate::host) fn wants(&self, kind: Kind) -> bool {
        match kind {
            Kind::Waiting => self.kinds.waiting,
            Kind::Finished | Kind::Failed => self.kinds.finished,
            Kind::Checks => self.kinds.checks,
            // A task stopped by its limit is a task that stopped: a phone
            // from before limits had a switch hears of it as it hears of a
            // failure, in place of the "finished" this notice stands in for.
            Kind::Limit => self.kinds.limits || self.kinds.finished,
            Kind::Reset => self.kinds.limits,
        }
    }
}

/// One notification for one phone, sealed and ready to post.
pub(in crate::host) struct Delivery {
    pub(in crate::host) device: Vec<u8>,
    /// The token it was sealed for, to drop it by if Apple has retired it.
    pub(in crate::host) token: Vec<u8>,
    body: String,
}

/// What became of a post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::host) enum Outcome {
    Sent,
    /// The token will never work again: forget it.
    Dead,
    /// Not sent, and not worth trying again.
    Dropped,
}

/// `notice` sealed for the phone `device`, as the push service takes it.
pub(in crate::host) fn prepare(
    host: &[u8; 16],
    notice: &Notice,
    device: &[u8],
    registration: &Registration,
) -> Result<Delivery, String> {
    let host_id = B64.encode(host);
    let mut payload = json!({
        "v": 1,
        "host": host_id,
        "kind": match notice.kind {
            Kind::Waiting => "waiting",
            Kind::Finished => "finished",
            Kind::Failed => "failed",
            Kind::Checks => "checks",
            // "limit" is a word only a phone that asked for limits knows.
            Kind::Limit if registration.kinds.limits => "limit",
            Kind::Limit => "failed",
            Kind::Reset => "reset",
        },
        "title": notice.title,
        "body": notice.body,
        "worktree": notice.worktree,
        "opens": match notice.opens {
            Opens::Chat(_) => "chat",
            Opens::Worktree => "worktree",
            Opens::PullRequest => "pull_request",
        },
    });
    if let Opens::Chat(terminal) = notice.opens {
        payload["terminal"] = json!(terminal.to_string());
    }
    wrap(
        host,
        &payload,
        device,
        registration,
        &notice.id,
        &notice.project,
    )
}

/// A test notification for the phone `device`, sealed like any other, so it
/// goes the whole way a real one does: this Mac, the push service, Apple
/// and the phone's notification extension.
pub(in crate::host) fn prepare_test(
    host: &[u8; 16],
    device: &[u8],
    registration: &Registration,
) -> Result<Delivery, String> {
    let payload = json!({
        "v": 1,
        "host": B64.encode(host),
        "kind": "test",
        "title": "Test notification",
        "body": "This Mac can reach this phone, even with ket closed.",
    });
    // A second test replaces the first rather than stacking.
    wrap(host, &payload, device, registration, "ket-test", "ket-test")
}

/// `payload` sealed for the phone `device`, in the push service's request.
fn wrap(
    host: &[u8; 16],
    payload: &serde_json::Value,
    device: &[u8],
    registration: &Registration,
    collapse: &str,
    thread: &str,
) -> Result<Delivery, String> {
    let sealed = seal(&registration.key, payload.to_string().as_bytes())?;
    let body = json!({
        "token": hex(&registration.token),
        "environment": match registration.environment {
            Environment::Sandbox => "sandbox",
            Environment::Production => "production",
        },
        "ciphertext": sealed,
        // Hashed with this Mac's id, so Apple and the service see neither a
        // worktree's id nor a project's, and two Macs' never collide.
        "collapse_id": digest(host, collapse),
        "thread_id": digest(host, thread),
    });
    Ok(Delivery {
        device: device.to_vec(),
        token: registration.token.clone(),
        body: body.to_string(),
    })
}

/// What the push service answered a post, or why nothing did.
pub(in crate::host) enum Answer {
    /// Its status, and the `error` it gave with it: for a 502, Apple's own
    /// reason, such as `TopicDisallowed`.
    Status(u16, Option<String>),
    /// No answer: curl couldn't run, or the network failed.
    Nothing(String),
}

impl Answer {
    /// What to do about it.
    pub(in crate::host) fn outcome(&self) -> Outcome {
        match self {
            Self::Status(200, _) => Outcome::Sent,
            Self::Status(410, _) => Outcome::Dead,
            _ => Outcome::Dropped,
        }
    }

    /// Why it didn't go, in words for the person who pressed Send test;
    /// `None` when Apple took it. `url` is the push service it went to.
    pub(in crate::host) fn trouble(&self, url: &str) -> Option<String> {
        let because = |error: &Option<String>| match error {
            Some(error) => format!(" ({error})"),
            None => String::new(),
        };
        Some(match self {
            Self::Status(200, _) => return None,
            Self::Status(410, error) => format!(
                "Apple won't take this phone's token{}. Open ket on the phone once, so it registers again.",
                because(error)
            ),
            Self::Status(429, _) => {
                "Too many notifications for this phone just now. Try again in a minute.".to_owned()
            }
            Self::Status(500, _) => {
                "The push service isn't set up: its APNs key, key id or team id is missing."
                    .to_owned()
            }
            Self::Status(502, Some(error)) => format!("Apple refused it: {error}."),
            Self::Status(502, None) => "The push service couldn't reach Apple.".to_owned(),
            Self::Status(400 | 413, error) => {
                format!("The push service refused the request{}.", because(error))
            }
            Self::Status(status, error) => {
                format!(
                    "The push service at {url} answered {status}{}.",
                    because(error)
                )
            }
            Self::Nothing(why) => format!("Couldn't reach the push service at {url}: {why}"),
        })
    }
}

/// Posts `delivery` to the push service at `url`, once more after a
/// moment if it didn't get through. Blocks: run it off the tick.
pub(in crate::host) fn post(url: &str, delivery: &Delivery) -> Answer {
    let endpoint = format!("{url}/v1/push");
    let mut answer = Answer::Nothing("not tried".to_owned());
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(RETRY_AFTER);
        }
        let (next, again) = match post_json_answer(&endpoint, &delivery.body, TIMEOUT) {
            // Apple, the service or the network having a bad moment.
            Ok((status, body)) => (Answer::Status(status, service_error(&body)), status == 502),
            Err(error @ PostError::Network(_)) => (Answer::Nothing(error.to_string()), true),
            Err(error) => (Answer::Nothing(error.to_string()), false),
        };
        answer = next;
        if !again {
            break;
        }
    }
    answer
}

/// [`post`], logging what didn't go.
pub(in crate::host) fn deliver(url: &str, delivery: &Delivery) -> Outcome {
    let answer = post(url, delivery);
    match &answer {
        Answer::Status(200 | 410, _) => {}
        Answer::Status(status, error) => {
            tracing::info!(
                status,
                error = error.as_deref().unwrap_or(""),
                "push: not sent"
            );
        }
        Answer::Nothing(error) => tracing::info!(%error, "push: not sent"),
    }
    answer.outcome()
}

/// The `error` field of the push service's answer, cut short: it names a
/// field it refused, or Apple's reason.
fn service_error(body: &str) -> Option<String> {
    let answer: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = answer.get("error")?.as_str()?;
    Some(error.chars().take(100).collect())
}

/// Whether anything can be sealed to `key`: not a low-order point, whose
/// shared secret would be all zeros.
pub(in crate::host) fn sealable(key: &[u8; 32]) -> bool {
    seal(key, &[]).is_ok()
}

/// `plaintext` sealed to `key`: the encapsulated key, then the ciphertext,
/// as base64.
fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<String, String> {
    type Suite = X25519HkdfSha256;
    let recipient = <Suite as Kem>::PublicKey::from_bytes(key)
        .map_err(|error| format!("a phone's push key won't load: {error:?}"))?;
    let mut rng = rand_core::UnwrapErr(rand_core::OsRng);
    let (encapsulated, ciphertext) =
        hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, Suite, _>(
            &OpModeS::Base,
            &recipient,
            INFO,
            plaintext,
            &[],
            &mut rng,
        )
        .map_err(|error| format!("a notification won't seal: {error:?}"))?;
    let mut sealed = encapsulated.to_bytes().to_vec();
    sealed.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(sealed))
}

/// The first 16 bytes of SHA-256 over this Mac's id and `name`, as hex.
fn digest(host: &[u8; 16], name: &str) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    context.update(host);
    context.update(name.as_bytes());
    hex(&context.finish().as_ref()[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
