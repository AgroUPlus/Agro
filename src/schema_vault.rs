//! The cloud vault over GraphQL: what backups an account has, and removing one.
//!
//! Labels only — the sealed bytes travel over `vault_http`. This is what the dashboard and the
//! app list from, and neither learns anything here the server could not already see.

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;
use crate::db_vault::{VaultBackup, VaultSection, KEEP_PER_ACCOUNT, MAX_SEALED_BYTES};
use crate::features::Feature;
use crate::schema::caller;

#[derive(SimpleObject, Clone)]
pub struct VaultBackupPayload {
    pub id: String,
    pub created_at: String,
    pub device_id: String,
    pub device_name: Option<String>,
    pub app_version: Option<String>,
    pub format: i64,
    /// Before compression and sealing.
    pub plain_bytes: i64,
    /// What is stored.
    pub sealed_bytes: i64,
    pub sections: Vec<VaultSection>,
    /// Whether the device chose to include its sign-ins, so the dashboard can say so plainly.
    pub includes_accounts: bool,
    pub sha256: String,
}

impl From<VaultBackup> for VaultBackupPayload {
    fn from(b: VaultBackup) -> Self {
        VaultBackupPayload {
            id: b.id,
            created_at: b.created_at,
            device_id: b.device_id,
            device_name: b.device_name,
            app_version: b.app_version,
            format: b.format,
            plain_bytes: b.plain_bytes,
            sealed_bytes: b.sealed_bytes,
            sections: b.sections,
            includes_accounts: b.includes_accounts,
            sha256: b.sha256,
        }
    }
}

/// The rules a client shows, so it never has to hard-code them and disagree.
#[derive(SimpleObject, Clone)]
pub struct VaultLimits {
    pub keep_per_account: i64,
    pub max_sealed_bytes: i64,
}

#[derive(Default)]
pub struct VaultQuery;

#[Object]
impl VaultQuery {
    /// Your backups, newest first.
    async fn vault_backups(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<VaultBackupPayload>> {
        let me = caller(ctx)?.username();
        let db = ctx.data::<Db>()?;
        Feature::CloudBackups.require(db)?;
        Ok(db.vault_backups(me)?.into_iter().map(Into::into).collect())
    }

    async fn vault_limits(&self) -> VaultLimits {
        VaultLimits {
            keep_per_account: KEEP_PER_ACCOUNT,
            max_sealed_bytes: MAX_SEALED_BYTES as i64,
        }
    }
}

#[derive(Default)]
pub struct VaultMutation;

#[Object]
impl VaultMutation {
    /// Deletes one of your backups. Not behind the feature switch: turning backups off must not
    /// stop anyone removing what is already kept.
    async fn delete_vault_backup(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<bool> {
        let me = caller(ctx)?.username();
        Ok(ctx.data::<Db>()?.delete_vault_backup(me, id.trim())?)
    }
}
