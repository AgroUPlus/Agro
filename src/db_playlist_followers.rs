//! Who keeps a live copy of a playlist: the accounts to tell when it changes.
//!
//! Following grants nothing. Every read still goes through `can_view_playlist`, so a follower who
//! loses access — the playlist made private, a friendship ended — keeps their row here and simply
//! gets refused, which is how their copy learns it is no longer shared.

use rusqlite::{params, Result};

use crate::db::Db;
use crate::db_playlists::{playlist_from_row, Playlist, PLAYLIST_COLUMNS};

impl Db {
    /// Records that `user_id` follows `playlist_id`. The caller checks they may open it first.
    pub fn follow_playlist(&self, playlist_id: &str, user_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO playlist_followers (playlist_id, user_id, followed_at)
             VALUES (?1, ?2, ?3)",
            params![playlist_id, user_id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn unfollow_playlist(&self, playlist_id: &str, user_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "DELETE FROM playlist_followers WHERE playlist_id = ?1 AND user_id = ?2",
            params![playlist_id, user_id],
        )?;
        Ok(count > 0)
    }

    pub fn is_following_playlist(&self, playlist_id: &str, user_id: &str) -> Result<bool> {
        let conn = self.read();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM playlist_followers WHERE playlist_id = ?1 AND user_id = ?2)",
            params![playlist_id, user_id],
            |row| row.get(0),
        )
    }

    /// The followed playlists that still exist, newest follow first. Access is the caller's to
    /// check; a deleted playlist takes its follower rows with it.
    pub fn followed_playlists(&self, user_id: &str) -> Result<Vec<Playlist>> {
        let conn = self.read();
        let columns = PLAYLIST_COLUMNS
            .split(", ")
            .map(|c| format!("p.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = conn.prepare(&format!(
            "SELECT {columns} FROM playlists p
               JOIN playlist_followers f ON f.playlist_id = p.id
              WHERE f.user_id = ?1 ORDER BY f.followed_at DESC"
        ))?;
        let rows = stmt.query_map(params![user_id], playlist_from_row)?;
        rows.collect()
    }

    /// Everyone to tell about a change to `playlist_id`: its followers. The owner is the caller's
    /// to add.
    pub fn playlist_follower_ids(&self, playlist_id: &str) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT user_id FROM playlist_followers WHERE playlist_id = ?1")?;
        let rows = stmt.query_map(params![playlist_id], |row| row.get(0))?;
        rows.collect()
    }
}
