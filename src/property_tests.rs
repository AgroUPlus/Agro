//! Properties that must hold for *every* input, not just the handful a hand-written case picks.
//!
//! Each of these functions sits on a trust boundary or feeds a key the whole sync model depends
//! on: a panic here is a remotely triggerable crash, and a normaliser that is not stable would
//! split one recording into two.

use crate::credentials::{hash_token, secure_eq, token_prefix};
use crate::norm::{normalize_artist, normalize_title, recording_key, variants_of};
use proptest::prelude::*;
use std::net::{Ipv4Addr, Ipv6Addr};

proptest! {
    #[test]
    fn normalisers_never_panic_on_any_text(s in any::<String>()) {
        let _ = recording_key(&s, &s);
        let _ = variants_of(&s);
    }

    #[test]
    fn normalising_twice_changes_nothing(s in any::<String>()) {
        let artist = normalize_artist(&s);
        prop_assert_eq!(normalize_artist(&artist), artist);
        let title = normalize_title(&s);
        prop_assert_eq!(normalize_title(&title), title);
    }

    #[test]
    fn letter_case_never_splits_a_recording(artist in "[a-zA-Z ]{0,24}", title in "[a-zA-Z ]{0,24}") {
        prop_assert_eq!(
            recording_key(&artist.to_lowercase(), &title.to_lowercase()),
            recording_key(&artist.to_uppercase(), &title.to_uppercase())
        );
    }

    #[test]
    fn normalised_text_has_no_stray_whitespace(s in any::<String>()) {
        let t = normalize_title(&s);
        prop_assert_eq!(t.trim(), t.as_str());
        prop_assert!(!t.contains("  "));
    }

    #[test]
    fn an_ipv4_audit_address_loses_its_host_octet(a: u8, b: u8, c: u8, d: u8) {
        let kept = crate::audit::truncate_ip(&Ipv4Addr::new(a, b, c, d).to_string()).unwrap();
        prop_assert_eq!(kept, format!("{a}.{b}.{c}.0/24"));
    }

    #[test]
    fn an_ipv6_audit_address_is_cut_to_its_network(segments in any::<[u16; 8]>()) {
        let addr = Ipv6Addr::from(segments);
        let kept = crate::audit::truncate_ip(&addr.to_string()).unwrap();
        prop_assert!(kept.ends_with("::/64"));
        prop_assert_eq!(kept.matches(':').count(), 5);
    }

    #[test]
    fn audit_truncation_never_panics_and_never_keeps_garbage(s in any::<String>()) {
        if let Some(kept) = crate::audit::truncate_ip(&s) {
            prop_assert!(kept.ends_with("/24") || kept.ends_with("/64"));
        }
    }

    #[test]
    fn secure_eq_agrees_with_plain_equality(a in any::<String>(), b in any::<String>()) {
        prop_assert_eq!(secure_eq(&a, &b), a == b);
        prop_assert!(secure_eq(&a, &a));
    }

    #[test]
    fn token_hashing_ignores_surrounding_whitespace(s in "[!-~]{1,64}") {
        prop_assert_eq!(hash_token(&format!("  {s}\n")), hash_token(&s));
        prop_assert_eq!(hash_token(&s).len(), 64);
    }

    #[test]
    fn the_lookup_prefix_is_never_longer_than_the_token(s in any::<String>()) {
        prop_assert!(token_prefix(&s).chars().count() <= s.chars().count());
    }
}
