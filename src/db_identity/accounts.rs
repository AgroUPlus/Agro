//! Account lifecycle: creation, lookup and passphrase login.

use super::*;
use crate::credentials;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// Fills in credential hashes for accounts that predate them.
    ///
    /// Migration 9 adds the columns but cannot populate them: hashing is not something SQL can do.
    /// The old plaintext `api_key` doubled as the passphrase, so it is hashed into
    /// `passphrase_hash` here — the operator keeps the passphrase they already know, while it
    /// stops being a bearer token.
    ///
    /// Device tokens are deliberately *not* migrated. Their plaintext column stays unreadable by
    /// the new lookup, so every device re-pairs exactly once. That is the intended break: a token
    /// that was stored in the clear should not survive the change that stopped storing it that way.
    pub fn migrate_credentials(&self) -> Result<usize> {
        let conn = self.conn.lock_recover();
        let mut stmt = conn.prepare(
            "SELECT id, api_key FROM users WHERE passphrase_hash = '' AND api_key <> ''",
        )?;
        let pending: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_>>()?;
        drop(stmt);

        let mut migrated = 0;
        for (id, api_key) in pending {
            let Ok(hash) = credentials::hash_passphrase(&api_key) else {
                continue;
            };
            conn.execute(
                "UPDATE users SET passphrase_hash = ?1 WHERE id = ?2",
                params![hash, id],
            )?;
            migrated += 1;
        }
        Ok(migrated)
    }

    pub fn account(&self, username: &str) -> Result<Option<Account>> {
        let conn = self.read();
        conn.query_row(
            &format!("SELECT {ACCOUNT_COLUMNS} FROM users WHERE username = ?1 COLLATE NOCASE"),
            params![username.trim()],
            account_from_row,
        )
        .optional()
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let conn = self.conn.lock_recover();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM users ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map([], account_from_row)?;
        rows.collect()
    }

    /// Creates an account with a hashed passphrase.
    ///
    /// Fails rather than overwriting when the username is taken. The old `create_user` had an
    /// `ON CONFLICT DO UPDATE SET api_key = excluded.api_key`, which meant creating an account that
    /// already existed silently *reset its credential* — a takeover primitive once anyone but the
    /// operator could call it.
    pub fn create_account(
        &self,
        username: &str,
        passphrase: &str,
        role: Role,
        state: AccountState,
    ) -> Result<Account> {
        let username = username.trim().to_string();
        let hash = credentials::hash_passphrase(passphrase).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e)))
        })?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let quota = if role.is_admin() {
            0
        } else {
            DEFAULT_GUEST_QUOTA
        };

        {
            let conn = self.conn.lock_recover();
            conn.execute(
                // No `settings_key`: the server does not mint the key to its users' settings any
                // more. The client generates one and enrols it sealed, through `enrol_vault_key`.
                "INSERT INTO users
                   (id, username, api_key, created_at, passphrase_hash, role, state, quota_bytes)
                 VALUES (?1, ?2, '', ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    username,
                    now,
                    hash,
                    role.as_str(),
                    state.as_str(),
                    quota,
                ],
            )?;
        }

        Ok(Account {
            id,
            username,
            role,
            state,
            quota_bytes: quota,
            can_archive: role.is_admin(),
            show_now_playing: false,
            show_stats: false,
        })
    }

    /// Verifies a passphrase and returns the account, or `None`.
    ///
    /// Unlike the `authenticate_user` this replaces, an unknown username is simply a failure. That
    /// function auto-registered whoever asked, which made the login endpoint an open signup form.
    pub fn verify_login(&self, username: &str, passphrase: &str) -> Result<Option<Account>> {
        if username.trim().is_empty() || passphrase.trim().is_empty() {
            return Ok(None);
        }
        let stored: Option<String> = {
            let conn = self.read();
            conn.query_row(
                "SELECT passphrase_hash FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username.trim()],
                |row| row.get(0),
            )
            .optional()?
        };
        let Some(stored) = stored else {
            return Ok(None);
        };
        if !credentials::verify_passphrase(passphrase, &stored) {
            return Ok(None);
        }
        self.account(username)
    }

    /// Changes the passphrase and re-seals the vault under it, in one transaction.
    ///
    /// The two must move together or not at all. The vault key is wrapped by a key derived from the
    /// passphrase, so a passphrase that changed without the envelope being re-sealed leaves settings
    /// that nothing can open — and there is no copy of the old passphrase to recover them with.
    ///
    /// This is why [`Self::enrol_vault_key`]'s write-once rule is not simply relaxed: that rule
    /// stops two devices racing to enrol *first* keys and stranding live settings, and it still
    /// holds. Re-sealing is a different operation with a different precondition — it requires
    /// proving the current passphrase — so it gets its own function rather than a loosened one.
    ///
    /// Every device token is revoked. A passphrase is changed because it may have leaked, and the
    /// tokens bought with it are exactly what the change is meant to invalidate.
    pub fn change_passphrase(
        &self,
        username: &str,
        current: &str,
        next: &str,
        vault: Option<(&str, &str)>,
    ) -> Result<bool, String> {
        if self
            .verify_login(username, current)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Ok(false);
        }
        if next.trim().len() < 12 {
            return Err("A passphrase needs to be at least 12 characters".into());
        }
        let hash = credentials::hash_passphrase(next)?;

        {
            let mut conn = self.conn.lock_recover();
            let tx = conn
                .transaction()
                .map_err(|e| format!("could not change the passphrase: {e}"))?;
            // Also marks the passphrase usable: an SSO account whose owner sets one for the
            // first time now has a second way in, which is what lets them unlink the identity.
            tx.execute(
                "UPDATE users SET passphrase_hash = ?2, passphrase_is_usable = 1
                  WHERE username = ?1 COLLATE NOCASE",
                params![username.trim(), hash],
            )
            .map_err(|e| format!("could not change the passphrase: {e}"))?;

            // Absent when the account never enrolled a vault key. There is nothing to re-seal, and
            // the client will enrol one at its next login.
            if let Some((salt, wrapped)) = vault {
                tx.execute(
                    "UPDATE users SET vault_salt = ?2, vault_key_wrapped = ?3
                      WHERE username = ?1 COLLATE NOCASE",
                    params![username.trim(), salt, wrapped],
                )
                .map_err(|e| format!("could not re-seal the vault: {e}"))?;
            }
            tx.commit()
                .map_err(|e| format!("could not change the passphrase: {e}"))?;
        }

        let _ = self.revoke_all_tokens(username, None);
        Ok(true)
    }

    // ── The second factor ───────────────────────────────────────────────────────────────────
    //
    // Enrolment is two steps on purpose: `begin` writes a secret nobody is held to yet, and
    // `confirm` is what makes it binding. See the module docs on `crate::totp`.

    /// How many admins the server has. Guards the last one against removal or demotion.
    pub fn admin_count(&self) -> Result<i64> {
        let conn = self.read();
        conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin'",
            [],
            |row| row.get(0),
        )
    }
}
