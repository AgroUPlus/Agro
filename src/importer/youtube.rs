//! Importing YouTube tracks and playlists.

use super::*;
use crate::db::Db;
use crate::db_playlist_items::NewPlaylistItem;
use regex::Regex;
use std::sync::LazyLock;

pub(super) static YT_WATCH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:youtube\.com/watch\?v=|youtu\.be/|music\.youtube\.com/watch\?v=)([a-zA-Z0-9_-]{11})",
    )
    .expect("regex")
});

pub(super) static YT_PLAYLIST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:youtube\.com/playlist\?list=|music\.youtube\.com/playlist\?list=)([a-zA-Z0-9_-]+)",
    )
    .expect("regex")
});

pub(super) async fn import_youtube_track(
    db: &Db,
    client: &reqwest::Client,
    video_id: &str,
) -> Result<ImportedPlaylist, String> {
    let oembed_url = format!(
        "https://www.youtube.com/oembed?url=https://www.youtube.com/watch?v={video_id}&format=json"
    );
    let json_text = fetch_cached(db, client, &oembed_url, 86400 * 14).await?;

    let oembed: serde_json::Value =
        serde_json::from_str(&json_text).map_err(|e| format!("YouTube oEmbed parse error: {e}"))?;

    let raw_title = oembed["title"]
        .as_str()
        .unwrap_or("YouTube Track")
        .to_string();
    let author = oembed["author_name"]
        .as_str()
        .unwrap_or("YouTube")
        .to_string();
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

pub(super) async fn import_youtube_playlist(
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
pub(super) fn parse_iso8601_duration(s: &str) -> Option<i64> {
    let re = Regex::new(r"PT(?:(\d+)M)?(?:(\d+)S)?").ok()?;
    let caps = re.captures(s)?;
    let mins: i64 = caps
        .get(1)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);
    let secs: i64 = caps
        .get(2)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);
    Some((mins * 60 + secs) * 1000)
}
