//! Creating, deleting and importing playlists, and the owner-only writes that predate
//! collaborative editing. Each change is announced to the owner and followers — see `notify`.

use async_graphql::{Context, Object};

use crate::db::Db;
use crate::importer;
use crate::playlist_visibility::PlaylistVisibility;
use crate::schema::{bounded, caller, forbidden};

use super::generated::refuse_generated;
use super::notify::{announce, announce_change, audience};
use super::payload::{
    requested_visibility, to_item_payload, to_playlist_payload, PlaylistItemPayload,
    PlaylistPayload, PlaylistTrackInput,
};

#[derive(Default)]
pub struct PlaylistWriteMutation;

#[Object]
impl PlaylistWriteMutation {
    /// Creates a new source-agnostic playlist.
    async fn create_playlist(
        &self,
        ctx: &Context<'_>,
        title: String,
        description: Option<String>,
        is_public: Option<bool>,
        visibility: Option<PlaylistVisibility>,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let clean_title = bounded(&title, 255, "title")?;
        let clean_desc = description
            .map(|d| bounded(&d, 1024, "description"))
            .transpose()?;

        let playlist = db.create_playlist(
            authed.username(),
            &clean_title,
            clean_desc.as_deref(),
            requested_visibility(visibility, is_public),
        )?;
        to_playlist_payload(db, playlist, authed.username())
    }

    /// Adds an abstract track to the end of a playlist. Owner only, and not revision-checked: this
    /// is how a playlist is first filled. Anyone else edits through `applyPlaylistEdits`.
    async fn add_track_to_playlist(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        track: PlaylistTrackInput,
    ) -> async_graphql::Result<PlaylistItemPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        refuse_generated(db, &playlist_id)?;

        let playlist = db
            .get_playlist(&playlist_id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;
        if playlist.user_id != authed.username() {
            return Err(forbidden("only the playlist owner may add tracks"));
        }

        let item = db.add_playlist_item(&playlist_id, authed.username(), track.into_item()?)?;
        announce_change(ctx, &playlist_id)?;
        to_item_payload(db, item)
    }

    /// Removes a track from a playlist by item ID. Owner only; see `addTrackToPlaylist`.
    async fn remove_track_from_playlist(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        item_id: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        refuse_generated(db, &playlist_id)?;

        let playlist = db
            .get_playlist(&playlist_id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;
        if playlist.user_id != authed.username() {
            return Err(forbidden("only the playlist owner may remove tracks"));
        }

        let removed = db.remove_playlist_item(&playlist_id, &item_id)?;
        if removed {
            announce_change(ctx, &playlist_id)?;
        }
        Ok(removed)
    }

    /// Sets who can open a playlist: `visibility`, or the older `isPublic` boolean. Owner only.
    /// Narrowing it narrows who can edit it too.
    async fn update_playlist_visibility(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        is_public: Option<bool>,
        visibility: Option<PlaylistVisibility>,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        refuse_generated(db, &playlist_id)?;

        if visibility.is_none() && is_public.is_none() {
            return Err(async_graphql::Error::new("give visibility or isPublic"));
        }
        let changed = db.update_playlist_visibility(
            &playlist_id,
            authed.username(),
            requested_visibility(visibility, is_public),
        )?;
        if changed {
            announce_change(ctx, &playlist_id)?;
        }
        Ok(changed)
    }

    /// Deletes a playlist. Followers are told it is gone, with no revision.
    async fn delete_playlist(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let users = audience(db, &playlist_id, authed.username())?;
        let deleted = db.delete_playlist(&playlist_id, authed.username())?;
        if deleted {
            announce(ctx, &users, &playlist_id, None)?;
        }
        Ok(deleted)
    }

    /// Imports a public playlist, album, or track from Spotify, Deezer, Apple Music, or YouTube
    /// and saves it as an Agro playlist.
    async fn import_external_playlist(
        &self,
        ctx: &Context<'_>,
        url: String,
        title_override: Option<String>,
        is_public: Option<bool>,
        visibility: Option<PlaylistVisibility>,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let imported = importer::import_from_url(db, url.trim())
            .await
            .map_err(async_graphql::Error::new)?;

        let title = title_override.unwrap_or(imported.title);
        let playlist = db.create_playlist(
            authed.username(),
            &title,
            imported.description.as_deref(),
            requested_visibility(visibility, is_public),
        )?;

        if !imported.tracks.is_empty() {
            db.add_playlist_items(&playlist.id, authed.username(), &imported.tracks)?;
        }
        let playlist = db
            .get_playlist(&playlist.id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;
        to_playlist_payload(db, playlist, authed.username())
    }
}
