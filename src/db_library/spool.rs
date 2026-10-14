//! The relay spool: what is staged on the server, and who owns it.

use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// How many spool bytes this account currently occupies.
    ///
    /// The one number a quota can honestly be checked against. `library_stats.total_bytes` cannot
    /// be used for it: that counts every archived track in the deployment into every account's
    /// total, so it reads the same for a guest holding nothing as for the admin.
    pub fn spool_bytes_for(&self, user_id: &str) -> Result<i64> {
        let conn = self.read();
        conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM spool_items WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
    }

    pub fn spool_insert(
        &self,
        content_hash: &str,
        size_bytes: i64,
        from_device: &str,
        user_id: &str,
        ttl_hours: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT INTO spool_items (content_hash, size_bytes, from_device, user_id,
                                      created_at, expires_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(content_hash) DO UPDATE SET
                 expires_at = excluded.expires_at, from_device = excluded.from_device",
            params![
                content_hash,
                size_bytes,
                from_device,
                user_id,
                now.to_rfc3339(),
                (now + chrono::Duration::hours(ttl_hours)).to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn spool_total_bytes(&self) -> Result<i64> {
        let conn = self.read();
        conn.query_row(
            "SELECT COALESCE(SUM(size_bytes),0) FROM spool_items",
            [],
            |r| r.get(0),
        )
    }

    pub fn spool_contains(&self, content_hash: &str) -> Result<bool> {
        let conn = self.read();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM spool_items WHERE content_hash = ?1",
                params![content_hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Spool entries to remove, oldest first — expired ones always, then whatever else it takes to
    /// get back under [`budget`].
    /// What to evict from **one account's** spool, oldest first, to bring it inside `budget`.
    ///
    /// Scoped to an account on purpose. This used to take one global budget and evict oldest-first
    /// across everybody, so one account filling the spool evicted another's staged files — a guest
    /// could push the admin's pending transfers off the disk just by uploading.
    ///
    /// Expired rows go regardless of the budget; that is the TTL, not the quota.
    pub fn spool_evictable_for(&self, user_id: &str, budget: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT content_hash, size_bytes, expires_at FROM spool_items
              WHERE user_id = ?1 ORDER BY created_at ASC",
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows: Vec<(String, i64, String)> = stmt
            .query_map(params![user_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<_>>()?;

        let mut total: i64 = rows.iter().map(|(_, size, _)| size).sum();
        let mut doomed = Vec::new();
        for (hash, size, expires_at) in rows {
            if expires_at <= now || total > budget {
                doomed.push((hash, size));
                total -= size;
            }
        }
        Ok(doomed)
    }

    /// Every account with something in the spool, for the periodic sweep.
    pub fn spool_users(&self) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt = conn.prepare("SELECT DISTINCT user_id FROM spool_items")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Which account spooled a file, so a fetch can be scoped to it.
    pub fn spool_owner(&self, content_hash: &str) -> Result<Option<String>> {
        let conn = self.read();
        conn.query_row(
            "SELECT user_id FROM spool_items WHERE content_hash = ?1",
            params![content_hash],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn spool_delete(&self, content_hash: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "DELETE FROM spool_items WHERE content_hash = ?1",
            params![content_hash],
        )?;
        Ok(())
    }
}
