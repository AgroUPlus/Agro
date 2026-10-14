//! Everything the server holds about one account, for the data-export request.

use super::*;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// Everything this server holds that is keyed to one account, as JSON.
    ///
    /// The counterpart to deletion: someone who can be erased should also be able to see what there
    /// was to erase. Deliberately assembled from the same table list `delete_user` uses, so the two
    /// cannot drift into a state where something is exportable but not deletable, or worse, the
    /// other way round.
    ///
    /// Credentials are excluded. A token hash is not useful to its owner and a passphrase hash is
    /// not theirs to have; what is here is the data, not the keys to it.
    pub fn export_account_data(&self, username: &str) -> Result<serde_json::Value> {
        let conn = self.conn.lock_recover();
        let mut export = serde_json::Map::new();

        let mut dump = |label: &str, sql: &str| -> Result<()> {
            let mut stmt = conn.prepare(sql)?;
            let columns: Vec<String> = stmt
                .column_names()
                .into_iter()
                .map(str::to_string)
                .collect();
            let rows = stmt
                .query_map(params![username.trim()], |row| {
                    let mut object = serde_json::Map::new();
                    for (i, name) in columns.iter().enumerate() {
                        let value = match row.get_ref(i)? {
                            rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                            rusqlite::types::ValueRef::Integer(v) => v.into(),
                            rusqlite::types::ValueRef::Real(v) => v.into(),
                            rusqlite::types::ValueRef::Text(v) => {
                                String::from_utf8_lossy(v).to_string().into()
                            }
                            // A blob is not renderable as JSON and none of these columns hold one
                            // that a person could use; its size is the honest answer.
                            rusqlite::types::ValueRef::Blob(v) => {
                                format!("<{} bytes>", v.len()).into()
                            }
                        };
                        object.insert(name.clone(), value);
                    }
                    Ok(serde_json::Value::Object(object))
                })?
                .collect::<Result<Vec<_>>>()?;
            export.insert(label.to_string(), serde_json::Value::Array(rows));
            Ok(())
        };

        dump(
            "account",
            "SELECT username, role, state, created_at, quota_bytes FROM users
              WHERE username = ?1 COLLATE NOCASE",
        )?;
        dump(
            "devices",
            "SELECT * FROM registered_nodes WHERE user_id = ?1",
        )?;
        dump(
            "listening_history",
            "SELECT * FROM scrobbles WHERE user_id = ?1",
        )?;
        dump(
            "friendships",
            "SELECT * FROM friendships WHERE user_id = ?1 OR friend_id = ?1",
        )?;
        dump(
            "track_drops",
            "SELECT * FROM track_drops WHERE from_user = ?1 OR to_user = ?1",
        )?;
        dump(
            "share_links",
            "SELECT * FROM short_links WHERE user_id = ?1",
        )?;
        dump(
            "library_holdings",
            "SELECT * FROM device_holdings WHERE user_id = ?1",
        )?;
        dump("handoff", "SELECT * FROM handoff_state WHERE user_id = ?1")?;
        // The label of each backup, not its sealed bytes: those are the device's to open, and a
        // copy is downloadable from the app or the dashboard's own route.
        dump(
            "vault_backups",
            "SELECT id, created_at, device_id, device_name, app_version, plain_bytes, sealed_bytes,
                    sections_json, includes_accounts, sha256
               FROM vault_backups WHERE user_id = ?1",
        )?;
        dump(
            "blend_memberships",
            "SELECT playlist_id, state, invited_at FROM blend_members
              WHERE username = ?1 COLLATE NOCASE",
        )?;
        dump(
            "jam_recaps",
            "SELECT id, created_at, payload_json FROM jam_recaps
              WHERE username = ?1 COLLATE NOCASE",
        )?;
        dump(
            "security_log",
            "SELECT at, kind, client_ip, device_label, detail FROM security_events
              WHERE user_id = ?1 COLLATE NOCASE",
        )?;
        dump(
            "linked_identities",
            "SELECT issuer, subject, linked_at FROM federated_identities
              WHERE user_id = (SELECT id FROM users WHERE username = ?1 COLLATE NOCASE)",
        )?;
        // The settings blob is included as stored — sealed. The server cannot open it, and the
        // client that exported it holds the key.
        dump(
            "settings_vault",
            "SELECT * FROM synced_settings WHERE user_id = ?1",
        )?;

        Ok(serde_json::Value::Object(export))
    }
}
