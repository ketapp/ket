//! Which of this Mac's addresses is its tailnet's.

use super::super::tailnet_ip_from;

#[test]
fn picks_tailnet_ipv4_from_interface_addresses() {
    use std::net::Ipv4Addr;

    let addresses = [
        Ipv4Addr::new(192, 168, 1, 4),
        Ipv4Addr::new(100, 63, 255, 255),
        Ipv4Addr::new(100, 64, 0, 1),
        Ipv4Addr::new(100, 127, 255, 254),
    ];
    assert_eq!(
        tailnet_ip_from(addresses),
        Some(Ipv4Addr::new(100, 64, 0, 1))
    );
    assert_eq!(
        tailnet_ip_from([Ipv4Addr::new(100, 127, 255, 254)]),
        Some(Ipv4Addr::new(100, 127, 255, 254))
    );
    assert_eq!(tailnet_ip_from([Ipv4Addr::new(100, 128, 0, 1)]), None);
}
