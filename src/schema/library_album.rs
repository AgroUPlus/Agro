//! One album of the library, with its tracks: the dashboard's album page.

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;

use super::library::authorize_library;

#[derive(SimpleObject, Clone)]
pub struct LibraryAlbumTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub duration_ms: i64,
}

#[derive(SimpleObject, Clone)]
pub struct LibraryAlbumPayload {
    pub id: String,
    pub title: String,
    pub artist: String,
    /// Fetch artwork from `/api/v1/cover/{coverKey}`.
    pub cover_key: Option<String>,
    pub year: Option<i64>,
    pub total_duration_ms: i64,
    pub tracks: Vec<LibraryAlbumTrack>,
}

#[derive(Default)]
pub struct LibraryAlbumQuery;

#[Object]
impl LibraryAlbumQuery {
    /// The album `id` (as `libraryBrowse` hands it out) in `userId`'s library. Who may look is the
    /// same as for `libraryBrowse`; null when the library holds none of it.
    async fn library_album(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        id: String,
    ) -> async_graphql::Result<Option<LibraryAlbumPayload>> {
        let include_archive = authorize_library(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let Some(album) = db.library_album(&user_id, &id, include_archive)? else {
            return Ok(None);
        };
        Ok(Some(LibraryAlbumPayload {
            id,
            title: album.title,
            artist: album.album_artist,
            cover_key: album.cover_key,
            year: album.year,
            total_duration_ms: album.tracks.iter().map(|t| t.duration_ms).sum(),
            tracks: album
                .tracks
                .into_iter()
                .map(|t| LibraryAlbumTrack {
                    id: t.content_hash,
                    title: t.title,
                    artist: t.artist,
                    track_no: t.track_no,
                    disc_no: t.disc_no,
                    duration_ms: t.duration_ms,
                })
                .collect(),
        }))
    }
}
