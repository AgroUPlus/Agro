//! Sealed friend list boundary tests.

use super::*;
use crate::ws::WsHub;

#[tokio::test]
pub(super) async fn a_handoff_may_not_carry_unbounded_sealed_bytes() {
    let h = harness();
    let huge = "A".repeat(9000);
    let response = h
        .run_as(
            &h.alpha,
            &format!(
                r#"mutation {{ updateHandoff(input: {{
                     userId: "alpha", trackUri: "u", trackTitle: "t", artistName: "a",
                     positionMs: 0, isPlaying: true, deviceId: "device-1",
                     encryptedPayload: "{huge}"
                   }}) }}"#
            ),
        )
        .await;
    assert!(
        !response.errors.is_empty(),
        "an oversized sealed payload was accepted"
    );
}

#[tokio::test]
pub(super) async fn the_friend_list_carries_the_sealed_copy_for_the_asking_device() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[
            Harness::copy_for("beta", "beta-phone", "for-beta-phone"),
            Harness::copy_for("beta", "beta-laptop", "for-beta-laptop"),
        ]),
    );

    // This query seeds the presence feed on a graph refresh. Without the copy it seeds a
    // placeholder, and a sealed friend reads as private until a socket frame replaces it.
    let response = h
        .run_as(
            &h.beta,
            r#"{ friends(deviceId: "beta-phone") { nowPlaying { encryptedPresence } } }"#,
        )
        .await;
    let body = response.data.to_string();
    assert!(
        body.contains("for-beta-phone"),
        "the device's own copy: {body}"
    );
    assert!(
        !body.contains("for-beta-laptop"),
        "another device's copy leaked: {body}"
    );
}

#[tokio::test]
pub(super) async fn the_friend_list_without_a_device_yields_no_sealed_copy() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    let response = h
        .run_as(&h.beta, "{ friends { nowPlaying { encryptedPresence } } }")
        .await;
    assert!(
        !response.data.to_string().contains("for-beta-phone"),
        "a keyless asker was handed a ciphertext: {:?}",
        response.data
    );
}

#[tokio::test]
pub(super) async fn the_friend_list_still_refuses_a_friend_who_has_not_opted_in() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    // Naming a device must not become a way around the visibility flag.
    let response = h
        .run_as(
            &h.beta,
            r#"{ friends(deviceId: "beta-phone") { nowPlaying { encryptedPresence } } }"#,
        )
        .await;
    let body = response.data.to_string();
    assert!(
        !body.contains("for-beta-phone"),
        "the flag was bypassed: {body}"
    );
}

#[tokio::test]
pub(super) async fn a_listener_is_pushed_the_sealed_copy_addressed_to_them() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.db.set_listen_along("beta", "alpha").unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[
            Harness::copy_for("beta", "beta-phone", "for-beta-phone"),
            Harness::copy_for("stranger", "stranger-phone", "for-stranger"),
        ]),
    );

    crate::schema_social::fan_out_presence(&h.db, &hub, "alpha");

    // A listener follows the session rather than watching it: without the copy they can name
    // neither the track nor the file, because the plaintext columns hold neither once sealed.
    let frames = hub.replay_after(0, Some("beta"), None).expect("in buffer");
    let listen_along = frames
        .iter()
        .find(|f| f.msg_type == "LISTEN_ALONG")
        .expect("the listener was sent no LISTEN_ALONG frame");
    let body = listen_along.payload.to_string();
    assert!(
        body.contains("for-beta-phone"),
        "the listener's own copy: {body}"
    );
    assert!(
        !body.contains("for-stranger"),
        "another account's copy leaked: {body}"
    );
}

#[tokio::test]
pub(super) async fn a_presence_frame_carries_only_the_recipients_own_copy() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[
            Harness::copy_for("beta", "beta-phone", "for-beta-phone"),
            Harness::copy_for("stranger", "stranger-phone", "for-stranger"),
        ]),
    );

    crate::schema_social::fan_out_presence(&h.db, &hub, "alpha");

    // Sent per friend rather than broadcast. A single fan-out would hand every friend the first
    // one's ciphertext — bytes none of the rest can open.
    let frames = hub.replay_after(0, Some("beta"), None).expect("in buffer");
    let presence = frames
        .iter()
        .find(|f| f.msg_type == "FRIEND_PRESENCE")
        .expect("no FRIEND_PRESENCE frame");
    let body = presence.payload.to_string();
    assert!(
        body.contains("for-beta-phone"),
        "the friend's own copy: {body}"
    );
    assert!(
        !body.contains("for-stranger"),
        "another account's copy leaked: {body}"
    );
}

/// What a stolen database would show of a sealed session.
///
/// The claim the whole feature makes, asserted against the rows rather than the API: the plaintext
/// columns hold a placeholder, the envelopes are opaque, and no copy contains the track.
#[tokio::test]
pub(super) async fn a_stolen_database_shows_no_trace_of_a_sealed_session() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Private Session",
        Some(&[Harness::copy_for(
            "beta",
            "beta-phone",
            "opaque-bytes-for-beta",
        )]),
    );

    let handoff = h.db.get_handoff("alpha").unwrap().expect("a session");
    assert_eq!(handoff.track_title, "Private Session");
    assert!(
        !handoff.track_title.contains("Kid A") && !handoff.artist_name.contains("Radiohead"),
        "the real track reached a plaintext column"
    );

    let copies = h.db.presence_ciphertexts_to("alpha", "beta").unwrap();
    assert_eq!(copies.len(), 1, "one copy per friend device");
    assert_eq!(copies[0].recipient_device_id, "beta-phone");
    assert_eq!(copies[0].ciphertext, "opaque-bytes-for-beta");

    // And nothing was sealed to anyone who was not asked for.
    assert!(
        h.db.presence_ciphertexts_to("alpha", "stranger")
            .unwrap()
            .is_empty(),
        "a copy exists for an account that is not a friend"
    );
}
