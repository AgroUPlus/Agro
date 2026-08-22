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
    pub show_now_playing: bool,
    pub show_stats: bool,
}

impl Account {
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
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
    "id, username, role, state, quota_bytes, show_now_playing, show_stats";

fn account_from_row(row: &rusqlite::Row<'_>) -> Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        username: row.get(1)?,
        role: Role::parse(&row.get::<_, String>(2)?),
        state: AccountState::parse(&row.get::<_, String>(3)?),
        quota_bytes: row.get(4)?,
        show_now_playing: row.get::<_, i64>(5)? != 0,
        show_stats: row.get::<_, i64>(6)? != 0,
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

    /// The key this account's synced settings are encrypted with.
    ///
    /// Its own column because the passphrase it used to be is now a hash. Generated per account and
    /// never presented to anyone — unlike the passphrase, which was both the encryption key and the
    /// bearer token.
    pub fn settings_key(&self, username: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT settings_key FROM users WHERE username = ?1 COLLATE NOCASE",
            params![username.trim()],
            |row| row.get(0),
        )
        .optional()
        .map(|key: Option<String>| key.unwrap_or_default())
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
                "INSERT INTO users
                   (id, username, api_key, created_at, passphrase_hash, role, state, quota_bytes, settings_key)
                 VALUES (?1, ?2, '', ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    username,
                    now,
                    hash,
                    role.as_str(),
                    state.as_str(),
                    quota,
                    credentials::mint_token().secret,
                ],
            )?;
        }

        Ok(Account {
            id,
            username,
            role,
            state,
            quota_bytes: quota,
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
        conn.execute(
            "INSERT INTO app_passwords (token, user_id, label, created_at, token_prefix, token_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                // The legacy plaintext column is a NOT NULL primary key, so it still needs a
                // distinct value. The hash is exactly that, and is not a usable credential.
                minted.hash,
                user_id,
                label.trim(),
                chrono::Utc::now().to_rfc3339(),
                minted.prefix,
                minted.hash,
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

        let candidates: Vec<(String, String, Option<String>, String)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT a.token_hash, u.username, a.last_used_at, a.label
                   FROM app_passwords a
                   JOIN users u ON u.id = a.user_id
                  WHERE a.token_prefix = ?1 AND a.token_hash <> ''",
            )?;
            let rows = stmt
                .query_map(params![prefix], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<Result<_>>()?;
            rows
        };

        let Some((_, username, last_used, label)) = candidates
            .into_iter()
            .find(|(hash, _, _, _): &(String, String, Option<String>, String)| {
                credentials::secure_eq(hash, &presented)
            })
        else {
            return Ok(None);
        };

        self.touch_token_if_stale(&presented, last_used.as_deref());
        // The label travels with the account because it is the name a *person* gave this device.
        // Without it the server had to invent one at registration, so the same phone ended up with
        // a typed name on its credential and a random one on its playing session.
        Ok(self.account(&username)?.map(|account| (account, label)))
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
}
