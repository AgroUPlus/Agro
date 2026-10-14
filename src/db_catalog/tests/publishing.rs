//! Publishing and merging recordings.

use super::*;

#[test]
fn a_first_publish_creates_a_recording() {
    let db = Db::new_in_memory().unwrap();
    let id = db
        .publish_recording(&published(&embedding(1), "Memories", "ytm:aaa"))
        .unwrap();
    assert!(!id.is_empty());
    assert_eq!(
        db.recording_for_source("ytm:aaa").unwrap().as_deref(),
        Some(id.as_str())
    );
}

/// The point of the whole catalogue: two encodings of one performance become one recording.
#[test]
fn a_re_encode_merges_into_the_recording_it_matches() {
    let db = Db::new_in_memory().unwrap();
    let original = embedding(2);
    let first = db
        .publish_recording(&published(&original, "Memories", "ytm:aaa"))
        .unwrap();
    let second = db
        .publish_recording(&published(
            &degraded(&original),
            "Memories",
            "navidrome:bbb",
        ))
        .unwrap();

    assert_eq!(first, second, "a re-encode became a second recording");
    assert_eq!(db.catalog_since(0, 10).unwrap().len(), 1);
    assert_eq!(
        db.sources_for_recording(&first).unwrap(),
        vec!["navidrome:bbb".to_string(), "ytm:aaa".to_string()]
    );
}

#[test]
fn unrelated_audio_does_not_merge() {
    let db = Db::new_in_memory().unwrap();
    let first = db
        .publish_recording(&published(&embedding(3), "One", "ytm:one"))
        .unwrap();
    let second = db
        .publish_recording(&published(&embedding(4), "Two", "ytm:two"))
        .unwrap();

    assert_ne!(first, second);
    assert_eq!(db.catalog_since(0, 10).unwrap().len(), 2);
}

/// A vector from another embedder is a different alphabet: comparing across them would be
/// confident nonsense, so it never becomes a candidate.
#[test]
fn a_different_model_never_matches() {
    let db = Db::new_in_memory().unwrap();
    let audio = embedding(5);
    let first = db
        .publish_recording(&published(&audio, "One", "ytm:a"))
        .unwrap();

    let mut other = published(&audio, "One", "ytm:b");
    other.model = "some-other-embedder".to_string();
    let second = db.publish_recording(&other).unwrap();

    assert_ne!(first, second, "the same audio merged across two models");
}

#[test]
fn a_different_duration_never_matches() {
    let db = Db::new_in_memory().unwrap();
    let audio = embedding(6);
    let first = db
        .publish_recording(&published(&audio, "One", "ytm:a"))
        .unwrap();

    let mut longer = published(&audio, "One", "ytm:b");
    longer.duration_ms = 210_000 + 30_000;
    let second = db.publish_recording(&longer).unwrap();

    assert_ne!(first, second, "audio 30 s longer merged anyway");
}

#[test]
fn metadata_is_filled_in_and_never_overwritten() {
    let db = Db::new_in_memory().unwrap();
    let audio = embedding(7);
    db.publish_recording(&published(&audio, "The Real Title", "ytm:a"))
        .unwrap();

    let mut worse = published(&degraded(&audio), "track01", "ytm:b");
    worse.album = Some("An Album".to_string());
    db.publish_recording(&worse).unwrap();

    let entry = &db.catalog_since(0, 10).unwrap()[0];
    assert_eq!(
        entry.title.as_deref(),
        Some("The Real Title"),
        "title overwritten"
    );
    assert_eq!(
        entry.album.as_deref(),
        Some("An Album"),
        "album not filled in"
    );
}

#[test]
fn an_empty_embedding_is_ignored_rather_than_stored() {
    let db = Db::new_in_memory().unwrap();
    let id = db
        .publish_recording(&PublishedRecording {
            embedding: Vec::new(),
            dim: DIM as i64,
            model: "nmfp-triplet".to_string(),
            version: 1,
            duration_ms: 0,
            title: None,
            artist: None,
            album: None,
            lyrics: None,
            lyrics_source: None,
            source_uri: Some("ytm:empty".to_string()),
        })
        .unwrap();
    assert!(id.is_empty());
    assert!(db.catalog_since(0, 10).unwrap().is_empty());
}

#[test]
fn publishing_lyrics_persists_and_returns_in_catalog_since() {
    let db = Db::new_in_memory().unwrap();
    let mut p = published(&embedding(1), "Song With Lyrics", "ytm:lyrics_test");
    p.lyrics = Some("[00:10.00] Test line".to_string());
    let id = db.publish_recording(&p).unwrap();
    assert!(!id.is_empty());

    let entries = db.catalog_since(0, 10).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].lyrics.as_deref(), Some("[00:10.00] Test line"));
}
