//! Playlist items and revision-checked edits, at the database layer.

#![cfg(test)]

use crate::db::Db;
use crate::db_playlist_edits::{EditError, PlaylistEdit};
use crate::db_playlist_items::NewPlaylistItem;
use crate::playlist_access::EditAccess;
use crate::playlist_visibility::PlaylistVisibility;

fn track(title: &str) -> NewPlaylistItem {
    NewPlaylistItem {
        title: title.to_string(),
        artist: "Artist".to_string(),
        album: None,
        duration_ms: Some(1000),
        artwork_url: None,
        origin_uri: None,
    }
}

fn add(title: &str) -> PlaylistEdit {
    PlaylistEdit::Add {
        track: track(title),
        after_item_id: None,
    }
}

fn titles(db: &Db, id: &str) -> Vec<String> {
    db.get_playlist_items(id)
        .unwrap()
        .into_iter()
        .map(|it| it.title)
        .collect()
}

fn revision(db: &Db, id: &str) -> i64 {
    db.get_playlist(id).unwrap().unwrap().revision
}

/// A public playlist alpha owns, open to edits at `access`, with beta as alpha's friend.
fn collaborative(access: EditAccess) -> (Db, String) {
    let db = Db::new_in_memory().unwrap();
    assert!(db.send_friend_request("beta", "alpha").unwrap());
    assert!(db.accept_friend_request("alpha", "beta").unwrap());
    let pl = db
        .create_playlist("alpha", "Mix", None, PlaylistVisibility::Public)
        .unwrap();
    db.update_playlist_edit_access(&pl.id, "alpha", access)
        .unwrap();
    (db, pl.id)
}

#[test]
fn playlist_lifecycle_and_items() {
    let db = Db::new_in_memory().unwrap();
    let pl = db
        .create_playlist(
            "alpha",
            "Road Trip",
            Some("Summer bops"),
            PlaylistVisibility::Private,
        )
        .unwrap();
    assert_eq!(pl.revision, 0);

    let first = db
        .add_playlist_item(&pl.id, "alpha", track("Get Lucky"))
        .unwrap();
    let second = db
        .add_playlist_item(&pl.id, "alpha", track("Instant Crush"))
        .unwrap();
    assert_eq!((first.position, second.position), (0, 1));
    assert_eq!(first.added_by.as_deref(), Some("alpha"));
    assert_eq!(revision(&db, &pl.id), 2);

    assert!(db.remove_playlist_item(&pl.id, &first.id).unwrap());
    let left = db.get_playlist_items(&pl.id).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(
        (left[0].id.as_str(), left[0].position),
        (second.id.as_str(), 0)
    );
    assert_eq!(revision(&db, &pl.id), 3);

    assert!(db
        .update_playlist_visibility(&pl.id, "alpha", PlaylistVisibility::Public)
        .unwrap());
    assert_eq!(revision(&db, &pl.id), 4);
    assert_eq!(db.list_public_playlists().unwrap()[0].id, pl.id);

    assert!(db.delete_playlist(&pl.id, "alpha").unwrap());
    assert!(db.get_playlist(&pl.id).unwrap().is_none());
    assert!(db.get_playlist_items(&pl.id).unwrap().is_empty());
}

#[test]
fn an_edit_against_an_old_revision_changes_nothing() {
    let (db, id) = collaborative(EditAccess::Friends);
    let base = revision(&db, &id);
    db.apply_playlist_edits(&id, "beta", base, &[add("One")])
        .unwrap();

    let stale = db.apply_playlist_edits(&id, "alpha", base, &[add("Two")]);
    assert!(matches!(stale, Err(EditError::Stale { current }) if current == base + 1));
    assert_eq!(titles(&db, &id), vec!["One"]);
}

#[test]
fn edits_apply_in_order_and_all_or_nothing() {
    let (db, id) = collaborative(EditAccess::Friends);
    let rev = db
        .apply_playlist_edits(
            &id,
            "alpha",
            revision(&db, &id),
            &[add("A"), add("B"), add("C")],
        )
        .unwrap();
    let items = db.get_playlist_items(&id).unwrap();
    let (a, c) = (items[0].id.clone(), items[2].id.clone());

    // C first, then a new track after A, then remove B: one revision for all three.
    let b = items[1].id.clone();
    let next = db
        .apply_playlist_edits(
            &id,
            "beta",
            rev,
            &[
                PlaylistEdit::Move {
                    item_id: c,
                    after_item_id: None,
                },
                PlaylistEdit::Add {
                    track: track("D"),
                    after_item_id: Some(a.clone()),
                },
                PlaylistEdit::Remove { item_id: b },
            ],
        )
        .unwrap();
    assert_eq!(next, rev + 1);
    assert_eq!(titles(&db, &id), vec!["C", "A", "D"]);
    let positions: Vec<i32> = db
        .get_playlist_items(&id)
        .unwrap()
        .iter()
        .map(|it| it.position)
        .collect();
    assert_eq!(positions, vec![0, 1, 2]);

    // A batch with one bad edit leaves the playlist exactly as it was.
    let failed = db.apply_playlist_edits(
        &id,
        "beta",
        next,
        &[
            add("E"),
            PlaylistEdit::Remove {
                item_id: "missing".into(),
            },
        ],
    );
    assert!(matches!(failed, Err(EditError::Invalid(_))));
    assert_eq!(titles(&db, &id), vec!["C", "A", "D"]);
    assert_eq!(revision(&db, &id), next);
}

#[test]
fn a_contributor_adds_and_removes_only_their_own() {
    let (db, id) = collaborative(EditAccess::Public);
    let rev = db
        .apply_playlist_edits(&id, "alpha", revision(&db, &id), &[add("Owner's")])
        .unwrap();
    let rev = db
        .apply_playlist_edits(&id, "gamma", rev, &[add("Gamma's")])
        .unwrap();
    let items = db.get_playlist_items(&id).unwrap();
    assert_eq!(items[1].added_by.as_deref(), Some("gamma"));

    let theirs = db.apply_playlist_edits(
        &id,
        "gamma",
        rev,
        &[PlaylistEdit::Remove {
            item_id: items[0].id.clone(),
        }],
    );
    assert!(matches!(theirs, Err(EditError::Forbidden(_))));
    let reorder = db.apply_playlist_edits(
        &id,
        "gamma",
        rev,
        &[PlaylistEdit::Move {
            item_id: items[1].id.clone(),
            after_item_id: None,
        }],
    );
    assert!(matches!(reorder, Err(EditError::Forbidden(_))));

    db.apply_playlist_edits(
        &id,
        "gamma",
        rev,
        &[PlaylistEdit::Remove {
            item_id: items[1].id.clone(),
        }],
    )
    .unwrap();
    assert_eq!(titles(&db, &id), vec!["Owner's"]);
}

#[test]
fn with_collaboration_off_only_the_owner_edits() {
    let (db, id) = collaborative(EditAccess::Friends);
    db.update_playlist_edit_access(&id, "alpha", EditAccess::Off)
        .unwrap();
    let rev = revision(&db, &id);

    assert!(matches!(
        db.apply_playlist_edits(&id, "beta", rev, &[add("X")]),
        Err(EditError::Forbidden(_))
    ));
    db.apply_playlist_edits(&id, "alpha", rev, &[add("X")])
        .unwrap();
}

#[test]
fn someone_who_cannot_open_it_is_told_it_does_not_exist() {
    let db = Db::new_in_memory().unwrap();
    let pl = db
        .create_playlist("alpha", "Mine", None, PlaylistVisibility::Private)
        .unwrap();

    assert!(matches!(
        db.apply_playlist_edits(&pl.id, "gamma", 0, &[add("X")]),
        Err(EditError::NotFound)
    ));
}

#[test]
fn details_are_the_owners_and_revision_checked() {
    let (db, id) = collaborative(EditAccess::Friends);
    let rev = revision(&db, &id);

    assert!(matches!(
        db.update_playlist_details(&id, "beta", rev, "Taken", None),
        Err(EditError::Forbidden(_))
    ));
    assert!(matches!(
        db.update_playlist_details(&id, "alpha", rev - 1, "Late", None),
        Err(EditError::Stale { .. })
    ));
    db.update_playlist_details(&id, "alpha", rev, "Renamed", Some("New"))
        .unwrap();
    assert_eq!(db.get_playlist(&id).unwrap().unwrap().title, "Renamed");
}

#[test]
fn followers_are_recorded_once_and_go_with_the_playlist() {
    let db = Db::new_in_memory().unwrap();
    let pl = db
        .create_playlist("alpha", "Mix", None, PlaylistVisibility::Public)
        .unwrap();
    db.follow_playlist(&pl.id, "beta").unwrap();
    db.follow_playlist(&pl.id, "beta").unwrap();

    assert_eq!(db.playlist_follower_ids(&pl.id).unwrap(), vec!["beta"]);
    assert_eq!(db.followed_playlists("beta").unwrap().len(), 1);

    db.delete_playlist(&pl.id, "alpha").unwrap();
    assert!(db.followed_playlists("beta").unwrap().is_empty());
    assert!(db.playlist_follower_ids(&pl.id).unwrap().is_empty());
}
