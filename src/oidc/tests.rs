use super::flow::*;
use super::*;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub(super) fn clear_env() {
    for key in [
        "AGRO_OIDC_ISSUER",
        "AGRO_OIDC_CLIENT_ID",
        "AGRO_OIDC_CLIENT_SECRET",
        "AGRO_OIDC_REDIRECT_URI",
    ] {
        std::env::remove_var(key);
    }
}

/// A half-configured provider must read as absent. The alternative is a button that appears and
/// then fails, which is worse than no button.
#[test]
pub(super) fn a_partial_configuration_is_no_configuration() {
    clear_env();
    std::env::set_var("AGRO_OIDC_ISSUER", "https://id.example.com");
    std::env::set_var("AGRO_OIDC_CLIENT_ID", "agro");
    assert!(Config::from_env().is_none());
    clear_env();
}

#[test]
pub(super) fn a_flow_is_single_use() {
    let store = FlowStore::new();
    store.insert(
        "state-one".into(),
        PendingFlow {
            started: Instant::now(),
            nonce: "n".into(),
            pkce_verifier: "v".into(),
            link_to: None,
        },
    );
    assert!(store.take("state-one").is_some());
    assert!(
        store.take("state-one").is_none(),
        "a redeemed state must not work twice"
    );
}

#[test]
pub(super) fn an_expired_flow_is_refused() {
    let store = FlowStore::new();
    store.insert(
        "old".into(),
        PendingFlow {
            started: Instant::now() - FLOW_TTL - Duration::from_secs(1),
            nonce: "n".into(),
            pkce_verifier: "v".into(),
            link_to: None,
        },
    );
    assert!(store.take("old").is_none());
}

#[test]
pub(super) fn an_unknown_state_is_refused() {
    assert!(FlowStore::new().take("never-issued").is_none());
}

/// The PKCE challenge is the SHA-256 of the verifier, base64url with no padding.
#[test]
pub(super) fn the_pkce_challenge_is_the_hash_of_the_verifier() {
    // RFC 7636 Appendix B.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = base64url(Sha256::digest(verifier.as_bytes()).as_slice());
    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[test]
pub(super) fn random_tokens_are_url_safe_and_unique() {
    let a = random_token();
    let b = random_token();
    assert_ne!(a, b);
    assert!(a
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
}

/// A provider's error text must not be able to carry markup into the page.
#[test]
pub(super) fn a_failure_message_is_encoded_into_the_fragment() {
    let response = fail("<script>alert(1)</script>");
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(!location.contains("<script>"), "{location}");
    assert!(location.starts_with("/#ssoError="));
}
