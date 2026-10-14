//! Sealed copies boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn a_friend_is_handed_only_the_copy_sealed_to_their_own_device() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[
            Harness::copy_for("beta", "beta-phone", "for-beta-phone"),
            Harness::copy_for("beta", "beta-laptop", "for-beta-laptop"),
            Harness::copy_for("stranger", "stranger-phone", "for-stranger"),
        ]),
    );

    let response = h
        .run_as(
            &h.beta,
            r#"{ friendsNowPlaying(deviceId: "beta-phone") { username encryptedPresence } }"#,
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
    assert!(
        !body.contains("for-stranger"),
        "another account's copy leaked: {body}"
    );
}

#[tokio::test]
pub(super) async fn a_device_with_no_copy_sealed_to_it_is_told_nothing_rather_than_given_someone_elses(
) {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    // A device signed in after the last track change: the registry knows it, nothing is sealed to
    // it yet. The correct answer is the placeholder, not the copy meant for the other phone.
    let response = h
        .run_as(
            &h.beta,
            r#"{ friendsNowPlaying(deviceId: "beta-tablet") { encryptedPresence } }"#,
        )
        .await;
    assert!(
        response.data.to_string().contains("null"),
        "expected no sealed copy: {:?}",
        response.data
    );
}

#[tokio::test]
pub(super) async fn asking_without_naming_a_device_yields_no_sealed_copy() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    // The dashboard holds no device key and asks without a device id. It must get the placeholder
    // rather than a ciphertext it cannot open.
    let response = h
        .run_as(&h.beta, "{ friendsNowPlaying { encryptedPresence } }")
        .await;
    assert!(
        !response.data.to_string().contains("for-beta-phone"),
        "a keyless asker was handed a ciphertext: {:?}",
        response.data
    );
}

#[tokio::test]
pub(super) async fn a_stranger_sees_no_sealed_session_at_all() {
    let h = harness();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for(
            "stranger",
            "stranger-phone",
            "for-stranger",
        )]),
    );

    // Sealing to somebody does not make them a friend. The feed is still the friend list.
    let response = h
        .run_as(
            &h.stranger,
            r#"{ friendsNowPlaying(deviceId: "stranger-phone") { username } }"#,
        )
        .await;
    assert_eq!(response.data.to_string(), r#"{friendsNowPlaying: []}"#);
}

#[tokio::test]
pub(super) async fn sealing_does_not_override_the_now_playing_flag() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.run_as(
        &h.alpha,
        "mutation { setVisibility(showNowPlaying: false) }",
    )
    .await;
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    // Encryption decides what the server can read. It has never decided who may look.
    let response = h
        .run_as(
            &h.beta,
            r#"{ friendsNowPlaying(deviceId: "beta-phone") { username } }"#,
        )
        .await;
    assert_eq!(response.data.to_string(), r#"{friendsNowPlaying: []}"#);
}

#[tokio::test]
pub(super) async fn a_heartbeat_keeps_the_sealed_copies_and_an_unsealed_handoff_clears_them() {
    let h = harness();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    // A heartbeat repeats metadata that has not changed and says nothing about the copies.
    h.set_playing_sealed("alpha", "Placeholder", None);
    assert_eq!(
        h.db.presence_ciphertext_for("alpha", "device-1", "beta", "beta-phone")
            .unwrap()
            .as_deref(),
        Some("for-beta-phone"),
        "a heartbeat erased copies it never mentioned"
    );

    // An empty set is the opposite instruction: this session is over, or is no longer sealed.
    h.set_playing_sealed("alpha", "Ordinary Track", Some(&[]));
    assert_eq!(
        h.db.presence_ciphertext_for("alpha", "device-1", "beta", "beta-phone")
            .unwrap(),
        None,
        "an unsealed handoff left the previous envelope in place"
    );
}

#[tokio::test]
pub(super) async fn unfriending_drops_the_copies_in_both_directions() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );
    h.db.remove_friend("beta", "alpha").unwrap();

    assert_eq!(
        h.db.presence_ciphertext_for("alpha", "device-1", "beta", "beta-phone")
            .unwrap(),
        None,
        "a withdrawn friendship left a sealed session behind"
    );
}

#[tokio::test]
pub(super) async fn withdrawing_a_device_key_drops_what_was_sealed_to_it() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.register_device_key("beta", "beta-phone", "key-beta-phone")
        .unwrap();
    h.set_playing_sealed(
        "alpha",
        "Placeholder",
        Some(&[Harness::copy_for("beta", "beta-phone", "for-beta-phone")]),
    );

    h.db.forget_device_key("beta", "beta-phone").unwrap();

    assert_eq!(
        h.db.presence_ciphertext_for("alpha", "device-1", "beta", "beta-phone")
            .unwrap(),
        None,
        "a copy sealed to a withdrawn key is unopenable and must not be kept"
    );
}
