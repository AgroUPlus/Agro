//! Importing an Apple Music playlist.

use super::*;
use crate::db::Db;
use crate::db_playlist_items::NewPlaylistItem;
use regex::Regex;
use std::sync::LazyLock;

pub(super) static APPLE_MUSIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"music\.apple\.com/(?:[a-zA-Z]{2}/)?(album|playlist)/([^/]+)/([a-zA-Z0-9._-]+)")
        .expect("regex")
});

pub(super) async fn import_apple_music(
    db: &Db,
    client: &reqwest::Client,
    url: &str,
    _kind: &str,
    _id: &str,
) -> Result<ImportedPlaylist, String> {
    let html = fetch_cached(db, client, url, 86400 * 7).await?;

    let mut title = "Apple Music Import".to_string();
    let mut tracks = Vec::new();

    // Look for <script type="application/ld+json">
    let ld_tag = "<script type=\"application/ld+json\">";
    if let Some(start) = html.find(ld_tag) {
        let sub = &html[start + ld_tag.len()..];
        if let Some(end) = sub.find("</script>") {
            let json_str = &sub[..end];
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(name) = data["name"].as_str() {
                    title = name.to_string();
                }

                if let Some(track_array) = data["tracks"].as_array() {
                    for item in track_array {
                        let t_title = item["name"].as_str().unwrap_or("").to_string();
                        let t_artist = item["byArtist"]["name"]
                            .as_str()
                            .unwrap_or("Unknown Artist")
                            .to_string();
                        let duration_ms =
                            item["duration"].as_str().and_then(parse_iso8601_duration);

                        if !t_title.is_empty() {
                            tracks.push(NewPlaylistItem {
                                title: t_title,
                                artist: t_artist,
                                album: Some(title.clone()),
                                duration_ms,
                                artwork_url: None,
                                origin_uri: Some(url.to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    if tracks.is_empty() {
        return Err("Could not extract tracks from Apple Music page.".to_string());
    }

    Ok(ImportedPlaylist {
        title,
        description: Some("Imported from Apple Music".to_string()),
        artwork_url: None,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// YOUTUBE & YOUTUBE MUSIC IMPORTER
// ---------------------------------------------------------------------------
