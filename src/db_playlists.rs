//! Source-agnostic playlists stored in Agro.
//!
//! A playlist is an abstract sequence of tracks identified by normalised metadata (`norm_artist`,
//! `norm_title`, `duration_ms`) rather than a hardcoded backend reference. Clients resolve each
//! track against their own local storage, Navidrome instance, or streaming fallbacks.
//!
//! Playlists can be private (owner-only) or public (visible to other users on this Agro server).

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;
use crate::norm;

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct PlaylistItem {
    pub id: String,
    pub playlist_id: String,
    pub position: i32,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub norm_artist: String,
    pub norm_title: String,
    pub artwork_url: Option<String>,
    pub origin_uri: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NewPlaylistItem {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub artwork_url: Option<String>,
    pub origin_uri: Option<String>,
}

impl Db {
    /// Creates a new playlist for the given user.
    pub fn create_playlist(
        &self,
        user_id: &str,
        title: &str,
        description: Option<&str>,
        is_public: bool,
    ) -> Result<Playlist> {
        let conn = self.conn.lock().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO playlists (id, user_id, title, description, is_public, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, user_id, title, description, is_public as i32, now, now],
        )?;

        Ok(Playlist {
            id,
            user_id: user_id.to_string(),
            title: title.to_string(),
            description: description.map(|s| s.to_string()),
            is_public,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    /// Fetches a playlist by ID.
    pub fn get_playlist(&self, id: &str) -> Result<Option<Playlist>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, description, is_public, created_at, updated_at
             FROM playlists WHERE id = ?1",
        )?;

        stmt.query_row(params![id], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                is_public: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })
        .optional()
    }

    /// Lists playlists owned by the user.
    pub fn list_user_playlists(&self, user_id: &str) -> Result<Vec<Playlist>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, description, is_public, created_at, updated_at
             FROM playlists WHERE user_id = ?1 ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map(params![user_id], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                is_public: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Lists all public playlists across all users on the server.
    pub fn list_public_playlists(&self) -> Result<Vec<Playlist>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, description, is_public, created_at, updated_at
             FROM playlists WHERE is_public = 1 ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                is_public: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Fetches all items in a playlist ordered by their position.
    pub fn get_playlist_items(&self, playlist_id: &str) -> Result<Vec<PlaylistItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, playlist_id, position, title, artist, album, duration_ms,
                    norm_artist, norm_title, artwork_url, origin_uri
             FROM playlist_items
             WHERE playlist_id = ?1
             ORDER BY position ASC",
        )?;

        let rows = stmt.query_map(params![playlist_id], |row| {
            Ok(PlaylistItem {
                id: row.get(0)?,
                playlist_id: row.get(1)?,
                position: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                album: row.get(5)?,
                duration_ms: row.get(6)?,
                norm_artist: row.get(7)?,
                norm_title: row.get(8)?,
                artwork_url: row.get(9)?,
                origin_uri: row.get(10)?,
            })
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Adds a track to the end of a playlist.
    pub fn add_playlist_item(&self, playlist_id: &str, item: NewPlaylistItem) -> Result<PlaylistItem> {
        let conn = self.conn.lock().unwrap();
        let item_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let norm_artist = norm::normalize_artist(&item.artist);
        let norm_title = norm::normalize_title(&item.title);

        let next_pos: i32 = conn.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
            |r| r.get(0),
        )?;

        conn.execute(
            "INSERT INTO playlist_items (
                id, playlist_id, position, title, artist, album, duration_ms,
                norm_artist, norm_title, artwork_url, origin_uri
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                item_id,
                playlist_id,
                next_pos,
                item.title,
                item.artist,
                item.album,
                item.duration_ms,
                norm_artist,
                norm_title,
                item.artwork_url,
                item.origin_uri,
            ],
        )?;

        conn.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now, playlist_id],
        )?;

        Ok(PlaylistItem {
            id: item_id,
            playlist_id: playlist_id.to_string(),
            position: next_pos,
            title: item.title,
            artist: item.artist,
            album: item.album,
            duration_ms: item.duration_ms,
            norm_artist,
            norm_title,
            artwork_url: item.artwork_url,
            origin_uri: item.origin_uri,
        })
    }

    /// Batch inserts tracks into a playlist (useful for importers).
    pub fn add_playlist_items(&self, playlist_id: &str, items: &[NewPlaylistItem]) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().to_rfc3339();

        let mut next_pos: i32 = tx.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
            params![playlist_id],
            |r| r.get(0),
        )?;

        let mut inserted = 0;
        for item in items {
            let item_id = uuid::Uuid::new_v4().to_string();
            let norm_artist = norm::normalize_artist(&item.artist);
            let norm_title = norm::normalize_title(&item.title);

            tx.execute(
                "INSERT INTO playlist_items (
                    id, playlist_id, position, title, artist, album, duration_ms,
                    norm_artist, norm_title, artwork_url, origin_uri
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    item_id,
                    playlist_id,
                    next_pos,
                    item.title,
                    item.artist,
                    item.album,
                    item.duration_ms,
                    norm_artist,
                    norm_title,
                    item.artwork_url,
                    item.origin_uri,
                ],
            )?;

            next_pos += 1;
            inserted += 1;
        }

        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now, playlist_id],
        )?;

        tx.commit()?;
        Ok(inserted)
    }

    /// Removes a specific item from a playlist and compacts the position indices.
    pub fn remove_playlist_item(&self, playlist_id: &str, item_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().to_rfc3339();

        let count = tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1 AND id = ?2",
            params![playlist_id, item_id],
        )?;

        if count > 0 {
            // Recompact positions
            let mut stmt = tx.prepare(
                "SELECT id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position ASC",
            )?;
            let item_ids: Vec<String> = stmt
                .query_map(params![playlist_id], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;

            for (pos, id) in item_ids.iter().enumerate() {
                tx.execute(
                    "UPDATE playlist_items SET position = ?1 WHERE id = ?2",
                    params![pos as i32, id],
                )?;
            }

            tx.execute(
                "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
                params![now, playlist_id],
            )?;
        }

        tx.commit()?;
        Ok(count > 0)
    }

    /// Updates the public/private visibility of a playlist.
    pub fn update_playlist_visibility(&self, playlist_id: &str, user_id: &str, is_public: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let count = conn.execute(
            "UPDATE playlists SET is_public = ?1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4",
            params![is_public as i32, now, playlist_id, user_id],
        )?;
        Ok(count > 0)
    }

    /// Deletes a playlist and its items (cascaded).
    pub fn delete_playlist(&self, playlist_id: &str, user_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "DELETE FROM playlists WHERE id = ?1 AND user_id = ?2",
            params![playlist_id, user_id],
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playlist_lifecycle_and_items() {
        let db = Db::new_in_memory().unwrap();

        let pl = db
            .create_playlist("alpha", "Road Trip", Some("Summer bops"), false)
            .unwrap();
        assert_eq!(pl.title, "Road Trip");
        assert!(!pl.is_public);

        let item1 = db
            .add_playlist_item(
                &pl.id,
                NewPlaylistItem {
                    title: "Get Lucky".to_string(),
                    artist: "Daft Punk".to_string(),
                    album: Some("Random Access Memories".to_string()),
                    duration_ms: Some(248000),
                    artwork_url: None,
                    origin_uri: Some("spotify:track:123".to_string()),
                },
            )
            .unwrap();
        assert_eq!(item1.position, 0);
        assert_eq!(item1.norm_artist, "daft punk");

        let item2 = db
            .add_playlist_item(
                &pl.id,
                NewPlaylistItem {
                    title: "Instant Crush".to_string(),
                    artist: "Daft Punk ft. Julian Casablancas".to_string(),
                    album: Some("Random Access Memories".to_string()),
                    duration_ms: Some(337000),
                    artwork_url: None,
                    origin_uri: None,
                },
            )
            .unwrap();
        assert_eq!(item2.position, 1);

        let items = db.get_playlist_items(&pl.id).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Get Lucky");
        assert_eq!(items[1].title, "Instant Crush");

        // Remove item 1 (index 0) and verify position recompacting
        assert!(db.remove_playlist_item(&pl.id, &item1.id).unwrap());
        let items_after = db.get_playlist_items(&pl.id).unwrap();
        assert_eq!(items_after.len(), 1);
        assert_eq!(items_after[0].id, item2.id);
        assert_eq!(items_after[0].position, 0);

        // Visibility toggle
        assert!(db.update_playlist_visibility(&pl.id, "alpha", true).unwrap());
        let public_lists = db.list_public_playlists().unwrap();
        assert_eq!(public_lists.len(), 1);
        assert_eq!(public_lists[0].id, pl.id);

        // Delete playlist
        assert!(db.delete_playlist(&pl.id, "alpha").unwrap());
        assert!(db.get_playlist(&pl.id).unwrap().is_none());
        assert!(db.get_playlist_items(&pl.id).unwrap().is_empty());
    }
}
