//! Publishing boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn what_one_account_publishes_another_can_read() {
    let h = harness();
    h.run_as(&h.alpha, &publish(&embedding_hex(1), "Memories", "ytm:aaa"))
        .await;

    // The whole point of a shared catalogue, and the thing that would silently stop working if a
    // future change scoped it to the caller.
    let response = h
        .run_as(&h.beta, "{ catalogSince(since: 0) { title sources } }")
        .await;
    let body = response.data.to_string();
    assert!(
        body.contains("Memories"),
        "beta could not read alpha's entry: {body}"
    );
    assert!(
        body.contains("ytm:aaa"),
        "the source was not shared: {body}"
    );
}

#[tokio::test]
pub(super) async fn a_source_published_by_one_account_resolves_for_another() {
    let h = harness();
    h.run_as(&h.alpha, &publish(&embedding_hex(2), "Memories", "ytm:bbb"))
        .await;

    let response = h
        .run_as(&h.beta, r#"{ recordingForSource(sourceUri: "ytm:bbb") }"#)
        .await;
    assert!(
        !response.data.to_string().contains("null"),
        "a shared source did not resolve: {:?}",
        response.data
    );
}

// ── What is withheld ────────────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn a_local_source_id_is_never_published() {
    let h = harness();
    // A `local:` id is a path on the publisher's device. The server drops it rather than trusting
    // the client not to send one — an older or modified client would.
    h.run_as(
        &h.alpha,
        &publish(
            &embedding_hex(3),
            "Memories",
            "local:/storage/emulated/0/Music/x.flac",
        ),
    )
    .await;

    let response = h
        .run_as(&h.beta, "{ catalogSince(since: 0) { title sources } }")
        .await;
    let body = response.data.to_string();
    assert!(
        body.contains("Memories"),
        "the recording itself should be shared: {body}"
    );
    assert!(
        !body.contains("storage/emulated"),
        "a filesystem path reached another account: {body}"
    );
}

// ── What is refused ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn an_embedding_from_another_model_is_refused_rather_than_stored() {
    let h = harness();
    let query = format!(
        r#"mutation {{ publishRecording(
             embedding: "{}", dim: 128, model: "", version: 1, durationMs: 210000
           ) }}"#,
        embedding_hex(4)
    );
    let response = h.run_as(&h.alpha, &query).await;
    assert!(!response.errors.is_empty(), "an unnamed model was accepted");
}

#[tokio::test]
pub(super) async fn a_blob_that_is_not_whole_vectors_is_refused() {
    let h = harness();
    // One byte short of a whole vector: a client that changed `dim` without changing its packing.
    let mut truncated = embedding_hex(5);
    truncated.truncate(truncated.len() - 2);
    let response = h
        .run_as(&h.alpha, &publish(&truncated, "Broken", "ytm:x"))
        .await;
    assert!(!response.errors.is_empty(), "a partial vector was accepted");
}

#[tokio::test]
pub(super) async fn an_oversized_embedding_is_refused() {
    let h = harness();
    // Longer than any recording: 128 bytes per vector, past the twenty-minute cap.
    let huge = "00".repeat(128 * 2 * 60 * 21);
    let response = h
        .run_as(&h.alpha, &publish(&huge, "Too long", "ytm:y"))
        .await;
    assert!(
        !response.errors.is_empty(),
        "an oversized embedding was accepted"
    );
}

#[tokio::test]
pub(super) async fn a_nonsense_dimension_is_refused() {
    let h = harness();
    let query = format!(
        r#"mutation {{ publishRecording(
             embedding: "{}", dim: 0, model: "nmfp-triplet", version: 1, durationMs: 210000
           ) }}"#,
        embedding_hex(6)
    );
    let response = h.run_as(&h.alpha, &query).await;
    assert!(!response.errors.is_empty(), "dim 0 was accepted");
}

// ── Merging, through the resolvers rather than the database ─────────────────────────────────

#[tokio::test]
pub(super) async fn two_accounts_publishing_one_recording_produce_one_entry() {
    let h = harness();
    let audio = embedding_hex(7);
    h.run_as(&h.alpha, &publish(&audio, "The Real Title", "ytm:aaa"))
        .await;
    h.run_as(&h.beta, &publish(&audio, "track01", "navidrome:bbb"))
        .await;

    let response = h
        .run_as(&h.alpha, "{ catalogSince(since: 0) { title sources } }")
        .await;
    let body = response.data.to_string();
    assert_eq!(body.matches("recordingId").count(), 0);
    assert!(
        body.contains("The Real Title"),
        "the first title should stand: {body}"
    );
    assert!(
        !body.contains("track01"),
        "a worse title overwrote a better one: {body}"
    );
    assert!(
        body.contains("ytm:aaa") && body.contains("navidrome:bbb"),
        "sources did not merge: {body}"
    );
}

// ── Publishing a batch ──────────────────────────────────────────────────────────────────────
