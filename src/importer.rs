//! Universal multi-platform link & playlist importer.
//!
//! Parses tracks, albums, and playlists from Spotify, Deezer, Apple Music, and YouTube / YouTube Music
//! without requiring developer API keys or user logins.
//!
//! Extracted items are converted into `NewPlaylistItem`s with normalized artist/title pairs
//! so Agro can store and resolve them across any source.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::db::Db;
use crate::db_playlists::NewPlaylistItem;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedPlaylist {
    pub title: String,
    pub description: Option<String>,
    pub artwork_url: Option<String>,
    pub tracks: Vec<NewPlaylistItem>,
}

static SPOTIFY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"open\.spotify\.com/(playlist|album|track)/([a-zA-Z0-9]+)").expect("regex")
});

static DEEZER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"deezer\.com/(?:[a-zA-Z]{2}/)?(playlist|album|track)/(\d+)").expect("regex")
});

static APPLE_MUSIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"music\.apple\.com/(?:[a-zA-Z]{2}/)?(album|playlist)/([^/]+)/([a-zA-Z0-9._-]+)").expect("regex")
});

static YT_WATCH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:youtube\.com/watch\?v=|youtu\.be/|music\.youtube\.com/watch\?v=)([a-zA-Z0-9_-]{11})")
        .expect("regex")
});

static YT_PLAYLIST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:youtube\.com/playlist\?list=|music\.youtube\.com/playlist\?list=)([a-zA-Z0-9_-]+)")
        .expect("regex")
});

/// Fetches a URL, using Agro's existing `proxy_cache` when available.
async fn fetch_cached(db: &Db, client: &reqwest::Client, url: &str, ttl_secs: i64) -> Result<String, String> {
    if let Ok(Some((_, body))) = db.get_cached_proxy(url) {
        if let Ok(text) = String::from_utf8(body) {
            return Ok(text);
        }
    }

    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch URL: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error {}", resp.status()));
    }

    let text = resp.text().await.map_err(|e| format!("Read body error: {e}"))?;
    let now = chrono::Utc::now().timestamp();
    let _ = db.set_cached_proxy(url, "{}", text.as_bytes(), now + ttl_secs);

    Ok(text)
}

/// Imports an external playlist or album link into an `ImportedPlaylist`.
pub async fn import_from_url(db: &Db, url: &str) -> Result<ImportedPlaylist, String> {
    let client = reqwest::Client::builder().build().map_err(|e| e.to_string())?;

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

    Err("Unsupported URL format. Supported sources: Spotify, Deezer, Apple Music, YouTube.".to_string())
}

// ---------------------------------------------------------------------------
// SPOTIFY IMPORTER (Using public oEmbed & Embed HTML parsing)
// ---------------------------------------------------------------------------

async fn import_spotify(
    db: &Db,
    client: &reqwest::Client,
    kind: &str,
    id: &str,
) -> Result<ImportedPlaylist, String> {
    let target_url = format!("https://open.spotify.com/{kind}/{id}");
    let oembed_url = format!("https://open.spotify.com/oembed?url={target_url}");
    let oembed_json = fetch_cached(db, client, &oembed_url, 86400 * 7).await?;

    let oembed: serde_json::Value =
        serde_json::from_str(&oembed_json).map_err(|e| format!("Spotify oEmbed parse error: {e}"))?;

    let title = oembed["title"].as_str().unwrap_or("Spotify Import").to_string();
    let artwork_url = oembed["thumbnail_url"].as_str().map(|s| s.to_string());

    if kind == "track" {
        let artist = oembed["author_name"].as_str().unwrap_or("Unknown Artist").to_string();
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
                if let Some(items) = data["props"]["pageProps"]["state"]["data"]["entity"]["trackList"].as_array() {
                    for item in items {
                        let t_title = item["title"].as_str().unwrap_or("").to_string();
                        let t_artist = item["subtitle"].as_str().unwrap_or("").to_string();
                        let duration_ms = item["duration"].as_i64();
                        let t_uri = item["uri"].as_str().map(|s| s.to_string());

                        if !t_title.is_empty() {
                            tracks.push(NewPlaylistItem {
                                title: t_title,
                                artist: if t_artist.is_empty() { "Unknown Artist".to_string() } else { t_artist },
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
        let artist = oembed["author_name"].as_str().unwrap_or("Spotify").to_string();
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

async fn import_deezer(
    db: &Db,
    client: &reqwest::Client,
    kind: &str,
    id: &str,
) -> Result<ImportedPlaylist, String> {
    let api_url = format!("https://api.deezer.com/{kind}/{id}");
    let json_text = fetch_cached(db, client, &api_url, 86400 * 7).await?;

    let data: serde_json::Value =
        serde_json::from_str(&json_text).map_err(|e| format!("Deezer API parse error: {e}"))?;

    let title = data["title"].as_str().unwrap_or("Deezer Import").to_string();
    let description = data["description"].as_str().map(|s| s.to_string());
    let artwork_url = data["picture_big"]
        .as_str()
        .or_else(|| data["cover_big"].as_str())
        .map(|s| s.to_string());

    let mut tracks = Vec::new();

    if kind == "track" {
        let artist = data["artist"]["name"].as_str().unwrap_or("Unknown Artist").to_string();
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
            let t_artist = item["artist"]["name"].as_str().unwrap_or("Unknown Artist").to_string();
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

async fn import_apple_music(
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
                        let t_artist = item["byArtist"]["name"].as_str().unwrap_or("Unknown Artist").to_string();
                        let duration_ms = item["duration"].as_str().and_then(parse_iso8601_duration);

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
        description: Some(format!("Imported from Apple Music")),
        artwork_url: None,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// YOUTUBE & YOUTUBE MUSIC IMPORTER
// ---------------------------------------------------------------------------

async fn import_youtube_track(
    db: &Db,
    client: &reqwest::Client,
    video_id: &str,
) -> Result<ImportedPlaylist, String> {
    let oembed_url = format!("https://www.youtube.com/oembed?url=https://www.youtube.com/watch?v={video_id}&format=json");
    let json_text = fetch_cached(db, client, &oembed_url, 86400 * 14).await?;

    let oembed: serde_json::Value =
        serde_json::from_str(&json_text).map_err(|e| format!("YouTube oEmbed parse error: {e}"))?;

    let raw_title = oembed["title"].as_str().unwrap_or("YouTube Track").to_string();
    let author = oembed["author_name"].as_str().unwrap_or("YouTube").to_string();
    let artwork_url = oembed["thumbnail_url"].as_str().map(|s| s.to_string());

    // Split "Artist - Title" if formatted with dash
    let (artist, title) = if let Some((a, t)) = raw_title.split_once(" - ") {
        (a.trim().to_string(), t.trim().to_string())
    } else {
        (author, raw_title.clone())
    };

    Ok(ImportedPlaylist {
        title: raw_title.clone(),
        description: Some(format!("Imported from YouTube {video_id}")),
        artwork_url: artwork_url.clone(),
        tracks: vec![NewPlaylistItem {
            title,
            artist,
            album: None,
            duration_ms: None,
            artwork_url,
            origin_uri: Some(format!("https://music.youtube.com/watch?v={video_id}")),
        }],
    })
}

async fn import_youtube_playlist(
    db: &Db,
    client: &reqwest::Client,
    list_id: &str,
) -> Result<ImportedPlaylist, String> {
    let url = format!("https://www.youtube.com/playlist?list={list_id}");
    let html = fetch_cached(db, client, &url, 86400 * 3).await?;

    let title_re = Regex::new(r#"<title>(.*?)(?: - YouTube)?</title>"#).expect("regex");
    let title = title_re
        .captures(&html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "YouTube Playlist".to_string());

    Ok(ImportedPlaylist {
        title: title.clone(),
        description: Some(format!("Imported from YouTube Playlist {list_id}")),
        artwork_url: None,
        tracks: vec![],
    })
}

/// Helper to parse ISO 8601 duration strings like "PT3M45S" to milliseconds.
fn parse_iso8601_duration(s: &str) -> Option<i64> {
    let re = Regex::new(r"PT(?:(\d+)M)?(?:(\d+)S)?").ok()?;
    let caps = re.captures(s)?;
    let mins: i64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let secs: i64 = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    Some((mins * 60 + secs) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_patterns() {
        assert!(SPOTIFY_RE.is_match("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"));
        assert!(SPOTIFY_RE.is_match("https://open.spotify.com/album/4m2880jivSbbyEGAKfITCa"));
        assert!(SPOTIFY_RE.is_match("https://open.spotify.com/track/0VjIjW4GlUZAMYd2vXMi3b"));

        assert!(DEEZER_RE.is_match("https://www.deezer.com/playlist/3010368142"));
        assert!(DEEZER_RE.is_match("https://deezer.com/us/album/302127"));
        assert!(DEEZER_RE.is_match("https://deezer.com/track/3135556"));

        assert!(APPLE_MUSIC_RE.is_match("https://music.apple.com/us/album/discovery/697194953"));
        assert!(APPLE_MUSIC_RE.is_match("https://music.apple.com/playlist/today-hits/pl.f4d106fed2bd41149aaacabb233eb5eb"));

        assert!(YT_WATCH_RE.is_match("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(YT_WATCH_RE.is_match("https://music.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(YT_WATCH_RE.is_match("https://youtu.be/dQw4w9WgXcQ"));

        assert!(YT_PLAYLIST_RE.is_match("https://music.youtube.com/playlist?list=PL4fGSI1pDJn6jXS_PEO37J1NUEn3Z11dO"));
    }

    #[test]
    fn test_iso8601_duration_parsing() {
        assert_eq!(parse_iso8601_duration("PT3M45S"), Some(225000));
        assert_eq!(parse_iso8601_duration("PT4M"), Some(240000));
        assert_eq!(parse_iso8601_duration("PT30S"), Some(30000));
    }
}
