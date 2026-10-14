//! Friend requests, codes, removal and blocks.

use super::*;
use crate::db::Db;
use crate::db_social::FriendState;
use crate::schema::{caller, normalise_username};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct SocialFriendsMutation;

#[Object]
impl SocialFriendsMutation {
    /// Asks to be someone's friend.
    ///
    /// Answers `true` for a request that was recorded and `false` for one that was not, without
    /// saying which of the several reasons applied — an existing edge, a block, or no such account
    /// all look the same from here, and telling them apart would let this be used to probe for
    /// accounts that chose not to be listed.
    async fn send_friend_request(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        let Some(profile) = db.profile(&subject)? else {
            return Ok(false);
        };
        // Reachable by the same rule `profile` uses: listed, or already connected.
        let state = db.friend_state(authed.username(), &subject)?;
        if state.is_some() || !profile.discoverable {
            return Ok(false);
        }

        let sent = db.send_friend_request(authed.username(), &subject)?;
        if sent {
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &subject,
                "FRIEND_REQUEST",
                serde_json::json!({ "from": authed.username() }),
            );
        }
        Ok(sent)
    }

    async fn accept_friend_request(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        let accepted = db.accept_friend_request(authed.username(), &subject)?;
        if accepted {
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &subject,
                "FRIEND_REQUEST",
                serde_json::json!({ "accepted_by": authed.username() }),
            );
        }
        Ok(accepted)
    }

    /// Mints a short-lived code for adding the caller as a friend in person.
    ///
    /// For the case the username search cannot serve: two people in the same room, one of whom is
    /// not `discoverable` and should not have to become so just to be added once. The code stands
    /// in for the search, not for the consent — redeeming it still produces the same friend edge
    /// the ordinary flow does.
    ///
    /// Any previous code for this account is dropped when a new one is minted, so only the code
    /// currently on screen works. Clients are expected to re-mint every few minutes while the
    /// panel is open and to call `revokeFriendCode` when it closes.
    async fn create_friend_code(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<FriendCodePayload> {
        let authed = caller(ctx)?;
        let code = ctx
            .data::<Db>()?
            .create_friend_code(authed.username(), FRIEND_CODE_TTL_MINUTES)?;
        Ok(FriendCodePayload {
            code: code.code,
            expires_at: code.expires_at,
            ttl_seconds: FRIEND_CODE_TTL_MINUTES * 60,
        })
    }

    /// Drops the caller's outstanding code, for a panel being closed or the app going away.
    async fn revoke_friend_code(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        ctx.data::<Db>()?.revoke_friend_codes(authed.username())?;
        Ok(true)
    }

    /// Spends a code and becomes friends with whoever minted it.
    ///
    /// Both directions at once, not a request to be accepted: the two people were standing
    /// together and one of them showed the other a screen, so the consent has already happened in
    /// a way a notification cannot improve on.
    ///
    /// Returns the username on success and null for every failure — unknown, expired, already
    /// spent, or the caller's own code. Distinguishing them would turn this into an oracle for
    /// which codes have existed.
    async fn redeem_friend_code(
        &self,
        ctx: &Context<'_>,
        code: String,
    ) -> async_graphql::Result<Option<String>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let Some(owner) = db.redeem_friend_code(&code)? else {
            return Ok(None);
        };
        // Redeeming your own code is a no-op rather than an error, and the code is spent either
        // way — a screen scanned by its own owner should not stay live for the next person.
        if owner.eq_ignore_ascii_case(authed.username()) {
            return Ok(None);
        }
        // A block from either side comes back as `Blocked` from `friend_state`, which is the one
        // state a code must not be able to talk its way past.
        if db.friend_state(authed.username(), &owner)? == Some(FriendState::Blocked) {
            return Ok(None);
        }

        // Two halves of one handshake. `send_friend_request` then `accept_friend_request` reuses
        // the paths that already know how to build the edge, rather than writing a third one that
        // has to agree with them.
        db.send_friend_request(&owner, authed.username())?;
        let accepted = db.accept_friend_request(authed.username(), &owner)?;
        if !accepted {
            return Ok(None);
        }

        ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
            &owner,
            "FRIEND_REQUEST",
            serde_json::json!({ "accepted_by": authed.username() }),
        );
        Ok(Some(owner))
    }

    /// Declines a request, or ends a friendship. The same operation on the data either way.
    async fn remove_friend(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        // Whoever is following whom, the session cannot outlive the friendship that permitted it.
        db.clear_listen_along(authed.username())?;
        db.clear_listen_along(&subject)?;
        let removed = db.remove_friend(authed.username(), &subject)?;
        if removed {
            // Tells the other side, so a request they sent stops sitting in their outgoing list
            // waiting for an answer that already happened.
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &subject,
                "FRIEND_REQUEST",
                serde_json::json!({ "declined_by": authed.username() }),
            );
        }
        Ok(removed)
    }

    async fn block_user(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        db.clear_listen_along(authed.username())?;
        db.clear_listen_along(&subject)?;
        db.block_user(authed.username(), &subject)
            .map_err(Into::into)
    }

    async fn unblock_user(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;
        db.unblock_user(authed.username(), &subject)
            .map_err(Into::into)
    }
}
