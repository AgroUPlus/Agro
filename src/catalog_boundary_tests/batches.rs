//! Batches boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn a_batch_files_every_entry_it_carries() {
    let h = harness();
    let query = format!(
        "mutation {{ publishRecordings(entries: [{}, {}]) {{ recordingId error }} }}",
        entry(&embedding_hex(20), "First", "ytm:one"),
        entry(&embedding_hex(21), "Second", "ytm:two")
    );
    let response = h.run_as(&h.alpha, &query).await;
    assert!(
        response.errors.is_empty(),
        "the batch failed: {:?}",
        response.errors
    );

    let body = response.data.to_string();
    assert_eq!(
        body.matches("recordingId").count(),
        2,
        "not one result per entry: {body}"
    );
    assert_eq!(
        body.matches("error: null").count(),
        2,
        "an entry reported a failure: {body}"
    );

    // Both are in the catalogue, and readable by the other account like anything else published.
    let read = h
        .run_as(&h.beta, "{ catalogSince(since: 0) { title } }")
        .await
        .data
        .to_string();
    assert!(
        read.contains("First") && read.contains("Second"),
        "the batch did not land: {read}"
    );
}

#[tokio::test]
pub(super) async fn one_bad_entry_does_not_cost_the_good_ones() {
    let h = harness();
    // The middle entry names no model, which `publishRecording` refuses outright. In a batch that
    // must cost the client that entry alone — losing the other two would mean one malformed row
    // could stop a whole library from ever syncing.
    let malformed_entry = format!(
        r#"{{ embedding: "{}", dim: 128, model: "", version: 1, durationMs: 210000 }}"#,
        embedding_hex(23)
    );
    let query = format!(
        "mutation {{ publishRecordings(entries: [{}, {}, {}]) {{ recordingId error }} }}",
        entry(&embedding_hex(22), "Good", "ytm:good"),
        malformed_entry,
        entry(&embedding_hex(24), "Also good", "ytm:alsogood")
    );
    let response = h.run_as(&h.alpha, &query).await;
    assert!(
        response.errors.is_empty(),
        "one bad entry failed the whole request: {:?}",
        response.errors
    );

    let read = h
        .run_as(&h.alpha, "{ catalogSince(since: 0) { title } }")
        .await
        .data
        .to_string();
    assert!(
        read.contains("Good"),
        "the first good entry was lost: {read}"
    );
    assert!(
        read.contains("Also good"),
        "the entry after the bad one was lost: {read}"
    );
}

#[tokio::test]
pub(super) async fn a_batch_larger_than_the_cap_is_refused() {
    let h = harness();
    // The entries are deliberately trivial: the cap is checked before anything is validated or
    // matched, so this asserts the refusal without building a megabyte of embeddings to be refused.
    let entries: Vec<String> = (0..26).map(|_| entry("00", "Bulk", "ytm:bulk")).collect();
    let query = format!(
        "mutation {{ publishRecordings(entries: [{}]) {{ recordingId }} }}",
        entries.join(", ")
    );
    let response = h.run_as(&h.alpha, &query).await;
    assert!(
        !response.errors.is_empty(),
        "a batch past the cap was accepted"
    );
}
