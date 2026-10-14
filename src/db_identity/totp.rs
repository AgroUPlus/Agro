//! Second factor: TOTP enrolment, verification and recovery codes.

use super::*;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// Starts an enrolment, returning the secret to show once.
    ///
    /// Overwrites any *unconfirmed* secret — restarting a scan that did not work must be possible.
    /// A confirmed one is refused, because silently replacing a working second factor is how an
    /// attacker with a live session would remove it.
    pub fn begin_totp_enrolment(&self, username: &str) -> Result<crate::totp::Enrolment, String> {
        if self.totp_is_confirmed(username).unwrap_or(false) {
            return Err("Two-factor authentication is already enabled on this account".into());
        }
        let enrolment = crate::totp::begin(username)?;
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE users SET totp_secret_enc = ?2, totp_confirmed_at = NULL, totp_last_step = NULL
              WHERE username = ?1 COLLATE NOCASE",
            params![username.trim(), enrolment.sealed],
        )
        .map_err(|e| format!("could not store the enrolment: {e}"))?;
        Ok(enrolment)
    }

    /// Confirms an enrolment by checking a code from the pending secret.
    ///
    /// On success the account gains a second factor and a set of recovery codes, and **every other
    /// device token is revoked**: whoever held the passphrase before this moment already traded it
    /// for tokens, and leaving those alive would make the whole exercise decorative.
    ///
    /// Returns the recovery codes, to be shown once.
    pub fn confirm_totp_enrolment(
        &self,
        username: &str,
        code: &str,
        spare_token_hash: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let sealed = self
            .totp_secret_column(username)
            .map_err(|e| format!("could not read the enrolment: {e}"))?
            .ok_or("No enrolment is in progress for this account")?;
        if self.totp_is_confirmed(username).unwrap_or(false) {
            return Err("Two-factor authentication is already enabled on this account".into());
        }

        let secret = crate::totp::unseal(&sealed)?;
        let now = chrono::Utc::now().timestamp() as u64;
        let step = crate::totp::verify(&secret, code, now)
            .ok_or("That code is not right. Check your authenticator and try again.")?;

        let codes = crate::totp::mint_recovery_codes();
        {
            let mut conn = self.conn.lock_recover();
            let tx = conn
                .transaction()
                .map_err(|e| format!("could not confirm the enrolment: {e}"))?;
            let user_id: String = tx
                .query_row(
                    "SELECT id FROM users WHERE username = ?1 COLLATE NOCASE",
                    params![username.trim()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("no such account: {e}"))?;
            tx.execute(
                "UPDATE users SET totp_confirmed_at = ?2, totp_last_step = ?3
                  WHERE username = ?1 COLLATE NOCASE",
                params![
                    username.trim(),
                    chrono::Utc::now().to_rfc3339(),
                    step as i64
                ],
            )
            .map_err(|e| format!("could not confirm the enrolment: {e}"))?;
            // Replaces any codes from an earlier enrolment, so a set someone wrote down two years
            // ago cannot be used against the factor enrolled today.
            tx.execute(
                "DELETE FROM totp_recovery_codes WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(|e| format!("could not clear old recovery codes: {e}"))?;
            let now_rfc = chrono::Utc::now().to_rfc3339();
            for code in &codes {
                tx.execute(
                    "INSERT INTO totp_recovery_codes (user_id, code_hash, created_at)
                     VALUES (?1, ?2, ?3)",
                    params![user_id, crate::totp::hash_recovery_code(code), now_rfc],
                )
                .map_err(|e| format!("could not store a recovery code: {e}"))?;
            }
            tx.commit()
                .map_err(|e| format!("could not confirm the enrolment: {e}"))?;
        }

        let _ = self.revoke_all_tokens(username, spare_token_hash);
        Ok(codes)
    }

    /// Checks a code — or a recovery code — against a confirmed enrolment.
    ///
    /// Returns `Ok(true)` when the second factor is satisfied. The replay guard lives here: a code
    /// is good for a whole 30-second step, so a step already accepted is refused even though the
    /// code itself is arithmetically correct. RFC 6238 §5.2 requires exactly this.
    pub fn verify_totp(&self, username: &str, code: &str) -> Result<TotpOutcome> {
        let Some(sealed) = self.totp_secret_column(username)? else {
            return Ok(TotpOutcome::NotEnrolled);
        };
        if !self.totp_is_confirmed(username)? {
            return Ok(TotpOutcome::NotEnrolled);
        }

        // Tried first, because a recovery code is not six digits and would never match a TOTP.
        if self.spend_recovery_code(username, code)? {
            return Ok(TotpOutcome::AcceptedRecoveryCode);
        }

        let Ok(secret) = crate::totp::unseal(&sealed) else {
            // A secret that will not open is a server misconfiguration, not a wrong code. Refusing
            // is right — failing *open* here would disable everyone's second factor at once — but
            // the operator has to be told, because no user can fix it.
            tracing::error!(
                "the stored TOTP secret for an account could not be opened;                  has AGRO_SECRET_KEY changed?"
            );
            return Ok(TotpOutcome::Rejected);
        };

        let now = chrono::Utc::now().timestamp() as u64;
        let Some(step) = crate::totp::verify(&secret, code, now) else {
            return Ok(TotpOutcome::Rejected);
        };

        // The guard, and the reason this is an UPDATE with a WHERE rather than a read followed by a
        // write: two requests presenting the same code at the same instant must not both win, and
        // only one of them can change a row from "last step < this" to "last step = this".
        let conn = self.conn.lock_recover();
        let accepted = conn.execute(
            "UPDATE users SET totp_last_step = ?2
              WHERE username = ?1 COLLATE NOCASE
                AND (totp_last_step IS NULL OR totp_last_step < ?2)",
            params![username.trim(), step as i64],
        )?;
        Ok(if accepted > 0 {
            TotpOutcome::Accepted
        } else {
            TotpOutcome::Replayed
        })
    }

    /// Turns the second factor off, discarding the secret and every recovery code.
    pub fn disable_totp(&self, username: &str) -> Result<bool> {
        let mut conn = self.conn.lock_recover();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE users
                SET totp_secret_enc = NULL, totp_confirmed_at = NULL, totp_last_step = NULL
              WHERE username = ?1 COLLATE NOCASE",
            params![username.trim()],
        )?;
        tx.execute(
            "DELETE FROM totp_recovery_codes
              WHERE user_id = (SELECT id FROM users WHERE username = ?1 COLLATE NOCASE)",
            params![username.trim()],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Whether this account has a *confirmed* second factor. A pending enrolment reads as `false`.
    pub fn totp_is_confirmed(&self, username: &str) -> Result<bool> {
        let conn = self.read();
        let confirmed: Option<Option<String>> = conn
            .query_row(
                "SELECT totp_confirmed_at FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username.trim()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(matches!(confirmed, Some(Some(at)) if !at.is_empty()))
    }

    /// How many unused recovery codes are left, for the account screen to warn on.
    pub fn recovery_codes_remaining(&self, username: &str) -> Result<i64> {
        let conn = self.read();
        conn.query_row(
            "SELECT COUNT(*) FROM totp_recovery_codes
              WHERE user_id = (SELECT id FROM users WHERE username = ?1 COLLATE NOCASE)
                AND used_at IS NULL",
            params![username.trim()],
            |row| row.get(0),
        )
    }

    /// Replaces the recovery codes with a fresh set, returning them to show once.
    pub fn regenerate_recovery_codes(&self, username: &str) -> Result<Vec<String>, String> {
        let codes = crate::totp::mint_recovery_codes();
        let mut conn = self.conn.lock_recover();
        let tx = conn
            .transaction()
            .map_err(|e| format!("could not issue recovery codes: {e}"))?;
        let user_id: String = tx
            .query_row(
                "SELECT id FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username.trim()],
                |row| row.get(0),
            )
            .map_err(|e| format!("no such account: {e}"))?;
        tx.execute(
            "DELETE FROM totp_recovery_codes WHERE user_id = ?1",
            params![user_id],
        )
        .map_err(|e| format!("could not clear the old codes: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        for code in &codes {
            tx.execute(
                "INSERT INTO totp_recovery_codes (user_id, code_hash, created_at)
                 VALUES (?1, ?2, ?3)",
                params![user_id, crate::totp::hash_recovery_code(code), now],
            )
            .map_err(|e| format!("could not store a recovery code: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("could not issue recovery codes: {e}"))?;
        Ok(codes)
    }

    /// Spends one recovery code, if `presented` is an unused one.
    ///
    /// Single-use: the row is marked rather than deleted, so the account screen can still say how
    /// many were issued and the audit log has something to point at.
    fn spend_recovery_code(&self, username: &str, presented: &str) -> Result<bool> {
        let hash = crate::totp::hash_recovery_code(presented);
        // An empty or whitespace-only input hashes to a real digest, which would match nothing —
        // but guarding here means a blank code cannot be a lucky collision away from working.
        if presented.trim().is_empty() {
            return Ok(false);
        }
        let conn = self.conn.lock_recover();
        let spent = conn.execute(
            "UPDATE totp_recovery_codes SET used_at = ?3
              WHERE user_id = (SELECT id FROM users WHERE username = ?1 COLLATE NOCASE)
                AND code_hash = ?2 AND used_at IS NULL",
            params![username.trim(), hash, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(spent > 0)
    }

    fn totp_secret_column(&self, username: &str) -> Result<Option<String>> {
        let conn = self.conn.lock_recover();
        let sealed: Option<Option<String>> = conn
            .query_row(
                "SELECT totp_secret_enc FROM users WHERE username = ?1 COLLATE NOCASE",
                params![username.trim()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(sealed.flatten().filter(|s| !s.is_empty()))
    }

    // ── Federated (OIDC) identities ─────────────────────────────────────────────────────────
    //
    // The rule that shapes all of this: **an unauthenticated callback can log into an already-
    // linked account or create a brand-new one, and can never attach itself to an existing one.**
    // Linking happens from inside a signed-in session and nowhere else. That makes the dangerous
    // case unreachable rather than defended against — there is no claim-matching heuristic here to
    // get wrong, because there is no claim matching at all.
}
