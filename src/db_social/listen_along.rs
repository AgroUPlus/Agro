//! Listen-along pairings between a listener and a host.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// Tunes `listener` in to `host`, replacing whatever they were following.
    pub fn set_listen_along(&self, listener: &str, host: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT OR REPLACE INTO listen_along (listener_id, host_id, started_at)
             VALUES (?1, ?2, ?3)",
            params![
                listener.trim().to_lowercase(),
                host.trim().to_lowercase(),
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn clear_listen_along(&self, listener: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let removed = conn.execute(
            "DELETE FROM listen_along WHERE listener_id = ?1 COLLATE NOCASE",
            params![listener.trim()],
        )?;
        Ok(removed > 0)
    }

    pub fn listen_along_of(&self, listener: &str) -> Result<Option<ListenAlong>> {
        let conn = self.read();
        let row = conn
            .query_row(
                "SELECT listener_id, host_id, started_at FROM listen_along
                  WHERE listener_id = ?1 COLLATE NOCASE",
                params![listener.trim()],
                |row| {
                    Ok(ListenAlong {
                        listener: row.get(0)?,
                        host: row.get(1)?,
                        started_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Everyone currently following `host`. This is the fan-out list for a `LISTEN_ALONG` push.
    ///
    /// Re-checks the friendship on the way out, so a listener who was removed as a friend stops
    /// receiving frames without anything having to remember to clean up their row.
    pub fn listeners_of(&self, host: &str) -> Result<Vec<String>> {
        // Collected and the lock released before the friendship checks below, each of which takes
        // the same mutex.
        let listeners: Vec<String> = {
            let conn = self.conn.lock_recover();
            let mut stmt = conn.prepare(
                "SELECT listener_id FROM listen_along WHERE host_id = ?1 COLLATE NOCASE",
            )?;
            let rows = stmt
                .query_map(params![host.trim()], |row| row.get(0))?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        let mut allowed = Vec::new();
        for listener in listeners {
            if self.are_friends(host, &listener)? {
                allowed.push(listener);
            }
        }
        Ok(allowed)
    }
}
