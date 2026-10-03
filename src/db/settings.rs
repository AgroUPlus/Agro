//! Settings an account syncs across its devices, sealed where they say anything about the user.

use rusqlite::{params, Result};

use super::Db;

impl Db {
    pub fn upsert_synced_settings(
        &self,
        user_id: &str,
        settings_blob: Option<&str>,
        has_server_url: Option<bool>,
        lyrics_fetch_online: Option<bool>,
        stream_format: Option<&str>,
        share: ShareSettingsInput<'_>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO synced_settings (user_id, settings_blob, has_server_url, lyrics_fetch_online, stream_format, share_domain, share_hosts, share_enabled, updated_at)
             VALUES (?1, ?2, COALESCE(?3, 0), ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(user_id) DO UPDATE SET
             settings_blob = COALESCE(excluded.settings_blob, synced_settings.settings_blob),
             has_server_url = COALESCE(?3, synced_settings.has_server_url),
             lyrics_fetch_online = COALESCE(excluded.lyrics_fetch_online, synced_settings.lyrics_fetch_online),
             stream_format = COALESCE(excluded.stream_format, synced_settings.stream_format),
             share_domain = COALESCE(excluded.share_domain, synced_settings.share_domain),
             share_hosts = COALESCE(excluded.share_hosts, synced_settings.share_hosts),
             share_enabled = COALESCE(excluded.share_enabled, synced_settings.share_enabled),
             updated_at = excluded.updated_at",
            params![
                user_id,
                settings_blob,
                has_server_url,
                lyrics_fetch_online,
                stream_format,
                share.domain,
                share.hosts,
                share.enabled,
                now
            ],
        )?;
        Ok(())
    }

    pub fn get_synced_settings(&self, user_id: &str) -> Result<Option<SyncedSettingsRecord>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT settings_blob, has_server_url, lyrics_fetch_online, stream_format,
                    share_domain, share_hosts, share_enabled, updated_at
             FROM synced_settings WHERE user_id = ?1",
        )?;
        let mut rows = stmt.query(params![user_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SyncedSettingsRecord {
                settings_blob: row.get(0)?,
                has_server_url: row.get::<_, i64>(1)? != 0,
                lyrics_fetch_online: row.get(2)?,
                stream_format: row.get(3)?,
                share_domain: row.get(4)?,
                share_hosts: row.get(5)?,
                share_enabled: row.get(6)?,
                updated_at: row.get(7)?,
            }))
        } else {
            Ok(None)
        }
    }
}

/// The share-link fields of a settings upsert, grouped so the function keeps a readable signature
/// rather than taking nine positional `Option`s in a row.
#[derive(Default, Clone, Copy)]
pub struct ShareSettingsInput<'a> {
    pub domain: Option<&'a str>,
    pub hosts: Option<&'a str>,
    pub enabled: Option<bool>,
}

pub struct SyncedSettingsRecord {
    /// The account's upstream settings, sealed by the client under a key this server does not
    /// have. Opaque here by design: it is stored, returned, and never inspected.
    pub settings_blob: Option<String>,
    /// Whether [`Self::settings_blob`] contains a server address. The one thing the server needs
    /// to know about the contents, stated outright rather than discovered by decrypting.
    pub has_server_url: bool,
    pub lyrics_fetch_online: Option<bool>,
    pub stream_format: Option<String>,
    /// The domain the players send share links out on, e.g. `frwd.top`.
    pub share_domain: Option<String>,
    /// Comma-separated hosts `/listen` may forward to.
    pub share_hosts: Option<String>,
    pub share_enabled: Option<bool>,
    pub updated_at: String,
}
