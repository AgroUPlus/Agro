//! Server state boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn a_guest_cannot_flip_server_wide_plugin_state() {
    let h = harness();
    let r = h
        .run_as(
            &h.guest,
            r#"mutation { togglePlugin(pluginId: "subsonic-navidrome", isEnabled: false) }"#,
        )
        .await;
    assert_forbidden(&r, "togglePlugin");
}

/// `plugins` reports "connected" by reading whichever account owns the first node, so for a guest
/// it answered with the admin's Navidrome address and username.
#[tokio::test]
pub(super) async fn a_guest_cannot_read_the_plugin_registry() {
    let h = harness();
    assert_forbidden(
        &h.run_as(&h.guest, "{ plugins { id isConnected } }").await,
        "plugins",
    );
}

/// The `/listen` allowlist is the only thing between the operator's share domain and an open
/// redirect.
#[tokio::test]
pub(super) async fn a_guest_cannot_widen_the_share_allowlist() {
    let h = harness();
    let r = h
        .run_as(
            &h.guest,
            r#"mutation { updateSyncedSettings(input: {
                 userId: "mallory", shareHosts: "evil.example", shareEnabled: true
               }) { shareHosts } }"#,
        )
        .await;
    assert_forbidden(&r, "share settings");
}

#[tokio::test]
pub(super) async fn a_malformed_share_host_is_refused_even_for_the_admin() {
    let h = harness();
    for bad in [
        "https://evil.example",
        "evil.example/path",
        "*.example.com",
        "localhost",
    ] {
        let r = h
            .run_as(
                &h.admin,
                &format!(
                    r#"mutation {{ updateSyncedSettings(input: {{
                         userId: "alpha", shareHosts: "{bad}"
                       }}) {{ shareHosts }} }}"#
                ),
            )
            .await;
        assert_forbidden(&r, &format!("share host {bad:?}"));
    }
}

// ── Other accounts' devices ─────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn a_guest_cannot_read_or_delete_another_accounts_holdings() {
    let h = harness();
    h.db.upsert_holding("alpha", "admin-desktop", &"a".repeat(64), None)
        .unwrap();

    // Naming their own account but the admin's device: `authorize` passes, and the device check is
    // what has to catch it.
    let read = h
        .run_as(
            &h.guest,
            r#"{ deviceHoldings(userId: "mallory", deviceId: "admin-desktop") }"#,
        )
        .await;
    assert_forbidden(&read, "deviceHoldings with a smuggled device id");

    let forget = h
        .run_as(
            &h.guest,
            &format!(
                r#"mutation {{ forgetHoldings(userId: "mallory", deviceId: "admin-desktop", hashes: ["{}"]) }}"#,
                "a".repeat(64)
            ),
        )
        .await;
    assert_forbidden(&forget, "forgetHoldings with a smuggled device id");
    assert_eq!(
        h.db.device_holding_hashes("alpha", "admin-desktop")
            .unwrap()
            .len(),
        1,
        "the admin's holding was deleted"
    );
}

// ── The library ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn a_guest_cannot_browse_the_library() {
    let h = harness();
    let r = h
        .run_as(
            &h.guest,
            r#"{ libraryBrowse(userId: "mallory", kind: ARTIST) { name } }"#,
        )
        .await;
    assert_forbidden(&r, "libraryBrowse");
}

// ── Fail-closed ─────────────────────────────────────────────────────────────────────────────
