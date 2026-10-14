//! Security log boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn a_guest_cannot_read_another_accounts_security_log() {
    let h = harness();
    h.db.record_event(
        crate::audit::Event::LoginSucceeded,
        crate::audit::Record::new().user("alpha").ip("203.0.113.9"),
    );
    let refused = h
        .run_as(&h.guest, r#"{ securityEvents(userId: "alpha") { kind } }"#)
        .await;
    assert_forbidden(&refused, "mallory reading alpha's security log");
}

#[tokio::test]
pub(super) async fn a_guest_can_read_their_own_security_log() {
    let h = harness();
    h.db.record_event(
        crate::audit::Event::LoginSucceeded,
        crate::audit::Record::new()
            .user("mallory")
            .ip("203.0.113.9"),
    );
    let allowed = h
        .run_as(
            &h.guest,
            r#"{ securityEvents(userId: "mallory") { kind clientIp } }"#,
        )
        .await;
    assert_allowed(&allowed, "mallory reading their own security log");
    let rendered = allowed.data.to_string();
    assert!(rendered.contains("login_succeeded"), "{rendered}");
    // Truncated on the way in, so the stored value is the network and not the host.
    assert!(rendered.contains("203.0.113.0/24"), "{rendered}");
    assert!(!rendered.contains("203.0.113.9\""), "{rendered}");
}

/// Omitting `userId` asks for every account's events. That is an administrator's view, and a guest
/// reaching it would get exactly what the per-account check refuses one query earlier.
#[tokio::test]
pub(super) async fn a_guest_cannot_read_the_server_wide_security_log() {
    let h = harness();
    let refused = h.run_as(&h.guest, r#"{ securityEvents { kind } }"#).await;
    assert_forbidden(&refused, "mallory reading the whole server's security log");
}

#[tokio::test]
pub(super) async fn an_admin_can_read_the_server_wide_security_log() {
    let h = harness();
    h.db.record_event(
        crate::audit::Event::LoginFailed,
        crate::audit::Record::new()
            .ip("198.51.100.7")
            .detail("username=ghost"),
    );
    let allowed = h
        .run_as(&h.admin, r#"{ securityEvents { kind detail } }"#)
        .await;
    assert_allowed(&allowed, "alpha reading the server-wide security log");
    assert!(allowed.data.to_string().contains("username=ghost"));
}

#[tokio::test]
pub(super) async fn nobody_can_read_the_security_log_without_an_identity() {
    let h = harness();
    assert_forbidden(
        &h.run_anonymously(r#"{ securityEvents { kind } }"#).await,
        "anonymous server-wide security log",
    );
    assert_forbidden(
        &h.run_anonymously(r#"{ securityEvents(userId: "alpha") { kind } }"#)
            .await,
        "anonymous scoped security log",
    );
}

/// Signing out other devices must be scoped like everything else, or it is a way to sign someone
/// else out of their account.
#[tokio::test]
pub(super) async fn a_guest_cannot_revoke_another_accounts_devices() {
    let h = harness();
    let alpha_token = h.db.mint_device_token("alpha", "laptop").unwrap();
    let refused = h
        .run_as(
            &h.guest,
            r#"mutation { revokeAllDevices(userId: "alpha") }"#,
        )
        .await;
    assert_forbidden(&refused, "mallory revoking alpha's devices");
    assert!(
        h.db.account_for_token(&alpha_token).unwrap().is_some(),
        "alpha's token must survive a refused revocation"
    );
}

// ── Erasure ─────────────────────────────────────────────────────────────────────────────────
//
// "Delete my account" has to mean it. A deletion that leaves the social graph, the listening
// history or a live share link behind is not one, and those are exactly the rows that used to
// survive it.
