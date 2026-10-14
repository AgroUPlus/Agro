//! Profiles, device keys and discovery.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    pub fn profile(&self, username: &str) -> Result<Option<Profile>> {
        let conn = self.read();
        let profile = conn
            .query_row(
                &format!(
                    "SELECT {PROFILE_COLUMNS} FROM users
                      WHERE username = ?1 COLLATE NOCASE AND state = 'active'"
                ),
                params![username.trim()],
                profile_from_row,
            )
            .optional()?;
        Ok(profile)
    }

    pub fn update_profile(
        &self,
        username: &str,
        display_name: Option<&str>,
        bio: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock_recover();
        // `COALESCE(?, column)` so omitting a field leaves it alone rather than blanking it — the
        // clients send partial updates and a null must not be a deletion.
        let changed = conn.execute(
            "UPDATE users
                SET display_name = COALESCE(?1, display_name),
                    bio          = COALESCE(?2, bio),
                    avatar_url   = COALESCE(?3, avatar_url)
              WHERE username = ?4 COLLATE NOCASE",
            params![display_name, bio, avatar_url, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Writes the account-wide key column.
    ///
    /// Kept, and kept written, only as the mirror older clients read. It is last-writer-wins by
    /// construction — which is the bug [`register_device_key`] exists to fix — so nothing that
    /// seals a message should read this. Use [`device_keys_for`](Self::device_keys_for).
    pub fn set_public_key(&self, username: &str, public_key: Option<&str>) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET public_key = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![public_key, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Records `public_key` as the key held by one device of `username`.
    ///
    /// An upsert on `(user_id, device_id)`, so a device that regenerated its keypair — a
    /// reinstall, or a restore that could not carry the private half — replaces its own entry and
    /// only its own. That is the entire difference from [`set_public_key`](Self::set_public_key),
    /// where writing the single column replaced every other device's key as a side effect.
    ///
    /// The `legacy` row migration 36 carried over is dropped once a real device claims the same
    /// key, so an account that has signed in again is not sealed to twice for one device.
    pub fn register_device_key(
        &self,
        username: &str,
        device_id: &str,
        public_key: &str,
    ) -> Result<()> {
        let conn = self.conn.lock_recover();
        let user = username.trim();
        let device = device_id.trim();
        let key = public_key.trim();
        conn.execute(
            "INSERT INTO user_device_keys (user_id, device_id, public_key, registered_at)
                  VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, device_id) DO UPDATE
                SET public_key = excluded.public_key, registered_at = excluded.registered_at",
            params![user, device, key, chrono::Utc::now().to_rfc3339()],
        )?;
        if device != LEGACY_DEVICE_ID {
            conn.execute(
                "DELETE FROM user_device_keys
                   WHERE user_id = ?1 COLLATE NOCASE AND device_id = ?2 AND public_key = ?3",
                params![user, LEGACY_DEVICE_ID, key],
            )?;
        }
        Ok(())
    }

    /// Every device key published for `username`, oldest registration first.
    ///
    /// Deciding whether the caller may ask is not this method's job — `deviceKeys` in
    /// `schema_social` gates on friendship, with the same message `dropTrack` uses, so that a key
    /// list cannot be used to probe for accounts.
    pub fn device_keys_for(&self, username: &str) -> Result<Vec<DeviceKey>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT device_id, public_key FROM user_device_keys
              WHERE user_id = ?1 COLLATE NOCASE
              ORDER BY registered_at ASC, device_id ASC",
        )?;
        let keys = stmt
            .query_map(params![username.trim()], |row| {
                Ok(DeviceKey {
                    device_id: row.get(0)?,
                    public_key: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(keys)
    }

    /// Removes one device's key, so a phone that is signed out stops being sealed to.
    ///
    /// Anything already sealed to it goes with it. A ciphertext addressed to a key that has been
    /// withdrawn can never be opened by anyone, so what is left behind is not a message the device
    /// might still come back for — it is a secret being stored on nobody's behalf.
    pub fn forget_device_key(&self, username: &str, device_id: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let removed = conn.execute(
            "DELETE FROM user_device_keys WHERE user_id = ?1 COLLATE NOCASE AND device_id = ?2",
            params![username.trim(), device_id.trim()],
        )?;
        Db::forget_presence_for_device_in(&conn, username, device_id.trim())?;
        conn.execute(
            "DELETE FROM drop_note_ciphertexts WHERE device_id = ?1",
            params![device_id.trim()],
        )?;
        Ok(removed > 0)
    }

    pub fn set_discoverable(&self, username: &str, discoverable: bool) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET discoverable = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![discoverable as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// The public user directory.
    ///
    /// Three conditions, all of which must hold: the account is active, it has opted into being
    /// discoverable, and it is not the searcher themselves. Blocks are *not* filtered here — that
    /// would make an absence from the results a signal — they are filtered by the resolver, which
    /// removes them along with everything else it will not show.
    ///
    /// Prefix-anchored rather than `%term%`: a substring search over a public directory lets anyone
    /// enumerate the whole thing one letter at a time.
    pub fn search_users(&self, searcher: &str, query: &str, limit: i64) -> Result<Vec<Profile>> {
        let term = query.trim().to_lowercase();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("{}%", term.replace(['%', '_'], ""));
        let conn = self.conn.lock_recover();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM users
              WHERE state = 'active' AND discoverable = 1
                AND username <> ?1 COLLATE NOCASE
                AND (username LIKE ?2 COLLATE NOCASE OR display_name LIKE ?2 COLLATE NOCASE)
              ORDER BY username
              LIMIT ?3"
        ))?;
        let found = stmt
            .query_map(
                params![searcher.trim(), pattern, limit.clamp(1, 20)],
                profile_from_row,
            )?
            .collect::<Result<Vec<_>>>()?;
        Ok(found)
    }

    /// Accounts waiting to be let in.
    pub fn pending_accounts(&self) -> Result<Vec<Profile>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM users WHERE state = 'pending' ORDER BY created_at"
        ))?;
        let pending = stmt
            .query_map([], profile_from_row)?
            .collect::<Result<Vec<_>>>()?;
        Ok(pending)
    }
}
