//! The Blend API: making one from friends, answering an invitation, leaving, changing the recipe.
//!
//! Consent runs both ways. Only accepted friends of the creator can be asked; nobody's listening is
//! read until they accept; and accepting — or creating one — needs stats open and incognito off,
//! the same standing consent a profile already shows to friends. A blend is visible to its joined
//! members alone, never to the creator's other friends (see `can_view_playlist`).

use async_graphql::{Context, Object};

use crate::blend_recipe::{BlendRefresh, BlendWindow};
use crate::db::Db;
use crate::features::Feature;
use crate::schema::{caller, forbidden};
use crate::ws::WsHub;

mod payload;

use payload::{
    consents, describe, open, settings, title_of, BlendPayload, BLEND_INVITE, MAX_INVITED,
};

/// Rewrites the blend if it is due and tells its members, so every path that reads one is current.
pub(crate) fn refresh_and_announce(
    ctx: &Context<'_>,
    db: &Db,
    playlist_id: &str,
) -> async_graphql::Result<()> {
    if !Feature::Blends.is_on(db) {
        return Ok(());
    }
    if let Some(revision) = db.refresh_blend_if_due(playlist_id)? {
        let playlist = db
            .get_playlist(playlist_id)?
            .ok_or_else(|| forbidden("blend vanished"))?;
        let users = crate::schema_playlists::audience(db, playlist_id, &playlist.user_id)?;
        crate::schema_playlists::announce(ctx, &users, playlist_id, Some(revision))?;
    }
    Ok(())
}

#[derive(Default)]
pub struct BlendQuery;

#[Object]
impl BlendQuery {
    /// One blend you are in, with its recipe and members. Rewritten first if it is due.
    async fn blend(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
    ) -> async_graphql::Result<BlendPayload> {
        let me = caller(ctx)?.username();
        let db = ctx.data::<Db>()?;
        Feature::Blends.require(db)?;
        open(db, &playlist_id, me)?;
        refresh_and_announce(ctx, db, &playlist_id)?;
        let (playlist, blend) = open(db, &playlist_id, me)?;
        describe(db, playlist, blend, me)
    }

    /// Blends you have been asked to and not answered.
    async fn blend_invites(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<BlendPayload>> {
        let me = caller(ctx)?.username();
        let db = ctx.data::<Db>()?;
        Feature::Blends.require(db)?;
        let mut out = Vec::new();
        for playlist in db.blend_invites(me)? {
            if let Some(blend) = db.blend(&playlist.id)? {
                out.push(describe(db, playlist, blend, me)?);
            }
        }
        Ok(out)
    }
}

#[derive(Default)]
pub struct BlendMutation;

#[Object]
impl BlendMutation {
    /// Makes a blend of you and up to seven accepted friends, who are asked rather than added.
    #[allow(clippy::too_many_arguments)]
    async fn create_blend(
        &self,
        ctx: &Context<'_>,
        title: String,
        members: Vec<String>,
        size: i64,
        mix: i64,
        window: BlendWindow,
        refresh: BlendRefresh,
    ) -> async_graphql::Result<BlendPayload> {
        let me = caller(ctx)?.username().to_string();
        let db = ctx.data::<Db>()?;
        Feature::Blends.require(db)?;
        let settings = settings(size, mix, window, refresh)?;
        let title = title_of(&title)?;
        consents(db, &me)?;

        let mut invited: Vec<String> = members.iter().map(|m| m.trim().to_lowercase()).collect();
        invited.sort();
        invited.dedup();
        invited.retain(|m| !m.eq_ignore_ascii_case(&me));
        if invited.is_empty() || invited.len() > MAX_INVITED {
            return Err(async_graphql::Error::new(
                "a blend asks between one and seven friends",
            ));
        }
        for name in &invited {
            if !db.are_friends(&me, name)? {
                return Err(forbidden("you can only ask accepted friends into a blend"));
            }
        }

        let playlist = db.create_blend(&me, &title, &invited, settings)?;
        ctx.data::<std::sync::Arc<WsHub>>()?.notify_users(
            &invited,
            BLEND_INVITE,
            serde_json::json!({ "playlistId": playlist.id }),
        );
        let blend = db
            .blend(&playlist.id)?
            .ok_or_else(|| forbidden("blend vanished"))?;
        describe(db, playlist, blend, &me)
    }

    /// Joins or declines a blend you were asked to. `false` when there was no such invitation.
    async fn answer_blend_invite(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        accept: bool,
    ) -> async_graphql::Result<bool> {
        let me = caller(ctx)?.username();
        let db = ctx.data::<Db>()?;
        Feature::Blends.require(db)?;
        if accept {
            consents(db, me)?;
        }
        let answered = db.answer_blend_invite(&playlist_id, me, accept)?;
        // A decline can be the last answer the blend was waiting for, so it writes it too.
        if answered {
            refresh_and_announce(ctx, db, &playlist_id)?;
        }
        Ok(answered)
    }

    /// Leaves a blend; it is rewritten without you. Its creator leaving ends it for everyone.
    async fn leave_blend(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
    ) -> async_graphql::Result<bool> {
        let me = caller(ctx)?.username();
        let db = ctx.data::<Db>()?;
        let (playlist, _) = open(db, &playlist_id, me)?;
        let users = crate::schema_playlists::audience(db, &playlist_id, &playlist.user_id)?;
        if playlist.user_id.eq_ignore_ascii_case(me) {
            let deleted = db.delete_playlist(&playlist_id, &playlist.user_id)?;
            crate::schema_playlists::announce(ctx, &users, &playlist_id, None)?;
            return Ok(deleted);
        }
        let left = db.leave_blend(&playlist_id, me)?;
        refresh_and_announce(ctx, db, &playlist_id)?;
        Ok(left)
    }

    /// Changes a blend's title and recipe. Its creator only; it is rewritten at once.
    #[allow(clippy::too_many_arguments)]
    async fn update_blend(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        title: String,
        size: i64,
        mix: i64,
        window: BlendWindow,
        refresh: BlendRefresh,
    ) -> async_graphql::Result<BlendPayload> {
        let me = caller(ctx)?.username();
        let db = ctx.data::<Db>()?;
        Feature::Blends.require(db)?;
        let (playlist, _) = open(db, &playlist_id, me)?;
        if !playlist.user_id.eq_ignore_ascii_case(me) {
            return Err(forbidden("only the person who made a blend can change it"));
        }
        db.update_blend(
            &playlist_id,
            &title_of(&title)?,
            settings(size, mix, window, refresh)?,
        )?;
        refresh_and_announce(ctx, db, &playlist_id)?;
        let (playlist, blend) = open(db, &playlist_id, me)?;
        describe(db, playlist, blend, me)
    }
}
