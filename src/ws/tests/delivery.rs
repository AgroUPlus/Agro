//! Who receives a message.

use super::*;

#[test]
fn a_broadcast_reaches_everyone() {
    assert!(is_for(&msg(None, None), Some("alpha"), Some("phone")));
    assert!(is_for(&msg(None, None), None, None));
}

#[test]
fn another_account_never_sees_it() {
    assert!(!is_for(
        &msg(Some("alpha"), None),
        Some("beta"),
        Some("phone")
    ));
    assert!(is_for(
        &msg(Some("alpha"), None),
        Some("alpha"),
        Some("phone")
    ));
}

#[test]
fn a_targeted_message_reaches_only_that_device() {
    let m = msg(Some("alpha"), Some("phone"));
    assert!(is_for(&m, Some("alpha"), Some("phone")));
    assert!(!is_for(&m, Some("alpha"), Some("laptop")));
    assert!(!is_for(&m, Some("alpha"), None));
}

/// The same-network test is what decides whether a private address is ever handed to another
/// account, so both directions of it are pinned here.
mod same_network {
    use super::*;

    fn hub_with(peers: &[(&str, &str, &str)]) -> WsHub {
        let hub = WsHub::new();
        for (user, device, egress) in peers {
            hub.set_egress_address(user, device, egress);
            hub.set_lan_address(user, device, "192.168.1.50:8702");
        }
        hub
    }

    #[test]
    fn one_public_address_means_one_network() {
        let hub = hub_with(&[
            ("alpha", "phone", "203.0.113.7"),
            ("beta", "laptop", "203.0.113.7"),
        ]);
        assert!(hub.same_network("alpha", "phone", "beta", "laptop"));
        assert!(hub.shares_network_with_user("alpha", "phone", "beta"));
    }

    #[test]
    fn different_public_addresses_mean_different_networks() {
        let hub = hub_with(&[
            ("alpha", "phone", "203.0.113.7"),
            ("beta", "laptop", "198.51.100.4"),
        ]);
        assert!(!hub.same_network("alpha", "phone", "beta", "laptop"));
        assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
    }

    /// A device nobody has heard from has no egress address, and an unknown answer is not a
    /// yes: it falls through to the relay rather than disclosing an address.
    #[test]
    fn an_unseen_device_is_never_on_your_network() {
        let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
        assert!(!hub.same_network("alpha", "phone", "beta", "laptop"));
        assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
    }

    /// IPv6 has no NAT, so two devices on one LAN never share a full address — they share the
    /// prefix their router advertises. Comparing the whole address would answer "no" for every
    /// real pair.
    #[test]
    fn ipv6_is_compared_on_the_routed_prefix() {
        assert!(same_egress("2001:db8:1:2::1000", "2001:db8:1:2::abcd"));
        assert!(!same_egress("2001:db8:1:2::1", "2001:db8:9:9::1"));
    }

    /// Mixing families, or anything unparseable, is refused rather than guessed at.
    #[test]
    fn an_unparseable_address_is_refused() {
        assert!(!same_egress("203.0.113.7", "::ffff:203.0.113.7"));
        assert!(!same_egress("not-an-address", "not-an-address"));
        assert!(!same_egress("", ""));
    }

    /// The token is the gate, so it has to be stable enough to use across a track change and
    /// scoped to the one listener it was minted for.
    #[test]
    fn a_grant_is_reused_for_the_same_pair_and_distinct_per_listener() {
        let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
        let first = hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap();
        let again = hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap();
        let other = hub.grant_p2p_token("alpha", "phone", "gamma", &[]).unwrap();
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    /// The bound key set is part of what a grant *is*, so it cannot be reused across a
    /// change to it. A listener that has just added a phone would otherwise be handed a grant
    /// that cannot seal to it, and one that has just signed a phone out would be handed a
    /// grant that still can.
    #[test]
    fn a_grant_is_reminted_when_the_listeners_keys_change() {
        let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
        let one = vec!["key-one".to_string()];
        let two = vec!["key-one".to_string(), "key-two".to_string()];

        let first = hub.grant_p2p_token("alpha", "phone", "beta", &one).unwrap();
        let same = hub.grant_p2p_token("alpha", "phone", "beta", &one).unwrap();
        assert_eq!(first, same, "an unchanged key set reuses the grant");

        let after = hub.grant_p2p_token("alpha", "phone", "beta", &two).unwrap();
        assert_ne!(first, after, "an added device must force a fresh grant");

        let back = hub.grant_p2p_token("alpha", "phone", "beta", &one).unwrap();
        assert_ne!(after, back, "a removed device must force a fresh grant too");
    }

    /// A device sharing its own account's network is still a match — the same-account case is
    /// the original point of the LAN transfer — but callers that mean "somebody else" filter
    /// the holder out themselves.
    #[test]
    fn your_own_second_device_is_on_your_network() {
        let hub = hub_with(&[
            ("alpha", "phone", "203.0.113.7"),
            ("alpha", "laptop", "203.0.113.7"),
        ]);
        assert!(hub.shares_network_with_user("alpha", "phone", "alpha"));
    }

    /// The device being asked about is never its own peer, or every host would look reachable
    /// from itself.
    #[test]
    fn a_device_is_not_its_own_peer() {
        let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
        assert!(!hub.shares_network_with_user("alpha", "phone", "alpha"));
    }

    /// A disconnect takes the address and every grant issued for that device with it.
    #[test]
    fn disconnecting_clears_the_address_and_the_grants() {
        let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
        let token = hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap();
        hub.clear_lan_address("alpha", "phone");
        assert!(hub.get_lan_address("alpha", "phone").is_none());
        assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
        assert_ne!(
            token,
            hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap(),
            "a grant must not survive the socket it was minted for"
        );
    }
}
