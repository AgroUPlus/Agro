//! Federated (OIDC) identities linked to accounts, and passphrase-less sign-in.

use super::*;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// The account an `(issuer, subject)` is linked to, if any.
    pub fn account_for_federated_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<Account>> {
        let username: Option<String> = {
            let conn = self.read();
            conn.query_row(
                "SELECT u.username FROM federated_identities f
                   JOIN users u ON u.id = f.user_id
                  WHERE f.issuer = ?1 AND f.subject = ?2",
                params![issuer.trim(), subject.trim()],
                |row| row.get(0),
            )
            .optional()?
        };
        match username {
            Some(name) => self.account(&name),
            None => Ok(None),
        }
    }

    /// Links an identity to an account that has already been authenticated by other means.
    ///
    /// Refuses when the identity is already linked somewhere — a subject maps to at most one
    /// account, and silently re-pointing it would strand the account it used to reach.
    pub fn link_federated_identity(
        &self,
        username: &str,
        issuer: &str,
        subject: &str,
        claims: Option<&str>,
    ) -> Result<(), String> {
        if let Ok(Some(existing)) = self.account_for_federated_identity(issuer, subject) {
            return if existing.username.eq_ignore_ascii_case(username.trim()) {
                Ok(())
            } else {
                Err("That identity is already linked to another account".into())
            };
        }
        let conn = self.conn.lock_recover();
        let user_id: String = conn
            .query_row(
                "SELECT id FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username.trim()],
                |row| row.get(0),
            )
            .map_err(|e| format!("no such account: {e}"))?;
        conn.execute(
            "INSERT INTO federated_identities (issuer, subject, user_id, linked_at, claims_snapshot)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                issuer.trim(),
                subject.trim(),
                user_id,
                chrono::Utc::now().to_rfc3339(),
                claims,
            ],
        )
        .map_err(|e| format!("could not link that identity: {e}"))?;
        Ok(())
    }

    /// Unlinks an identity.
    ///
    /// Refuses to remove the last way into an account. An account created through OIDC has a
    /// passphrase it has never been shown, so unlinking without setting one first would lock its
    /// owner out of it permanently.
    pub fn unlink_federated_identity(
        &self,
        username: &str,
        issuer: &str,
        subject: &str,
    ) -> Result<bool, String> {
        let remaining = self
            .federated_identities(username)
            .map_err(|e| e.to_string())?
            .len();
        if remaining <= 1 && !self.has_usable_passphrase(username).unwrap_or(false) {
            return Err(
                "Set a passphrase before unlinking, or there would be no way back into this account"
                    .into(),
            );
        }
        let conn = self.conn.lock_recover();
        let removed = conn
            .execute(
                "DELETE FROM federated_identities
                  WHERE issuer = ?1 AND subject = ?2
                    AND user_id = (SELECT id FROM users WHERE username = ?3 COLLATE NOCASE)",
                params![issuer.trim(), subject.trim(), username.trim()],
            )
            .map_err(|e| format!("could not unlink that identity: {e}"))?;
        Ok(removed > 0)
    }

    /// The identities linked to an account, as `(issuer, subject, linked_at)`.
    pub fn federated_identities(&self, username: &str) -> Result<Vec<(String, String, String)>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT f.issuer, f.subject, f.linked_at FROM federated_identities f
               JOIN users u ON u.id = f.user_id
              WHERE u.username = ?1 COLLATE NOCASE
              ORDER BY f.linked_at",
        )?;
        let rows = stmt
            .query_map(params![username.trim()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Whether this account has a passphrase its owner could actually use.
    ///
    /// An account created through OIDC gets a generated passphrase it is never shown, which exists
    /// only so the column is not empty. `passphrase_is_usable` records whether anyone has ever been
    /// told what it is — a hash alone cannot answer that.
    pub fn has_usable_passphrase(&self, username: &str) -> Result<bool> {
        let conn = self.read();
        let usable: Option<i64> = conn
            .query_row(
                "SELECT passphrase_is_usable FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username.trim()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(usable.unwrap_or(1) != 0)
    }

    /// Records that this account's passphrase is one nobody has ever been shown.
    ///
    /// Set on accounts created through SSO. It is what [`Self::unlink_federated_identity`] consults
    /// before removing the last way into an account.
    pub fn mark_passphrase_unusable(&self, username: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE users SET passphrase_is_usable = 0 WHERE username = ?1 COLLATE NOCASE",
            params![username.trim()],
        )?;
        Ok(())
    }

    /// Finds a username derived from `preferred` that nobody is using.
    ///
    /// **Never returns an existing account.** On a collision it suffixes a number, because falling
    /// back to the account that already holds the name is precisely the takeover this whole design
    /// exists to prevent: an IdP admin who sets `preferred_username` to `alpha` must get `alpha2`,
    /// not `alpha`.
    pub fn available_username_like(&self, preferred: &str) -> Option<String> {
        let base = crate::login::normalise_username(preferred)?;
        if self.account(&base).ok().flatten().is_none() {
            return Some(base);
        }
        // Trimmed so the suffix cannot push it past the 32-character limit the rule enforces.
        let stem: String = base.chars().take(28).collect();
        (2..=9999).find_map(|n| {
            let candidate = format!("{stem}{n}");
            self.account(&candidate)
                .ok()
                .flatten()
                .is_none()
                .then_some(candidate)
        })
    }
}
