//! Cover art lookup for the public popularity chart, via the iTunes Search API.
//!
//! No credentials, no account linkage: the only input is a track's title and artist, both already
//! public on `/api/v1/popular` before this call happens — unlike a `ytm:` source id, which is a
//! per-account-published identifier that stays behind the authenticated catalogue (see
//! `crate::popular`'s module doc). Results are cached for a day in the same `proxy_cache` table
//! the privacy-relay proxy uses (`crate::proxy`), keyed by the search URL, so a popular track's
//! artwork is fetched from Apple once per day rather than once per chart request — and the lookup
//! happens server-side rather than from the visitor's own browser, for the same reason the relay
//! proxy exists: a visitor's IP never reaches the third party.

use crate::db::Db;

const CACHE_TTL_SECS: i64 = 24 * 60 * 60;

/// Looks up cover art for a recording. Returns `None` on any failure or if nothing was found — a
/// missing cover is never worth failing the whole chart over, and a miss is cached too, so a track
/// with no iTunes match does not repeat the lookup on every request.
pub(crate) async fn find_cover(
    http_client: &reqwest::Client,
    db: &Db,
    artist: &str,
    title: &str,
) -> Option<String> {
    let query = format!("{artist} {title}");
    let url = format!(
        "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=1",
        urlencoding::encode(&query)
    );

    if let Ok(Some((_headers, cached))) = db.get_cached_proxy(&url) {
        return if cached.is_empty() {
            None
        } else {
            String::from_utf8(cached).ok()
        };
    }

    let artwork = fetch_artwork(http_client, &url).await;
    let expires_at = chrono::Utc::now().timestamp() + CACHE_TTL_SECS;
    let _ = db.set_cached_proxy(
        &url,
        "{}",
        artwork.as_deref().unwrap_or("").as_bytes(),
        expires_at,
    );
    artwork
}

async fn fetch_artwork(http_client: &reqwest::Client, url: &str) -> Option<String> {
    let body: serde_json::Value = http_client.get(url).send().await.ok()?.json().await.ok()?;
    extract_artwork(&body)
}

/// Pulled out of [`fetch_artwork`] so the JSON shape can be tested without a network call.
fn extract_artwork(body: &serde_json::Value) -> Option<String> {
    body["results"]
        .get(0)?
        .get("artworkUrl100")?
        .as_str()
        // iTunes serves a 100x100 thumbnail by default; every size up to the original is
        // available at the same path with the dimensions swapped in.
        .map(|s| s.replace("100x100", "600x600"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_is_upscaled_from_the_100px_thumbnail() {
        let body = serde_json::json!({
            "results": [{"artworkUrl100": "https://example.com/art/100x100bb.jpg"}]
        });
        assert_eq!(
            extract_artwork(&body).as_deref(),
            Some("https://example.com/art/600x600bb.jpg")
        );
    }

    #[test]
    fn no_results_means_no_cover() {
        assert_eq!(extract_artwork(&serde_json::json!({"results": []})), None);
    }

    #[tokio::test]
    async fn a_cached_hit_is_returned_without_a_network_call() {
        let db = Db::new_in_memory().unwrap();
        let query = "Radiohead All I Need";
        let url = format!(
            "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=1",
            urlencoding::encode(query)
        );
        db.set_cached_proxy(&url, "{}", b"https://example.com/cover.jpg", 9_999_999_999)
            .unwrap();

        let client = reqwest::Client::new();
        let cover = find_cover(&client, &db, "Radiohead", "All I Need").await;
        assert_eq!(cover.as_deref(), Some("https://example.com/cover.jpg"));
    }

    #[tokio::test]
    async fn a_cached_miss_stays_a_miss_without_a_network_call() {
        let db = Db::new_in_memory().unwrap();
        let query = "Nobody Nothing";
        let url = format!(
            "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=1",
            urlencoding::encode(query)
        );
        db.set_cached_proxy(&url, "{}", b"", 9_999_999_999).unwrap();

        let client = reqwest::Client::new();
        let cover = find_cover(&client, &db, "Nobody", "Nothing").await;
        assert_eq!(cover, None);
    }
}
