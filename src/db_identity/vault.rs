//! The zero-knowledge vault envelope: the server stores only the wrapped key and its salt.

use super::*;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// The account's vault key, sealed under its passphrase, and the salt that sealing used.
    ///
    /// Handed to a client at login so it can unwrap the key and read its own settings. Inert on
    /// this server: unwrapping needs the passphrase, and all that is kept of that is an Argon2
    /// hash. `(None, None)` for an account that has not set one up yet — a client seeing that
    /// generates a fresh key and enrols it.
    ///
    /// This replaced `settings_key`, which was a key the *server* minted and stored in the row
    /// beside the ciphertext it opened, and which therefore protected the settings from nobody who
    /// could read the file. See migration 27.
    pub fn vault_envelope(&self, username: &str) -> Result<(Option<String>, Option<String>)> {
        let conn = self.conn.lock_recover();
        conn.query_row(
            "SELECT vault_salt, vault_key_wrapped FROM users WHERE username = ?1 COLLATE NOCASE",
            params![username.trim()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map(|found| found.unwrap_or((None, None)))
    }

    /// Records a client-generated vault key, sealed under the account passphrase.
    ///
    /// Write-once by design: the `WHERE vault_key_wrapped IS NULL` clause means a second device
    /// enrolling concurrently cannot replace a key that is already sealing live settings, which
    /// would strand them. Rotating a key is a different operation — it has to re-seal the settings
    /// in the same breath — and deliberately is not this one. Returns whether the enrolment landed.
    pub fn enrol_vault_key(&self, username: &str, salt: &str, wrapped: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET vault_salt = ?2, vault_key_wrapped = ?3
              WHERE username = ?1 COLLATE NOCASE AND vault_key_wrapped IS NULL",
            params![username.trim(), salt, wrapped],
        )?;
        Ok(changed > 0)
    }
}
