//! Per-device app passwords: minting, listing and revoking them.

use crate::db::Db;
use async_graphql::{Context, Object, SimpleObject};

use super::authorize;

/// An app password as it is listed back. The token itself is deliberately absent: a credential is
/// shown once, when it is created, and is not recoverable afterwards.
#[derive(SimpleObject, Clone)]
pub struct AppPassword {
    /// Stable handle for this one credential. Labels repeat; this does not.
    pub id: i64,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// The one time a token is returned. Shown once, at creation.
#[derive(SimpleObject, Clone)]
pub struct AppPasswordCreated {
    pub label: String,
    pub token: String,
}

#[derive(Default)]
pub struct AppPasswordsQuery;

#[Object]
impl AppPasswordsQuery {
    async fn app_passwords(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<Vec<AppPassword>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .list_app_passwords(&user_id)?
            .into_iter()
            .map(|record| AppPassword {
                id: record.id,
                label: record.label,
                created_at: record.created_at,
                last_used_at: record.last_used_at,
            })
            .collect())
    }

    // ── Library ─────────────────────────────────────────────────────────────────────────────
}

#[derive(Default)]
pub struct AppPasswordsMutation;

#[Object]
impl AppPasswordsMutation {
    /// Issues a credential for one client, so that client can be revoked on its own rather than
    /// by rotating the account passphrase every other device is using.
    async fn create_app_password(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        label: String,
    ) -> async_graphql::Result<AppPasswordCreated> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let label = label.trim().to_string();
        if label.is_empty() {
            return Err("An app password needs a label, so you can tell which device it is".into());
        }
        // Minted, not generated from the passphrase wordlist, and stored as a hash. The previous
        // form wrote a plaintext four-word token straight into the legacy column, which the
        // hashed-token lookup cannot match — so every credential this issued was dead on arrival.
        let token = db.mint_device_token(&user_id, &label)?;
        db.record_event(
            crate::audit::Event::TokenMinted,
            crate::audit::Record::new()
                .user(&user_id)
                .device(label.clone()),
        );
        Ok(AppPasswordCreated { label, token })
    }

    /// Revokes one credential by the id `appPasswords` reported.
    ///
    /// Deliberately not by label. A label is a human note, chosen by the client and freely
    /// repeated — a client that re-logs in on every launch leaves a row each time, all of them
    /// named the same thing. Revoking by label signed out every one of them at once, which is
    /// precisely the opposite of the per-device revocation these credentials exist to provide.
    async fn revoke_app_password(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        id: i64,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let revoked = db.revoke_app_password(&user_id, id)?;
        if revoked {
            db.record_event(
                crate::audit::Event::TokenRevoked,
                crate::audit::Record::new()
                    .user(&user_id)
                    .detail(format!("credential {id}")),
            );
        }
        Ok(revoked)
    }
}
