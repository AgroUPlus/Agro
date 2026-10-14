//! Signup invites.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// Mints an invite code. Admin-only at the resolver; this does not check.
    pub fn create_invite(
        &self,
        created_by: &str,
        max_uses: i64,
        ttl_hours: Option<i64>,
    ) -> Result<Invite> {
        let invite = Invite {
            code: crate::credentials::mint_token().secret,
            created_by: created_by.trim().to_lowercase(),
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: ttl_hours
                .map(|h| (chrono::Utc::now() + chrono::Duration::hours(h)).to_rfc3339()),
            max_uses: max_uses.max(1),
            used_count: 0,
            revoked: false,
        };
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT INTO invites (code, created_by, created_at, expires_at, max_uses, used_count, revoked)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
            params![
                invite.code,
                invite.created_by,
                invite.created_at,
                invite.expires_at,
                invite.max_uses
            ],
        )?;
        Ok(invite)
    }

    /// Spends one use of a code, or reports that it cannot be spent.
    ///
    /// The check and the increment are one transaction: two people redeeming a single-use code at
    /// the same moment must not both get in.
    pub fn redeem_invite(&self, code: &str) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.conn.lock_recover();
        let tx = conn.transaction()?;
        let spent = tx.execute(
            "UPDATE invites SET used_count = used_count + 1
              WHERE code = ?1 AND revoked = 0 AND used_count < max_uses
                AND (expires_at IS NULL OR expires_at > ?2)",
            params![code.trim(), now],
        )?;
        tx.commit()?;
        Ok(spent > 0)
    }

    /// Hands a use back, for a signup that redeemed a code and then could not finish.
    ///
    /// Never below zero: a refund that outran its redemption would make a code usable more often
    /// than the operator allowed, which is the one thing `max_uses` exists to prevent.
    pub fn refund_invite(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE invites SET used_count = used_count - 1
              WHERE code = ?1 AND used_count > 0",
            params![code.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Removes an invite outright, so a spent or revoked one stops cluttering the list.
    ///
    /// Distinct from revoking: revoking stops a code working but keeps the record, which is what
    /// you want the moment you realise a code has escaped. Deleting is the tidying-up afterwards.
    pub fn delete_invite(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let removed = conn.execute("DELETE FROM invites WHERE code = ?1", params![code.trim()])?;
        Ok(removed > 0)
    }

    pub fn list_invites(&self) -> Result<Vec<Invite>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT code, created_by, created_at, expires_at, max_uses, used_count, revoked
               FROM invites ORDER BY created_at DESC",
        )?;
        let invites = stmt
            .query_map([], |row| {
                Ok(Invite {
                    code: row.get(0)?,
                    created_by: row.get(1)?,
                    created_at: row.get(2)?,
                    expires_at: row.get(3)?,
                    max_uses: row.get(4)?,
                    used_count: row.get(5)?,
                    revoked: row.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(invites)
    }

    pub fn revoke_invite(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE invites SET revoked = 1 WHERE code = ?1",
            params![code.trim()],
        )?;
        Ok(changed > 0)
    }
}
