//! The account's security surface: sign-in history, linked identities, the passphrase
//! and the vault key.

use crate::db::Db;
use async_graphql::{Context, Object, SimpleObject};

use super::{authorize, caller, require_admin};

/// An identity provider account linked to an Agro account.
#[derive(SimpleObject, Clone)]
pub struct FederatedIdentity {
    pub issuer: String,
    /// The provider's stable identifier for the person. The only value treated as identity.
    pub subject: String,
    pub linked_at: String,
}

/// One entry in the security log.
#[derive(SimpleObject, Clone)]
pub struct SecurityEventPayload {
    pub id: i64,
    pub at: String,
    /// The account this concerns. Absent on a failed login for a username that does not exist.
    pub user_id: Option<String>,
    /// A stable machine-readable kind — see `audit::Event`.
    pub kind: String,
    /// The network the request came from, truncated to a /24 or /64. Never a full address.
    pub client_ip: Option<String>,
    pub device_label: Option<String>,
    pub detail: Option<String>,
}

#[derive(Default)]
pub struct SecurityQuery;

#[Object]
impl SecurityQuery {
    /// The SSO identities linked to an account. Self-scoped.
    async fn federated_identities(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<Vec<FederatedIdentity>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .federated_identities(&user_id)?
            .into_iter()
            .map(|(issuer, subject, linked_at)| FederatedIdentity {
                issuer,
                subject,
                linked_at,
            })
            .collect())
    }

    /// The security log for one account, or — for an administrator passing no `userId` — the whole
    /// server.
    ///
    /// Self-scoped through `authorize`, so this is not a way to read anyone else's sign-in history:
    /// naming another account is refused exactly as it is everywhere else. The server-wide view is
    /// separately gated on `require_admin`, because "no `userId`" must not read as "any user".
    async fn security_events(
        &self,
        ctx: &Context<'_>,
        user_id: Option<String>,
        limit: Option<i64>,
    ) -> async_graphql::Result<Vec<SecurityEventPayload>> {
        let db = ctx.data::<Db>()?;
        let scope = match user_id.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            Some(user) => {
                authorize(ctx, user)?;
                Some(user.to_string())
            }
            None => {
                require_admin(ctx)?;
                None
            }
        };
        Ok(db
            .security_events(scope.as_deref(), limit.unwrap_or(100))?
            .into_iter()
            .map(|e| SecurityEventPayload {
                id: e.id,
                at: e.at,
                user_id: e.user_id,
                kind: e.kind,
                client_ip: e.client_ip,
                device_label: e.device_label,
                detail: e.detail,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct SecurityMutation;

#[Object]
impl SecurityMutation {
    /// Registers this account's vault key, sealed by the client under the account passphrase.
    ///
    /// The server takes two opaque strings and can do nothing with either: unwrapping needs the
    /// passphrase, and it keeps only an Argon2 hash of that. What it is storing is the means for
    /// the *user* to recover their settings on a new device, not the means for this machine to read
    /// them.
    ///
    /// Enrolment is once per account. A second attempt returns `false` rather than erroring —
    /// two devices racing to set up the same account is an ordinary thing to happen, and the loser
    /// should fetch the winner's envelope and unwrap it, not treat the race as a failure.
    async fn enrol_vault_key(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        vault_salt: String,
        vault_key_wrapped: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        // Bounded so the column cannot be used as free storage. A salt and a sealed 32-byte key
        // are both far smaller than this; the limit only has to be obviously sufficient.
        for (name, value) in [
            ("vaultSalt", &vault_salt),
            ("vaultKeyWrapped", &vault_key_wrapped),
        ] {
            if value.trim().is_empty() || value.len() > 512 {
                return Err(format!("{name} is missing or too long").into());
            }
        }
        Ok(ctx.data::<Db>()?.enrol_vault_key(
            &user_id,
            vault_salt.trim(),
            vault_key_wrapped.trim(),
        )?)
    }

    /// Removes a linked SSO identity from the caller's account.
    ///
    /// Refuses when it would remove the last way in — an account created through SSO has a
    /// passphrase it has never been shown, so unlinking without setting one first is a lockout.
    async fn unlink_federated_identity(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        issuer: String,
        subject: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let removed = db
            .unlink_federated_identity(&user_id, &issuer, &subject)
            .map_err(async_graphql::Error::new)?;
        if removed {
            db.record_event(
                crate::audit::Event::IdentityUnlinked,
                crate::audit::Record::new()
                    .user(&user_id)
                    .detail(format!("{issuer} subject {subject}")),
            );
        }
        Ok(removed)
    }

    /// Changes the caller's passphrase, re-sealing the settings vault under the new one.
    ///
    /// The client does the sealing: it unwraps the vault key with the old passphrase, wraps it
    /// again with the new one, and sends both halves. The server never sees either passphrase in a
    /// form it could keep, and never sees the vault key at all — the same property the vault had
    /// before this mutation existed.
    ///
    /// **Every device is signed out, including the caller's.** A passphrase is changed because it
    /// may have leaked, and the tokens bought with it are the thing being invalidated.
    async fn change_passphrase(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        current_passphrase: String,
        new_passphrase: String,
        new_vault_salt: Option<String>,
        new_vault_key_wrapped: Option<String>,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;

        // Both halves of the envelope or neither. One without the other would write a salt that
        // does not match the wrapped key, which is a vault nothing can open.
        let vault = match (new_vault_salt.as_deref(), new_vault_key_wrapped.as_deref()) {
            (Some(salt), Some(wrapped)) => Some((salt, wrapped)),
            (None, None) => None,
            _ => return Err("Send both newVaultSalt and newVaultKeyWrapped, or neither".into()),
        };

        let changed = db
            .change_passphrase(&user_id, &current_passphrase, &new_passphrase, vault)
            .map_err(async_graphql::Error::new)?;
        if !changed {
            return Err("That passphrase was not accepted".into());
        }
        db.record_event(
            crate::audit::Event::PassphraseChanged,
            crate::audit::Record::new().user(&user_id),
        );
        Ok(true)
    }

    /// Signs out every other device on the account.
    ///
    /// The blunt instrument that per-device revocation does not cover: a passphrase that may have
    /// leaked has already been traded for tokens, and revoking them one at a time from a list means
    /// noticing every one of them. This spares only the device making the call — by token hash, not
    /// by label, for the same reason `revokeAppPassword` refuses to work by label.
    ///
    /// Returns how many were revoked.
    async fn revoke_all_devices(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<i64> {
        authorize(ctx, &user_id)?;
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        // An empty hash would spare nothing and sign the caller out too. That is the correct
        // reading of "this request did not arrive with a token" — it only happens in a test
        // harness — but it must be deliberate rather than an accident of an empty string.
        let spare = Some(authed.token_hash.as_str()).filter(|h| !h.is_empty());
        let revoked = db.revoke_all_tokens(&user_id, spare)?;
        db.record_event(
            crate::audit::Event::AllTokensRevoked,
            crate::audit::Record::new()
                .user(&user_id)
                .device(authed.device_label.clone())
                .detail(format!("{revoked} revoked")),
        );
        Ok(revoked as i64)
    }
}
