//! Sessions boundary tests.

use super::*;
use async_graphql::Value;

/// `authorize` used to return `Ok` when nobody was authenticated, to leave room for a first-run
/// window in the middleware. Both halves are gone.
#[tokio::test]
pub(super) async fn an_unauthenticated_request_is_refused_everywhere() {
    let h = harness();
    for query in [
        r#"{ me(username: "alpha") { id } }"#,
        r#"{ users { username } }"#,
        r#"{ playbackHandoff(userId: "alpha") { trackTitle } }"#,
        r#"{ listeningStats(userId: "alpha") { playsTotal } }"#,
        r#"{ agroWrapped(userId: "alpha", year: 2026) { totalPlays } }"#,
        r#"{ replayChartStanding(year: 2026) { percentile } }"#,
        r#"{ circleRecap(year: 2026) { members } }"#,
        r#"{ syncedSettings(userId: "alpha") { serverUrl } }"#,
        r#"mutation { createAccount(username: "x") { passphrase } }"#,
        r#"mutation { togglePlugin(pluginId: "p", isEnabled: true) }"#,
    ] {
        assert_forbidden(&h.run_anonymously(query).await, query);
    }
}

// ── What a guest *can* do ───────────────────────────────────────────────────────────────────

/// The boundary is only correct if it still lets a guest be a guest.
#[tokio::test]
pub(super) async fn a_guest_can_still_use_their_own_account() {
    let h = harness();

    assert_allowed(
        &h.run_as(
            &h.guest,
            r#"{ me(username: "mallory") { id username role } }"#,
        )
        .await,
        "own account",
    );
    assert_allowed(
        &h.run_as(
            &h.guest,
            r#"{ listeningStats(userId: "mallory") { playsTotal } }"#,
        )
        .await,
        "own stats",
    );
    assert_allowed(
        &h.run_as(
            &h.guest,
            r#"{ activeNodes(userId: "mallory") { deviceId } }"#,
        )
        .await,
        "own devices",
    );
    assert_allowed(
        &h.run_as(
            &h.guest,
            r#"mutation { updateSyncedSettings(input: { userId: "mallory", streamFormat: "OPUS" }) { streamFormat } }"#,
        )
        .await,
        "own non-sharing settings",
    );
}

#[tokio::test]
pub(super) async fn a_guest_quota_defaults_to_ten_megabytes_and_the_admin_has_none() {
    let h = harness();
    assert_eq!(
        h.guest.effective_quota(),
        Some(10 * 1024 * 1024),
        "guest quota"
    );
    assert_eq!(h.admin.effective_quota(), None, "admin is uncapped");
}

#[tokio::test]
pub(super) async fn usernames_are_validated() {
    let h = harness();
    for bad in ["", "  ", "has space", "sql'injection", &"x".repeat(64)] {
        let r = h
            .run_as(
                &h.admin,
                &format!(r#"mutation {{ createAccount(username: "{bad}") {{ passphrase }} }}"#),
            )
            .await;
        assert_forbidden(&r, &format!("username {bad:?}"));
    }
}

/// Pairing hands over a device token, and the QR carries that rather than the passphrase —
/// photographing the old one handed over the account permanently.
#[tokio::test]
pub(super) async fn pairing_carries_a_device_token_not_a_passphrase() {
    let h = harness();
    let r = h
        .run_as(
            &h.admin,
            r#"mutation { pairDevice(userId: "alpha", label: "Living room laptop") { token qrData label } }"#,
        )
        .await;
    assert_allowed(&r, "pairDevice");

    let Value::Object(ref data) = r.data else {
        panic!("unexpected shape: {:?}", r.data)
    };
    let Some(Value::Object(payload)) = data.get("pairDevice") else {
        panic!("no payload: {:?}", r.data)
    };
    let text = |key: &str| match payload.get(key) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("{key} was {other:?}"),
    };

    let device_token = text("token");
    let qr = text("qrData");

    assert!(!device_token.is_empty());
    assert_eq!(text("label"), "Living room laptop");
    assert!(
        qr.contains(&device_token),
        "the QR does not carry the device token"
    );
    assert!(
        !qr.contains("alpha-pass"),
        "the QR still carries the passphrase: {qr}"
    );
    assert!(h.db.account_for_token(&device_token).unwrap().is_some());
}
