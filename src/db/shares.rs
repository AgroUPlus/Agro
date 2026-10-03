//! Ephemeral shares: a hosted audio page that expires.

use rusqlite::{params, Result};

use super::{unix_now, Db};

impl Db {
    pub fn create_ephemeral_share(
        &self,
        user_id: &str,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
        audio_url: &str,
        ttl_hours: i64,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let expires_at = (chrono::Utc::now() + chrono::Duration::hours(ttl_hours)).to_rfc3339();
        conn.execute(
            "INSERT INTO ephemeral_shares (token, user_id, track_title, artist_name, album_name, audio_url, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![token, user_id, track_title, artist_name, album_name, audio_url, expires_at],
        )?;
        Ok(token)
    }

    pub fn get_ephemeral_share(&self, token: &str) -> Result<Option<ShareRecord>> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let mut stmt = conn.prepare("SELECT track_title, artist_name, album_name, audio_url, expires_at FROM ephemeral_shares WHERE token = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![token, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ShareRecord {
                track_title: row.get(0)?,
                artist_name: row.get(1)?,
                album_name: row.get(2)?,
                audio_url: row.get(3)?,
                expires_at: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Bumps an ephemeral share's hit counter. Aggregate only — see migration 6.
    pub fn record_share_click(&self, token: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE ephemeral_shares SET click_count = click_count + 1, last_clicked_at = ?2
             WHERE token = ?1",
            params![token, unix_now()],
        );
    }
}

pub struct ShareRecord {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub audio_url: String,
    pub expires_at: String,
}
