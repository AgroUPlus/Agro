//! The retention sweep removes what has expired and nothing else.

use crate::db::retention::HANDOFF_ROW_TTL_DAYS;
use crate::db::*;
use rusqlite::params;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

fn ago(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
}

fn count(db: &Db, table: &str) -> i64 {
    db.conn
        .lock()
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// The point of the sweep: expiry stopped a row being *usable* long ago, but only this makes
/// it stop being *readable* by anyone who takes the file.
#[test]
fn expired_rows_are_deleted_and_live_ones_are_not() {
    let db = db();
    {
        let conn = db.conn.lock().unwrap();
        for (token, expires) in [("dead", ago(1)), ("live", ago(-1))] {
            conn.execute(
                "INSERT INTO ephemeral_shares
                    (token, user_id, track_title, artist_name, album_name, audio_url, expires_at)
                 VALUES (?1, 'alpha', 't', 'a', NULL, 'http://x', ?2)",
                params![token, expires],
            )
            .unwrap();
        }
        for (code, expires) in [("DEAD", ago(1)), ("LIVE", ago(-1))] {
            conn.execute(
                "INSERT INTO friend_codes (code, user_id, created_at, expires_at)
                 VALUES (?1, 'alpha', ?2, ?3)",
                params![code, ago(2), expires],
            )
            .unwrap();
        }
        let now = chrono::Utc::now().timestamp();
        for (id, expires) in [
            ("dead", Some(now - 60)),
            ("live", Some(now + 3600)),
            // No deadline named, so it does not expire and must survive.
            ("forever", None),
        ] {
            conn.execute(
                "INSERT INTO short_links (id, target_url, user_id, created_at, expires_at)
                 VALUES (?1, 'http://x', 'alpha', ?2, ?3)",
                params![id, now, expires],
            )
            .unwrap();
        }
    }

    db.sweep_retention();

    assert_eq!(count(&db, "ephemeral_shares"), 1);
    assert_eq!(count(&db, "friend_codes"), 1);
    assert_eq!(count(&db, "short_links"), 2);
}

/// A device that went quiet keeps its row — the account still wants to know it exists — but
/// the queue it was playing is what a stolen database would read, and that goes.
#[test]
fn a_stale_handoff_keeps_its_row_but_loses_its_queue() {
    let db = db();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO handoff_state
                (user_id, device_id, track_uri, track_title, artist_name, position_ms,
                 is_playing, updated_at, queue_json, queue_index)
             VALUES ('alpha', 'laptop', 'u', 't', 'a', 91000, 0, ?1, '[\"one\",\"two\"]', 1)",
            params![ago(10)],
        )
        .unwrap();

    db.sweep_retention();

    let (queue, index, position): (Option<String>, Option<i64>, i64) = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT queue_json, queue_index, position_ms FROM handoff_state",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(queue, None, "the stale queue should have been scrubbed");
    assert_eq!(index, None);
    assert_eq!(position, 0);
    assert_eq!(count(&db, "handoff_state"), 1, "the row itself stays");
}

#[test]
fn a_long_dead_handoff_row_is_removed_entirely() {
    let db = db();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO handoff_state
                (user_id, device_id, track_uri, track_title, artist_name, position_ms,
                 is_playing, updated_at)
             VALUES ('alpha', 'retired', 'u', 't', 'a', 0, 0, ?1)",
            params![ago(HANDOFF_ROW_TTL_DAYS + 1)],
        )
        .unwrap();

    db.sweep_retention();

    assert_eq!(count(&db, "handoff_state"), 0);
}

/// A device in daily use must not have the queue pulled out from under it.
#[test]
fn an_active_handoff_is_left_alone() {
    let db = db();
    db.update_handoff(
        "alpha",
        "u",
        "t",
        "a",
        None,
        None,
        42_000,
        180_000,
        true,
        "phone",
        Some("[\"one\"]"),
        Some(0),
        None,
        None,
        None,
    )
    .unwrap();

    db.sweep_retention();

    let handoff = db.get_handoff("alpha").unwrap().expect("row survives");
    assert_eq!(handoff.position_ms, 42_000);
    assert!(handoff.queue_json.is_some(), "an active queue must survive");
}
