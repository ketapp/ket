//! The host's identity for phones, and the phones it has paired with, kept
//! between runs (Epic 5b.2).
//!
//! In `remote/` in the data directory, owner-only, the way `ssh` keeps its
//! keys:
//!
//! - `identity.json` — the host's id and its Curve25519 key pair. The private
//!   half is the one secret here: whoever has it can be this host to a phone.
//! - `devices.json` — the phones granted, by public key, with the name each
//!   gave itself. Nothing secret, but nobody else's business.
//!
//! Not the Keychain, which the plan names, and deliberately for now: a
//! Keychain item trusts the binary that created it, and a ket rebuilt all day
//! is a different binary every time — a password prompt after every build.
//! Everything that reads or writes the key goes through this module, so
//! moving it is one change here.
//!
//! One identity per data directory, like the host itself: a sandboxed
//! `XDG_DATA_HOME` is a different host to a phone, and cannot impersonate the
//! owner's.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde::{Deserialize, Serialize};

use super::host_error;
use super::wire::DeviceRole;
use crate::{Result, paths};

/// Where the identity and the device list live.
pub fn dir() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join("remote"))
}

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    id: String,
    public: String,
    private: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredDevice {
    device: String,
    name: String,
    paired_at: u64,
    /// Missing for a grant made before roles existed; the host gives it full
    /// access when it next connects and writes the role then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<DeviceRole>,
}

/// Roles persisted for paired devices, keyed by their public key.
pub(super) type Roles = std::collections::HashMap<Vec<u8>, DeviceRole>;

/// This data directory's host identity, if it has one yet. Never creates it.
pub fn load() -> Result<Option<ket_remote::Host>> {
    load_in(&dir()?)
}

/// [`load`], against a specific directory rather than this data directory's
/// own — the seam a test uses instead of the real XDG data dir.
pub fn load_in(dir: &Path) -> Result<Option<ket_remote::Host>> {
    match std::fs::read(dir.join("identity.json")) {
        Ok(bytes) => restore(&bytes, &dir.join("devices.json")).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(host_error(error)),
    }
}

/// This data directory's host identity and paired phones, created the first
/// time it is asked for.
pub fn load_or_create() -> Result<ket_remote::Host> {
    load_or_create_in(&dir()?)
}

/// [`load_or_create`], against a specific directory — see [`load_in`].
pub fn load_or_create_in(dir: &Path) -> Result<ket_remote::Host> {
    let identity = dir.join("identity.json");
    let host = match std::fs::read(&identity) {
        Ok(bytes) => restore(&bytes, &dir.join("devices.json"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let host = ket_remote::Host::generate().map_err(host_error)?;
            save_in(dir, &host)?;
            host
        }
        Err(error) => return Err(host_error(error)),
    };
    Ok(host)
}

fn restore(identity: &[u8], devices: &Path) -> Result<ket_remote::Host> {
    let stored: StoredIdentity = serde_json::from_slice(identity).map_err(host_error)?;
    let decode = |text: &str| B64.decode(text).map_err(host_error);
    let id: [u8; 16] = decode(&stored.id)?
        .try_into()
        .map_err(|_| host_error("identity.json: an id of the wrong size"))?;
    let keys = ket_remote::Keypair::from_parts(decode(&stored.private)?, decode(&stored.public)?)
        .map_err(host_error)?;

    let grants = match std::fs::read(devices) {
        Ok(bytes) => serde_json::from_slice::<Vec<StoredDevice>>(&bytes)
            .map_err(host_error)?
            .into_iter()
            .filter_map(|device| {
                Some(ket_remote::Grant {
                    device: B64.decode(&device.device).ok()?,
                    name: device.name,
                    paired_at: device.paired_at,
                })
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(host_error(error)),
    };
    Ok(ket_remote::Host::restore(id, keys, grants))
}

/// Writes the identity and the device list. Call it after pairing, naming or
/// revoking a phone.
pub fn save(host: &ket_remote::Host) -> Result<()> {
    save_preserving_roles_in(&dir()?, host)
}

/// [`save`], against a specific directory — see [`load_in`].
///
/// Not defaulted when the existing roles can't be read: saving anyway would
/// strip every grant's role, and a grant without one is revoked the next time
/// its phone connects.
fn save_preserving_roles_in(dir: &Path, host: &ket_remote::Host) -> Result<()> {
    let roles = load_roles_in(dir)?;
    save_in_with_roles(dir, host, &roles)
}

/// Loads scoped authority. Legacy devices have no entry and fail closed.
pub(super) fn load_roles() -> Result<Roles> {
    load_roles_in(&dir()?)
}

/// [`load_roles`], against a specific directory — see [`load_in`].
fn load_roles_in(dir: &Path) -> Result<Roles> {
    let path = dir.join("devices.json");
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<Vec<StoredDevice>>(&bytes)
            .map_err(host_error)?
            .into_iter()
            .filter_map(|device| Some((B64.decode(device.device).ok()?, device.role?)))
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Roles::new()),
        Err(error) => Err(host_error(error)),
    }
}

/// Saves identity and grants with their explicit roles.
pub(super) fn save_with_roles(host: &ket_remote::Host, roles: &Roles) -> Result<()> {
    save_in_with_roles(&dir()?, host, roles)
}

/// [`save`], against a specific directory — see [`load_in`].
pub fn save_in(dir: &Path, host: &ket_remote::Host) -> Result<()> {
    save_in_with_roles(dir, host, &Roles::new())
}

fn save_in_with_roles(dir: &Path, host: &ket_remote::Host, roles: &Roles) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(host_error)?;

    let identity = StoredIdentity {
        id: B64.encode(host.id),
        public: B64.encode(&host.keys().public),
        private: B64.encode(host.keys().private_for_storage()),
    };
    write_private(
        &dir.join("identity.json"),
        &serde_json::to_vec_pretty(&identity).map_err(host_error)?,
    )?;

    let devices: Vec<StoredDevice> = host
        .grants()
        .iter()
        .map(|grant| StoredDevice {
            device: B64.encode(&grant.device),
            name: grant.name.clone(),
            paired_at: grant.paired_at,
            role: roles.get(&grant.device).copied(),
        })
        .collect();
    write_private(
        &dir.join("devices.json"),
        &serde_json::to_vec_pretty(&devices).map_err(host_error)?,
    )
}

/// Writes a file owner-only from the moment it exists, then renames it into
/// place, so there is never a readable copy and never a half-written one.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(host_error)?;
    file.write_all(bytes).map_err(host_error)?;
    file.sync_all().map_err(host_error)?;
    std::fs::rename(&tmp, path).map_err(host_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::wire::DeviceRole;

    /// A unique scratch directory for one test, cleaned up by the caller.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ket-identity-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_grants_role_round_trips_through_devices_json() {
        let dir = temp_dir("roles-roundtrip");
        let generated = ket_remote::Host::generate().unwrap();
        let host = ket_remote::Host::restore(
            generated.id,
            generated.keys().clone(),
            vec![ket_remote::Grant {
                device: vec![1, 2, 3],
                name: "phone".to_owned(),
                paired_at: 1,
            }],
        );

        let mut roles = Roles::new();
        roles.insert(vec![1, 2, 3], DeviceRole::Operator);
        save_in_with_roles(&dir, &host, &roles).unwrap();

        let reloaded = load_roles_in(&dir).unwrap();
        assert_eq!(reloaded.get(&vec![1, 2, 3]), Some(&DeviceRole::Operator));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_grant_saved_without_a_role_has_none_on_reload() {
        // A grant saved before roles existed has none on reload; the host
        // gives it full access when it next connects, not before.
        let dir = temp_dir("roles-absent");
        let generated = ket_remote::Host::generate().unwrap();
        let host = ket_remote::Host::restore(
            generated.id,
            generated.keys().clone(),
            vec![ket_remote::Grant {
                device: vec![9, 9],
                name: "legacy".to_owned(),
                paired_at: 1,
            }],
        );

        save_in_with_roles(&dir, &host, &Roles::new()).unwrap();

        assert!(load_roles_in(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_devices_file_has_no_roles_rather_than_erroring() {
        let dir = temp_dir("roles-missing");
        std::fs::create_dir_all(&dir).unwrap();

        assert!(load_roles_in(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_devices_file_is_an_error_not_silently_empty_roles() {
        let dir = temp_dir("roles-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("devices.json"), b"not json").unwrap();

        assert!(load_roles_in(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_propagates_a_broken_devices_file_instead_of_stripping_roles() {
        // The regression this guards against: `save` once defaulted to empty
        // roles on any read failure, so every existing grant's role was
        // silently dropped and each phone got revoked on its next connect.
        let dir = temp_dir("save-propagates-error");
        let generated = ket_remote::Host::generate().unwrap();
        let host = ket_remote::Host::restore(
            generated.id,
            generated.keys().clone(),
            vec![ket_remote::Grant {
                device: vec![4, 5, 6],
                name: "phone".to_owned(),
                paired_at: 1,
            }],
        );

        let mut roles = Roles::new();
        roles.insert(vec![4, 5, 6], DeviceRole::Administrator);
        save_in_with_roles(&dir, &host, &roles).unwrap();

        // Corrupt the file an unrelated write left behind.
        std::fs::write(dir.join("devices.json"), b"not json").unwrap();

        let result = save_preserving_roles_in(&dir, &host);
        assert!(result.is_err());

        // The corrupt file — and whatever role it had on disk — is untouched:
        // a failed save must not go on to overwrite it with role-less grants.
        let raw = std::fs::read_to_string(dir.join("devices.json")).unwrap();
        assert_eq!(raw, "not json");
        std::fs::remove_dir_all(&dir).ok();
    }
}
