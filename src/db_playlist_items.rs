//! The tracks in a playlist: reading them, and the owner-only writes that predate collaborative
//! editing. Edits by anyone else go through `db_playlist_edits`, which checks the revision first.

use rusqlite::{params, Connection, Result};

use crate::db::Db;
use crate::db_playlists::bump_revision;
use crate::norm;

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
    /// The account that added it. Absent only on a row written before anyone else could.
    pub added_by: Option<String>,
    pub added_at: Option<String>,
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

/// Inserts one item at `position` without touching any other row; the caller renumbers.
pub(crate) fn insert_item(
    conn: &Connection,
    playlist_id: &str,
    position: i32,
    actor: &str,
    item: &NewPlaylistItem,
) -> Result<PlaylistItem> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let norm_artist = norm::normalize_artist(&item.artist);
    let norm_title = norm::normalize_title(&item.title);

    conn.execute(
        "INSERT INTO playlist_items (
            id, playlist_id, position, title, artist, album, duration_ms,
            norm_artist, norm_title, artwork_url, origin_uri, added_by, added_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            id,
            playlist_id,
            position,
            item.title,
            item.artist,
            item.album,
            item.duration_ms,
            norm_artist,
            norm_title,
            item.artwork_url,
            item.origin_uri,
            actor,
            now,
        ],
    )?;

    Ok(PlaylistItem {
        id,
        playlist_id: playlist_id.to_string(),
        position,
        title: item.title.clone(),
        artist: item.artist.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms,
        norm_artist,
        norm_title,
        artwork_url: item.artwork_url.clone(),
        origin_uri: item.origin_uri.clone(),
        added_by: Some(actor.to_string()),
        added_at: Some(now),
    })
}

/// The item ids of a playlist, in order.
pub(crate) fn item_ids(conn: &Connection, playlist_id: &str) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position ASC")?;
    let rows = stmt.query_map(params![playlist_id], |r| r.get(0))?;
    rows.collect()
}

/// Writes `ordered` back as positions 0, 1, 2… — the one way positions are ever assigned after an
/// edit, so they never have gaps or duplicates.
pub(crate) fn renumber(conn: &Connection, ordered: &[String]) -> Result<()> {
    for (pos, id) in ordered.iter().enumerate() {
        conn.execute(
            "UPDATE playlist_items SET position = ?1 WHERE id = ?2",
            params![pos as i32, id],
        )?;
    }
    Ok(())
}

fn next_position(conn: &Connection, playlist_id: &str) -> Result<i32> {
    conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
        params![playlist_id],
        |r| r.get(0),
    )
}

impl Db {
    /// Fetches all items in a playlist ordered by their position.
    pub fn get_playlist_items(&self, playlist_id: &str) -> Result<Vec<PlaylistItem>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT id, playlist_id, position, title, artist, album, duration_ms,
                    norm_artist, norm_title, artwork_url, origin_uri, added_by, added_at
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
                added_by: row.get(11)?,
                added_at: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Adds a track to the end of a playlist, as `actor`.
    pub fn add_playlist_item(
        &self,
        playlist_id: &str,
        actor: &str,
        item: NewPlaylistItem,
    ) -> Result<PlaylistItem> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let position = next_position(&tx, playlist_id)?;
        let added = insert_item(&tx, playlist_id, position, actor, &item)?;
        bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(added)
    }

    /// Batch inserts tracks at the end of a playlist, as `actor` (useful for importers).
    pub fn add_playlist_items(
        &self,
        playlist_id: &str,
        actor: &str,
        items: &[NewPlaylistItem],
    ) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let start = next_position(&tx, playlist_id)?;
        for (i, item) in items.iter().enumerate() {
            insert_item(&tx, playlist_id, start + i as i32, actor, item)?;
        }
        bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(items.len())
    }

    /// Removes a specific item from a playlist and compacts the position indices.
    pub fn remove_playlist_item(&self, playlist_id: &str, item_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let count = tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1 AND id = ?2",
            params![playlist_id, item_id],
        )?;
        if count > 0 {
            renumber(&tx, &item_ids(&tx, playlist_id)?)?;
            bump_revision(&tx, playlist_id)?;
        }
        tx.commit()?;
        Ok(count > 0)
    }
}
