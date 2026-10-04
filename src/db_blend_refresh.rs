//! Writing a blend's tracks: reading each consenting member's listening and choosing from it.
//!
//! Lazy on purpose: a blend nobody opens is never computed, and there is no timer to run. The read
//! that finds one due pays for one aggregate query per member, over the scrobbles index.

use rusqlite::{params, Result};

use crate::blend::{generate, MemberTaste, Recipe, TasteTrack};
use crate::blend_recipe::{DAY, TASTE_DEPTH};
use crate::db::Db;
use crate::db_playlist_items::{insert_item, NewPlaylistItem};
use crate::db_playlists::bump_revision;

impl Db {
    /// Writes the blend's tracks again if it is due, returning the new revision when it was.
    pub fn refresh_blend_if_due(&self, playlist_id: &str) -> Result<Option<i64>> {
        let Some(blend) = self.blend(playlist_id)? else {
            return Ok(None);
        };
        let now = chrono::Utc::now();
        if !blend.is_due(now.timestamp()) {
            return Ok(None);
        }
        // Not written until everyone asked has answered. A blend written from whoever had joined so
        // far would be rewritten as each of the rest arrived, and the playlist people had started
        // listening to would change under them. It stays due, so the last answer writes it.
        let members = self.blend_members(playlist_id)?;
        if members.iter().any(|m| !m.joined) {
            return Ok(None);
        }
        let since = blend
            .window
            .days()
            .map(|d| (now - chrono::Duration::days(d)).to_rfc3339());

        let mut tastes = Vec::new();
        for member in members {
            let consents = self
                .profile(&member.username)?
                .is_some_and(|p| p.shows_stats());
            if consents {
                let tracks = self.taste(&member.username, since.as_deref())?;
                tastes.push(MemberTaste {
                    username: member.username,
                    tracks,
                });
            }
        }
        let seed = seed_for(playlist_id, now.timestamp() / DAY);
        let picks = generate(
            &tastes,
            Recipe {
                size: blend.size as usize,
                mix: blend.mix as u8,
                seed,
            },
        );

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
        )?;
        for (position, pick) in picks.iter().enumerate() {
            let item = NewPlaylistItem {
                title: pick.track.title.clone(),
                artist: pick.track.artist.clone(),
                album: pick.track.album.clone(),
                duration_ms: pick.track.duration_ms,
                artwork_url: None,
                origin_uri: None,
            };
            insert_item(&tx, playlist_id, position as i32, &pick.from[0], &item)?;
        }
        tx.execute(
            "UPDATE blends SET refreshed_at = ?2 WHERE playlist_id = ?1",
            params![playlist_id, now.to_rfc3339()],
        )?;
        let revision = bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(Some(revision))
    }

    /// One account's most-played tracks since `since`, most-played first.
    fn taste(&self, username: &str, since: Option<&str>) -> Result<Vec<TasteTrack>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT track_title, artist_name, MAX(album_name), MAX(duration_secs), COUNT(*) AS plays
               FROM scrobbles
              WHERE user_id = ?1 AND (?2 IS NULL OR played_at >= ?2)
              GROUP BY artist_name, track_title
              ORDER BY plays DESC, MAX(played_at) DESC
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![username, since, TASTE_DEPTH], |row| {
            Ok(TasteTrack {
                title: row.get(0)?,
                artist: row.get(1)?,
                album: row.get(2)?,
                duration_ms: row
                    .get::<_, Option<i64>>(3)?
                    .filter(|s| *s > 0)
                    .map(|s| s * 1000),
                plays: row.get(4)?,
            })
        })?;
        rows.collect()
    }
}

/// A seed that changes once a day per blend, so a refresh reorders and a re-read does not.
fn seed_for(playlist_id: &str, day: i64) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{playlist_id}:{day}").as_bytes());
    u64::from_le_bytes(
        digest[..8]
            .try_into()
            .expect("a SHA-256 digest is 32 bytes"),
    )
}
