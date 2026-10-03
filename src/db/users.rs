//! Accounts at the level of the original `users` table: creating, finding, listing and removing
//! them. Roles, quotas and state live in `db_identity`.

use rusqlite::{params, OptionalExtension, Result};

use super::Db;

impl Db {
    pub fn create_user(&self, username: &str, api_key: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let user_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO users (id, username, api_key, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(username) DO UPDATE SET api_key = excluded.api_key",
            params![user_id, username, api_key, now],
        )?;
        Ok(user_id)
    }

    pub fn get_or_create_user(
        &self,
        username: &str,
        preferred_passphrase: Option<&str>,
    ) -> Result<(String, String)> {
        if let Some((id, _, key)) = self.get_user_by_username(username)? {
            return Ok((id, key));
        }
        let passphrase = preferred_passphrase
            .filter(|p| !p.trim().is_empty())
            .map(String::from)
            .unwrap_or_else(crate::passphrase::generate_passphrase);
        let user_id = self.create_user(username, &passphrase)?;
        Ok((user_id, passphrase))
    }

    /// Removes an account and everything that belongs to it. Deliberately thorough: leaving a
    /// user's nodes, session and settings behind would let a recreated account inherit them.
    pub fn delete_user(&self, username: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let user_id: Option<String> = conn
            .query_row(
                "SELECT id FROM users WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )
            .optional()?;
        let Some(user_id) = user_id else {
            return Ok(false);
        };
        // Every table that stores a username or a user id, or this is not a deletion.
        //
        // It used to be five of them. What survived was the whole social graph — friendships in
        // both directions, drops sent and received, jam membership and votes — plus every scrobble,
        // which is a listening history, and every live share link, which kept working after the
        // account that minted it was gone. "Deleted" has to mean deleted.
        //
        // Two columns are named differently everywhere, so this is a list rather than a loop: some
        // tables key on `users.id` and most key on the username, and `track_drops`/`friendships`
        // key on *two* user columns each.
        let by_id: &[&str] = &[
            "app_passwords",
            "totp_recovery_codes",
            "federated_identities",
        ];
        for table in by_id {
            conn.execute(
                &format!("DELETE FROM {table} WHERE user_id = ?1"),
                params![user_id],
            )?;
        }

        let by_username: &[&str] = &[
            "registered_nodes",
            "device_holdings",
            "handoff_state",
            "synced_settings",
            "scrobbles",
            "friend_codes",
            "ephemeral_shares",
            "short_links",
            "spool_items",
            "upload_sessions",
        ];
        for table in by_username {
            conn.execute(
                &format!("DELETE FROM {table} WHERE user_id = ?1"),
                params![username],
            )?;
        }

        // Friendship is two rows, one per direction. Removing only the row this account owns leaves
        // the other person still holding a friendship with somebody who no longer exists.
        conn.execute(
            "DELETE FROM friendships WHERE user_id = ?1 OR friend_id = ?1",
            params![username],
        )?;
        // Sealed copies are keyed by drop, not by account, so they have to go *before* the drops
        // they hang off — once the `track_drops` row is gone there is nothing left to find them by.
        //
        // They were being left behind entirely: no foreign key declares a cascade for this table,
        // and nothing else deletes from it. A deleted account's ciphertexts accumulated forever,
        // which is both the plainest possible contradiction of "deleted" above and a store of
        // secrets kept on behalf of nobody.
        conn.execute(
            "DELETE FROM drop_note_ciphertexts
              WHERE drop_id IN (SELECT id FROM track_drops WHERE from_user = ?1 OR to_user = ?1)",
            params![username],
        )?;
        // A drop is addressed: sent and received both have to go.
        conn.execute(
            "DELETE FROM track_drops WHERE from_user = ?1 OR to_user = ?1",
            params![username],
        )?;
        // The published keys themselves. Left behind, a recreated account would inherit the public
        // keys of the devices the old one had registered, and senders fetching the registry would
        // seal to keys nobody holds.
        conn.execute(
            "DELETE FROM user_device_keys WHERE user_id = ?1 COLLATE NOCASE",
            params![username],
        )?;
        // Presence copies are addressed the same way a drop is: published and received both go.
        conn.execute(
            "DELETE FROM handoff_presence_ciphertexts WHERE user_id = ?1 OR recipient_user_id = ?1",
            params![username],
        )?;
        conn.execute(
            "DELETE FROM listen_along WHERE listener_id = ?1 OR host_id = ?1",
            params![username],
        )?;
        conn.execute(
            "DELETE FROM jam_members WHERE username = ?1",
            params![username],
        )?;
        conn.execute(
            "DELETE FROM jam_votes WHERE username = ?1",
            params![username],
        )?;
        conn.execute(
            "DELETE FROM jam_skips WHERE username = ?1",
            params![username],
        )?;
        conn.execute(
            "DELETE FROM jam_tracks WHERE added_by = ?1",
            params![username],
        )?;
        conn.execute("DELETE FROM jams WHERE host = ?1", params![username])?;

        // The audit trail is the one thing kept, and only in a form that names nobody: "an account
        // was deleted" is a fact the operator needs, and rows still carrying the username would be
        // a record of the person who asked to be forgotten.
        conn.execute(
            "UPDATE security_events SET user_id = NULL, client_ip = NULL, device_label = NULL
              WHERE user_id = ?1",
            params![username],
        )?;

        conn.execute("DELETE FROM users WHERE id = ?1", params![user_id])?;
        Ok(true)
    }

    pub fn list_users(&self) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt = conn.prepare("SELECT username FROM users ORDER BY created_at ASC")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut users = Vec::new();
        for r in rows {
            users.push(r?);
        }
        if users.is_empty() {
            users.push("alpha".to_string());
        }
        Ok(users)
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<(String, String, String)>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT id, username, api_key FROM users WHERE username = ?1")?;
        let mut rows = stmt.query(params![username])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }
}
