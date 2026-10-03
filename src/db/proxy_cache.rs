//! The cache behind `/proxy`: upstream responses kept until their own expiry.

use rusqlite::{params, Result};

use super::Db;

impl Db {
    pub fn get_cached_proxy(&self, url: &str) -> Result<Option<(String, Vec<u8>)>> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut stmt = conn
            .prepare("SELECT headers, body FROM proxy_cache WHERE url = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![url, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn set_cached_proxy(
        &self,
        url: &str,
        headers: &str,
        body: &[u8],
        expires_at: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO proxy_cache (url, headers, body, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![url, headers, body, expires_at],
        )?;
        Ok(())
    }
}
