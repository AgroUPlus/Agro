//! One album of an account's library, track by track — what the dashboard's album page lists.
//!
//! Scoped exactly as `library_browse` is: tracks a device of this account holds, plus the server
//! archive when `include_archive` says it counts. An album id is the one `library_browse` hands
//! out, `album artist ␁ album`.

use rusqlite::{named_params, Result};

use crate::db::Db;
use crate::db_library::album_key;

#[derive(Debug, Clone)]
pub struct AlbumTrack {
    pub content_hash: String,
    pub title: String,
    pub artist: String,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub duration_ms: i64,
}

#[derive(Debug, Clone)]
pub struct LibraryAlbum {
    pub title: String,
    pub album_artist: String,
    pub cover_key: Option<String>,
    pub year: Option<i64>,
    pub tracks: Vec<AlbumTrack>,
}

impl Db {
    /// The album `album_id` names in `user_id`'s library, or `None` when it holds none of it.
    pub fn library_album(
        &self,
        user_id: &str,
        album_id: &str,
        include_archive: bool,
    ) -> Result<Option<LibraryAlbum>> {
        let Some((album_artist, album)) = album_id.split_once('\u{1}') else {
            return Ok(None);
        };
        let archive_clause = if include_archive {
            "OR t.archived_path IS NOT NULL"
        } else {
            ""
        };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT t.content_hash, t.title, t.artist, t.track_no, t.disc_no, t.duration_ms, t.year
               FROM library_tracks t
              WHERE (EXISTS (SELECT 1 FROM device_holdings h
                             WHERE h.content_hash = t.content_hash AND h.user_id = :user)
                     {archive_clause})
                AND COALESCE(t.album_artist, t.artist) = :artist
                AND COALESCE(t.album, '') = :album
              ORDER BY COALESCE(t.disc_no, 1), t.track_no, t.title"
        ))?;
        let mut year = None;
        let rows = stmt.query_map(
            named_params! { ":user": user_id, ":artist": album_artist, ":album": album },
            |row| {
                Ok((
                    AlbumTrack {
                        content_hash: row.get(0)?,
                        title: row.get(1)?,
                        artist: row.get(2)?,
                        track_no: row.get(3)?,
                        disc_no: row.get(4)?,
                        duration_ms: row.get(5)?,
                    },
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        )?;
        let mut tracks = Vec::new();
        for row in rows {
            let (track, track_year) = row?;
            year = year.or(track_year);
            tracks.push(track);
        }
        if tracks.is_empty() {
            return Ok(None);
        }
        Ok(Some(LibraryAlbum {
            title: if album.is_empty() {
                "Unknown Album".to_string()
            } else {
                album.to_string()
            },
            cover_key: (!album.is_empty()).then(|| album_key(album_artist, album)),
            album_artist: album_artist.to_string(),
            year,
            tracks,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_library::LibraryTrack;

    fn track(hash: &str, title: &str, album: &str, track_no: i64) -> LibraryTrack {
        LibraryTrack {
            content_hash: hash.to_string(),
            title: title.to_string(),
            artist: "Artist".to_string(),
            album: Some(album.to_string()),
            album_artist: None,
            track_no: Some(track_no),
            disc_no: None,
            year: Some(2020),
            genre: None,
            duration_ms: 1000,
            size_bytes: 1,
            format: None,
            bitrate_kbps: None,
            archived_path: Some(format!("/archive/{hash}")),
        }
    }

    #[test]
    fn lists_one_album_in_track_order_and_nothing_else() {
        let db = Db::new_in_memory().unwrap();
        db.upsert_library_track(&track("b", "Second", "Record", 2))
            .unwrap();
        db.upsert_library_track(&track("a", "First", "Record", 1))
            .unwrap();
        db.upsert_library_track(&track("c", "Elsewhere", "Other", 1))
            .unwrap();

        let album = db
            .library_album("alpha", "Artist\u{1}Record", true)
            .unwrap()
            .unwrap();
        let titles: Vec<_> = album.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["First", "Second"]);
        assert_eq!(album.year, Some(2020));
        assert_eq!(album.cover_key, Some(album_key("Artist", "Record")));
    }

    #[test]
    fn the_archive_counts_only_when_it_is_part_of_the_library() {
        let db = Db::new_in_memory().unwrap();
        db.upsert_library_track(&track("a", "First", "Record", 1))
            .unwrap();

        assert!(db
            .library_album("alpha", "Artist\u{1}Record", false)
            .unwrap()
            .is_none());
        assert!(db
            .library_album("alpha", "not an id", true)
            .unwrap()
            .is_none());
    }
}
