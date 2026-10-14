//! Library browsing.

use super::*;

#[test]
fn browsing_without_a_device_lists_everything_as_present() {
    // Regression: the "all devices" query does not mention `:device`, and binding it anyway
    // made rusqlite reject the statement — so the default view of the library, the one you get
    // before choosing anything, was the one that could not run.
    let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
    let items = db
        .library_browse("alpha", None, BrowseKind::Album, None, 50, 0, true)
        .expect("a browse with no device selected must work");
    assert_eq!(items.len(), 1);
    assert!(items[0].present_on_device, "nothing to be missing from");
    assert!(items[0].cover_key.is_some(), "an album can carry artwork");
}

#[test]
fn browsing_greys_out_what_the_chosen_device_lacks() {
    let db = db_with(&[
        (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
        (track("h2", "Pixies", "Debaser", 172_000), "phone"),
    ]);
    let items = db
        .library_browse("alpha", Some("phone"), BrowseKind::Track, None, 50, 0, true)
        .unwrap();
    let missing: Vec<&str> = items
        .iter()
        .filter(|item| !item.present_on_device)
        .map(|item| item.title.as_str())
        .collect();
    assert_eq!(
        missing,
        vec!["Come As You Are"],
        "only the laptop has that one"
    );
}

#[test]
fn browsing_by_artist_has_no_cover_to_point_at() {
    let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
    let items = db
        .library_browse("alpha", None, BrowseKind::Artist, None, 50, 0, true)
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(
        items[0].cover_key.is_none(),
        "an artist has no one album to borrow art from"
    );
}

#[test]
fn browse_search_treats_wildcards_as_literal_text() {
    let db = db_with(&[
        (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
        (track("h2", "Artist", "100% Real", 100_000), "laptop"),
    ]);
    let items = db
        .library_browse("alpha", None, BrowseKind::Track, Some("%"), 50, 0, true)
        .unwrap();
    assert_eq!(
        items.len(),
        1,
        "`%` must match the track with a percent sign, not everything"
    );
    assert_eq!(items[0].title, "100% Real");
}
