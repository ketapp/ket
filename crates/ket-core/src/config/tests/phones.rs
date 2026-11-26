//! The `[phones]` table.

use super::super::*;

#[test]
fn a_relay_elsewhere_is_off_unless_one_is_named() {
    let config = Config::default();
    assert_eq!(config.phones.hosted_relay(), None);
    assert!(!toml::to_string(&config).unwrap().contains("relay ="));

    let own: Config = toml::from_str("[phones]\nrelay = \" wss://relay.example.test \"\n").unwrap();
    assert_eq!(
        own.phones.hosted_relay().as_deref(),
        Some("wss://relay.example.test")
    );
    // ket's own, only once it is offered.
    for url in [HOSTED_RELAY, "wss://relay.ketapp.dev/"] {
        let ours: Config = toml::from_str(&format!("[phones]\nrelay = \"{url}\"\n")).unwrap();
        assert_eq!(ours.phones.hosted_relay().is_some(), HOSTED_RELAY_OFFERED);
    }
    let blank: Config = toml::from_str("[phones]\nrelay = \"  \"\n").unwrap();
    assert_eq!(blank.phones.hosted_relay(), None);
}
