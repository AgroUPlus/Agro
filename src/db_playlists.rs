//! Source-agnostic playlists stored in Agro.
//!
//! A playlist is an abstract sequence of tracks identified by normalised metadata (`norm_artist`,
//! `norm_title`, `duration_ms`) rather than a hardcoded backend reference. Clients resolve each
//! track against their own local storage, Navidrome instance, or streaming fallbacks.
//!
//! Who can open one is `playlist_visibility`; who can change it is `playlist_access`. Its items
//! are `db_playlist_items`, edits to them `db_playlist_edits`, and who keeps a live copy
//! `db_playlist_followers`.

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;
use crate::playlist_access::EditAccess;
use crate::playlist_visibility::PlaylistVisibility;

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: bool,
    /// Open to the owner's accepted friends. Meaningless once `is_public` is set.
    pub friends_only: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Counts every change, so an edit can name the version it was made against.
    pub revision: i64,
    pub edit_access: EditAccess,
}

/// The one column order every playlist query selects, read back by [`playlist_from_row`].
pub(crate) const PLAYLIST_COLUMNS: &str =
    "id, user_id, title, description, is_public, friends_only, created_at, updated_at, revision, edit_access";

pub(crate) fn playlist_from_row(row: &rusqlite::Row<'_>) -> Result<Playlist> {
    Ok(Playlist {
        id: row.get(0)?,
        user_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        is_public: row.get::<_, i32>(4)? != 0,
        friends_only: row.get::<_, i32>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        revision: row.get(8)?,
        edit_access: EditAccess::from_stored(row.get(9)?),
    })
}

/// Marks a playlist as changed: one more revision, and a fresh `updated_at`. Every write to a
/// playlist or its items goes through here, inside the same transaction as the write itself.
pub(crate) fn bump_revision(conn: &rusqlite::Connection, playlist_id: &str) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.query_row(
        "UPDATE playlists SET revision = revision + 1, updated_at = ?1 WHERE id = ?2 RETURNING revision",
        params![now, playlist_id],
        |row| row.get(0),
    )
}

impl Db {
    /// Creates a new playlist for the given user. Nobody else may edit it until its owner says so.
    pub fn create_playlist(
        &self,
        user_id: &str,
        title: &str,
        description: Option<&str>,
        visibility: PlaylistVisibility,
    ) -> Result<Playlist> {
        let conn = self.conn.lock().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let (is_public, friends_only) = visibility.flags();

        conn.execute(
            "INSERT INTO playlists (id, user_id, title, description, is_public, friends_only, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, user_id, title, description, is_public as i32, friends_only as i32, now, now],
        )?;

        Ok(Playlist {
            id,
            user_id: user_id.to_string(),
            title: title.to_string(),
            description: description.map(|s| s.to_string()),
            is_public,
            friends_only,
            created_at: now.clone(),
            updated_at: now,
            revision: 0,
            edit_access: EditAccess::Off,
        })
    }

    /// Fetches a playlist by ID.
    pub fn get_playlist(&self, id: &str) -> Result<Option<Playlist>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {PLAYLIST_COLUMNS} FROM playlists WHERE id = ?1"),
            params![id],
            playlist_from_row,
        )
        .optional()
    }

    /// Lists playlists owned by the user.
    pub fn list_user_playlists(&self, user_id: &str) -> Result<Vec<Playlist>> {
        self.select_playlists(
            "WHERE user_id = ?1 ORDER BY updated_at DESC",
            params![user_id],
        )
    }

    /// Lists all public playlists across all users on the server.
    pub fn list_public_playlists(&self) -> Result<Vec<Playlist>> {
        self.select_playlists("WHERE is_public = 1 ORDER BY updated_at DESC", [])
    }

    /// The playlists `owner` has shared with friends only. Not those that are public or private.
    pub fn list_friends_only_playlists(&self, owner: &str) -> Result<Vec<Playlist>> {
        self.select_playlists(
            "WHERE user_id = ?1 AND friends_only = 1 AND is_public = 0 ORDER BY updated_at DESC",
            params![owner],
        )
    }

    pub(crate) fn select_playlists(
        &self,
        clause: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Playlist>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PLAYLIST_COLUMNS} FROM playlists {clause}"
        ))?;
        let rows = stmt.query_map(params, playlist_from_row)?;
        rows.collect()
    }

    /// Sets who can open a playlist. Only its owner can; anyone else changes nothing and gets `false`.
    ///
    /// Narrowing who can open it narrows who can edit it in the same write, so no one is ever left
    /// able to change a playlist they can no longer see — see [`EditAccess::clamped_to`].
    pub fn update_playlist_visibility(
        &self,
        playlist_id: &str,
        user_id: &str,
        visibility: PlaylistVisibility,
    ) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let current: Option<i32> = tx
            .query_row(
                "SELECT edit_access FROM playlists WHERE id = ?1 AND user_id = ?2",
                params![playlist_id, user_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Ok(false);
        };
        let edit_access = EditAccess::from_stored(current).clamped_to(visibility);
        let (is_public, friends_only) = visibility.flags();
        tx.execute(
            "UPDATE playlists SET is_public = ?1, friends_only = ?2, edit_access = ?3 WHERE id = ?4",
            params![
                is_public as i32,
                friends_only as i32,
                edit_access.stored(),
                playlist_id
            ],
        )?;
        bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(true)
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
