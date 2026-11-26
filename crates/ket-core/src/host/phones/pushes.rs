//! Where each paired phone asked to be sent notifications while it isn't
//! connected, and for what: `RegisterPush`, protocol 1.34.
//!
//! Kept in `push.json` beside `devices.json`, by device key, so a
//! registration outlives the host and goes with the phone's grant: revoking
//! a phone drops it, and so does the push service saying Apple has retired
//! its token. A file of its own rather than a field of the device list,
//! which is rebuilt from the grants on every save and would lose it.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ket_remote::proto;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::host::{host_error, identity};

/// The shortest and longest token taken, as the push service takes them.
/// Apple's are 32 bytes and it says they may grow.
const LEAST_TOKEN: usize = 32;
const MOST_TOKEN: usize = 100;

/// One phone's registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::host) struct Registration {
    /// The phone's APNs device token.
    pub(in crate::host) token: Vec<u8>,
    pub(in crate::host) environment: Environment,
    /// The X25519 public key each notification is sealed to.
    pub(in crate::host) key: [u8; 32],
    pub(in crate::host) kinds: Kinds,
    /// Unix seconds.
    pub(in crate::host) registered_at: u64,
}

/// Which of Apple's push services the token belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::host) enum Environment {
    Sandbox,
    Production,
}

/// What the phone asked to be told about, as its Settings › Notifications
/// lists them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::host) struct Kinds {
    /// An agent stopped on a permission prompt, a question or a plan.
    #[serde(default)]
    pub(in crate::host) waiting: bool,
    /// An agent finished its task, or failed.
    #[serde(default)]
    pub(in crate::host) finished: bool,
    /// A check on a worktree's pull request failed.
    #[serde(default)]
    pub(in crate::host) checks: bool,
    /// An agent's plan ran out mid-task, or its limit has reset.
    #[serde(default)]
    pub(in crate::host) limits: bool,
}

impl Kinds {
    fn any(self) -> bool {
        self.waiting || self.finished || self.checks || self.limits
    }
}

/// Every phone's registration, by device key.
pub(in crate::host) type Pushes = HashMap<Vec<u8>, Registration>;

impl Registration {
    /// What a phone sent, checked: `Ok(None)` when it asked for nothing,
    /// which is the same as unregistering, and `Err` saying what was wrong.
    pub(in crate::host) fn from_proto(
        register: &proto::RegisterPush,
        now: u64,
    ) -> std::result::Result<Option<Self>, &'static str> {
        if !(LEAST_TOKEN..=MOST_TOKEN).contains(&register.token.len()) {
            return Err("a push token must be 32 to 100 bytes");
        }
        let key: [u8; 32] = register
            .push_key
            .as_slice()
            .try_into()
            .map_err(|_| "a push key must be a 32-byte X25519 public key")?;
        // A key nothing can be sealed to, such as all zeros, would show the
        // phone as getting notifications when none could ever be opened.
        if !super::push::sealable(&key) {
            return Err("a push key must be a usable X25519 public key");
        }
        let environment = match proto::PushEnvironment::try_from(register.environment) {
            Ok(proto::PushEnvironment::Sandbox) => Environment::Sandbox,
            Ok(proto::PushEnvironment::Production) => Environment::Production,
            _ => return Err("a push registration must name its environment"),
        };
        let mut kinds = Kinds::default();
        for kind in &register.kinds {
            // A kind from a newer phone is passed over, not refused.
            match proto::PushKind::try_from(*kind) {
                Ok(proto::PushKind::Waiting) => kinds.waiting = true,
                Ok(proto::PushKind::Finished) => kinds.finished = true,
                Ok(proto::PushKind::Checks) => kinds.checks = true,
                Ok(proto::PushKind::Limits) => kinds.limits = true,
                _ => {}
            }
        }
        if !kinds.any() {
            return Ok(None);
        }
        Ok(Some(Self {
            token: register.token.clone(),
            environment,
            key,
            kinds,
            registered_at: now,
        }))
    }
}

/// One registration as `push.json` holds it.
#[derive(Serialize, Deserialize)]
struct Stored {
    /// The phone's public key, base64url, as `devices.json` names it.
    device: String,
    /// Base64url.
    token: String,
    environment: Environment,
    /// Base64url.
    key: String,
    kinds: Kinds,
    registered_at: u64,
}

/// The registrations in this data directory, for the devices still granted.
/// None when the file isn't there.
pub(in crate::host) fn load(granted: &[Vec<u8>]) -> Result<Pushes> {
    load_in(&identity::dir()?, granted)
}

fn load_in(dir: &Path, granted: &[Vec<u8>]) -> Result<Pushes> {
    let bytes = match std::fs::read(dir.join("push.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Pushes::new()),
        Err(error) => return Err(host_error(error)),
    };
    let stored: Vec<Stored> = serde_json::from_slice(&bytes).map_err(host_error)?;
    Ok(stored
        .into_iter()
        .filter_map(|stored| {
            let device = B64.decode(&stored.device).ok()?;
            granted.contains(&device).then_some(())?;
            let registration = Registration {
                token: B64.decode(&stored.token).ok()?,
                environment: stored.environment,
                key: B64.decode(&stored.key).ok()?.try_into().ok()?,
                kinds: stored.kinds,
                registered_at: stored.registered_at,
            };
            Some((device, registration))
        })
        .collect())
}

/// Writes every registration, owner-only, in place of the last file.
pub(in crate::host) fn save(pushes: &Pushes) -> Result<()> {
    let mut stored: Vec<Stored> = pushes
        .iter()
        .map(|(device, registration)| Stored {
            device: B64.encode(device),
            token: B64.encode(&registration.token),
            environment: registration.environment,
            key: B64.encode(registration.key),
            kinds: registration.kinds,
            registered_at: registration.registered_at,
        })
        .collect();
    // In a steady order, so an unchanged set writes an unchanged file.
    stored.sort_by(|a, b| a.device.cmp(&b.device));
    identity::write_private(
        &identity::dir()?.join("push.json"),
        &serde_json::to_vec_pretty(&stored).map_err(host_error)?,
    )
}

/// Whether `device` has a registration on disk: for listing devices with no
/// host running.
pub(in crate::host) fn registered(device: &[u8]) -> bool {
    let Ok(dir) = identity::dir() else {
        return false;
    };
    load_in(&dir, &[device.to_vec()]).is_ok_and(|pushes| pushes.contains_key(device))
}
