//! Lyrics lookups, proxied to LRCLIB.

use async_graphql::{Context, Object, SimpleObject};

use super::{bounded, caller, MAX_TAG_LEN};

#[derive(SimpleObject, Clone)]
pub struct LyricsAndCoverPayload {
    pub synced_lrc: String,
    pub cover_art_url: String,
    pub is_synced: bool,
}

#[derive(Default)]
pub struct LyricsMutation;

#[Object]
impl LyricsMutation {
    /// Looks a track's lyrics up at LRCLIB.
    ///
    /// Authenticated, and the inputs are bounded. This took no token at all and made an outbound
    /// HTTP request per call with strings the caller chose — an unauthenticated amplification
    /// primitive pointed at someone else's server.
    async fn fetch_lyrics_and_cover(
        &self,
        ctx: &Context<'_>,
        artist: String,
        title: String,
    ) -> async_graphql::Result<LyricsAndCoverPayload> {
        caller(ctx)?;
        let artist = bounded(&artist, MAX_TAG_LEN, "artist")?;
        let title = bounded(&title, MAX_TAG_LEN, "title")?;
        let client = reqwest::Client::new();
        let url = format!(
            "https://lrclib.net/api/get?artist_name={}&track_name={}",
            urlencoding::encode(&artist),
            urlencoding::encode(&title)
        );

        let synced_lrc = if let Ok(resp) = client.get(&url).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json["syncedLyrics"]
                    .as_str()
                    .unwrap_or("[00:00.00] Synchronized lyrics not found")
                    .to_string()
            } else {
                "[00:00.00] Synchronized lyrics unavailable".to_string()
            }
        } else {
            "[00:00.00] LRCLIB service unreachable".to_string()
        };

        Ok(LyricsAndCoverPayload {
            synced_lrc,
            cover_art_url: "https://images.unsplash.com/photo-1514525253161-7a46d19cd819?w=500"
                .to_string(),
            is_synced: true,
        })
    }
}
