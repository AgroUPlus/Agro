//! Lyrics and subscriptions boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn lyrics_carry_the_source_that_supplied_them() {
    let h = harness();
    let query = format!(
        r#"mutation {{ publishRecording(
             embedding: "{}", dim: 128, model: "nmfp-triplet", version: 1, durationMs: 210000,
             title: "With Words", lyrics: "a line", lyricsSource: "LRCLIB"
           ) }}"#,
        embedding_hex(30)
    );
    h.run_as(&h.alpha, &query).await;

    let body = h
        .run_as(
            &h.beta,
            "{ catalogSince(since: 0) { lyrics lyricsSource } }",
        )
        .await
        .data
        .to_string();
    assert!(body.contains("a line"), "the lyrics did not trade: {body}");
    assert!(
        body.contains("LRCLIB"),
        "the attribution did not trade with them: {body}"
    );
}

#[tokio::test]
pub(super) async fn attribution_without_lyrics_is_dropped() {
    let h = harness();
    // Naming a source for text that was never sent describes nothing, and would otherwise sit in
    // the catalogue as an attribution for lyrics some later client supplies from somewhere else.
    let query = format!(
        r#"mutation {{ publishRecording(
             embedding: "{}", dim: 128, model: "nmfp-triplet", version: 1, durationMs: 210000,
             title: "No Words", lyricsSource: "LRCLIB"
           ) }}"#,
        embedding_hex(31)
    );
    h.run_as(&h.alpha, &query).await;

    let body = h
        .run_as(&h.alpha, "{ catalogSince(since: 0) { lyricsSource } }")
        .await
        .data
        .to_string();
    assert!(
        !body.contains("LRCLIB"),
        "attribution was kept without any lyrics: {body}"
    );
}

// ── Artist subscriptions ────────────────────────────────────────────────────────────────────

/// A subscription is one account's, the way a friend list is.
///
/// The catalogue above is deliberately shared — that is the point of it — so it is worth pinning
/// that the *subscriptions* over the top of it are not. `authorize` is what enforces this, and it
/// is one line in each resolver; a future refactor that drops it would leave every test above
/// passing.
#[tokio::test]
pub(super) async fn one_account_cannot_read_or_change_anothers_subscriptions() {
    let h = harness();
    h.run_as(
        &h.alpha,
        r#"mutation { subscribeArtist(userId: "alpha", artist: "Boards of Canada") { artistId } }"#,
    )
    .await;

    let readable = h
        .run_as(
            &h.beta,
            r#"{ subscribedArtists(userId: "alpha") { displayName } }"#,
        )
        .await;
    assert!(
        !readable.errors.is_empty(),
        "beta read alpha's subscriptions: {:?}",
        readable.data
    );

    let writable = h
        .run_as(
            &h.beta,
            r#"mutation { subscribeArtist(userId: "alpha", artist: "Autechre") { artistId } }"#,
        )
        .await;
    assert!(
        !writable.errors.is_empty(),
        "beta subscribed on alpha's behalf"
    );

    // And alpha still has exactly what alpha asked for.
    let own = h
        .run_as(
            &h.alpha,
            r#"{ subscribedArtists(userId: "alpha") { displayName } }"#,
        )
        .await;
    let body = own.data.to_string();
    assert!(body.contains("Boards of Canada"), "{body}");
    assert!(
        !body.contains("Autechre"),
        "beta's write landed anyway: {body}"
    );
}

/// Publishing joins a recording to an artist, so a subscriber sees it without anything else running.
#[tokio::test]
pub(super) async fn a_published_recording_reaches_a_subscriber_of_its_artist() {
    let h = harness();
    h.run_as(
        &h.beta,
        r#"mutation { subscribeArtist(userId: "beta", artist: "an artist") { artistId } }"#,
    )
    .await;

    // The shared `publish` helper above files everything under "An Artist" — a different spelling
    // of what beta subscribed to, which is the case the normalisation exists for.
    h.run_as(
        &h.alpha,
        &publish(&embedding_hex(7), "Something New", "ytm:new"),
    )
    .await;

    let response = h
        .run_as(
            &h.beta,
            r#"{ newReleases(userId: "beta", since: 0) { title artist } }"#,
        )
        .await;
    let body = response.data.to_string();
    assert!(
        body.contains("Something New"),
        "a subscriber did not see the release: {body} {:?}",
        response.errors
    );
}
