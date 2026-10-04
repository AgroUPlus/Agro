//! What the Blend API answers with, and the checks every resolver shares.

use async_graphql::SimpleObject;

use crate::blend_recipe::{Blend, BlendRefresh, BlendSettings, BlendWindow};
use crate::db::Db;
use crate::db_playlists::Playlist;
use crate::schema::{bounded, forbidden};

/// Sizes a client may ask for. Fixed, so a blend is never a 4,000-track query by accident.
pub(super) const SIZES: [i64; 3] = [25, 50, 100];
/// Friends a creator may ask at once, besides themselves.
pub(super) const MAX_INVITED: usize = 7;
pub(crate) const BLEND_INVITE: &str = "BLEND_INVITE";

#[derive(SimpleObject, Clone)]
pub struct BlendMemberPayload {
    pub username: String,
    /// False while they have been asked and not yet answered.
    pub joined: bool,
}

#[derive(SimpleObject, Clone)]
pub struct BlendPayload {
    pub playlist_id: String,
    pub title: String,
    pub created_by: String,
    pub is_creator: bool,
    pub members: Vec<BlendMemberPayload>,
    pub size: i64,
    pub mix: i64,
    pub window: BlendWindow,
    pub refresh: BlendRefresh,
    pub refreshed_at: Option<String>,
    /// When it rewrites itself next. Absent when frozen or not written yet.
    pub next_refresh_at: Option<String>,
}

pub(super) fn describe(
    db: &Db,
    playlist: Playlist,
    blend: Blend,
    viewer: &str,
) -> async_graphql::Result<BlendPayload> {
    let members = db
        .blend_members(&playlist.id)?
        .into_iter()
        .map(|m| BlendMemberPayload {
            username: m.username,
            joined: m.joined,
        })
        .collect();
    let next = blend
        .next_refresh_at()
        .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
        .map(|at| at.to_rfc3339());
    Ok(BlendPayload {
        is_creator: playlist.user_id.eq_ignore_ascii_case(viewer),
        playlist_id: playlist.id,
        title: playlist.title,
        created_by: playlist.user_id,
        members,
        size: blend.size,
        mix: blend.mix,
        window: blend.window,
        refresh: blend.refresh,
        refreshed_at: blend.refreshed_at,
        next_refresh_at: next,
    })
}

/// The blend and its playlist, if `viewer` may open it. One refusal for "no such blend" and "not
/// yours to see", so an id is not a way to learn which blends exist.
pub(super) fn open(
    db: &Db,
    playlist_id: &str,
    viewer: &str,
) -> async_graphql::Result<(Playlist, Blend)> {
    let refused = || forbidden("no blend you are in has that id");
    let playlist = db.get_playlist(playlist_id)?.ok_or_else(refused)?;
    if !playlist.is_generated() || !db.can_view_playlist(&playlist, viewer)? {
        return Err(refused());
    }
    let blend = db.blend(playlist_id)?.ok_or_else(refused)?;
    Ok((playlist, blend))
}

pub(super) fn consents(db: &Db, username: &str) -> async_graphql::Result<()> {
    if db.profile(username)?.is_some_and(|p| p.shows_stats()) {
        Ok(())
    } else {
        Err(forbidden(
            "a Blend reads your listening, so it needs your stats shared and incognito off",
        ))
    }
}

pub(super) fn settings(
    size: i64,
    mix: i64,
    window: BlendWindow,
    refresh: BlendRefresh,
) -> async_graphql::Result<BlendSettings> {
    if !SIZES.contains(&size) {
        return Err(async_graphql::Error::new(
            "a blend holds 25, 50 or 100 tracks",
        ));
    }
    if !(0..=100).contains(&mix) {
        return Err(async_graphql::Error::new(
            "mix runs from 0 (common ground) to 100 (discovery)",
        ));
    }
    Ok(BlendSettings {
        size,
        mix,
        window,
        refresh,
    })
}

pub(super) fn title_of(raw: &str) -> async_graphql::Result<String> {
    let title = bounded(raw, 255, "title")?;
    if title.is_empty() {
        return Err(async_graphql::Error::new("a blend needs a title"));
    }
    Ok(title)
}
