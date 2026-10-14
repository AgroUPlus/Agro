//! Device bearer tokens: minting, lookup, idle expiry and revocation.

use super::*;
use crate::credentials;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// Mints a device token, stores only its hash, and returns the secret to show once.
    pub fn mint_device_token(&self, username: &str, label: &str) -> Result<String> {
        let minted = credentials::mint_token();
        let conn = self.conn.lock_recover();
        let user_id: String = conn.query_row(
            "SELECT id FROM users WHERE username = ?1 COLLATE NOCASE",
            params![username.trim()],
            |row| row.get(0),
        )?;
        let expires_at = token_ttl_days()
            .map(|days| (chrono::Utc::now() + chrono::Duration::days(days)).to_rfc3339());
        conn.execute(
            "INSERT INTO app_passwords
                 (token, user_id, label, created_at, token_prefix, token_hash, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                // The legacy plaintext column is a NOT NULL primary key, so it still needs a
                // distinct value. The hash is exactly that, and is not a usable credential.
                minted.hash,
                user_id,
                label.trim(),
                chrono::Utc::now().to_rfc3339(),
                minted.prefix,
                minted.hash,
                expires_at,
            ],
        )?;
        Ok(minted.secret)
    }

    /// Resolves a presented bearer token to its account.
    ///
    /// Looked up by the token's clear prefix — an indexed equality — then confirmed by comparing
    /// hashes in constant time. The passphrase is deliberately *not* accepted here any more; it
    /// buys a device token through `login` and is never itself presented.
    /// Resolves a bearer token to its account **and the label its owner gave that device**.
    pub fn account_for_token(&self, token: &str) -> Result<Option<(Account, String)>> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        let prefix = credentials::token_prefix(token);
        let presented = credentials::hash_token(token);

        type Candidate = (
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            String,
        );
        let candidates: Vec<Candidate> = {
            let conn = self.read();
            let mut stmt = conn.prepare(
                "SELECT a.token_hash, u.username, a.last_used_at, a.label, a.expires_at,
                        a.created_at
                   FROM app_passwords a
                   JOIN users u ON u.id = a.user_id
                  WHERE a.token_prefix = ?1 AND a.token_hash <> ''",
            )?;
            let rows = stmt
                .query_map(params![prefix], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })?
                .collect::<Result<_>>()?;
            rows
        };

        let Some((_, username, last_used, label, expires_at, created_at)) = candidates
            .into_iter()
            .find(|(hash, ..): &Candidate| credentials::secure_eq(hash, &presented))
        else {
            return Ok(None);
        };

        // Expiry is checked *after* the constant-time hash comparison, so an expired token and a
        // forged one take the same path and cannot be told apart by timing.
        if token_has_expired(expires_at.as_deref(), last_used.as_deref(), &created_at) {
            return Ok(None);
        }

        self.touch_token_if_stale(&presented, last_used.as_deref());
        // The label travels with the account because it is the name a *person* gave this device.
        // Without it the server had to invent one at registration, so the same phone ended up with
        // a typed name on its credential and a random one on its playing session.
        Ok(self.account(&username)?.map(|account| (account, label)))
    }

    /// Revokes every token on an account, optionally sparing one.
    ///
    /// This is the operation that gives every other credential change its meaning. Enrolling a
    /// second factor, changing a passphrase or disabling 2FA all have the same hole without it:
    /// whoever held the old passphrase already traded it for a token, and that token outlives the
    /// change that was supposed to shut them out.
    ///
    /// `except` is the *stored hash* of the caller's own token, so "sign out my other devices"
    /// does not sign out the device asking. It is the hash rather than the secret because that is
    /// what [`crate::auth::AuthedUser`] carries and what the table stores; nothing here needs the
    /// secret. Pass `None` to revoke everything including the caller's.
    ///
    /// Returns how many were revoked.
    pub fn revoke_all_tokens(&self, username: &str, except: Option<&str>) -> Result<usize> {
        let spare = except.unwrap_or_default();
        let conn = self.conn.lock_recover();
        let removed = conn.execute(
            "DELETE FROM app_passwords
              WHERE user_id = (SELECT id FROM users WHERE username = ?1 COLLATE NOCASE)
                AND token_hash <> ?2",
            params![username.trim(), spare],
        )?;
        Ok(removed)
    }

    /// Deletes tokens that have expired or gone idle.
    ///
    /// Enforcement does not depend on this — [`Self::account_for_token`] refuses an expired token
    /// whether or not it has been swept. This is housekeeping, so a table of dead credentials does
    /// not grow forever.
    pub fn sweep_expired_tokens(&self) -> Result<usize> {
        let now = chrono::Utc::now();
        let idle_days = token_idle_days();
        let conn = self.conn.lock_recover();

        let mut removed = conn.execute(
            "DELETE FROM app_passwords WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![now.to_rfc3339()],
        )?;

        if idle_days > 0 {
            let cutoff = (now - chrono::Duration::days(idle_days)).to_rfc3339();
            // `created_at` stands in for a token that has never been presented, so a token minted
            // and forgotten is swept on the same schedule as one that was used and abandoned.
            removed += conn.execute(
                "DELETE FROM app_passwords WHERE COALESCE(last_used_at, created_at) < ?1",
                params![cutoff],
            )?;
        }
        Ok(removed)
    }

    /// Refreshes `last_used_at`, but only once an hour per token.
    ///
    /// Best-effort: a failure here must never turn a valid request into a 401.
    fn touch_token_if_stale(&self, token_hash: &str, last_used: Option<&str>) {
        let now = chrono::Utc::now();
        let stale = match last_used.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) {
            Some(seen) => {
                (now - seen.with_timezone(&chrono::Utc)).num_seconds() >= LAST_USED_REFRESH_SECS
            }
            None => true,
        };
        if !stale {
            return;
        }
        let conn = self.conn.lock_recover();
        let _ = conn.execute(
            "UPDATE app_passwords SET last_used_at = ?1 WHERE token_hash = ?2",
            params![now.to_rfc3339(), token_hash],
        );
    }
}
