//! Telling the owner and followers that a playlist changed.
//!
//! The message is the id and the new revision and nothing else — never a title or a track. A
//! client that cares fetches the playlist, and that fetch re-checks access, so a follower who has
//! just lost it learns so from the refusal rather than from anything in the push.

use std::sync::Arc;

use async_graphql::Context;
use serde_json::json;

use crate::db::Db;
use crate::ws::WsHub;

pub const PLAYLIST_UPDATED: &str = "PLAYLIST_UPDATED";
pub const PLAYLIST_FOLLOW: &str = "PLAYLIST_FOLLOW";

/// Tells `user`'s own devices they started or stopped following `playlist_id`, so a follow made
/// in the dashboard reaches the app without waiting for its next look. The id and the direction:
/// a device that wants the playlist fetches it, and that fetch checks access as any other.
pub fn announce_follow(
    ctx: &Context<'_>,
    user: &str,
    playlist_id: &str,
    following: bool,
) -> async_graphql::Result<()> {
    let hub = ctx.data::<Arc<WsHub>>()?;
    hub.notify_user(
        user,
        PLAYLIST_FOLLOW,
        json!({ "id": playlist_id, "following": following }),
    );
    Ok(())
}

/// Who to tell about `playlist_id`: its owner's other devices and everyone following it. Read
/// *before* a delete, which takes the follower rows with it.
pub fn audience(db: &Db, playlist_id: &str, owner: &str) -> async_graphql::Result<Vec<String>> {
    let mut users = db.playlist_follower_ids(playlist_id)?;
    if !users.iter().any(|u| u == owner) {
        users.push(owner.to_string());
    }
    Ok(users)
}

/// Sends `PLAYLIST_UPDATED` to `audience`. `revision` is absent when the playlist is gone.
pub fn announce(
    ctx: &Context<'_>,
    audience: &[String],
    playlist_id: &str,
    revision: Option<i64>,
) -> async_graphql::Result<()> {
    let hub = ctx.data::<Arc<WsHub>>()?;
    hub.notify_users(
        audience,
        PLAYLIST_UPDATED,
        json!({ "id": playlist_id, "revision": revision }),
    );
    Ok(())
}

/// Reads the playlist's audience and current revision, and announces it.
pub fn announce_change(ctx: &Context<'_>, playlist_id: &str) -> async_graphql::Result<()> {
    let db = ctx.data::<Db>()?;
    let Some(playlist) = db.get_playlist(playlist_id)? else {
        return Ok(());
    };
    let users = audience(db, playlist_id, &playlist.user_id)?;
    announce(ctx, &users, playlist_id, Some(playlist.revision))
}
