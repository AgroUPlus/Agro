//! Track rows.

use super::*;

#[test]
fn archived_path_survives_a_later_report_without_one() {
    let db = Db::new_in_memory().unwrap();
    let mut t = track("h1", "A", "B", 100_000);
    t.archived_path = Some("A/Album/01 - B.flac".to_string());
    db.upsert_library_track(&t).unwrap();

    t.archived_path = None;
    db.upsert_library_track(&t).unwrap();

    assert_eq!(
        db.library_track("h1")
            .unwrap()
            .unwrap()
            .archived_path
            .as_deref(),
        Some("A/Album/01 - B.flac")
    );
}
