//! Deleting an account leaves nothing on the server that can say who it was.
//!
//! The scan at the end is the point. It does not check a list of tables — a list is exactly what
//! went stale every time a feature added one — but reads every text cell in the database for the
//! name or the id. A new table that stores either fails this test until erasure learns about it.

use crate::db::*;
use crate::db_drops::NewDrop;
use crate::db_identity::{AccountState, Role};
use crate::db_jam::JamMode;
use crate::db_playlist_items::NewPlaylistItem;
use crate::playlist_visibility::PlaylistVisibility;
use rusqlite::params;

const GONE: &str = "zoltan";

fn item(title: &str) -> NewPlaylistItem {
    NewPlaylistItem {
        title: title.into(),
        artist: "Artist".into(),
        album: None,
        duration_ms: Some(1000),
        artwork_url: None,
        origin_uri: None,
    }
}

fn jam_track(db: &Db, jam_id: &str, by: &str, title: &str) -> String {
    db.add_jam_track(
        jam_id,
        by,
        "u:1",
        title,
        "Artist",
        None,
        1000,
        false,
        JamMode::Open,
        None,
        None,
    )
    .unwrap()
    .0
}

/// Every text cell anywhere that is the name, holds it as a JSON string, or is the account id.
fn traces(db: &Db, user_id: &str) -> Vec<String> {
    let conn = db.conn.lock().unwrap();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name <> 'sqlite_sequence'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let mut found = Vec::new();
    for table in tables {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for column in columns {
            let hits: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM {table}
                          WHERE typeof({column}) = 'text'
                            AND (lower({column}) = ?1 OR instr(lower({column}), ?2) > 0
                                 OR {column} = ?3)"
                    ),
                    params![GONE, format!("\"{GONE}\""), user_id],
                    |r| r.get(0),
                )
                .unwrap();
            if hits > 0 {
                found.push(format!("{table}.{column} ({hits})"));
            }
        }
    }
    found
}

#[test]
fn a_deleted_account_leaves_no_trace_and_others_keep_what_is_theirs() {
    let db = Db::new_in_memory().unwrap();
    for name in ["alpha", GONE] {
        db.create_account(
            name,
            "a-long-passphrase",
            Role::Member,
            AccountState::Active,
        )
        .unwrap();
    }
    let user_id: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM users WHERE username = ?1",
            params![GONE],
            |r| r.get(0),
        )
        .unwrap();

    db.send_friend_request(GONE, "alpha").unwrap();
    db.accept_friend_request("alpha", GONE).unwrap();
    let drop = NewDrop {
        track_title: "Sent".into(),
        artist_name: "A".into(),
        ..Default::default()
    };
    db.create_drop(GONE, "alpha", &drop).unwrap();
    db.subscribe_artist(GONE, "Some Artist", None).unwrap();
    db.create_invite(GONE, 1, None).unwrap();

    // Their playlist, followed by alpha; alpha's playlist, which they added to and follow.
    let theirs = db
        .create_playlist(GONE, "Theirs", None, PlaylistVisibility::Friends)
        .unwrap();
    db.add_playlist_item(&theirs.id, GONE, item("in theirs"))
        .unwrap();
    db.follow_playlist(&theirs.id, "alpha").unwrap();
    let alphas = db
        .create_playlist("alpha", "Alpha's", None, PlaylistVisibility::Friends)
        .unwrap();
    db.add_playlist_item(&alphas.id, GONE, item("they added this"))
        .unwrap();
    db.follow_playlist(&alphas.id, GONE).unwrap();

    // A jam they host, and one of alpha's they played in and left a recap of.
    let hosted = db.create_jam(GONE, JamMode::Open).unwrap();
    db.join_jam(&hosted.id, "alpha").unwrap();
    jam_track(&db, &hosted.id, "alpha", "in their jam");
    let room = db.create_jam("alpha", JamMode::Open).unwrap();
    db.join_jam(&room.id, GONE).unwrap();
    let played = jam_track(&db, &room.id, GONE, "their pick");
    db.mark_jam_track_played(&room.id, &played).unwrap();
    let room = db.jam_by_id(&room.id).unwrap().unwrap();
    db.record_jam_recap(&room, GONE).unwrap().unwrap();
    db.record_jam_recap(&room, "alpha").unwrap().unwrap();

    // A blend they made, and one of alpha's their listening was written into.
    db.conn
        .lock()
        .unwrap()
        .execute("UPDATE users SET show_stats = 1", [])
        .unwrap();
    let play = crate::db::ScrobbleEntry {
        track_title: "Their Favourite".into(),
        artist_name: "Their Artist".into(),
        album_name: None,
        genre: None,
        duration_secs: 200,
        played_at: chrono::Utc::now().to_rfc3339(),
        play_uid: None,
    };
    db.record_scrobbles(GONE, "phone", None, &[play]).unwrap();
    let recipe = crate::blend_recipe::BlendSettings {
        size: 25,
        mix: 50,
        window: crate::blend_recipe::BlendWindow::AllTime,
        refresh: crate::blend_recipe::BlendRefresh::Weekly,
    };
    let made = db
        .create_blend(GONE, "Their blend", &["alpha".to_string()], recipe)
        .unwrap();
    db.answer_blend_invite(&made.id, "alpha", true).unwrap();
    let alphas_blend = db
        .create_blend("alpha", "Alpha's blend", &[GONE.to_string()], recipe)
        .unwrap();
    db.answer_blend_invite(&alphas_blend.id, GONE, true)
        .unwrap();
    assert!(db.refresh_blend_if_due(&alphas_blend.id).unwrap().is_some());

    assert!(
        !traces(&db, &user_id).is_empty(),
        "the setup should leave traces to remove"
    );
    assert!(db.delete_user(GONE).unwrap());

    assert_eq!(
        traces(&db, &user_id),
        Vec::<String>::new(),
        "the account is still named"
    );

    // What belonged to alpha is still alpha's, minus the name.
    let kept = db.get_playlist(&alphas.id).unwrap();
    assert!(
        kept.is_some(),
        "alpha's playlist went with the account that added to it"
    );
    let recap = &db.jam_recaps("alpha").unwrap()[0].recap;
    assert_eq!(recap.tracks[0].title, "their pick");
    assert_eq!(recap.tracks[0].added_by, "");

    // A jam they hosted goes whole, not just its row.
    let orphans: i64 = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM jam_tracks WHERE jam_id = ?1",
            params![hosted.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "their jam's queue outlived it");
}

#[test]
fn a_blend_they_were_in_is_rewritten_without_them() {
    let db = Db::new_in_memory().unwrap();
    for name in ["alpha", GONE] {
        db.create_account(
            name,
            "a-long-passphrase",
            Role::Member,
            AccountState::Active,
        )
        .unwrap();
    }
    db.conn
        .lock()
        .unwrap()
        .execute("UPDATE users SET show_stats = 1", [])
        .unwrap();
    for (who, title) in [(GONE, "theirs"), ("alpha", "alpha's")] {
        let play = crate::db::ScrobbleEntry {
            track_title: title.into(),
            artist_name: format!("{who} artist"),
            album_name: None,
            genre: None,
            duration_secs: 200,
            played_at: chrono::Utc::now().to_rfc3339(),
            play_uid: None,
        };
        db.record_scrobbles(who, "phone", None, &[play]).unwrap();
    }
    let recipe = crate::blend_recipe::BlendSettings {
        size: 25,
        mix: 50,
        window: crate::blend_recipe::BlendWindow::AllTime,
        refresh: crate::blend_recipe::BlendRefresh::Frozen,
    };
    let blend = db
        .create_blend("alpha", "Ours", &[GONE.to_string()], recipe)
        .unwrap();
    db.answer_blend_invite(&blend.id, GONE, true).unwrap();
    db.refresh_blend_if_due(&blend.id).unwrap();
    assert_eq!(db.get_playlist_items(&blend.id).unwrap().len(), 2);

    db.delete_user(GONE).unwrap();

    // Frozen, so only the deletion can have made it due.
    assert!(db.refresh_blend_if_due(&blend.id).unwrap().is_some());
    let titles: Vec<_> = db
        .get_playlist_items(&blend.id)
        .unwrap()
        .into_iter()
        .map(|i| i.title)
        .collect();
    assert_eq!(titles, ["alpha's"]);
}
