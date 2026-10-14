//! Re-normalisation of existing rows.

use super::*;

/// A normalisation change leaves every existing row behind, and only Rust can catch them up.
///
/// Simulated the way it actually happens: the row was written under an older `norm.rs` whose
/// output differs from today's. Nothing about the file changed, so nothing but the derived
/// columns may change — and a second pass must find nothing left to do, or a scheduled reindex
/// would rewrite the whole index on every run.
#[test]
fn reindex_catches_up_rows_normalised_by_an_older_convention() {
    let db = db_with(&[(
        track(
            "h1",
            "Boards of Canada",
            "Roygbiv (Remastered 2011)",
            200_000,
        ),
        "laptop",
    )]);

    {
        let conn = db.conn.lock_recover();
        conn.execute(
            "UPDATE library_tracks
             SET norm_artist = 'stale', norm_title = 'stale', norm_variants = 'stale'
             WHERE content_hash = 'h1'",
            [],
        )
        .unwrap();
    }

    let first = db.reindex_normalisation(100).unwrap();
    assert_eq!(first.tracks, 1);
    assert!(first.done);

    let expected = recording_key("Boards of Canada", "Roygbiv (Remastered 2011)");
    let conn = db.conn.lock_recover();
    let (artist, title, variants): (String, String, String) = conn
        .query_row(
            "SELECT norm_artist, norm_title, norm_variants FROM library_tracks
             WHERE content_hash = 'h1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(artist, expected.artist);
    assert_eq!(title, expected.title);
    assert_eq!(variants, expected.variants);
    drop(conn);

    let second = db.reindex_normalisation(100).unwrap();
    assert_eq!(second.tracks, 0);
    assert!(second.done);
}

/// The batch bounds the write, and `done` is what says to come back.
#[test]
fn reindex_is_bounded_and_resumable() {
    let db = db_with(&[
        (track("h1", "A", "One", 100_000), "laptop"),
        (track("h2", "B", "Two", 100_000), "laptop"),
    ]);
    {
        let conn = db.conn.lock_recover();
        conn.execute("UPDATE library_tracks SET norm_title = 'stale'", [])
            .unwrap();
    }

    let first = db.reindex_normalisation(1).unwrap();
    assert_eq!(first.tracks, 1);
    assert!(!first.done, "one of two rewritten is not finished");

    let second = db.reindex_normalisation(1).unwrap();
    assert_eq!(second.tracks, 1);
    assert!(second.done);
}
