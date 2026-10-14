//! SQL for the music library index.
//!
//! A second `impl Db` block rather than more of `db.rs`, which is already long and about something
//! else. Same connection, same single write lock.
//!
//! Everything here keys on **username** in its `user_id` column, matching `registered_nodes`,
//! `handoff_state` and `synced_settings`. Only `app_passwords.user_id` holds the `users.id` UUID.

use rusqlite::Result;

/// One file the server knows about, whether or not it holds the bytes.
#[derive(Debug, Clone)]
pub struct LibraryTrack {
    pub content_hash: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub format: Option<String>,
    pub bitrate_kbps: Option<i64>,
    pub archived_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PeerSourceInfo {
    pub device_id: String,
    pub petname: String,
    pub lan_address: Option<String>,
    pub last_seen_at: String,
}

/// An upload in flight.
#[derive(Debug, Clone)]
pub struct UploadSession {
    pub upload_id: String,
    pub user_id: String,
    pub device_id: String,
    pub content_hash: String,
    pub size_bytes: i64,
    pub received_bytes: i64,
    pub target: String,
    /// Declared by the client at `begin_upload`, so a restart mid-transfer does not lose it.
    pub extension: Option<String>,
}

/// What a browse request is listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseKind {
    Artist,
    Album,
    Track,
}

/// One tile in the library view.
#[derive(Debug, Clone)]
pub struct BrowseItem {
    /// Stable within its kind: a content hash for a track, `artist\x01album` for an album, the
    /// artist name for an artist.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// The key to fetch artwork with, when this item can have any. See [`album_key`].
    pub cover_key: Option<String>,
    pub track_count: i64,
    /// False when the selected device is missing this — the whole reason for the view.
    pub present_on_device: bool,
    /// Number of active registered devices that hold this item.
    pub source_count: i64,
}

/// A path-safe, stable identifier for an album's artwork.
///
/// A hash rather than the names themselves: album and artist tags contain slashes, dots, null
/// bytes and every other thing that has no business reaching a filename, and this value *is* the
/// filename under the covers directory. Case- and whitespace-folded so two spellings of the same
/// album share one cover instead of storing it twice.
pub fn album_key(album_artist: &str, album: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(album_artist.trim().to_lowercase().as_bytes());
    hasher.update([0x01]);
    hasher.update(album.trim().to_lowercase().as_bytes());
    hex::encode(hasher.finalize())[..32].to_string()
}

#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub track_count: i64,
    pub archived_count: i64,
    pub total_bytes: i64,
    pub spool_bytes: i64,
}

fn row_to_track(row: &rusqlite::Row<'_>) -> Result<LibraryTrack> {
    Ok(LibraryTrack {
        content_hash: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        album_artist: row.get(4)?,
        track_no: row.get(5)?,
        disc_no: row.get(6)?,
        year: row.get(7)?,
        genre: row.get(8)?,
        duration_ms: row.get(9)?,
        size_bytes: row.get(10)?,
        format: row.get(11)?,
        bitrate_kbps: row.get(12)?,
        archived_path: row.get(13)?,
    })
}

/// What one pass of [`Db::reindex_normalisation`] rewrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReindexOutcome {
    /// `library_tracks` rows whose `norm_*` columns were stale and have been rewritten.
    pub tracks: usize,
    /// `playlist_items` rows likewise.
    pub playlist_items: usize,
    /// Whether every stale row is now fixed. False means call again for the next batch.
    pub done: bool,
}

fn row_to_upload(row: &rusqlite::Row<'_>) -> Result<UploadSession> {
    Ok(UploadSession {
        upload_id: row.get(0)?,
        user_id: row.get(1)?,
        device_id: row.get(2)?,
        content_hash: row.get(3)?,
        size_bytes: row.get(4)?,
        received_bytes: row.get(5)?,
        target: row.get(6)?,
        extension: row.get(7)?,
    })
}

mod browse;
mod covers;
mod holdings;
mod reindex;
mod spool;
#[cfg(test)]
mod tests;
mod tracks;
mod uploads;
