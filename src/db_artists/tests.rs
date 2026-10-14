use crate::db::Db;

pub(super) fn db() -> Db {
    Db::new_in_memory().unwrap()
}

#[test]
pub(super) fn one_artist_however_the_name_is_spelled() {
    let db = db();
    let a = db
        .upsert_artist("Tyler, The Creator", None)
        .unwrap()
        .unwrap();
    let b = db
        .upsert_artist("tyler the creator", None)
        .unwrap()
        .unwrap();
    assert_eq!(a, b, "the same person, spelled two ways");
}

#[test]
pub(super) fn the_first_spelling_is_the_one_that_is_shown() {
    let db = db();
    let id = db.upsert_artist("Bjork", None).unwrap().unwrap();
    db.upsert_artist("BJORK", None).unwrap();
    assert_eq!(db.artist_by_id(&id).unwrap().unwrap().display_name, "Bjork");
}

#[test]
pub(super) fn an_external_id_is_filled_in_once_and_never_erased() {
    let db = db();
    let id = db.upsert_artist("Aphex Twin", None).unwrap().unwrap();
    db.upsert_artist("Aphex Twin", Some("ytm:UC123")).unwrap();
    assert_eq!(
        db.artist_by_id(&id)
            .unwrap()
            .unwrap()
            .external_id
            .as_deref(),
        Some("ytm:UC123")
    );

    // A later publish from a source that has no channel must not take it away again.
    db.upsert_artist("Aphex Twin", None).unwrap();
    assert_eq!(
        db.artist_by_id(&id)
            .unwrap()
            .unwrap()
            .external_id
            .as_deref(),
        Some("ytm:UC123")
    );
}

#[test]
pub(super) fn a_nameless_artist_is_not_a_row() {
    let db = db();
    assert_eq!(db.upsert_artist("   ", None).unwrap(), None);
    assert_eq!(db.upsert_artist("", None).unwrap(), None);
}

#[test]
pub(super) fn subscribing_twice_is_one_subscription() {
    let db = db();
    db.subscribe_artist("alpha", "Portishead", None).unwrap();
    db.subscribe_artist("alpha", "portishead", None).unwrap();
    assert_eq!(db.subscribed_artists("alpha").unwrap().len(), 1);
}

#[test]
pub(super) fn subscriptions_belong_to_one_account() {
    let db = db();
    db.subscribe_artist("alpha", "Boards of Canada", None)
        .unwrap();
    assert!(db.subscribed_artists("mallory").unwrap().is_empty());
    assert!(!db.is_subscribed_to("mallory", "Boards of Canada").unwrap());
    assert!(db.is_subscribed_to("alpha", "boards of canada").unwrap());
}

#[test]
pub(super) fn unsubscribing_reports_whether_there_was_anything_to_remove() {
    let db = db();
    let artist = db
        .subscribe_artist("alpha", "Massive Attack", None)
        .unwrap()
        .unwrap();
    assert!(db.unsubscribe_artist("alpha", &artist.artist_id).unwrap());
    assert!(!db.unsubscribe_artist("alpha", &artist.artist_id).unwrap());
    assert!(db.subscribed_artists("alpha").unwrap().is_empty());
}
