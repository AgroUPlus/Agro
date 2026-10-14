//! Reading the catalogue.

use super::*;

/// A client asks for what it has not seen, and gets it in an order it can resume from.
#[test]
fn the_catalogue_is_readable_from_a_cursor() {
    let db = Db::new_in_memory().unwrap();
    db.publish_recording(&published(&embedding(8), "One", "ytm:8"))
        .unwrap();
    db.publish_recording(&published(&embedding(9), "Two", "ytm:9"))
        .unwrap();

    let all = db.catalog_since(0, 10).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all[0].updated_at <= all[1].updated_at, "oldest first");

    let newest = all.iter().map(|r| r.updated_at).max().unwrap();
    assert!(
        db.catalog_since(newest, 10).unwrap().is_empty(),
        "a caught-up client gets nothing"
    );
}
