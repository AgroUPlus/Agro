//! Following a playlist: keeping a live copy of one someone else owns.

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;
use crate::schema::caller;

use super::payload::{to_playlist_payload, PlaylistPayload};

#[derive(SimpleObject, Clone)]
pub struct FollowedPlaylists {
    /// The followed playlists the caller can still open.
    pub playlists: Vec<PlaylistPayload>,
    /// Followed playlists the caller can no longer open: made private, or a friendship ended. A
    /// deleted playlist is in neither list.
    pub revoked_ids: Vec<String>,
}

#[derive(Default)]
pub struct FollowQuery;

#[Object]
impl FollowQuery {
    async fn followed_playlists(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<FollowedPlaylists> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let me = authed.username();

        let mut playlists = Vec::new();
        let mut revoked_ids = Vec::new();
        for p in db.followed_playlists(me)? {
            if db.can_view_playlist(&p, me)? {
                playlists.push(to_playlist_payload(db, p, me)?);
            } else {
                revoked_ids.push(p.id);
            }
        }
        Ok(FollowedPlaylists {
            playlists,
            revoked_ids,
        })
    }
}

#[derive(Default)]
pub struct FollowMutation;

#[Object]
impl FollowMutation {
    /// Starts following a playlist the caller may open, and answers it. Following one's own
    /// playlist is allowed and changes nothing an owner would notice.
    async fn follow_playlist(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let me = authed.username();

        // A playlist that does not exist and one the caller may not open answer the same, so a
        // follow cannot be used to probe for ids.
        let playlist = match db.get_playlist(&id)? {
            Some(p) if db.can_view_playlist(&p, me)? => p,
            _ => return Err(async_graphql::Error::new("playlist not found")),
        };
        db.follow_playlist(&playlist.id, me)?;
        super::notify::announce_follow(ctx, me, &playlist.id, true)?;
        to_playlist_payload(db, playlist, me)
    }

    async fn unfollow_playlist(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let unfollowed = db.unfollow_playlist(&id, authed.username())?;
        if unfollowed {
            super::notify::announce_follow(ctx, authed.username(), &id, false)?;
        }
        Ok(unfollowed)
    }
}
