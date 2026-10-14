//! Editing the caller's profile, visibility and incognito.

use super::*;
use crate::db::Db;
use crate::db_social::LEGACY_DEVICE_ID;
use crate::schema::{bounded, caller, forbidden};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct SocialProfileMutation;

#[Object]
impl SocialProfileMutation {
    /// Edits the caller's own profile. Fields left out are left alone rather than blanked.
    async fn update_profile(
        &self,
        ctx: &Context<'_>,
        display_name: Option<String>,
        bio: Option<String>,
        avatar_url: Option<String>,
    ) -> async_graphql::Result<ProfilePayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        // Every one of these is stored and rendered on someone else's screen.
        let display_name = display_name
            .map(|v| bounded(&v, 64, "Display name"))
            .transpose()?;
        let bio = bio.map(|v| bounded(&v, 280, "Bio")).transpose()?;
        let avatar_url = avatar_url.map(|v| validated_avatar(&v)).transpose()?;

        db.update_profile(
            authed.username(),
            display_name.as_deref(),
            bio.as_deref(),
            avatar_url.as_deref(),
        )?;
        let profile = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        Ok(profile_payload(&profile, None, false))
    }

    /// Publishes or withdraws one device's identity key.
    ///
    /// `device_id` names which of the caller's devices this key belongs to, and only that entry is
    /// touched. Omitting it writes the `legacy` entry — which is what a client that predates the
    /// registry is doing whether it knows it or not, and it is the behaviour those clients already
    /// had: one key for the account.
    ///
    /// `users.public_key` is still written, because older clients read it and nothing else. It is
    /// last-writer-wins, so it will hold whichever device published most recently; that is
    /// tolerable now only because nothing that seals a note reads it any more.
    async fn set_public_key(
        &self,
        ctx: &Context<'_>,
        public_key: Option<String>,
        device_id: Option<String>,
    ) -> async_graphql::Result<ProfilePayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let key = public_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(k) = key {
            if k.len() > 512 {
                return Err("Public key is too long (max 512 bytes)".into());
            }
        }

        let device = match device_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => bounded(id, 128, "deviceId")?,
            None => LEGACY_DEVICE_ID.to_string(),
        };
        match key {
            Some(k) => db.register_device_key(authed.username(), &device, k)?,
            // Withdrawing removes this device's entry only. The other devices on the account keep
            // receiving sealed messages, which is the entire point of the registry.
            None => {
                db.forget_device_key(authed.username(), &device)?;
            }
        }

        db.set_public_key(authed.username(), key)?;
        let profile = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        Ok(profile_payload(&profile, None, false))
    }

    /// The switches that decide what a friend can see, whether strangers can find you, and whether
    /// your plays feed the fleet-wide Popular on Agro chart.
    ///
    /// One mutation rather than one per switch, so the privacy screen writes what the user sees in
    /// a single round trip and cannot land half-applied. Each switch is optional and one left out is
    /// left alone.
    ///
    /// This matters more than it looks: the flags are independent decisions, and requiring all of
    /// them on every call means a client flipping one has to resend its idea of the rest. Two
    /// devices doing that concurrently silently undo each other — a switch turned on over here gets
    /// reverted by a stale copy sent from over there.
    #[allow(clippy::too_many_arguments)]
    async fn set_visibility(
        &self,
        ctx: &Context<'_>,
        show_now_playing: Option<bool>,
        show_stats: Option<bool>,
        discoverable: Option<bool>,
        share_library: Option<bool>,
        show_activity: Option<bool>,
        popular_opt_in: Option<bool>,
    ) -> async_graphql::Result<ProfilePayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let current = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        let show_now_playing = show_now_playing.unwrap_or(current.show_now_playing);
        let show_stats = show_stats.unwrap_or(current.show_stats);
        let discoverable = discoverable.unwrap_or(current.discoverable);

        db.set_visibility(authed.username(), show_now_playing, show_stats)?;
        db.set_discoverable(authed.username(), discoverable)?;
        if let Some(share) = share_library {
            db.set_share_library(authed.username(), share)?;
        }
        if let Some(show) = show_activity {
            db.set_show_activity(authed.username(), show)?;
        }
        if let Some(opt_in) = popular_opt_in {
            db.set_popular_opt_in(authed.username(), opt_in)?;
        }

        // Turning now-playing off ends every session following it. Leaving them attached would mean
        // a switch the user just set to "off" still feeding the thing it was meant to stop.
        if !show_now_playing {
            for listener in db.listeners_of(authed.username())? {
                db.clear_listen_along(&listener)?;
                ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                    &listener,
                    "LISTEN_ALONG",
                    serde_json::json!({ "stopped": true, "host": authed.username() }),
                );
            }
        }

        let profile = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        Ok(profile_payload(&profile, None, false))
    }

    /// Goes quiet, or stops being quiet.
    ///
    /// Its own mutation rather than a sixth flag on `set_visibility`, because it is a different
    /// kind of thing: the others are standing consents, this is a temporary override of all of
    /// them. Bundling it would mean leaving incognito had to restore the rest from a client's idea
    /// of what they were, and a stale copy would silently turn a privacy switch back on.
    ///
    /// The server is the authority. Every client reads this value rather than keeping its own, so
    /// one device going quiet takes the whole account quiet — a phone still announcing the account
    /// from a pocket is exactly the failure a device-local incognito has.
    async fn set_incognito(
        &self,
        ctx: &Context<'_>,
        incognito: bool,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        db.set_incognito(authed.username(), incognito)?;

        // Going quiet ends every session following this account, for the same reason turning
        // now-playing off does: a listener left attached keeps receiving the thing incognito was
        // turned on to stop.
        if incognito {
            for listener in db.listeners_of(authed.username())? {
                db.clear_listen_along(&listener)?;
                ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                    &listener,
                    "LISTEN_ALONG",
                    serde_json::json!({ "stopped": true, "host": authed.username() }),
                );
            }
        }

        // Tells this account's *own* devices, so the switch appears flipped on all of them rather
        // than only on the one it was touched from. Friends are told nothing: that an account went
        // quiet is itself a disclosure, and the refusal they meet is the same one a stranger gets.
        ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
            authed.username(),
            "SETTINGS_SYNC",
            serde_json::json!({ "incognito": incognito }),
        );

        Ok(incognito)
    }
}
