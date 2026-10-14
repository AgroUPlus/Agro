//! LAN address validation and storage.

use super::*;

/// The regression that made the volatile design worth fixing rather than reverting: an
/// address set once at startup and cleared on a dropped socket never came back, because
/// neither client re-registers. The handshake carrying it is what closes that loop, so these
/// pin the shape of what the handshake is allowed to hand over.
#[test]
fn a_plausible_host_port_is_accepted() {
    for good in [
        "192.168.1.50:8702",
        "10.0.0.5:8702",
        "phone.local:8702",
        "[fe80::1]:8702",
    ] {
        assert!(is_plausible_host_port(good), "should accept {good}");
    }
}

/// The value is handed to another device as somewhere to connect to, so anything carrying a
/// scheme, a path or a credential has to be refused rather than forwarded.
#[test]
fn a_malformed_or_dangerous_address_is_refused() {
    for bad in [
        "",
        "192.168.1.50",   // no port
        "192.168.1.50:",  // empty port
        "192.168.1.50:0", // port zero
        "192.168.1.50:notaport",
        "192.168.1.50:99999",       // will not fit a u16
        ":8702",                    // no host
        "http://192.168.1.50:8702", // a scheme
        "192.168.1.50:8702/steal",  // a path
        "user:pass@10.0.0.1:8702",  // a credential
        // A zone id names an interface on the *sender's* machine, so it means nothing to the
        // peer being told to connect there.
        "[fe80::1%eth0]:8702",
    ] {
        assert!(!is_plausible_host_port(bad), "should refuse {bad:?}");
    }
}

#[test]
fn lan_addresses_are_stored_and_retrieved_in_memory() {
    let hub = WsHub::new();
    assert_eq!(hub.get_lan_address("alpha", "phone"), None);

    hub.set_lan_address("alpha", "phone", "192.168.1.50:8702");
    assert_eq!(
        hub.get_lan_address("alpha", "phone"),
        Some("192.168.1.50:8702".to_string())
    );

    hub.clear_lan_address("alpha", "phone");
    assert_eq!(hub.get_lan_address("alpha", "phone"), None);
}

#[test]
fn lan_addresses_are_scoped_by_user_and_device() {
    let hub = WsHub::new();
    hub.set_lan_address("alpha", "phone", "192.168.1.50:8702");
    hub.set_lan_address("beta", "phone", "10.0.0.5:8702");

    assert_eq!(
        hub.get_lan_address("alpha", "phone"),
        Some("192.168.1.50:8702".to_string())
    );
    assert_eq!(
        hub.get_lan_address("beta", "phone"),
        Some("10.0.0.5:8702".to_string())
    );
    assert_eq!(hub.get_lan_address("alpha", "laptop"), None);
}
