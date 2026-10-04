//! The cloud vault at rest: sealed backups, and the label each device wrote on its envelope.
//!
//! The server cannot open a backup — it is sealed under a key derived from the account's vault
//! key, which never leaves the account's devices (see `security-and-vault.md`). What it can show
//! is the label: when, from which device, how big, and how many records of each kind. That is what
//! the dashboard lists, and it says nothing about what any record is.

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;

/// How many backups an account keeps. The newest replace the oldest: a backup is for getting a
/// device back, and three is enough to step past one that turned out to be wrong.
pub const KEEP_PER_ACCOUNT: i64 = 3;

/// The largest sealed backup accepted. Compressed before sealing, a whole library with years of
/// listening fits well inside it; a file past it is not a backup this app wrote.
pub const MAX_SEALED_BYTES: usize = 8 * 1024 * 1024;

/// One section of a backup and how many records it held.
#[derive(
    Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, async_graphql::SimpleObject,
)]
pub struct VaultSection {
    pub name: String,
    pub count: i64,
}

/// What a device says about a backup it is sending. Checked by the caller before it gets here.
#[derive(Clone, Debug)]
pub struct VaultLabel {
    pub device_id: String,
    pub device_name: Option<String>,
    pub app_version: Option<String>,
    pub format: i64,
    pub plain_bytes: i64,
    pub sections: Vec<VaultSection>,
}

/// A stored backup without its contents.
#[derive(Clone, Debug)]
pub struct VaultBackup {
    pub id: String,
    pub created_at: String,
    pub device_id: String,
    pub device_name: Option<String>,
    pub app_version: Option<String>,
    pub format: i64,
    pub plain_bytes: i64,
    pub sealed_bytes: i64,
    pub sections: Vec<VaultSection>,
    pub includes_accounts: bool,
    pub sha256: String,
}

const COLUMNS: &str = "id, created_at, device_id, device_name, app_version, format, plain_bytes,
                       sealed_bytes, sections_json, includes_accounts, sha256";

fn from_row(row: &rusqlite::Row<'_>) -> Result<VaultBackup> {
    let sections: String = row.get(8)?;
    Ok(VaultBackup {
        id: row.get(0)?,
        created_at: row.get(1)?,
        device_id: row.get(2)?,
        device_name: row.get(3)?,
        app_version: row.get(4)?,
        format: row.get(5)?,
        plain_bytes: row.get(6)?,
        sealed_bytes: row.get(7)?,
        sections: serde_json::from_str(&sections).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?,
        includes_accounts: row.get::<_, i64>(9)? != 0,
        sha256: row.get(10)?,
    })
}

impl Db {
    /// Keeps a sealed backup and lets the oldest go past [`KEEP_PER_ACCOUNT`], in one transaction.
    pub fn store_vault_backup(
        &self,
        username: &str,
        label: &VaultLabel,
        sealed: &[u8],
    ) -> Result<VaultBackup> {
        use sha2::{Digest, Sha256};
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let sha256 = format!("{:x}", Sha256::digest(sealed));
        let sections = serde_json::to_string(&label.sections)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let includes_accounts = label.sections.iter().any(|s| s.name == "ACCOUNTS");

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO vault_backups (id, user_id, created_at, device_id, device_name, app_version,
                 format, plain_bytes, sealed_bytes, sections_json, includes_accounts, sha256, blob)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                username,
                now,
                label.device_id,
                label.device_name,
                label.app_version,
                label.format,
                label.plain_bytes,
                sealed.len() as i64,
                sections,
                includes_accounts as i64,
                sha256,
                sealed
            ],
        )?;
        tx.execute(
            "DELETE FROM vault_backups WHERE user_id = ?1 AND id NOT IN (
                 SELECT id FROM vault_backups WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2)",
            params![username, KEEP_PER_ACCOUNT],
        )?;
        let stored = tx.query_row(
            &format!("SELECT {COLUMNS} FROM vault_backups WHERE id = ?1"),
            params![id],
            from_row,
        )?;
        tx.commit()?;
        Ok(stored)
    }

    /// This account's backups, newest first, without their contents.
    pub fn vault_backups(&self, username: &str) -> Result<Vec<VaultBackup>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM vault_backups WHERE user_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![username], from_row)?;
        rows.collect()
    }

    /// One backup's sealed bytes and label, if it is this account's.
    pub fn vault_backup_blob(
        &self,
        username: &str,
        id: &str,
    ) -> Result<Option<(VaultBackup, Vec<u8>)>> {
        let conn = self.read();
        conn.query_row(
            &format!("SELECT {COLUMNS}, blob FROM vault_backups WHERE id = ?1 AND user_id = ?2"),
            params![id, username],
            |row| Ok((from_row(row)?, row.get::<_, Vec<u8>>(11)?)),
        )
        .optional()
    }

    /// `false` when there was no such backup of this account's — the same answer for "not yours".
    pub fn delete_vault_backup(&self, username: &str, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM vault_backups WHERE id = ?1 AND user_id = ?2",
            params![id, username],
        )?;
        Ok(removed > 0)
    }
}
