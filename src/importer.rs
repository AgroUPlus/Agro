//! Universal multi-platform link & playlist importer.
//!
//! Parses tracks, albums, and playlists from Spotify, Deezer, Apple Music, and YouTube / YouTube Music
//! without requiring developer API keys or user logins.
//!
//! Extracted items are converted into `NewPlaylistItem`s with normalized artist/title pairs
//! so Agro can store and resolve them across any source.

use crate::db::Db;
use crate::db_playlist_items::NewPlaylistItem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedPlaylist {
    pub title: String,
    pub description: Option<String>,
    pub artwork_url: Option<String>,
    pub tracks: Vec<NewPlaylistItem>,
}

/// Fetches a URL, using Agro's existing `proxy_cache` when available.
async fn fetch_cached(
    db: &Db,
    client: &reqwest::Client,
    url: &str,
    ttl_secs: i64,
) -> Result<String, String> {
    if let Ok(Some((_, body))) = db.get_cached_proxy(url) {
        if let Ok(text) = String::from_utf8(body) {
            return Ok(text);
        }
    }

    let resp = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0",
        )
        .send()
        .await
        .map_err(|e| format!("Failed to fetch URL: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error {}", resp.status()));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Read body error: {e}"))?;
    let now = chrono::Utc::now().timestamp();
    let _ = db.set_cached_proxy(url, "{}", text.as_bytes(), now + ttl_secs);

    Ok(text)
}

/// Imports an external playlist or album link into an `ImportedPlaylist`.
pub async fn import_from_url(db: &Db, url: &str) -> Result<ImportedPlaylist, String> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| e.to_string())?;

    if let Some(caps) = SPOTIFY_RE.captures(url) {
        let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        return import_spotify(db, &client, kind, id).await;
    }

    if let Some(caps) = DEEZER_RE.captures(url) {
        let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        return import_deezer(db, &client, kind, id).await;
    }

    if let Some(caps) = APPLE_MUSIC_RE.captures(url) {
        let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let id = caps.get(3).map(|m| m.as_str()).unwrap_or("");
        return import_apple_music(db, &client, url, kind, id).await;
    }

    if let Some(caps) = YT_PLAYLIST_RE.captures(url) {
        let list_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        return import_youtube_playlist(db, &client, list_id).await;
    }

    if let Some(caps) = YT_WATCH_RE.captures(url) {
        let video_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        return import_youtube_track(db, &client, video_id).await;
    }

    Err(
        "Unsupported URL format. Supported sources: Spotify, Deezer, Apple Music, YouTube."
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// SPOTIFY IMPORTER (Using public oEmbed & Embed HTML parsing)
// ---------------------------------------------------------------------------

mod apple_music;
mod deezer;
mod spotify;
mod youtube;
use apple_music::import_apple_music;
use apple_music::APPLE_MUSIC_RE;
use deezer::import_deezer;
use deezer::DEEZER_RE;
use spotify::import_spotify;
use spotify::SPOTIFY_RE;
use youtube::import_youtube_playlist;
use youtube::import_youtube_track;
use youtube::parse_iso8601_duration;
use youtube::YT_PLAYLIST_RE;
use youtube::YT_WATCH_RE;
#[cfg(test)]
mod tests;
