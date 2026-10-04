//! Synced settings keep their sealed blob opaque.

use crate::db::*;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

fn share() -> ShareSettingsInput<'static> {
    ShareSettingsInput {
        domain: None,
        hosts: None,
        enabled: None,
    }
}

/// The blob goes in and comes out unchanged, and nothing on the way tries to interpret it.
#[test]
fn a_settings_blob_round_trips_untouched() {
    let db = db();
    let sealed = "6e6f742d612d75726c-deadbeef";
    db.upsert_synced_settings(
        "alpha",
        Some(sealed),
        Some(true),
        Some(true),
        Some("FLAC"),
        share(),
    )
    .unwrap();

    let got = db.get_synced_settings("alpha").unwrap().unwrap();
    assert_eq!(got.settings_blob.as_deref(), Some(sealed));
    assert!(got.has_server_url);
}

/// What a stolen database would actually show. The sealed bytes are the only trace of the
/// address, and the old plaintext columns are never written to again.
#[test]
fn no_readable_address_reaches_the_table() {
    let db = db();
    db.upsert_synced_settings(
        "alpha",
        Some("0ff1ce-sealed-bytes"),
        Some(true),
        None,
        None,
        share(),
    )
    .unwrap();

    let conn = db.conn.lock().unwrap();
    let (blob, url, user): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT settings_blob, server_url, server_username FROM synced_settings",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(blob, "0ff1ce-sealed-bytes");
    assert_eq!(url, None, "the legacy plaintext column must stay empty");
    assert_eq!(user, None);
}

/// A partial update — toggling a preference — must not blank the sealed settings.
#[test]
fn updating_a_preference_leaves_the_blob_alone() {
    let db = db();
    db.upsert_synced_settings(
        "alpha",
        Some("sealed"),
        Some(true),
        Some(true),
        None,
        share(),
    )
    .unwrap();
    db.upsert_synced_settings("alpha", None, None, Some(false), None, share())
        .unwrap();

    let got = db.get_synced_settings("alpha").unwrap().unwrap();
    assert_eq!(got.settings_blob.as_deref(), Some("sealed"));
    assert_eq!(got.lyrics_fetch_online, Some(false));
    assert!(got.has_server_url, "and the flag is not reset either");
}

/// Clearing the address has to be expressible, or `syncMode` would be stuck reporting
/// Navidrome for ever once it had been set.
#[test]
fn the_server_url_flag_can_be_turned_off() {
    let db = db();
    db.upsert_synced_settings("alpha", Some("sealed"), Some(true), None, None, share())
        .unwrap();
    db.upsert_synced_settings("alpha", Some("resealed"), Some(false), None, None, share())
        .unwrap();

    assert!(
        !db.get_synced_settings("alpha")
            .unwrap()
            .unwrap()
            .has_server_url
    );
}
