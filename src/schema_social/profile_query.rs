//! Reading profiles, keys, second-factor status and invites.

use super::*;
use crate::db::Db;
use crate::db_social::FriendState;
use crate::schema::{caller, forbidden, normalise_username, require_admin};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct SocialProfileQuery;

#[Object]
impl SocialProfileQuery {
    /// Whether the signed-in account is currently quiet.
    ///
    /// Only ever answered for the caller. There is deliberately no version of this question about
    /// somebody else: an incognito account is not returned to anyone at all, and saying "they have
    /// gone quiet" would be exactly the disclosure incognito exists to prevent. A friend meets the
    /// same refusal a stranger does.
    ///
    /// Needed because [`Mutation::set_incognito`] returns the new value but nothing could *read*
    /// it — a client that has just launched has no way to learn the account is already quiet, and
    /// would carry on recording until someone touched the switch.
    async fn incognito(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?
            .incognito)
    }

    /// The state of the caller's second factor.
    async fn totp_status(&self, ctx: &Context<'_>) -> async_graphql::Result<TotpStatus> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(TotpStatus {
            is_enabled: db.totp_is_confirmed(authed.username())?,
            recovery_codes_remaining: db.recovery_codes_remaining(authed.username())?,
            is_available: crate::totp::is_configured(),
        })
    }

    /// Whether the signed-in account has active 2FA / TOTP configured.
    async fn has_totp(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let has = db
            .totp_is_confirmed(authed.username())
            .map_err(|e| async_graphql::Error::new(format!("Failed to check TOTP: {e}")))?;
        Ok(has)
    }

    /// One account, as the caller is allowed to see it.
    ///
    /// Not gated by [`require_visible`]: the card itself — name, bio, avatar — is what someone
    /// needs in order to decide whether to send a request, so it is readable by any signed-in
    /// account for any discoverable or already-connected profile. What the flags gate is the
    /// *contents*: now playing and stats have their own resolvers and their own checks.
    async fn profile(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<Option<ProfilePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        let Some(profile) = db.profile(&subject)? else {
            return Ok(None);
        };
        let state = db.friend_state(authed.username(), &subject)?;

        // Someone who is neither discoverable nor connected to the caller is not theirs to look up.
        let reachable = authed.username().eq_ignore_ascii_case(&subject)
            || profile.discoverable
            || matches!(
                state,
                Some(FriendState::Accepted) | Some(FriendState::Pending)
            );
        if !reachable || state == Some(FriendState::Blocked) {
            return Ok(None);
        }

        let outgoing = db
            .outgoing_requests(authed.username())?
            .iter()
            .any(|e| e.profile.username.eq_ignore_ascii_case(&subject));
        Ok(Some(profile_payload(&profile, state, outgoing)))
    }

    /// Every identity key `username` has published, one per device.
    ///
    /// Gated on friendship with the same refusal `dropTrack` uses, and for the same reason: a key
    /// list that answered differently for a stranger and for an account that does not exist would
    /// be a way to probe for accounts. Reading your own list is always allowed — a client needs it
    /// to seal a copy to its own other devices.
    ///
    /// A sender seals one copy per entry here. An account with no entries is one whose devices
    /// have never published a key, and the caller should fall back to sending the note in clear or
    /// not at all — never to sealing it to nobody.
    async fn device_keys(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<Vec<DeviceKeyPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        if !authed.username().eq_ignore_ascii_case(&subject)
            && !db.are_friends(authed.username(), &subject)?
        {
            return Err(forbidden("no such account, or it is not visible to you"));
        }

        Ok(db
            .device_keys_for(&subject)?
            .into_iter()
            .map(|key| DeviceKeyPayload {
                device_id: key.device_id,
                public_key: key.public_key,
            })
            .collect())
    }

    /// The public directory. Only accounts that asked to be listed appear in it.
    async fn search_users(
        &self,
        ctx: &Context<'_>,
        query: String,
        limit: Option<i64>,
    ) -> async_graphql::Result<Vec<ProfilePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let found = db.search_users(authed.username(), &query, limit.unwrap_or(20))?;
        let mut payloads = Vec::new();
        for profile in found {
            let state = db.friend_state(authed.username(), &profile.username)?;
            // A block removes the account from results in both directions. Someone the caller
            // blocked should not resurface, and someone who blocked the caller should not be
            // findable by them.
            if state == Some(FriendState::Blocked) {
                continue;
            }
            payloads.push(profile_payload(&profile, state, false));
        }
        Ok(payloads)
    }

    /// Accounts waiting to be let in. The approval queue, for the dashboard.
    async fn pending_accounts(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<ProfilePayload>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .pending_accounts()?
            .iter()
            .map(|p| profile_payload(p, None, false))
            .collect())
    }

    async fn invites(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<InvitePayload>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .list_invites()?
            .into_iter()
            .map(|i| InvitePayload {
                code: i.code,
                created_by: i.created_by,
                created_at: i.created_at,
                expires_at: i.expires_at,
                max_uses: i.max_uses,
                used_count: i.used_count,
                revoked: i.revoked,
            })
            .collect())
    }
}
