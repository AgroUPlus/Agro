//! Reading playlists: the ones the caller can find, one by id, and a cheap check for changes.

use std::collections::HashSet;

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;
use crate::schema::{caller, forbidden};

use super::payload::{to_playlist_payload, PlaylistPayload};

/// The most ids one `playlistRevisions` call may ask about.
const MAX_REVISION_IDS: usize = 500;

/// Where a followed playlist stands, without its tracks.
#[derive(SimpleObject, Clone)]
pub struct PlaylistRevision {
    pub id: String,
    /// Absent whenever `accessible` is false, so a refused id reveals nothing about the playlist.
    pub revision: Option<i64>,
    /// False when it is gone or the caller may no longer open it — the two are not told apart.
    pub accessible: bool,
}

#[derive(Default)]
pub struct PlaylistReadQuery;

#[Object]
impl PlaylistReadQuery {
    /// Lists the playlists the caller can open and find: their own, every public one on the
    /// server, and the friends-only ones their accepted friends have shared. A private playlist
    /// is never listed for anyone but its owner.
    async fn playlists(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<PlaylistPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let me = authed.username();

        let mut found = db.list_user_playlists(me)?;
        let mut seen: HashSet<String> = found.iter().map(|p| p.id.clone()).collect();
        for p in db.list_public_playlists()? {
            if seen.insert(p.id.clone()) {
                found.push(p);
            }
        }
        for friend in db.friends(me)? {
            for p in db.list_friends_only_playlists(&friend.username)? {
                if !seen.contains(&p.id) && db.can_view_playlist(&p, me)? {
                    seen.insert(p.id.clone());
                    found.push(p);
                }
            }
        }

        found
            .into_iter()
            .map(|p| to_playlist_payload(db, p, me))
            .collect()
    }

    /// Fetches a single playlist by ID if the caller may open it: they own it, it is public, or it
    /// is friends-only and they are an accepted friend of its owner.
    async fn playlist(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let playlist = db
            .get_playlist(&id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;
        if !db.can_view_playlist(&playlist, authed.username())? {
            return Err(forbidden(
                "you do not have permission to view this playlist",
            ));
        }
        to_playlist_payload(db, playlist, authed.username())
    }

    /// The current revision of each of `ids` the caller may open, so a client holding copies can
    /// tell which have changed without downloading any of them.
    async fn playlist_revisions(
        &self,
        ctx: &Context<'_>,
        ids: Vec<String>,
    ) -> async_graphql::Result<Vec<PlaylistRevision>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        if ids.len() > MAX_REVISION_IDS {
            return Err(format!("ask about at most {MAX_REVISION_IDS} playlists at once").into());
        }

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let open = match db.get_playlist(&id)? {
                Some(p) if db.can_view_playlist(&p, authed.username())? => Some(p.revision),
                _ => None,
            };
            out.push(PlaylistRevision {
                id,
                revision: open,
                accessible: open.is_some(),
            });
        }
        Ok(out)
    }
}
