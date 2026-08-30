//! Accounts, roles and bearer tokens.
//!
//! A second `impl Db` block, the way [`crate::db_library`] is one, so `db.rs` does not grow another
//! few hundred lines of unrelated SQL.
//!
//! The model this replaces had exactly one kind of account. Every token could do everything, the
//! account passphrase *was* the bearer token, and both it and the device tokens were stored in the
//! clear. What lives here instead: one admin who owns the deployment, guests who can reach nothing
//! but their own data, and credentials that are hashed at rest.

use rusqlite::{params, OptionalExtension, Result};

use crate::credentials;
use crate::db::Db;

/// How much spool a new guest may occupy, in bytes. Ten mebibytes — enough to stage a queue, not
/// enough to be storage.
pub const DEFAULT_GUEST_QUOTA: i64 = 10 * 1024 * 1024;

/// How stale `last_used_at` is allowed to get before a request pays to refresh it.
///
/// This used to be written on *every* authenticated request, which put a database write — under
/// the global connection mutex — in the hot path of every read. The dashboard only shows this to
/// the hour, so an hour is all the precision it needs to be worth.
const LAST_USED_REFRESH_SECS: i64 = 3600;

/// How long a token may go unused before it stops working, from `AGRO_TOKEN_IDLE_DAYS`.
///
/// Idle expiry rather than absolute expiry, because the devices holding these tokens are a phone
/// and a terminal, not a browser tab: a credential that expires on a schedule makes a sync daemon
/// stop syncing at 3am for no reason a user can see. A credential that expires because nothing has
/// presented it in half a year is a device that is gone.
///
/// `0` disables it. The default is six months.
const DEFAULT_TOKEN_IDLE_DAYS: i64 = 180;

/// Whether a token should no longer be accepted, by absolute expiry or by disuse.
///
/// Unparseable timestamps are treated as *not* expired. A corrupt column must not silently log
/// everybody out, and the sweep will not remove the row either — the mismatch stays visible rather
/// than becoming a fleet-wide outage.
fn token_has_expired(expires_at: Option<&str>, last_used_at: Option<&str>, created_at: &str) -> bool {
    let now = chrono::Utc::now();

    if let Some(parsed) = expires_at.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) {
        if parsed.with_timezone(&chrono::Utc) <= now {
            return true;
        }
    }

    let idle_days = token_idle_days();
    if idle_days == 0 {
        return false;
    }
    // `last_used_at` is NULL until the token is first presented, so a token that was minted and
    // never redeemed ages from `created_at` instead. This is the same `COALESCE` the sweep in
    // `sweep_expired_tokens` uses, and the two must agree: a token the sweep would delete must not
    // still authenticate in the window before the sweep runs.
    let Some(seen) = last_used_at
        .or(Some(created_at))
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    else {
        return false;
    };
    (now - seen.with_timezone(&chrono::Utc)).num_days() >= idle_days
}

/// An optional absolute lifetime for newly minted tokens, from `AGRO_TOKEN_TTL_DAYS`.
///
/// Unset — the default — means no fixed expiry, because the devices holding these tokens are a
/// phone and a terminal and a sync daemon that stops working on a schedule is a support ticket.
/// Operators who want a hard ceiling can set one; idle expiry covers the common case on its own.
fn token_ttl_days() -> Option<i64> {
    std::env::var("AGRO_TOKEN_TTL_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
}

fn token_idle_days() -> i64 {
    std::env::var("AGRO_TOKEN_IDLE_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(DEFAULT_TOKEN_IDLE_DAYS)
}

/// What checking a second factor concluded.
///
/// Four outcomes rather than a bool, because the caller treats them differently: a replay must not
/// read as a wrong code (it means someone is reusing a code that was already spent, which is worth
/// logging as its own thing), and "not enrolled" must not read as "accepted".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TotpOutcome {
    Accepted,
    /// A recovery code was spent. The caller records this separately: it means the authenticator is
    /// gone, or someone else has the codes.
    AcceptedRecoveryCode,
    Rejected,
    /// Arithmetically correct, but for a time step already used.
    Replayed,
    NotEnrolled,
}

impl TotpOutcome {
    pub fn is_satisfied(self) -> bool {
        matches!(self, TotpOutcome::Accepted | TotpOutcome::AcceptedRecoveryCode)
    }
}

/// One row of the security log, as read back for display.
#[derive(Clone, Debug)]
pub struct SecurityEvent {
    pub id: i64,
    pub at: String,
    pub user_id: Option<String>,
    pub kind: String,
    pub client_ip: Option<String>,
    pub device_label: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Owns the deployment: the library, the settings, and every other account.
    Admin,
    /// A guest. Their own queue, their own friends, and nothing else.
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Anything that is not exactly `admin` is a member.
    ///
    /// Deliberately not a round-trip of [`Self::as_str`]: a corrupt or unrecognised value in the
    /// column must fall to the *least* privileged reading, never to the most.
    pub fn parse(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("admin") {
            Role::Admin
        } else {
            Role::Member
        }
    }

    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountState {
    /// Invited and registered, but not yet let in by the admin.
    Pending,
    Active,
    /// Was active; the admin took it away. Distinct from deletion so the history survives.
    Suspended,
}

impl AccountState {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountState::Pending => "pending",
            AccountState::Active => "active",
            AccountState::Suspended => "suspended",
        }
    }

    /// Unrecognised values read as `Suspended`, for the same reason [`Role::parse`] falls to
    /// `Member`: an unreadable state must not be an open door.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "active" => AccountState::Active,
            "pending" => AccountState::Pending,
            _ => AccountState::Suspended,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, AccountState::Active)
    }
}

/// An account as the authorization layer needs to see it.
#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub state: AccountState,
    /// `0` means unlimited, which is what the admin has.
    pub quota_bytes: i64,
    pub can_archive: bool,
    pub show_now_playing: bool,
    pub show_stats: bool,
}

impl Account {
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    pub fn can_archive(&self) -> bool {
        self.role.is_admin() || self.can_archive
    }

    /// The admin is never capped. A quota on the person who owns the disk is theatre.
    pub fn effective_quota(&self) -> Option<i64> {
        if self.role.is_admin() || self.quota_bytes <= 0 {
            None
        } else {
            Some(self.quota_bytes)
        }
    }
}

const ACCOUNT_COLUMNS: &str =
    "id, username, role, state, quota_bytes, show_now_playing, show_stats, COALESCE(can_archive, 0)";

fn account_from_row(row: &rusqlite::Row<'_>) -> Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        username: row.get(1)?,
        role: Role::parse(&row.get::<_, String>(2)?),
        state: AccountState::parse(&row.get::<_, String>(3)?),
        quota_bytes: row.get(4)?,
        show_now_playing: row.get::<_, i64>(5)? != 0,
        show_stats: row.get::<_, i64>(6)? != 0,
        can_archive: row.get::<_, i64>(7)? != 0,
    })
}

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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET vault_salt = ?2, vault_key_wrapped = ?3
              WHERE username = ?1 COLLATE NOCASE AND vault_key_wrapped IS NULL",
            params![username.trim(), salt, wrapped],
        )?;
        Ok(changed > 0)
    }

    pub fn account(&self, username: &str) -> Result<Option<Account>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {ACCOUNT_COLUMNS} FROM users WHERE username = ?1 COLLATE NOCASE"),
            params![username.trim()],
            account_from_row,
        )
        .optional()
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare(&format!("SELECT {ACCOUNT_COLUMNS} FROM users ORDER BY created_at ASC"))?;
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
        let quota = if role.is_admin() { 0 } else { DEFAULT_GUEST_QUOTA };

        {
            let conn = self.conn.lock().unwrap();
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
            let conn = self.conn.lock().unwrap();
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

    /// Mints a device token, stores only its hash, and returns the secret to show once.
    pub fn mint_device_token(&self, username: &str, label: &str) -> Result<String> {
        let minted = credentials::mint_token();
        let conn = self.conn.lock().unwrap();
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

        type Candidate = (String, String, Option<String>, String, Option<String>, String);
        let candidates: Vec<Candidate> = {
            let conn = self.conn.lock().unwrap();
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
        if self.verify_login(username, current).map_err(|e| e.to_string())?.is_none() {
            return Ok(false);
        }
        if next.trim().len() < 12 {
            return Err("A passphrase needs to be at least 12 characters".into());
        }
        let hash = credentials::hash_passphrase(next)?;

        {
            let mut conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
            let mut conn = self.conn.lock().unwrap();
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
                params![username.trim(), chrono::Utc::now().to_rfc3339(), step as i64],
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
        let conn = self.conn.lock().unwrap();
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
        let mut conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let mut conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let spent = conn.execute(
            "UPDATE totp_recovery_codes SET used_at = ?3
              WHERE user_id = (SELECT id FROM users WHERE username = ?1 COLLATE NOCASE)
                AND code_hash = ?2 AND used_at IS NULL",
            params![username.trim(), hash, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(spent > 0)
    }

    fn totp_secret_column(&self, username: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
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

    /// The account an `(issuer, subject)` is linked to, if any.
    pub fn account_for_federated_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<Account>> {
        let username: Option<String> = {
            let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut export = serde_json::Map::new();

        let mut dump = |label: &str, sql: &str| -> Result<()> {
            let mut stmt = conn.prepare(sql)?;
            let columns: Vec<String> =
                stmt.column_names().into_iter().map(str::to_string).collect();
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
        dump("devices", "SELECT * FROM registered_nodes WHERE user_id = ?1")?;
        dump("listening_history", "SELECT * FROM scrobbles WHERE user_id = ?1")?;
        dump(
            "friendships",
            "SELECT * FROM friendships WHERE user_id = ?1 OR friend_id = ?1",
        )?;
        dump(
            "track_drops",
            "SELECT * FROM track_drops WHERE from_user = ?1 OR to_user = ?1",
        )?;
        dump("share_links", "SELECT * FROM short_links WHERE user_id = ?1")?;
        dump("library_holdings", "SELECT * FROM device_holdings WHERE user_id = ?1")?;
        dump("handoff", "SELECT * FROM handoff_state WHERE user_id = ?1")?;
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
        dump("settings_vault", "SELECT * FROM synced_settings WHERE user_id = ?1")?;

        Ok(serde_json::Value::Object(export))
    }

    /// Appends one event to the security log.
    ///
    /// Best-effort and infallible by design: a failure is logged and swallowed. The alternative is
    /// that a full disk or a locked database turns every login into a 500, which is a worse outcome
    /// than a gap in the record. Callers therefore do not have to decide what to do when auditing
    /// fails, which means they will not decide wrongly.
    pub fn record_event(&self, event: crate::audit::Event, record: crate::audit::Record) {
        let conn = self.conn.lock().unwrap();
        let result = conn.execute(
            "INSERT INTO security_events (at, user_id, kind, client_ip, device_label, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                chrono::Utc::now().to_rfc3339(),
                record.user_id,
                event.as_str(),
                record.client_ip,
                record.device_label,
                record.detail,
            ],
        );
        if let Err(e) = result {
            tracing::warn!("could not record security event {}: {e}", event.as_str());
        }
    }

    /// The most recent events, newest first.
    ///
    /// `username` scopes to one account; `None` is the server-wide view and is admin-only at the
    /// resolver. `limit` is clamped, because an audit view is not a bulk export.
    pub fn security_events(
        &self,
        username: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SecurityEvent>> {
        let limit = limit.clamp(1, 500);
        let conn = self.conn.lock().unwrap();
        let read = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<Vec<SecurityEvent>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map(args, |row| {
                    Ok(SecurityEvent {
                        id: row.get(0)?,
                        at: row.get(1)?,
                        user_id: row.get(2)?,
                        kind: row.get(3)?,
                        client_ip: row.get(4)?,
                        device_label: row.get(5)?,
                        detail: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>>>()?;
            Ok(rows)
        };

        match username {
            Some(name) => read(
                "SELECT id, at, user_id, kind, client_ip, device_label, detail
                   FROM security_events
                  WHERE user_id = ?1 COLLATE NOCASE
                  ORDER BY at DESC, id DESC LIMIT ?2",
                params![name.trim(), limit],
            ),
            None => read(
                "SELECT id, at, user_id, kind, client_ip, device_label, detail
                   FROM security_events
                  ORDER BY at DESC, id DESC LIMIT ?1",
                params![limit],
            ),
        }
    }

    /// Drops events past the retention window.
    ///
    /// An audit log that grows forever is the privacy liability it was meant to prevent. Runs on
    /// the same sweeper as everything else.
    pub fn sweep_security_events(&self) -> Result<usize> {
        let days = std::env::var("AGRO_AUDIT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|d| *d > 0)
            .unwrap_or(180);
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "DELETE FROM security_events WHERE at < ?1",
            params![cutoff],
        )?)
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();

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
            Some(seen) => (now - seen.with_timezone(&chrono::Utc)).num_seconds()
                >= LAST_USED_REFRESH_SECS,
            None => true,
        };
        if !stale {
            return;
        }
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE app_passwords SET last_used_at = ?1 WHERE token_hash = ?2",
            params![now.to_rfc3339(), token_hash],
        );
    }

    pub fn set_account_state(&self, username: &str, state: AccountState) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET state = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![state.as_str(), username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_account_quota(&self, username: &str, quota_bytes: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET quota_bytes = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![quota_bytes.max(0), username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_can_archive(&self, username: &str, can_archive: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET can_archive = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![if can_archive { 1 } else { 0 }, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Whether accepted friends may browse this account's library.
    ///
    /// Its own setter rather than another parameter on `set_visibility`: sharing a music
    /// collection is a bigger decision than showing a now-playing line, and bundling it into the
    /// same call is how it ends up being turned on by a client that meant to set something else.
    pub fn set_share_library(&self, username: &str, share: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET share_library = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![share as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Whether accepted friends may read this account's listening history.
    ///
    /// Its own setter for the same reason `set_share_library` is one: a history is a bigger
    /// disclosure than a now-playing line, and it should never be switched on as a side effect of
    /// a client sending a whole visibility struct back with one field changed.
    pub fn set_show_activity(&self, username: &str, show: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET show_activity = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![show as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Turns the account quiet, or lets it speak again.
    ///
    /// Deliberately separate from [`Self::set_visibility`]: the switches there are standing
    /// consents the user set once, and incognito is a temporary override of all of them. Folding
    /// the two together would mean leaving incognito had to restore the other switches from
    /// somewhere, which is how a privacy setting silently comes back on.
    pub fn set_incognito(&self, username: &str, incognito: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET incognito = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![incognito as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_visibility(
        &self,
        username: &str,
        show_now_playing: bool,
        show_stats: bool,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET show_now_playing = ?1, show_stats = ?2
              WHERE username = ?3 COLLATE NOCASE",
            params![show_now_playing as i64, show_stats as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// How many admins the server has. Guards the last one against removal or demotion.
    pub fn admin_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin'",
            [],
            |row| row.get(0),
        )
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    /// A new account starts with no vault key. The client is what notices and enrols one.
    #[test]
    fn a_fresh_account_has_no_vault_envelope() {
        let db = db();
        db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
            .unwrap();
        assert_eq!(db.vault_envelope("alpha").unwrap(), (None, None));
    }

    #[test]
    fn an_enrolled_vault_key_comes_back_verbatim() {
        let db = db();
        db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
            .unwrap();

        assert!(db.enrol_vault_key("alpha", "s4lt", "sealed-key").unwrap());
        assert_eq!(
            db.vault_envelope("alpha").unwrap(),
            (Some("s4lt".into()), Some("sealed-key".into()))
        );
    }

    /// Two devices setting up the same account at once must not have the second overwrite the
    /// first — the settings already sealed under the first key would become unreadable.
    #[test]
    fn a_vault_key_cannot_be_replaced_once_set() {
        let db = db();
        db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
            .unwrap();

        assert!(db.enrol_vault_key("alpha", "first-salt", "first-key").unwrap());
        assert!(
            !db.enrol_vault_key("alpha", "second-salt", "second-key").unwrap(),
            "the second enrolment should report that it did not land"
        );
        assert_eq!(
            db.vault_envelope("alpha").unwrap(),
            (Some("first-salt".into()), Some("first-key".into())),
            "the key that is already sealing settings must survive"
        );
    }

    /// The property the whole design exists for: what the server stores about an account's
    /// settings must not be enough to read them. It holds a passphrase *hash* and a key sealed
    /// under the passphrase — never the passphrase, and so never the key.
    #[test]
    fn the_server_stores_nothing_that_unwraps_the_vault() {
        let db = db();
        db.create_account("alpha", "correct horse battery staple", Role::Admin, AccountState::Active)
            .unwrap();
        db.enrol_vault_key("alpha", "s4lt", "sealed-key").unwrap();

        let conn = db.conn.lock().unwrap();
        let (hash, salt, wrapped, api_key): (String, String, String, String) = conn
            .query_row(
                "SELECT passphrase_hash, vault_salt, vault_key_wrapped, api_key FROM users",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();

        for stored in [&hash, &salt, &wrapped, &api_key] {
            assert!(
                !stored.contains("correct horse battery staple"),
                "the passphrase must not be recoverable from {stored}"
            );
        }
        assert!(hash.starts_with("$argon2"), "hashed, not stored: {hash}");

        // And the column that used to hold a server-minted key to these settings is gone from the
        // write path entirely.
        assert_eq!(api_key, "", "api_key is no longer a key to anything");
    }

    #[test]
    fn an_account_round_trips_with_a_hashed_passphrase() {
        let db = db();
        db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
            .unwrap();

        let found = db.account("alpha").unwrap().unwrap();
        assert!(found.is_admin());
        assert!(found.state.is_active());
        assert_eq!(found.effective_quota(), None, "admins are uncapped");

        assert!(db.verify_login("alpha", "open sesame").unwrap().is_some());
        assert!(db.verify_login("alpha", "wrong").unwrap().is_none());
    }

    /// The hole this replaces: logging in as nobody used to *create* nobody.
    #[test]
    fn logging_in_as_an_unknown_user_does_not_create_it() {
        let db = db();
        assert!(db.verify_login("intruder", "whatever").unwrap().is_none());
        assert!(db.account("intruder").unwrap().is_none());
        assert!(db.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn a_username_cannot_be_taken_over_by_recreating_it() {
        let db = db();
        db.create_account("alpha", "original", Role::Admin, AccountState::Active)
            .unwrap();
        assert!(db
            .create_account("alpha", "attacker", Role::Member, AccountState::Active)
            .is_err());
        assert!(db.verify_login("alpha", "original").unwrap().is_some());
        assert!(db.verify_login("alpha", "attacker").unwrap().is_none());
    }

    #[test]
    fn a_device_token_authenticates_and_is_not_stored_in_the_clear() {
        let db = db();
        db.create_account("guest", "pw", Role::Member, AccountState::Active)
            .unwrap();
        let secret = db.mint_device_token("guest", "phone").unwrap();

        let (account, label) = db.account_for_token(&secret).unwrap().unwrap();
        assert_eq!(account.username, "guest");
        assert!(!account.is_admin());
        assert_eq!(account.effective_quota(), Some(DEFAULT_GUEST_QUOTA));
        // The name its owner gave it travels with the token, so registration need not invent one.
        assert_eq!(label, "phone");

        let conn = db.conn.lock().unwrap();
        let stored: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM app_passwords WHERE token_hash = ?1 OR token = ?1",
                params![secret],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, 0, "the raw token must not appear in the database");
    }

    #[test]
    fn a_wrong_token_resolves_to_nobody() {
        let db = db();
        db.create_account("guest", "pw", Role::Member, AccountState::Active)
            .unwrap();
        let secret = db.mint_device_token("guest", "phone").unwrap();

        assert!(db.account_for_token("").unwrap().is_none());
        assert!(db.account_for_token("nonsense").unwrap().is_none());
        // Same prefix, wrong secret: the prefix is only an index, never the check.
        let forged = format!("{}{}", &secret[..8], "x".repeat(40));
        assert!(db.account_for_token(&forged).unwrap().is_none());
    }

    /// The passphrase is no longer a bearer token. This is the escalation path being closed.
    #[test]
    fn the_passphrase_is_not_accepted_as_a_token() {
        let db = db();
        db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
            .unwrap();
        assert!(db.account_for_token("open sesame").unwrap().is_none());
    }

    #[test]
    fn unrecognised_role_and_state_values_fall_to_the_safe_reading() {
        assert_eq!(Role::parse("wizard"), Role::Member);
        assert_eq!(Role::parse(""), Role::Member);
        assert_eq!(Role::parse("ADMIN"), Role::Admin);
        assert_eq!(AccountState::parse("nonsense"), AccountState::Suspended);
        assert_eq!(AccountState::parse(""), AccountState::Suspended);
    }

    #[test]
    fn legacy_plaintext_passphrases_are_hashed_on_startup() {
        let db = db();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO users (id, username, api_key, created_at, passphrase_hash, role, state, quota_bytes)
                 VALUES ('legacy-id', 'legacy', 'old-passphrase', '2020-01-01T00:00:00Z', '', 'admin', 'active', 0)",
                [],
            )
            .unwrap();
        }

        assert_eq!(db.migrate_credentials().unwrap(), 1);
        assert!(db.verify_login("legacy", "old-passphrase").unwrap().is_some());
        // ...but it is no longer a token.
        assert!(db.account_for_token("old-passphrase").unwrap().is_none());
        // Running twice must not re-hash what is already done.
        assert_eq!(db.migrate_credentials().unwrap(), 0);
    }

    /// The second factor, end to end.
    mod second_factor {
        use super::*;

        /// `AGRO_SECRET_KEY` is process-wide, so these share the lock `crate::totp`'s tests use.
        fn enrolled_account(db: &Db) -> (String, String) {
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            let enrolment = db.begin_totp_enrolment("alpha").unwrap();
            let now = chrono::Utc::now().timestamp() as u64;
            let code = current_code(&enrolment.secret_base32, now);
            let codes = db.confirm_totp_enrolment("alpha", &code, None).unwrap();
            (enrolment.secret_base32, codes[0].clone())
        }

        /// What the phone would be showing at `at`.
        fn current_code(secret: &str, at: u64) -> String {
            crate::totp::generate_at(secret, at).expect("secret should be usable")
        }

        fn key() -> impl Drop {
            crate::totp::tests::with_key()
        }

        #[test]
        fn an_account_starts_without_a_second_factor() {
            let _k = key();
            let db = db();
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            assert!(!db.totp_is_confirmed("alpha").unwrap());
            assert_eq!(
                db.verify_totp("alpha", "000000").unwrap(),
                TotpOutcome::NotEnrolled
            );
        }

        /// A secret that has been generated but not proved must not be enforced — otherwise a scan
        /// that silently failed locks the account.
        #[test]
        fn a_pending_enrolment_is_not_a_second_factor() {
            let _k = key();
            let db = db();
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            db.begin_totp_enrolment("alpha").unwrap();
            assert!(!db.totp_is_confirmed("alpha").unwrap());
        }

        #[test]
        fn confirming_with_a_real_code_enables_it_and_issues_recovery_codes() {
            let _k = key();
            let db = db();
            let (_secret, _code) = enrolled_account(&db);
            assert!(db.totp_is_confirmed("alpha").unwrap());
            assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 10);
        }

        #[test]
        fn confirming_with_a_wrong_code_does_not_enable_it() {
            let _k = key();
            let db = db();
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            db.begin_totp_enrolment("alpha").unwrap();
            assert!(db.confirm_totp_enrolment("alpha", "000000", None).is_err());
            assert!(!db.totp_is_confirmed("alpha").unwrap());
        }

        /// The whole point of enrolling: tokens bought with the passphrase alone stop working.
        #[test]
        fn enrolling_signs_out_every_other_device() {
            let _k = key();
            let db = db();
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            let stale = db.mint_device_token("alpha", "attacker's laptop").unwrap();
            let mine = db.mint_device_token("alpha", "my phone").unwrap();

            let enrolment = db.begin_totp_enrolment("alpha").unwrap();
            let now = chrono::Utc::now().timestamp() as u64;
            let code = current_code(&enrolment.secret_base32, now);
            db.confirm_totp_enrolment("alpha", &code, Some(&credentials::hash_token(&mine)))
                .unwrap();

            assert!(db.account_for_token(&stale).unwrap().is_none());
            assert!(
                db.account_for_token(&mine).unwrap().is_some(),
                "the device doing the enrolling keeps working"
            );
        }

        /// RFC 6238 §5.2. A code is valid for a whole step; without this, one observed over a
        /// shoulder works again for the rest of that window.
        #[test]
        fn a_code_cannot_be_used_twice() {
            let _k = key();
            let db = db();
            let (secret, _) = enrolled_account(&db);
            // Confirmation already consumed the current step, so move to the next one.
            let later = chrono::Utc::now().timestamp() as u64 + 30;
            let code = current_code(&secret, later);

            // Verified against the wall clock, so drive it through the real entry point twice.
            let first = db.verify_totp("alpha", &code).unwrap();
            let second = db.verify_totp("alpha", &code).unwrap();
            assert!(
                first == TotpOutcome::Accepted || first == TotpOutcome::Replayed,
                "unexpected first outcome: {first:?}"
            );
            if first == TotpOutcome::Accepted {
                assert_eq!(second, TotpOutcome::Replayed, "the same code worked twice");
            }
        }

        #[test]
        fn a_recovery_code_is_accepted_once_and_then_spent() {
            let _k = key();
            let db = db();
            let (_secret, recovery) = enrolled_account(&db);

            assert_eq!(
                db.verify_totp("alpha", &recovery).unwrap(),
                TotpOutcome::AcceptedRecoveryCode
            );
            assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 9);
            assert_eq!(
                db.verify_totp("alpha", &recovery).unwrap(),
                TotpOutcome::Rejected,
                "a spent recovery code must not work again"
            );
        }

        /// Written down by hand, so read back by hand.
        #[test]
        fn a_recovery_code_is_case_and_dash_insensitive() {
            let _k = key();
            let db = db();
            let (_secret, recovery) = enrolled_account(&db);
            let typed = recovery.to_uppercase().replace('-', "");
            assert_eq!(
                db.verify_totp("alpha", &typed).unwrap(),
                TotpOutcome::AcceptedRecoveryCode
            );
        }

        #[test]
        fn regenerating_invalidates_the_old_codes() {
            let _k = key();
            let db = db();
            let (_secret, old) = enrolled_account(&db);
            let fresh = db.regenerate_recovery_codes("alpha").unwrap();

            assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 10);
            assert_eq!(
                db.verify_totp("alpha", &old).unwrap(),
                TotpOutcome::Rejected,
                "a code from the previous set must not still work"
            );
            assert_eq!(
                db.verify_totp("alpha", &fresh[0]).unwrap(),
                TotpOutcome::AcceptedRecoveryCode
            );
        }

        /// Replacing a *working* second factor without proving anything is how someone with a
        /// stolen session removes it.
        #[test]
        fn a_confirmed_enrolment_cannot_be_silently_restarted() {
            let _k = key();
            let db = db();
            enrolled_account(&db);
            assert!(db.begin_totp_enrolment("alpha").is_err());
            assert!(db.totp_is_confirmed("alpha").unwrap());
        }

        #[test]
        fn disabling_clears_the_secret_and_the_recovery_codes() {
            let _k = key();
            let db = db();
            let (_secret, recovery) = enrolled_account(&db);
            assert!(db.disable_totp("alpha").unwrap());

            assert!(!db.totp_is_confirmed("alpha").unwrap());
            assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 0);
            assert_eq!(
                db.verify_totp("alpha", &recovery).unwrap(),
                TotpOutcome::NotEnrolled
            );
        }

        /// One account's factor must not satisfy another's.
        #[test]
        fn a_recovery_code_only_works_for_its_own_account() {
            let _k = key();
            let db = db();
            let (_secret, recovery) = enrolled_account(&db);
            db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
                .unwrap();
            let enrolment = db.begin_totp_enrolment("mallory").unwrap();
            let now = chrono::Utc::now().timestamp() as u64;
            let code = current_code(&enrolment.secret_base32, now);
            db.confirm_totp_enrolment("mallory", &code, None).unwrap();

            assert_eq!(
                db.verify_totp("mallory", &recovery).unwrap(),
                TotpOutcome::Rejected
            );
        }
    }

    /// The identity boundary that the whole OIDC design rests on.
    mod federated_identity {
        use super::*;

        fn db_with_alpha() -> Db {
            let db = db();
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            db
        }

        /// **The central invariant.** An identity provider's `preferred_username` is a value its
        /// own administrator can edit. If a callback could match it against an existing account,
        /// setting it to `alpha` would hand over `alpha`. It must produce a *new* account instead.
        #[test]
        fn a_username_that_collides_never_resolves_to_the_existing_account() {
            let db = db_with_alpha();
            let derived = db.available_username_like("alpha").unwrap();
            assert_ne!(derived, "alpha", "an SSO login must never land on an existing account");
            assert_eq!(derived, "alpha2");
            assert!(db.account(&derived).unwrap().is_none(), "and it must be free");
        }

        #[test]
        fn a_free_username_is_used_as_is() {
            let db = db_with_alpha();
            assert_eq!(db.available_username_like("brand-new").unwrap(), "brand-new");
        }

        #[test]
        fn suffixes_keep_climbing_past_repeated_collisions() {
            let db = db_with_alpha();
            db.create_account("alpha2", "x", Role::Member, AccountState::Active)
                .unwrap();
            assert_eq!(db.available_username_like("alpha").unwrap(), "alpha3");
        }

        /// The IdP chooses this string; it must go through the same rule every other username does.
        #[test]
        fn a_hostile_preferred_username_is_normalised_or_refused() {
            let db = db_with_alpha();
            assert_eq!(db.available_username_like("Alpha").unwrap(), "alpha2");
            assert!(db.available_username_like("../../etc/passwd").is_none());
            assert!(db.available_username_like("").is_none());
            assert!(db.available_username_like("   ").is_none());
        }

        #[test]
        fn an_unlinked_identity_resolves_to_nobody() {
            let db = db_with_alpha();
            assert!(db
                .account_for_federated_identity("https://id.example.com", "sub-1")
                .unwrap()
                .is_none());
        }

        #[test]
        fn a_linked_identity_resolves_to_its_account() {
            let db = db_with_alpha();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();
            let found = db
                .account_for_federated_identity("https://id.example.com", "sub-1")
                .unwrap()
                .unwrap();
            assert_eq!(found.username, "alpha");
        }

        /// A subject maps to at most one account. Re-pointing it would strand the first.
        #[test]
        fn an_identity_cannot_be_relinked_to_a_second_account() {
            let db = db_with_alpha();
            db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
                .unwrap();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();

            assert!(db
                .link_federated_identity("mallory", "https://id.example.com", "sub-1", None)
                .is_err());
            assert_eq!(
                db.account_for_federated_identity("https://id.example.com", "sub-1")
                    .unwrap()
                    .unwrap()
                    .username,
                "alpha"
            );
        }

        /// Two providers can both be the same person.
        #[test]
        fn one_account_may_link_more_than_one_provider() {
            let db = db_with_alpha();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();
            db.link_federated_identity("alpha", "https://other.example.com", "sub-9", None)
                .unwrap();
            assert_eq!(db.federated_identities("alpha").unwrap().len(), 2);
        }

        /// The same subject string from a *different* issuer is a different person.
        #[test]
        fn the_issuer_is_part_of_the_identity() {
            let db = db_with_alpha();
            db.link_federated_identity("alpha", "https://id.example.com", "shared-sub", None)
                .unwrap();
            assert!(db
                .account_for_federated_identity("https://evil.example.com", "shared-sub")
                .unwrap()
                .is_none());
        }

        #[test]
        fn linking_the_same_identity_twice_is_harmless() {
            let db = db_with_alpha();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();
            assert_eq!(db.federated_identities("alpha").unwrap().len(), 1);
        }

        /// An SSO account's passphrase is generated and never shown, so unlinking its only identity
        /// would leave nobody able to get in.
        #[test]
        fn the_last_way_into_an_account_cannot_be_unlinked() {
            let db = db_with_alpha();
            db.mark_passphrase_unusable("alpha").unwrap();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();

            assert!(db
                .unlink_federated_identity("alpha", "https://id.example.com", "sub-1")
                .is_err());
            assert_eq!(db.federated_identities("alpha").unwrap().len(), 1);
        }

        /// Setting a passphrase is what makes unlinking safe.
        #[test]
        fn unlinking_is_allowed_once_a_passphrase_exists() {
            let db = db_with_alpha();
            db.mark_passphrase_unusable("alpha").unwrap();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();
            db.change_passphrase("alpha", "open sesame", "a much longer new one", None)
                .unwrap();

            assert!(db
                .unlink_federated_identity("alpha", "https://id.example.com", "sub-1")
                .unwrap());
            assert!(db.federated_identities("alpha").unwrap().is_empty());
        }

        /// An account that has a real passphrase can always unlink.
        #[test]
        fn an_account_with_a_passphrase_can_unlink_freely() {
            let db = db_with_alpha();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();
            assert!(db
                .unlink_federated_identity("alpha", "https://id.example.com", "sub-1")
                .unwrap());
        }

        /// One account must not be able to unlink another's identity by naming it.
        #[test]
        fn unlinking_is_scoped_to_the_owning_account() {
            let db = db_with_alpha();
            db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
                .unwrap();
            db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
                .unwrap();

            assert!(!db
                .unlink_federated_identity("mallory", "https://id.example.com", "sub-1")
                .unwrap());
            assert_eq!(db.federated_identities("alpha").unwrap().len(), 1);
        }
    }

    /// Changing a passphrase has to move the vault with it, or the settings become unreadable.
    mod passphrase_change {
        use super::*;

        fn account(db: &Db) -> String {
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            db.enrol_vault_key("alpha", "salt-one", "wrapped-under-one")
                .unwrap();
            db.mint_device_token("alpha", "laptop").unwrap()
        }

        #[test]
        fn the_wrong_current_passphrase_changes_nothing() {
            let db = db();
            let token = account(&db);
            assert!(!db
                .change_passphrase("alpha", "not the passphrase", "a much longer new one", None)
                .unwrap());
            assert!(db.verify_login("alpha", "open sesame").unwrap().is_some());
            assert!(db.account_for_token(&token).unwrap().is_some());
        }

        #[test]
        fn a_successful_change_swaps_the_passphrase_and_the_envelope() {
            let db = db();
            account(&db);
            assert!(db
                .change_passphrase(
                    "alpha",
                    "open sesame",
                    "a much longer new one",
                    Some(("salt-two", "wrapped-under-two")),
                )
                .unwrap());

            assert!(db.verify_login("alpha", "open sesame").unwrap().is_none());
            assert!(db.verify_login("alpha", "a much longer new one").unwrap().is_some());
            assert_eq!(
                db.vault_envelope("alpha").unwrap(),
                (Some("salt-two".into()), Some("wrapped-under-two".into()))
            );
        }

        /// The tokens bought with the old passphrase are exactly what changing it invalidates.
        #[test]
        fn changing_the_passphrase_signs_every_device_out() {
            let db = db();
            let token = account(&db);
            db.change_passphrase("alpha", "open sesame", "a much longer new one", None)
                .unwrap();
            assert!(db.account_for_token(&token).unwrap().is_none());
        }

        /// The write-once rule on first enrolment still holds — re-sealing is a different door.
        #[test]
        fn the_first_vault_enrolment_is_still_write_once() {
            let db = db();
            account(&db);
            assert!(
                !db.enrol_vault_key("alpha", "salt-two", "wrapped-under-two").unwrap(),
                "enrol_vault_key must still refuse to replace a live key"
            );
        }

        #[test]
        fn a_short_passphrase_is_refused() {
            let db = db();
            account(&db);
            assert!(db.change_passphrase("alpha", "open sesame", "short", None).is_err());
            assert!(db.verify_login("alpha", "open sesame").unwrap().is_some());
        }
    }

    /// A token that never stops working makes every other credential control decorative, so these
    /// pin both halves: that expiry is enforced, and that it does not fire on anyone it shouldn't.
    mod token_lifecycle {
        use super::*;

        fn account_with_token(db: &Db) -> String {
            db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
                .unwrap();
            db.mint_device_token("alpha", "laptop").unwrap()
        }

        /// Backdates a token's `last_used_at`, standing in for a device that stopped checking in.
        fn last_used_days_ago(db: &Db, days: i64) {
            let when = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
            db.conn
                .lock()
                .unwrap()
                .execute("UPDATE app_passwords SET last_used_at = ?1", params![when])
                .unwrap();
        }

        #[test]
        fn a_freshly_minted_token_works() {
            let db = db();
            let token = account_with_token(&db);
            assert!(db.account_for_token(&token).unwrap().is_some());
        }

        #[test]
        fn a_token_past_its_expires_at_is_refused() {
            let db = db();
            let token = account_with_token(&db);
            let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
            db.conn
                .lock()
                .unwrap()
                .execute("UPDATE app_passwords SET expires_at = ?1", params![past])
                .unwrap();
            assert!(db.account_for_token(&token).unwrap().is_none());
        }

        #[test]
        fn a_token_with_a_future_expires_at_still_works() {
            let db = db();
            let token = account_with_token(&db);
            let future = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
            db.conn
                .lock()
                .unwrap()
                .execute("UPDATE app_passwords SET expires_at = ?1", params![future])
                .unwrap();
            assert!(db.account_for_token(&token).unwrap().is_some());
        }

        /// The regression that matters most: every token in the wild today has a NULL `expires_at`,
        /// and must keep working exactly as it did before migration 29.
        #[test]
        fn a_token_with_no_expires_at_is_not_treated_as_expired() {
            let db = db();
            let token = account_with_token(&db);
            let stored: Option<String> = db
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT expires_at FROM app_passwords", [], |r| r.get(0))
                .unwrap();
            assert_eq!(stored, None, "minting must not set an expiry by default");
            assert!(db.account_for_token(&token).unwrap().is_some());
        }

        #[test]
        fn a_token_idle_past_the_window_is_refused() {
            let db = db();
            let token = account_with_token(&db);
            last_used_days_ago(&db, DEFAULT_TOKEN_IDLE_DAYS + 1);
            assert!(db.account_for_token(&token).unwrap().is_none());
        }

        #[test]
        fn a_token_used_within_the_window_still_works() {
            let db = db();
            let token = account_with_token(&db);
            last_used_days_ago(&db, DEFAULT_TOKEN_IDLE_DAYS - 1);
            assert!(db.account_for_token(&token).unwrap().is_some());
        }

        /// The sweep and the accept-check must agree. A token the sweep would delete must not
        /// still authenticate in the window before the sweep next runs, and vice versa.
        #[test]
        fn the_sweep_removes_exactly_what_the_check_refuses() {
            let db = db();
            let token = account_with_token(&db);
            last_used_days_ago(&db, DEFAULT_TOKEN_IDLE_DAYS + 1);

            assert!(db.account_for_token(&token).unwrap().is_none());
            assert_eq!(db.sweep_expired_tokens().unwrap(), 1);
            assert_eq!(db.sweep_expired_tokens().unwrap(), 0, "sweep is idempotent");
        }

        #[test]
        fn the_sweep_spares_a_live_token() {
            let db = db();
            let token = account_with_token(&db);
            assert_eq!(db.sweep_expired_tokens().unwrap(), 0);
            assert!(db.account_for_token(&token).unwrap().is_some());
        }

        /// Without this, enrolling a second factor changes nothing for whoever already traded the
        /// passphrase for a token.
        #[test]
        fn revoking_everything_leaves_no_token_working() {
            let db = db();
            let first = account_with_token(&db);
            let second = db.mint_device_token("alpha", "phone").unwrap();

            assert_eq!(db.revoke_all_tokens("alpha", None).unwrap(), 2);
            assert!(db.account_for_token(&first).unwrap().is_none());
            assert!(db.account_for_token(&second).unwrap().is_none());
        }

        /// "Sign out my other devices" must not sign out the device asking.
        #[test]
        fn revoking_can_spare_the_caller() {
            let db = db();
            let mine = account_with_token(&db);
            let other = db.mint_device_token("alpha", "phone").unwrap();

            let spare = credentials::hash_token(&mine);
            assert_eq!(db.revoke_all_tokens("alpha", Some(&spare)).unwrap(), 1);
            assert!(db.account_for_token(&mine).unwrap().is_some());
            assert!(db.account_for_token(&other).unwrap().is_none());
        }

        /// Revocation is scoped to one account, or it is a denial of service against the server.
        #[test]
        fn revoking_one_account_leaves_another_alone() {
            let db = db();
            let alpha = account_with_token(&db);
            db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
                .unwrap();
            let mallory = db.mint_device_token("mallory", "laptop").unwrap();

            db.revoke_all_tokens("mallory", None).unwrap();
            assert!(db.account_for_token(&alpha).unwrap().is_some());
            assert!(db.account_for_token(&mallory).unwrap().is_none());
        }
    }
}
