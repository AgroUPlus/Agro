//! Importing a Spotify playlist.

use super::*;
use crate::db::Db;
use crate::db_playlist_items::NewPlaylistItem;
use regex::Regex;
use std::sync::LazyLock;

pub(super) static SPOTIFY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"open\.spotify\.com/(playlist|album|track)/([a-zA-Z0-9]+)").expect("regex")
});

pub(super) async fn import_spotify(
    db: &Db,
    client: &reqwest::Client,
    kind: &str,
    id: &str,
) -> Result<ImportedPlaylist, String> {
    let target_url = format!("https://open.spotify.com/{kind}/{id}");
    let oembed_url = format!("https://open.spotify.com/oembed?url={target_url}");
    let oembed_json = fetch_cached(db, client, &oembed_url, 86400 * 7).await?;

    let oembed: serde_json::Value = serde_json::from_str(&oembed_json)
        .map_err(|e| format!("Spotify oEmbed parse error: {e}"))?;

    let title = oembed["title"]
        .as_str()
        .unwrap_or("Spotify Import")
        .to_string();
    let artwork_url = oembed["thumbnail_url"].as_str().map(|s| s.to_string());

    if kind == "track" {
        let artist = oembed["author_name"]
            .as_str()
            .unwrap_or("Unknown Artist")
            .to_string();
        return Ok(ImportedPlaylist {
            title: title.clone(),
            description: Some(format!("Imported from Spotify track {id}")),
            artwork_url: artwork_url.clone(),
            tracks: vec![NewPlaylistItem {
                title,
                artist,
                album: None,
                duration_ms: None,
                artwork_url,
                origin_uri: Some(target_url),
            }],
        });
    }

    // For playlists & albums, fetch the embed iframe which contains structured track data
    let embed_url = format!("https://open.spotify.com/embed/{kind}/{id}");
    let html = fetch_cached(db, client, &embed_url, 86400 * 3).await?;

    let mut tracks = Vec::new();

    // Extract next.js data script if present
    if let Some(start) = html.find("<script id=\"__NEXT_DATA__\" type=\"application/json\">") {
        let sub = &html[start + 51..];
        if let Some(end) = sub.find("</script>") {
            let json_str = &sub[..end];
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(items) =
                    data["props"]["pageProps"]["state"]["data"]["entity"]["trackList"].as_array()
                {
                    for item in items {
                        let t_title = item["title"].as_str().unwrap_or("").to_string();
                        let t_artist = item["subtitle"].as_str().unwrap_or("").to_string();
                        let duration_ms = item["duration"].as_i64();
                        let t_uri = item["uri"].as_str().map(|s| s.to_string());

                        if !t_title.is_empty() {
                            tracks.push(NewPlaylistItem {
                                title: t_title,
                                artist: if t_artist.is_empty() {
                                    "Unknown Artist".to_string()
                                } else {
                                    t_artist
                                },
                                album: None,
                                duration_ms,
                                artwork_url: artwork_url.clone(),
                                origin_uri: t_uri.or_else(|| Some(target_url.clone())),
                            });
                        }
                    }
                }
            }
        }
    }

    if tracks.is_empty() {
        // Fallback: at least import the album/playlist entity itself
        let artist = oembed["author_name"]
            .as_str()
            .unwrap_or("Spotify")
            .to_string();
        tracks.push(NewPlaylistItem {
            title: title.clone(),
            artist,
            album: Some(title.clone()),
            duration_ms: None,
            artwork_url: artwork_url.clone(),
            origin_uri: Some(target_url),
        });
    }

    Ok(ImportedPlaylist {
        title,
        description: Some(format!("Imported from Spotify {kind}")),
        artwork_url,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// DEEZER IMPORTER (Using open public REST API)
// ---------------------------------------------------------------------------
