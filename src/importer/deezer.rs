//! Importing a Deezer playlist.

use super::*;
use crate::db::Db;
use crate::db_playlist_items::NewPlaylistItem;
use regex::Regex;
use std::sync::LazyLock;

pub(super) static DEEZER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"deezer\.com/(?:[a-zA-Z]{2}/)?(playlist|album|track)/(\d+)").expect("regex")
});

pub(super) async fn import_deezer(
    db: &Db,
    client: &reqwest::Client,
    kind: &str,
    id: &str,
) -> Result<ImportedPlaylist, String> {
    let api_url = format!("https://api.deezer.com/{kind}/{id}");
    let json_text = fetch_cached(db, client, &api_url, 86400 * 7).await?;

    let data: serde_json::Value =
        serde_json::from_str(&json_text).map_err(|e| format!("Deezer API parse error: {e}"))?;

    let title = data["title"]
        .as_str()
        .unwrap_or("Deezer Import")
        .to_string();
    let description = data["description"].as_str().map(|s| s.to_string());
    let artwork_url = data["picture_big"]
        .as_str()
        .or_else(|| data["cover_big"].as_str())
        .map(|s| s.to_string());

    let mut tracks = Vec::new();

    if kind == "track" {
        let artist = data["artist"]["name"]
            .as_str()
            .unwrap_or("Unknown Artist")
            .to_string();
        let album = data["album"]["title"].as_str().map(|s| s.to_string());
        let duration_ms = data["duration"].as_i64().map(|d| d * 1000);
        let link = data["link"].as_str().map(|s| s.to_string());

        tracks.push(NewPlaylistItem {
            title: title.clone(),
            artist,
            album,
            duration_ms,
            artwork_url: artwork_url.clone(),
            origin_uri: link,
        });
    } else if let Some(items) = data["tracks"]["data"].as_array() {
        for item in items {
            let t_title = item["title"].as_str().unwrap_or("").to_string();
            let t_artist = item["artist"]["name"]
                .as_str()
                .unwrap_or("Unknown Artist")
                .to_string();
            let t_album = item["album"]["title"].as_str().map(|s| s.to_string());
            let duration_ms = item["duration"].as_i64().map(|d| d * 1000);
            let link = item["link"].as_str().map(|s| s.to_string());

            if !t_title.is_empty() {
                tracks.push(NewPlaylistItem {
                    title: t_title,
                    artist: t_artist,
                    album: t_album,
                    duration_ms,
                    artwork_url: artwork_url.clone(),
                    origin_uri: link,
                });
            }
        }
    }

    Ok(ImportedPlaylist {
        title,
        description,
        artwork_url,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// APPLE MUSIC IMPORTER (Using HTML JSON-LD parsing)
// ---------------------------------------------------------------------------
