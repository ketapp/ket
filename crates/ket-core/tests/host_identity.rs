//! The host's identity and paired-device list, via the `_in(dir, ...)` seam
//! (mirroring `crate::backlog`'s and `crate::shell`'s own) rather than the
//! real XDG data directory.

use ket_core::host::identity::{load_in, load_or_create_in, save_in};
use ket_remote::{Grant, Host};

mod common;
use common::Sandbox;

#[test]
fn a_directory_with_nothing_in_it_has_no_identity() {
    let sandbox = Sandbox::new("identity-empty");
    let dir = sandbox.path("remote");
    std::fs::create_dir_all(&dir).unwrap();

    assert!(load_in(&dir).unwrap().is_none());
}

#[test]
fn a_missing_directory_also_has_no_identity() {
    let sandbox = Sandbox::new("identity-missing-dir");
    let dir = sandbox.path("does-not-exist");

    assert!(load_in(&dir).unwrap().is_none());
}

#[test]
fn save_then_load_restores_the_same_id_and_keys() {
    let sandbox = Sandbox::new("identity-roundtrip");
    let dir = sandbox.path("remote");

    let host = Host::generate().expect("generate");
    save_in(&dir, &host).unwrap();

    let restored = load_in(&dir).unwrap().expect("identity was saved");
    assert_eq!(restored.id, host.id);
    assert_eq!(restored.keys().public, host.keys().public);
    assert_eq!(
        restored.keys().private_for_storage(),
        host.keys().private_for_storage()
    );
    assert!(restored.grants().is_empty());
}

#[test]
fn save_then_load_restores_every_grant() {
    let sandbox = Sandbox::new("identity-grants");
    let dir = sandbox.path("remote");

    let generated = Host::generate().expect("generate");
    let phone_a = Grant {
        device: vec![1, 2, 3, 4],
        name: "My iPhone".to_owned(),
        paired_at: 1_700_000_000,
    };
    let phone_b = Grant {
        device: vec![5, 6, 7, 8],
        name: "My iPad".to_owned(),
        paired_at: 1_700_000_100,
    };
    let host = Host::restore(
        generated.id,
        generated.keys().clone(),
        vec![phone_a.clone(), phone_b.clone()],
    );

    save_in(&dir, &host).unwrap();
    let restored = load_in(&dir).unwrap().expect("identity was saved");

    assert_eq!(restored.grants().len(), 2);
    assert_eq!(restored.grants()[0].device, phone_a.device);
    assert_eq!(restored.grants()[0].name, phone_a.name);
    assert_eq!(restored.grants()[0].paired_at, phone_a.paired_at);
    assert_eq!(restored.grants()[1].name, phone_b.name);
}

#[test]
fn a_malformed_identity_file_is_an_error_not_a_silent_none() {
    let sandbox = Sandbox::new("identity-malformed");
    let dir = sandbox.path("remote");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("identity.json"), "not json").unwrap();

    assert!(load_in(&dir).is_err());
}

#[test]
fn an_identity_with_no_device_file_yet_has_no_grants() {
    let sandbox = Sandbox::new("identity-no-devices-file");
    let dir = sandbox.path("remote");

    let host = Host::generate().expect("generate");
    save_in(&dir, &host).unwrap();
    std::fs::remove_file(dir.join("devices.json")).unwrap();

    let restored = load_in(&dir).unwrap().expect("identity was saved");
    assert!(restored.grants().is_empty());
}

#[test]
fn load_or_create_makes_one_the_first_time_and_reuses_it_after() {
    let sandbox = Sandbox::new("identity-load-or-create");
    let dir = sandbox.path("remote");

    let first = load_or_create_in(&dir).unwrap();
    let second = load_or_create_in(&dir).unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(first.keys().public, second.keys().public);
}

#[test]
fn saved_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new("identity-permissions");
    let dir = sandbox.path("remote");
    let host = Host::generate().expect("generate");
    save_in(&dir, &host).unwrap();

    let mode = std::fs::metadata(dir.join("identity.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
