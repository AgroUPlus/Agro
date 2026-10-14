//! Resumable upload sessions.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    #[allow(clippy::too_many_arguments)]
    pub fn create_upload(
        &self,
        upload_id: &str,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
        size_bytes: i64,
        target: &str,
        extension: Option<&str>,
        ttl_hours: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT INTO upload_sessions
                 (upload_id, user_id, device_id, content_hash, size_bytes, received_bytes,
                  target, created_at, expires_at, extension)
             VALUES (?1,?2,?3,?4,?5,0,?6,?7,?8,?9)",
            params![
                upload_id,
                user_id,
                device_id,
                content_hash,
                size_bytes,
                target,
                now.to_rfc3339(),
                (now + chrono::Duration::hours(ttl_hours)).to_rfc3339(),
                extension,
            ],
        )?;
        Ok(())
    }

    /// An unfinished upload of the same file by the same device, so a dropped transfer resumes.
    pub fn resumable_upload(
        &self,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
    ) -> Result<Option<UploadSession>> {
        let conn = self.read();
        conn.query_row(
            "SELECT upload_id, user_id, device_id, content_hash, size_bytes, received_bytes, target,
                    extension
             FROM upload_sessions
             WHERE user_id = ?1 AND device_id = ?2 AND content_hash = ?3
               AND expires_at > ?4
             ORDER BY created_at DESC LIMIT 1",
            params![
                user_id,
                device_id,
                content_hash,
                chrono::Utc::now().to_rfc3339()
            ],
            row_to_upload,
        )
        .optional()
    }

    pub fn upload_session(&self, upload_id: &str) -> Result<Option<UploadSession>> {
        let conn = self.read();
        conn.query_row(
            "SELECT upload_id, user_id, device_id, content_hash, size_bytes, received_bytes, target,
                    extension
             FROM upload_sessions WHERE upload_id = ?1",
            params![upload_id],
            row_to_upload,
        )
        .optional()
    }

    pub fn set_upload_received(&self, upload_id: &str, received: i64) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE upload_sessions SET received_bytes = ?2 WHERE upload_id = ?1",
            params![upload_id, received],
        )?;
        Ok(())
    }

    pub fn delete_upload(&self, upload_id: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "DELETE FROM upload_sessions WHERE upload_id = ?1",
            params![upload_id],
        )?;
        Ok(())
    }

    /// Upload sessions whose TTL has passed, so their `.part` files can be removed too.
    pub fn expired_uploads(&self) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT upload_id FROM upload_sessions WHERE expires_at <= ?1")?;
        let rows = stmt.query_map(params![chrono::Utc::now().to_rfc3339()], |row| row.get(0))?;
        rows.collect()
    }

    // ── Spool ───────────────────────────────────────────────────────────────────────────────
}
