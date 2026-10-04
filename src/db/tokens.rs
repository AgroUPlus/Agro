//! Proving who a caller is: the passphrase, the account key, and per-device app passwords.

use rusqlite::{params, OptionalExtension, Result};

use super::Db;

impl Db {
    pub fn authenticate_user(&self, username: &str, passphrase: &str) -> Result<bool> {
        if username.trim().is_empty() || passphrase.trim().is_empty() {
            return Ok(false);
        }
        if let Some((_, _, stored_pass)) = self.get_user_by_username(username)? {
            Ok(stored_pass.trim() == passphrase.trim())
        } else {
            // Frictionless first-time auto-registration with provided passphrase
            let _ = self.create_user(username, passphrase)?;
            Ok(true)
        }
    }

    pub fn validate_api_key(&self, api_key: &str) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, username FROM users WHERE api_key = ?1")?;
        let mut rows = stmt.query(params![api_key])?;
        if let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let username: String = row.get(1)?;
            Ok(Some((id, username)))
        } else {
            Ok(None)
        }
    }

    /// How many accounts exist. Zero means the server has never been set up, which is the only
    /// state in which an unauthenticated request is allowed to create one.
    pub fn user_count(&self) -> Result<i64> {
        let conn = self.read();
        conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
    }

    /// Resolves a bearer token to its username, accepting either the account passphrase or one of
    /// its app passwords. Returns None for anything else — including an empty token.
    pub fn user_for_token(&self, token: &str) -> Result<Option<String>> {
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let account: Option<String> = conn
            .query_row(
                "SELECT username FROM users WHERE api_key = ?1",
                params![token],
                |row| row.get(0),
            )
            .optional()?;
        if account.is_some() {
            return Ok(account);
        }

        let via_app_password: Option<String> = conn
            .query_row(
                "SELECT u.username FROM app_passwords a
                 JOIN users u ON u.id = a.user_id
                 WHERE a.token = ?1",
                params![token],
                |row| row.get(0),
            )
            .optional()?;
        if via_app_password.is_some() {
            let now = chrono::Utc::now().to_rfc3339();
            let _ = conn.execute(
                "UPDATE app_passwords SET last_used_at = ?1 WHERE token = ?2",
                params![now, token],
            );
        }
        Ok(via_app_password)
    }

    pub fn create_app_password(&self, username: &str, label: &str, token: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let user_id: String = conn.query_row(
            "SELECT id FROM users WHERE username = ?1",
            params![username],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO app_passwords (token, user_id, label, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![token, user_id, label, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Never returns the token itself: a credential is shown once, at creation.
    pub fn list_app_passwords(&self, username: &str) -> Result<Vec<AppPasswordRecord>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT a.rowid, a.label, a.created_at, a.last_used_at FROM app_passwords a
             JOIN users u ON u.id = a.user_id
             WHERE u.username = ?1 COLLATE NOCASE ORDER BY a.created_at DESC",
        )?;
        let rows = stmt.query_map(params![username.trim()], |row| {
            Ok(AppPasswordRecord {
                id: row.get(0)?,
                label: row.get(1)?,
                created_at: row.get(2)?,
                last_used_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Revokes one credential, identified by the id [`list_app_passwords`] reported.
    ///
    /// Scoped to the account in the same statement rather than checked beforehand: an id is just a
    /// number, and a caller who guesses someone else's must not have it deleted for them.
    pub fn revoke_app_password(&self, username: &str, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM app_passwords WHERE rowid = ?1 AND user_id = (
                 SELECT id FROM users WHERE username = ?2 COLLATE NOCASE
             )",
            params![id, username.trim()],
        )?;
        Ok(removed > 0)
    }
}

pub struct AppPasswordRecord {
    /// The row's `rowid`, used as the public handle for one credential.
    ///
    /// Labels are chosen by the client and are not unique — several devices calling themselves
    /// `wander-desktop` are the normal case, not an edge one — so a label cannot identify which
    /// credential to revoke. The token itself obviously cannot be the handle. `rowid` is stable,
    /// unique and reveals nothing.
    pub id: i64,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}
