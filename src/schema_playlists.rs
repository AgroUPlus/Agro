//! GraphQL resolvers for source-agnostic playlists and external link ingestion.

use async_graphql::{Context, InputObject, Object, SimpleObject};

use crate::db::Db;
use crate::db_playlists::{NewPlaylistItem, Playlist, PlaylistItem};
use crate::importer;
use crate::schema::{bounded, caller, forbidden};

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
}

#[derive(SimpleObject, Clone)]
pub struct PlaylistPayload {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub created_at: String,
    pub updated_at: String,
    pub item_count: i32,
    pub items: Vec<PlaylistItemPayload>,
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

fn to_item_payload(item: PlaylistItem) -> PlaylistItemPayload {
    PlaylistItemPayload {
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
    }
}

fn to_playlist_payload(db: &Db, p: Playlist) -> async_graphql::Result<PlaylistPayload> {
    let items = db.get_playlist_items(&p.id)?;
    let item_count = items.len() as i32;
    Ok(PlaylistPayload {
        id: p.id,
        user_id: p.user_id,
        title: p.title,
        description: p.description,
        is_public: p.is_public,
        created_at: p.created_at,
        updated_at: p.updated_at,
        item_count,
        items: items.into_iter().map(to_item_payload).collect(),
    })
}

#[derive(Default)]
pub struct PlaylistsQuery;

#[Object]
impl PlaylistsQuery {
    /// Lists all playlists accessible to the caller: their own private/public playlists
    /// plus all public playlists on the server.
    async fn playlists(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<PlaylistPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let mut user_lists = db.list_user_playlists(authed.username())?;
        let public_lists = db.list_public_playlists()?;

        // Merge and deduplicate by ID
        let mut seen = std::collections::HashSet::new();
        for p in &user_lists {
            seen.insert(p.id.clone());
        }

        for p in public_lists {
            if !seen.contains(&p.id) {
                user_lists.push(p);
            }
        }

        let mut payloads = Vec::new();
        for p in user_lists {
            payloads.push(to_playlist_payload(db, p)?);
        }
        Ok(payloads)
    }

    /// Fetches a single playlist by ID if it belongs to the caller or is marked public.
    async fn playlist(&self, ctx: &Context<'_>, id: String) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let playlist = db
            .get_playlist(&id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;

        if !playlist.is_public && playlist.user_id != authed.username() {
            return Err(forbidden("you do not have permission to view this private playlist"));
        }

        to_playlist_payload(db, playlist)
    }
}

#[derive(Default)]
pub struct PlaylistsMutation;

#[Object]
impl PlaylistsMutation {
    /// Creates a new source-agnostic playlist.
    async fn create_playlist(
        &self,
        ctx: &Context<'_>,
        title: String,
        description: Option<String>,
        is_public: Option<bool>,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let clean_title = bounded(&title, 255, "title")?;
        let clean_desc = match description {
            Some(d) => Some(bounded(&d, 1024, "description")?),
            None => None,
        };

        let playlist = db.create_playlist(
            authed.username(),
            &clean_title,
            clean_desc.as_deref(),
            is_public.unwrap_or(false),
        )?;

        to_playlist_payload(db, playlist)
    }

    /// Adds an abstract track to a playlist.
    async fn add_track_to_playlist(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        track: PlaylistTrackInput,
    ) -> async_graphql::Result<PlaylistItemPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let playlist = db
            .get_playlist(&playlist_id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;

        if playlist.user_id != authed.username() {
            return Err(forbidden("only the playlist owner may add tracks"));
        }

        let clean_title = bounded(&track.title, 512, "title")?;
        let clean_artist = bounded(&track.artist, 512, "artist")?;

        let item = db.add_playlist_item(
            &playlist_id,
            NewPlaylistItem {
                title: clean_title,
                artist: clean_artist,
                album: track.album,
                duration_ms: track.duration_ms,
                artwork_url: track.artwork_url,
                origin_uri: track.origin_uri,
            },
        )?;

        Ok(to_item_payload(item))
    }

    /// Removes a track from a playlist by item ID.
    async fn remove_track_from_playlist(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        item_id: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let playlist = db
            .get_playlist(&playlist_id)?
            .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;

        if playlist.user_id != authed.username() {
            return Err(forbidden("only the playlist owner may remove tracks"));
        }

        Ok(db.remove_playlist_item(&playlist_id, &item_id)?)
    }

    /// Updates playlist visibility (public vs private).
    async fn update_playlist_visibility(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        is_public: bool,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        Ok(db.update_playlist_visibility(&playlist_id, authed.username(), is_public)?)
    }

    /// Deletes a playlist.
    async fn delete_playlist(&self, ctx: &Context<'_>, playlist_id: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        Ok(db.delete_playlist(&playlist_id, authed.username())?)
    }

    /// Imports a public playlist, album, or track from Spotify, Deezer, Apple Music, or YouTube
    /// and saves it as an Agro playlist.
    async fn import_external_playlist(
        &self,
        ctx: &Context<'_>,
        url: String,
        title_override: Option<String>,
        is_public: Option<bool>,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let imported = importer::import_from_url(db, url.trim())
            .await
            .map_err(|e| async_graphql::Error::new(e))?;

        let title = title_override.unwrap_or(imported.title);
        let playlist = db.create_playlist(
            authed.username(),
            &title,
            imported.description.as_deref(),
            is_public.unwrap_or(false),
        )?;

        if !imported.tracks.is_empty() {
            db.add_playlist_items(&playlist.id, &imported.tracks)?;
        }

        to_playlist_payload(db, playlist)
    }
}
