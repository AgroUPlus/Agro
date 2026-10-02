//! Keeping short links few and alive: one per target, and none that nobody uses.
//!
//! A second `impl Db` block, the way [`crate::db_identity`] is one. Minting a link used to add a
//! row every time, and a row without a deadline stayed for good — so the same track shared ten
//! times was ten links, all kept. Two rules replace that, both stated in `SHARE_LINKS.md` §6a:
//!
//! - Sharing a target the account already has a live link for hands back that link.
//! - A link nobody has opened or re-shared for [`IDLE_DAYS`] is deleted, and its id is kept as a
//!   tombstone so `/listen` can tell the visitor it was deleted rather than that it never existed.

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;

/// How long a link may go unopened and unshared before it is deleted.
pub const IDLE_DAYS: i64 = 30;

/// How long the id of a deleted link is remembered. A year is long past anyone still holding a
/// message with it in; after that it falls back to the ordinary refusal.
pub const TOMBSTONE_DAYS: i64 = 365;

const DAY_SECS: i64 = 24 * 60 * 60;

impl Db {
    /// The account's live, open-ended link to `target_url` from `source`, if it has one.
    ///
    /// Finding it counts as the link being used — it is about to be sent again — so its idle clock
    /// restarts. Only links with no deadline are reused: a link minted to expire was asked for as a
    /// one-off, and handing it out again would quietly extend what its owner meant to end.
    pub fn reuse_short_link(
        &self,
        user_id: &str,
        target_url: &str,
        source: Option<&str>,
    ) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM short_links
                  WHERE user_id = ?1 AND target_url = ?2 AND source IS ?3 AND expires_at IS NULL
                  ORDER BY created_at DESC LIMIT 1",
                params![user_id, target_url, source],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = &id {
            conn.execute(
                "UPDATE short_links SET last_shared_at = ?2 WHERE id = ?1",
                params![id, chrono::Utc::now().timestamp()],
            )?;
        }
        Ok(id)
    }

    /// Whether `id` was a link deleted for going unused, as opposed to one that never existed.
    pub fn short_link_retired(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM retired_short_links WHERE id = ?1",
            params![id],
            |_| Ok(()),
        )
        .optional()
        .map(|found| found.is_some())
    }

    /// Deletes links nobody has opened or re-shared for [`IDLE_DAYS`], keeping their ids as
    /// tombstones, and forgets tombstones older than [`TOMBSTONE_DAYS`]. Returns how many links went.
    ///
    /// A link's last use is the latest of when it was made, last opened and last re-shared. One
    /// transaction, so a link is never deleted without its tombstone or the other way round.
    pub fn sweep_idle_short_links(&self) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let idle_before = now - IDLE_DAYS * DAY_SECS;
        let tx = conn.transaction()?;
        const IDLE: &str =
            "max(created_at, coalesce(last_clicked_at, 0), coalesce(last_shared_at, 0)) < ?1";
        tx.execute(
            &format!(
                "INSERT OR REPLACE INTO retired_short_links (id, retired_at)
                 SELECT id, ?2 FROM short_links WHERE {IDLE}"
            ),
            params![idle_before, now],
        )?;
        let removed = tx.execute(
            &format!("DELETE FROM short_links WHERE {IDLE}"),
            params![idle_before],
        )?;
        tx.execute(
            "DELETE FROM retired_short_links WHERE retired_at < ?1",
            params![now - TOMBSTONE_DAYS * DAY_SECS],
        )?;
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(
        db: &Db,
        id: &str,
        target: &str,
        created: i64,
        clicked: Option<i64>,
        shared: Option<i64>,
    ) {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO short_links (id, target_url, user_id, created_at, source, last_clicked_at, last_shared_at)
                 VALUES (?1, ?2, 'alpha', ?3, NULL, ?4, ?5)",
                params![id, target, created, clicked, shared],
            )
            .unwrap();
    }

    fn exists(db: &Db, id: &str) -> bool {
        db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM short_links WHERE id = ?1",
                params![id],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some()
    }

    #[test]
    fn sharing_a_target_again_hands_back_the_same_link() {
        let db = Db::new_in_memory().unwrap();
        let now = chrono::Utc::now().timestamp();
        link(
            &db,
            "abc1234",
            "https://music.youtube.com/watch?v=x",
            now,
            None,
            None,
        );

        let again = db
            .reuse_short_link("alpha", "https://music.youtube.com/watch?v=x", None)
            .unwrap();
        assert_eq!(again.as_deref(), Some("abc1234"));
        // Another account's link, or another target, is never handed out.
        assert_eq!(
            db.reuse_short_link("beta", "https://music.youtube.com/watch?v=x", None)
                .unwrap(),
            None
        );
        assert_eq!(
            db.reuse_short_link("alpha", "https://music.youtube.com/watch?v=y", None)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_link_unused_for_thirty_days_is_deleted_and_remembered() {
        let db = Db::new_in_memory().unwrap();
        let now = chrono::Utc::now().timestamp();
        let old = now - (IDLE_DAYS + 1) * DAY_SECS;
        link(&db, "idle000", "https://a.example/1", old, None, None);
        link(
            &db,
            "clicked",
            "https://a.example/2",
            old,
            Some(now - DAY_SECS),
            None,
        );
        link(
            &db,
            "reshare",
            "https://a.example/3",
            old,
            None,
            Some(now - DAY_SECS),
        );
        link(&db, "fresh00", "https://a.example/4", now, None, None);

        assert_eq!(db.sweep_idle_short_links().unwrap(), 1);

        assert!(!exists(&db, "idle000"));
        assert!(db.short_link_retired("idle000").unwrap());
        for kept in ["clicked", "reshare", "fresh00"] {
            assert!(exists(&db, kept), "{kept} was used recently and must stay");
            assert!(!db.short_link_retired(kept).unwrap());
        }
        // An id that never existed is not reported as deleted.
        assert!(!db.short_link_retired("nothere").unwrap());
    }

    #[test]
    fn reusing_a_link_restarts_its_idle_clock() {
        let db = Db::new_in_memory().unwrap();
        let old = chrono::Utc::now().timestamp() - (IDLE_DAYS + 1) * DAY_SECS;
        link(&db, "oldlink", "https://a.example/5", old, None, None);

        db.reuse_short_link("alpha", "https://a.example/5", None)
            .unwrap();

        assert_eq!(db.sweep_idle_short_links().unwrap(), 0);
        assert!(exists(&db, "oldlink"));
    }
}
