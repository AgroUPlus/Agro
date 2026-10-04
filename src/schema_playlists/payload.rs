//! What a playlist looks like over GraphQL, and the input a track arrives as.

use async_graphql::{InputObject, SimpleObject};

use crate::db::Db;
use crate::db_library::album_key;
use crate::db_playlist_items::{NewPlaylistItem, PlaylistItem};
use crate::db_playlists::Playlist;
use crate::playlist_access::{EditAccess, PlaylistRole};
use crate::playlist_visibility::PlaylistVisibility;
use crate::schema::bounded;

#[derive(SimpleObject, Clone)]
pub struct PlaylistItemPayload {
    pub id: String,
    pub playlist_id: String,
    pub position: i32,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub norm_artist: String,
    pub norm_title: String,
    pub artwork_url: Option<String>,
    pub origin_uri: Option<String>,
    /// Who added it. Only meaningful to show once others can edit, but always sent.
    pub added_by: Option<String>,
    pub added_at: Option<String>,
    /// The album's cover on this server, when it has one: fetch `/api/v1/cover/{coverKey}`. Most
    /// items carry no artwork of their own, so this is what lets a playlist show any.
    pub cover_key: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct PlaylistPayload {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: bool,
    /// Who can open it. `isPublic` stays for clients that predate the three levels.
    pub visibility: PlaylistVisibility,
    pub created_at: String,
    pub updated_at: String,
    pub item_count: i32,
    pub items: Vec<PlaylistItemPayload>,
    /// The version an edit must name to be accepted.
    pub revision: i64,
    /// Who besides the owner may edit it.
    pub edit_access: EditAccess,
    /// What the caller may do with it.
    pub my_role: PlaylistRole,
    pub is_following: bool,
    pub total_duration_ms: i64,
    /// Written by Agro from its members' listening (see `blend`); nobody edits it by hand.
    pub is_blend: bool,
}

#[derive(InputObject, Clone)]
pub struct PlaylistTrackInput {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub artwork_url: Option<String>,
    pub origin_uri: Option<String>,
}

/// The longest any single track field may be.
const MAX_FIELD: usize = 512;

impl PlaylistTrackInput {
    /// Checked rather than trusted: every one of these is stored and rendered to other accounts.
    pub fn into_item(self) -> async_graphql::Result<NewPlaylistItem> {
        let optional =
            |value: Option<String>, field: &str| -> async_graphql::Result<Option<String>> {
                value.map(|v| bounded(&v, MAX_FIELD, field)).transpose()
            };
        Ok(NewPlaylistItem {
            title: bounded(&self.title, MAX_FIELD, "title")?,
            artist: bounded(&self.artist, MAX_FIELD, "artist")?,
            album: optional(self.album, "album")?,
            duration_ms: self.duration_ms,
            artwork_url: optional(self.artwork_url, "artworkUrl")?,
            origin_uri: optional(self.origin_uri, "originUri")?,
        })
    }
}

pub fn to_item_payload(db: &Db, item: PlaylistItem) -> async_graphql::Result<PlaylistItemPayload> {
    let cover_key = match item.album.as_deref().filter(|a| !a.trim().is_empty()) {
        Some(album) => {
            let key = album_key(&item.artist, album);
            db.cover_extension(&key)?.map(|_| key)
        }
        None => None,
    };
    Ok(PlaylistItemPayload {
        id: item.id,
        playlist_id: item.playlist_id,
        position: item.position,
        title: item.title,
        artist: item.artist,
        album: item.album,
        duration_ms: item.duration_ms,
        norm_artist: item.norm_artist,
        norm_title: item.norm_title,
        artwork_url: item.artwork_url,
        origin_uri: item.origin_uri,
        added_by: item.added_by,
        added_at: item.added_at,
        cover_key,
    })
}

/// `p` as `viewer` sees it. The caller has already checked they may open it.
pub fn to_playlist_payload(
    db: &Db,
    p: Playlist,
    viewer: &str,
) -> async_graphql::Result<PlaylistPayload> {
    let items = db.get_playlist_items(&p.id)?;
    let my_role = db.playlist_role(&p, viewer)?;
    let is_following = db.is_following_playlist(&p.id, viewer)?;
    let visibility = p.visibility();
    let is_blend = p.is_generated();
    Ok(PlaylistPayload {
        item_count: items.len() as i32,
        total_duration_ms: items.iter().filter_map(|it| it.duration_ms).sum(),
        items: items
            .into_iter()
            .map(|item| to_item_payload(db, item))
            .collect::<async_graphql::Result<_>>()?,
        id: p.id,
        user_id: p.user_id,
        title: p.title,
        description: p.description,
        is_public: p.is_public,
        visibility,
        created_at: p.created_at,
        updated_at: p.updated_at,
        revision: p.revision,
        edit_access: p.edit_access.clamped_to(visibility),
        my_role,
        is_following,
        is_blend,
    })
}

/// The level a client asked for: `visibility` when it knows the three levels, otherwise the
/// `isPublic` boolean every client has always sent. Nothing asked for means private.
pub fn requested_visibility(
    visibility: Option<PlaylistVisibility>,
    is_public: Option<bool>,
) -> PlaylistVisibility {
    visibility.unwrap_or_else(|| PlaylistVisibility::from_public_flag(is_public.unwrap_or(false)))
}
