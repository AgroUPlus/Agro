//! The exact fields Wanda selects, asserted against the built schema.
//!
//! Both clients name these in a hand-written selection set rather than through generated bindings,
//! so a rename here is not a compile error anywhere — it is a query that fails at runtime, on a
//! device, against a server that has already shipped. `async-graphql` camel-cases the Rust names,
//! which is the other half of what these pin: `peer_lan_address` reaching the wire as anything but
//! `peerLanAddress` breaks the direct-transfer tier silently.

use super::*;

async fn assert_selects(query: &str) {
    let h = harness();
    let response = h.run_as(&h.alpha, query).await;
    // A resolver may legitimately answer null here; an *unknown field* is the failure this
    // catches, and that arrives as an error rather than as data.
    assert!(
        response.errors.is_empty(),
        "selection was rejected by the schema: {:?}",
        response.errors
    );
}

#[tokio::test]
async fn listen_along_now_playing_selects_the_direct_transfer_fields() {
    assert_selects(
        "{ listenAlong { host listeners nowPlaying {
             username trackUri trackTitle artistName albumName artworkUrl positionMs
             isPlaying updatedAt deviceId contentHash peerLanAddress peerLanToken
         } } }",
    )
    .await;
}

/// Wanda's `AgroJamRecapApi.recaps` selection, field for field.
#[tokio::test]
async fn jam_recaps_select_what_wanda_reads() {
    let track = "title artist artworkUrl trackUri addedBy durationMs approvals skipVotes";
    assert_selects(&format!(
        "{{ jamRecaps {{ id createdAt recap {{
             startedAt endedAt durationMs people tracksOmitted
             tracks {{ {track} }}
             topContributor {{ username tracks }}
             mostLoved {{ {track} }}
             mostSkipped {{ {track} }}
         }} }} }}"
    ))
    .await;
}

#[tokio::test]
async fn friends_now_playing_selects_the_same_fields() {
    assert_selects(
        "{ friendsNowPlaying {
             username trackTitle artistName positionMs isPlaying updatedAt
             deviceId contentHash peerLanAddress peerLanToken
         } }",
    )
    .await;
}

#[tokio::test]
async fn jam_now_playing_selects_the_holder_and_the_route() {
    assert_selects(
        "{ jam { id nowPlaying {
             trackId title artist artworkUrl durationMs positionMs
             skipVotes skipsNeeded youSkipped
             addedBy deviceId contentHash peerLanAddress peerLanToken
         } } }",
    )
    .await;
}

/// The handoff and jam-queue mutations are the other direction: what the clients *send*.
#[tokio::test]
async fn the_handoff_input_accepts_a_content_hash() {
    assert_selects(
        r#"mutation { updateHandoff(input: {
             userId: "alpha", trackUri: "u", trackTitle: "t", artistName: "a",
             positionMs: 0, isPlaying: true, deviceId: "phone", contentHash: "abc123"
         }) }"#,
    )
    .await;
}

/// Queueing names the device and the bytes, which is what lets the rest of the room play the
/// queueing member's own copy instead of each hunting for the track by name.
#[tokio::test]
async fn the_jam_queue_mutation_accepts_a_device_and_a_hash() {
    let h = harness();
    // Not in a jam, so this is refused on those grounds — but an unknown *argument* is caught
    // by the schema before any resolver runs, which is what this is checking.
    let response = h
        .run_as(
            &h.alpha,
            r#"mutation { addJamTrack(
                 trackUri: "u", title: "t", artist: "a",
                 deviceId: "phone", contentHash: "abc123"
               ) { id } }"#,
        )
        .await;
    let unknown_argument = response
        .errors
        .iter()
        .any(|e| e.message.contains("Unknown argument"));
    assert!(
        !unknown_argument,
        "the mutation rejected an argument the client sends: {:?}",
        response.errors
    );
}
