//! SQL for the music library index.
//!
//! A second `impl Db` block rather than more of `db.rs`, which is already long and about something
//! else. Same connection, same single write lock.
//!
//! Everything here keys on **username** in its `user_id` column, matching `registered_nodes`,
//! `handoff_state` and `synced_settings`. Only `app_passwords.user_id` holds the `users.id` UUID.

use crate::db::Db;
use crate::norm::{recording_key, DURATION_TOLERANCE_MS};
use rusqlite::{params, OptionalExtension, Result};

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

impl Db {
    // ── Index ───────────────────────────────────────────────────────────────────────────────

    pub fn library_track(&self, content_hash: &str) -> Result<Option<LibraryTrack>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT content_hash, title, artist, album, album_artist, track_no, disc_no, year,
                    genre, duration_ms, size_bytes, format, bitrate_kbps, archived_path
             FROM library_tracks WHERE content_hash = ?1",
            params![content_hash],
            row_to_track,
        )
        .optional()
    }

    /// Inserts or refreshes an index entry.
    ///
    /// `norm_artist`/`norm_title` are computed here, from the metadata as given, so the whole
    /// index shares one convention no matter which client or which version reported it.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_library_track(&self, track: &LibraryTrack) -> Result<()> {
        let key = recording_key(&track.artist, &track.title);
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO library_tracks (
                 content_hash, title, artist, album, album_artist, track_no, disc_no, year, genre,
                 duration_ms, size_bytes, format, bitrate_kbps, norm_artist, norm_title,
                 norm_variants, archived_path, first_seen_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?18)
             ON CONFLICT(content_hash) DO UPDATE SET
                 title=excluded.title, artist=excluded.artist, album=excluded.album,
                 album_artist=excluded.album_artist, track_no=excluded.track_no,
                 disc_no=excluded.disc_no, year=excluded.year, genre=excluded.genre,
                 duration_ms=excluded.duration_ms, size_bytes=excluded.size_bytes,
                 format=excluded.format, bitrate_kbps=excluded.bitrate_kbps,
                 norm_artist=excluded.norm_artist, norm_title=excluded.norm_title,
                 norm_variants=excluded.norm_variants,
                 -- An existing archive location is never cleared by a later report from a device
                 -- that only holds its own copy.
                 archived_path=COALESCE(excluded.archived_path, library_tracks.archived_path),
                 updated_at=excluded.updated_at",
            params![
                track.content_hash,
                track.title,
                track.artist,
                track.album,
                track.album_artist,
                track.track_no,
                track.disc_no,
                track.year,
                track.genre,
                track.duration_ms,
                track.size_bytes,
                track.format,
                track.bitrate_kbps,
                key.artist,
                key.title,
                key.variants,
                track.archived_path,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn set_archived_path(&self, content_hash: &str, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE library_tracks SET archived_path = ?2, updated_at = ?3 WHERE content_hash = ?1",
            params![content_hash, path, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // ── Holdings ────────────────────────────────────────────────────────────────────────────

    pub fn upsert_holding(
        &self,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
        local_ref: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO device_holdings (device_id, user_id, content_hash, local_ref, reported_at)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(device_id, content_hash) DO UPDATE SET
                 local_ref = excluded.local_ref, reported_at = excluded.reported_at",
            params![
                device_id,
                user_id,
                content_hash,
                local_ref,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Forgets holdings, scoped to the account that owns them.
    ///
    /// `user_id` is not decoration: this filtered on `device_id` alone, and device ids are chosen
    /// by the client, so any account could delete another account's holding rows by naming its
    /// device. The resolver's own check is not enough on its own — it verifies the *caller*, while
    /// the device id is the part that was smuggled.
    pub fn forget_holdings(
        &self,
        user_id: &str,
        device_id: &str,
        hashes: &[String],
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut removed = 0;
        for hash in hashes {
            removed += conn.execute(
                "DELETE FROM device_holdings
                  WHERE user_id = ?1 AND device_id = ?2 AND content_hash = ?3",
                params![user_id, device_id, hash],
            )?;
        }
        Ok(removed)
    }

    /// What a device holds. Scoped by account for the same reason [`Self::forget_holdings`] is.
    pub fn device_holding_hashes(&self, user_id: &str, device_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content_hash FROM device_holdings
              WHERE user_id = ?1 AND device_id = ?2 ORDER BY content_hash",
        )?;
        let rows = stmt.query_map(params![user_id, device_id], |row| row.get(0))?;
        rows.collect()
    }

    /// How many spool bytes this account currently occupies.
    ///
    /// The one number a quota can honestly be checked against. `library_stats.total_bytes` cannot
    /// be used for it: that counts every archived track in the deployment into every account's
    /// total, so it reads the same for a guest holding nothing as for the admin.
    pub fn spool_bytes_for(&self, user_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM spool_items WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
    }

    /// Whether this account registered this device. Backs `require_own_device`.
    pub fn device_belongs_to(&self, user_id: &str, device_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM registered_nodes WHERE user_id = ?1 AND device_id = ?2",
            params![user_id, device_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Tracks another device of this account holds that [`device_id`] does not.
    ///
    /// Two filters, and both matter:
    ///
    /// 1. **Not the same file** — no `device_holdings` row for this device and that hash.
    /// 2. **Not the same recording** — nothing this device *does* hold normalises to the same
    ///    artist and title, with the same performance variants, within
    ///    [`DURATION_TOLERANCE_MS`]. Without this the user is offered a FLAC of a song they
    ///    already own at 128 kbps, over and over, because the bytes differ.
    ///
    /// The duration comparison is why this is a join rather than a `NOT IN`: it needs a tolerance,
    /// not equality.
    pub fn missing_on_device(
        &self,
        user_id: &str,
        device_id: &str,
        limit: i64,
    ) -> Result<Vec<LibraryTrack>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT t.content_hash, t.title, t.artist, t.album, t.album_artist,
                    t.track_no, t.disc_no, t.year, t.genre, t.duration_ms, t.size_bytes,
                    t.format, t.bitrate_kbps, t.archived_path
             FROM library_tracks t
             JOIN device_holdings other
               ON other.content_hash = t.content_hash
              AND other.user_id = ?1
              AND other.device_id <> ?2
             JOIN registered_nodes rn
               ON rn.user_id = other.user_id AND rn.device_id = other.device_id
             WHERE 1=1 
               AND NOT EXISTS (
                 SELECT 1 FROM device_holdings mine
                 WHERE mine.device_id = ?2 AND mine.content_hash = t.content_hash)
               AND NOT EXISTS (
                 SELECT 1 FROM device_holdings mine
                 JOIN library_tracks mt ON mt.content_hash = mine.content_hash
                 WHERE mine.device_id = ?2
                   AND mt.norm_artist   = t.norm_artist
                   AND mt.norm_title    = t.norm_title
                   AND mt.norm_variants = t.norm_variants
                   AND t.duration_ms > 0 AND mt.duration_ms > 0
                   AND ABS(mt.duration_ms - t.duration_ms) <= ?3)
             ORDER BY t.artist, t.album, t.track_no
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![user_id, device_id, DURATION_TOLERANCE_MS, limit],
            row_to_track,
        )?;
        rows.collect()
    }

    /// Finds other registered devices of this account that hold the specified content hash.
    pub fn peer_sources_for_track(
        &self,
        user_id: &str,
        content_hash: &str,
    ) -> Result<Vec<PeerSourceInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            // An inner join, and no `COALESCE` on `last_seen_at`. A holding whose device is gone
            // is not a source that happens to be offline — it is not a source at all. The old
            // left join listed it anyway and, worse, substituted `datetime('now')` for the missing
            // timestamp, so a device that no longer exists was advertised as *just seen*: the
            // caller computes `is_online` from that value. That is what offers a download which can
            // never complete.
            "SELECT h.device_id, COALESCE(NULLIF(rn.petname, ''), h.device_id), rn.last_seen_at
             FROM device_holdings h
             JOIN registered_nodes rn
               ON rn.device_id = h.device_id
              AND rn.user_id = h.user_id
             WHERE h.content_hash = ?1 AND h.user_id = ?2",
        )?;
        let rows = stmt.query_map(params![content_hash, user_id], |row| {
            Ok(PeerSourceInfo {
                device_id: row.get(0)?,
                petname: row.get(1)?,
                lan_address: None,
                last_seen_at: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Tracks this device holds that the server has already filed into the library.
    ///
    /// The candidate list for "you can free this space" — the device's copy is redundant because
    /// the server has one. Only the index is consulted here; whether the archived file is really
    /// on disk is checked by the caller, which is the half that needs the filesystem.
    ///
    /// Deliberately not filtered by user beyond the holding row: a device belongs to one account,
    /// and `archived_path` is set by this server alone.
    pub fn reclaimable_on_device(
        &self,
        user_id: &str,
        device_id: &str,
        limit: i64,
    ) -> Result<Vec<LibraryTrack>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT t.content_hash, t.title, t.artist, t.album, t.album_artist,
                    t.track_no, t.disc_no, t.year, t.genre, t.duration_ms, t.size_bytes,
                    t.format, t.bitrate_kbps, t.archived_path
             FROM library_tracks t
             JOIN device_holdings h
               ON h.content_hash = t.content_hash
              AND h.user_id = ?1
              AND h.device_id = ?2
             WHERE t.archived_path IS NOT NULL
             ORDER BY t.size_bytes DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![user_id, device_id, limit], row_to_track)?;
        rows.collect()
    }

    /// Totals for one account's library.
    ///
    /// `include_archive` is what separates "my music" from "everything on this server". With it
    /// always on — which is how this behaved — a member opening the dashboard was shown the
    /// operator's whole archive as their own fleet total.
    pub fn library_stats(&self, user_id: &str, include_archive: bool) -> Result<LibraryStats> {
        let conn = self.conn.lock().unwrap();
        let (track_count, archived_count, total_bytes) = conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN t.archived_path IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(t.size_bytes), 0)
             FROM library_tracks t
             WHERE EXISTS (SELECT 1 FROM device_holdings h
                           WHERE h.content_hash = t.content_hash AND h.user_id = ?1)
                OR (?2 AND t.archived_path IS NOT NULL)",
            params![user_id, include_archive],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let spool_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM spool_items WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        Ok(LibraryStats {
            track_count,
            archived_count,
            total_bytes,
            spool_bytes,
        })
    }

    // ── Browsing ────────────────────────────────────────────────────────────────────────────

    /// The account's library as something you can look at, one page at a time.
    ///
    /// The scope is "every file any of this account's devices has reported, plus everything the
    /// server itself holds" — the same definition [`Self::library_stats`] counts, so the totals in
    /// the header and the rows underneath cannot disagree.
    ///
    /// `present_on_device` is the point of the whole thing: it is what greys a row out. It answers
    /// *exactly* — this device reported this content hash — rather than fuzzily. The fuzzy matcher
    /// in [`crate::norm`] exists for deciding what to *offer* a device, where a different rip of
    /// the same recording counts as already having it; for "is this file here", that would report a
    /// library as complete when the files are not the ones listed.
    pub fn library_browse(
        &self,
        user_id: &str,
        device_id: Option<&str>,
        kind: BrowseKind,
        search: Option<&str>,
        limit: i64,
        offset: i64,
        include_archive: bool,
    ) -> Result<Vec<BrowseItem>> {
        let conn = self.conn.lock().unwrap();

        // The scope and the search, written once because all three kinds need the same ones and
        // three copies of them would drift.
        // `include_archive` decides whether the server's own archive counts as part of this
        // account's library. It used to be unconditional — `OR t.archived_path IS NOT NULL` —
        // which handed every account the operator's entire collection as though it were theirs.
        let archive_clause = if include_archive {
            "OR t.archived_path IS NOT NULL"
        } else {
            ""
        };
        let scope = format!(
            "FROM library_tracks t
             WHERE (EXISTS (SELECT 1 FROM device_holdings h
                            WHERE h.content_hash = t.content_hash AND h.user_id = :user)
                    {archive_clause})
               -- ESCAPE is not optional: SQLite has no default escape character, so without it
               -- the backslashes the search term is escaped with are matched literally and a
               -- search containing % or _ finds nothing at all.
               AND (:search IS NULL
                    OR t.title LIKE :like ESCAPE '\\'
                    OR t.artist LIKE :like ESCAPE '\\'
                    OR COALESCE(t.album, '') LIKE :like ESCAPE '\\'
                    OR COALESCE(t.album_artist, '') LIKE :like ESCAPE '\\')"
        );

        // Present when *this* device holds it. With no device selected nothing is greyed out, so
        // the expression is a constant rather than a join that would always be false.
        
        let source_count = "(SELECT COUNT(DISTINCT h.device_id) FROM device_holdings h JOIN registered_nodes rn ON rn.device_id = h.device_id AND rn.user_id = h.user_id WHERE h.content_hash = t.content_hash AND h.user_id = :user) + (CASE WHEN t.archived_path IS NOT NULL THEN 1 ELSE 0 END)";
let present = match device_id {
            Some(_) => {
                "EXISTS (SELECT 1 FROM device_holdings h
                         WHERE h.content_hash = t.content_hash AND h.device_id = :device)"
            }
            None => "1",
        };

        let sql = match kind {
            BrowseKind::Track => format!(
                "SELECT t.content_hash,
                        t.title,
                        COALESCE(t.album_artist, t.artist),
                        t.album,
                        1,
                        MIN({present}),
                        MAX({source_count})
                 {scope}
                 GROUP BY t.content_hash
                 ORDER BY COALESCE(t.album_artist, t.artist), t.album, t.track_no, t.title
                 LIMIT :limit OFFSET :offset"
            ),
            BrowseKind::Album => format!(
                "SELECT COALESCE(t.album_artist, t.artist) || char(1) || COALESCE(t.album, ''),
                        COALESCE(t.album, 'Unknown Album'),
                        COALESCE(t.album_artist, t.artist),
                        t.album,
                        COUNT(*),
                        MIN({present}),
                        MAX({source_count})
                 {scope}
                 GROUP BY COALESCE(t.album_artist, t.artist), COALESCE(t.album, '')
                 ORDER BY COALESCE(t.album_artist, t.artist), COALESCE(t.album, '')
                 LIMIT :limit OFFSET :offset"
            ),
            BrowseKind::Artist => format!(
                "SELECT COALESCE(t.album_artist, t.artist),
                        COALESCE(t.album_artist, t.artist),
                        COALESCE(t.album_artist, t.artist),
                        NULL,
                        COUNT(*),
                        MIN({present}),
                        MAX({source_count})
                 {scope}
                 GROUP BY COALESCE(t.album_artist, t.artist)
                 ORDER BY COALESCE(t.album_artist, t.artist)
                 LIMIT :limit OFFSET :offset"
            ),
        };

        // `%` and `_` are wildcards in LIKE, so a search for "50%" would otherwise match anything.
        let like = search.map(|term| {
            format!(
                "%{}%",
                term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
            )
        });
        // Built rather than declared, because `:device` only appears in the statement when a
        // device was selected — rusqlite rejects a bound name the SQL does not mention, so binding
        // it unconditionally made every all-devices query fail outright.
        let mut params: Vec<(&str, &dyn rusqlite::ToSql)> = vec![
            (":user", &user_id),
            (":search", &search),
            (":like", &like),
            (":limit", &limit),
            (":offset", &offset),
        ];
        if let Some(device) = device_id.as_ref() {
            params.push((":device", device));
        }

        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params.as_slice())?;

        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            let album_artist: String = row.get(2)?;
            let album: Option<String> = row.get(3)?;
            items.push(BrowseItem {
                id: row.get(0)?,
                title: row.get(1)?,
                subtitle: album_artist.clone(),
                // Only albums and their tracks have a cover to point at; an artist row has no one
                // album to borrow one from.
                cover_key: album
                    .as_deref()
                    .filter(|_| kind != BrowseKind::Artist)
                    .map(|name| album_key(&album_artist, name)),
                track_count: row.get(4)?,
                // `MIN` over the group: an album counts as present only when every one of its
                // tracks is. A half-copied album shown as complete is the one answer this view must
                // never give.
                present_on_device: row.get::<_, i64>(5)? != 0,
                source_count: row.get(6)?,
            });
        }
        Ok(items)
    }

    // ── Cover art ───────────────────────────────────────────────────────────────────────────

    pub fn set_cover(
        &self,
        album_key: &str,
        album_artist: &str,
        album: &str,
        extension: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO library_covers
                 (album_key, album_artist, album, extension, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s', 'now'))",
            params![album_key, album_artist, album, extension],
        )?;
        Ok(())
    }

    /// The stored cover's file extension, or `None` when the album has no artwork.
    pub fn cover_extension(&self, album_key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT extension FROM library_covers WHERE album_key = ?1",
            params![album_key],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn has_cover(&self, album_key: &str) -> bool {
        matches!(self.cover_extension(album_key), Ok(Some(_)))
    }

    /// One archived file per album, for the backfill pass to read artwork out of.
    pub fn albums_for_cover_backfill(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(t.album_artist, t.artist), t.album, MIN(t.archived_path)
             FROM library_tracks t
             WHERE t.archived_path IS NOT NULL AND COALESCE(t.album, '') != ''
             GROUP BY COALESCE(t.album_artist, t.artist), t.album",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    // ── Upload sessions ─────────────────────────────────────────────────────────────────────

    pub fn create_upload(
        &self,
        upload_id: &str,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
        size_bytes: i64,
        target: &str,
        extension: Option<&str>,
        ttl_hours: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO upload_sessions
                 (upload_id, user_id, device_id, content_hash, size_bytes, received_bytes,
                  target, created_at, expires_at, extension)
             VALUES (?1,?2,?3,?4,?5,0,?6,?7,?8,?9)",
            params![
                upload_id,
                user_id,
                device_id,
                content_hash,
                size_bytes,
                target,
                now.to_rfc3339(),
                (now + chrono::Duration::hours(ttl_hours)).to_rfc3339(),
                extension,
            ],
        )?;
        Ok(())
    }

    /// An unfinished upload of the same file by the same device, so a dropped transfer resumes.
    pub fn resumable_upload(
        &self,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
    ) -> Result<Option<UploadSession>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT upload_id, user_id, device_id, content_hash, size_bytes, received_bytes, target,
                    extension
             FROM upload_sessions
             WHERE user_id = ?1 AND device_id = ?2 AND content_hash = ?3
               AND expires_at > ?4
             ORDER BY created_at DESC LIMIT 1",
            params![
                user_id,
                device_id,
                content_hash,
                chrono::Utc::now().to_rfc3339()
            ],
            row_to_upload,
        )
        .optional()
    }

    pub fn upload_session(&self, upload_id: &str) -> Result<Option<UploadSession>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT upload_id, user_id, device_id, content_hash, size_bytes, received_bytes, target,
                    extension
             FROM upload_sessions WHERE upload_id = ?1",
            params![upload_id],
            row_to_upload,
        )
        .optional()
    }

    pub fn set_upload_received(&self, upload_id: &str, received: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE upload_sessions SET received_bytes = ?2 WHERE upload_id = ?1",
            params![upload_id, received],
        )?;
        Ok(())
    }

    pub fn delete_upload(&self, upload_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM upload_sessions WHERE upload_id = ?1",
            params![upload_id],
        )?;
        Ok(())
    }

    /// Upload sessions whose TTL has passed, so their `.part` files can be removed too.
    pub fn expired_uploads(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT upload_id FROM upload_sessions WHERE expires_at <= ?1")?;
        let rows = stmt.query_map(params![chrono::Utc::now().to_rfc3339()], |row| row.get(0))?;
        rows.collect()
    }

    // ── Spool ───────────────────────────────────────────────────────────────────────────────

    pub fn spool_insert(
        &self,
        content_hash: &str,
        size_bytes: i64,
        from_device: &str,
        user_id: &str,
        ttl_hours: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO spool_items (content_hash, size_bytes, from_device, user_id,
                                      created_at, expires_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(content_hash) DO UPDATE SET
                 expires_at = excluded.expires_at, from_device = excluded.from_device",
            params![
                content_hash,
                size_bytes,
                from_device,
                user_id,
                now.to_rfc3339(),
                (now + chrono::Duration::hours(ttl_hours)).to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn spool_total_bytes(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(SUM(size_bytes),0) FROM spool_items", [], |r| {
            r.get(0)
        })
    }

    pub fn spool_contains(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM spool_items WHERE content_hash = ?1",
                params![content_hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Spool entries to remove, oldest first — expired ones always, then whatever else it takes to
    /// get back under [`budget`].
    /// What to evict from **one account's** spool, oldest first, to bring it inside `budget`.
    ///
    /// Scoped to an account on purpose. This used to take one global budget and evict oldest-first
    /// across everybody, so one account filling the spool evicted another's staged files — a guest
    /// could push the admin's pending transfers off the disk just by uploading.
    ///
    /// Expired rows go regardless of the budget; that is the TTL, not the quota.
    pub fn spool_evictable_for(&self, user_id: &str, budget: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content_hash, size_bytes, expires_at FROM spool_items
              WHERE user_id = ?1 ORDER BY created_at ASC",
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows: Vec<(String, i64, String)> = stmt
            .query_map(params![user_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<_>>()?;

        let mut total: i64 = rows.iter().map(|(_, size, _)| size).sum();
        let mut doomed = Vec::new();
        for (hash, size, expires_at) in rows {
            if expires_at <= now || total > budget {
                doomed.push((hash, size));
                total -= size;
            }
        }
        Ok(doomed)
    }

    /// Every account with something in the spool, for the periodic sweep.
    pub fn spool_users(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT user_id FROM spool_items")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Which account spooled a file, so a fetch can be scoped to it.
    pub fn spool_owner(&self, content_hash: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id FROM spool_items WHERE content_hash = ?1",
            params![content_hash],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn spool_delete(&self, content_hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM spool_items WHERE content_hash = ?1",
            params![content_hash],
        )?;
        Ok(())
    }

    /// Removes a track from the library index entirely.
    
    pub fn delete_library_item(&self, kind: BrowseKind, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        match kind {
            BrowseKind::Track => {
                conn.execute("DELETE FROM device_holdings WHERE content_hash = ?1", rusqlite::params![id])?;
                let deleted = conn.execute("DELETE FROM library_tracks WHERE content_hash = ?1", rusqlite::params![id])?;
                Ok(deleted > 0)
            }
            BrowseKind::Album => {
                let mut parts = id.splitn(2, '');
                let album_artist = parts.next().unwrap_or("");
                let album = parts.next().unwrap_or("");
                conn.execute(
                    "DELETE FROM device_holdings WHERE content_hash IN (SELECT content_hash FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1 AND COALESCE(album, '') = ?2)",
                    rusqlite::params![album_artist, album]
                )?;
                let deleted = conn.execute(
                    "DELETE FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1 AND COALESCE(album, '') = ?2",
                    rusqlite::params![album_artist, album]
                )?;
                Ok(deleted > 0)
            }
            BrowseKind::Artist => {
                conn.execute(
                    "DELETE FROM device_holdings WHERE content_hash IN (SELECT content_hash FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1)",
                    rusqlite::params![id]
                )?;
                let deleted = conn.execute(
                    "DELETE FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1",
                    rusqlite::params![id]
                )?;
                Ok(deleted > 0)
            }
        }
    }
pub fn delete_library_track(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM device_holdings WHERE content_hash = ?1",
            params![content_hash],
        )?;
        let deleted = conn.execute(
            "DELETE FROM library_tracks WHERE content_hash = ?1",
            params![content_hash],
        )?;
        Ok(deleted > 0)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track(hash: &str, artist: &str, title: &str, duration_ms: i64) -> LibraryTrack {
        LibraryTrack {
            content_hash: hash.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album: Some("Album".to_string()),
            album_artist: None,
            track_no: Some(1),
            disc_no: None,
            year: None,
            genre: None,
            duration_ms,
            size_bytes: 1_000,
            format: Some("flac".to_string()),
            bitrate_kbps: None,
            archived_path: None,
        }
    }

    /// Reproduces the "download 136 tracks nobody has" report.
    ///
    /// A device that is unregistered — retired, re-paired, or given a new device id — leaves its
    /// `device_holdings` rows behind, because `delete_node` only touches `registered_nodes`.
    /// `missing_on_device` then joins against those orphans and offers every one of their tracks to
    /// every remaining device, sourced from a machine that no longer exists.
    #[test]
    fn holdings_of_an_unregistered_device_are_still_offered() {
        let db = db_with(&[
            (track("h1", "BoC", "Roygbiv", 200_000), "old-laptop"),
            (track("h2", "BoC", "Olson", 100_000), "old-laptop"),
        ]);
        db.upsert_node(
            "old-laptop",
            "alpha",
            crate::db::NodeName::Set("Old laptop"),
            "wander",
            None,
            None,
        )
        .unwrap();

        assert_eq!(db.missing_on_device("alpha", "phone", 50).unwrap().len(), 2);

        // The device is retired. Its holdings are the only record that these files ever existed
        // anywhere, and nothing removes them.
        assert!(db.delete_node("alpha", "old-laptop").unwrap());

        let still_offered = db.missing_on_device("alpha", "phone", 50).unwrap();
        assert_eq!(
            still_offered.len(),
            0,
            "a device that is gone cannot be a source: {} track(s) still offered",
            still_offered.len()
        );
    }

    fn db_with(tracks: &[(LibraryTrack, &str)]) -> Db {
        let db = Db::new_in_memory().unwrap();
        for (t, device) in tracks {
            db.upsert_library_track(t).unwrap();
            // Registered as well as holding. A device only reports holdings after it has
            // registered, and since holdings from a device that no longer exists are no longer a
            // source, a fixture that skips this is testing a state that cannot occur.
            db.upsert_node(
                device,
                "alpha",
                crate::db::NodeName::KeepOr(device),
                "wander",
                None,
                None,
            )
            .unwrap();
            db.upsert_holding("alpha", device, &t.content_hash, None)
                .unwrap();
        }
        db
    }

    #[test]
    fn browsing_without_a_device_lists_everything_as_present() {
        // Regression: the "all devices" query does not mention `:device`, and binding it anyway
        // made rusqlite reject the statement — so the default view of the library, the one you get
        // before choosing anything, was the one that could not run.
        let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
        let items = db
            .library_browse("alpha", None, BrowseKind::Album, None, 50, 0, true)
            .expect("a browse with no device selected must work");
        assert_eq!(items.len(), 1);
        assert!(items[0].present_on_device, "nothing to be missing from");
        assert!(items[0].cover_key.is_some(), "an album can carry artwork");
    }

    #[test]
    fn browsing_greys_out_what_the_chosen_device_lacks() {
        let db = db_with(&[
            (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
            (track("h2", "Pixies", "Debaser", 172_000), "phone"),
        ]);
        let items = db
            .library_browse("alpha", Some("phone"), BrowseKind::Track, None, 50, 0, true)
            .unwrap();
        let missing: Vec<&str> = items
            .iter()
            .filter(|item| !item.present_on_device)
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(missing, vec!["Come As You Are"], "only the laptop has that one");
    }

    #[test]
    fn browsing_by_artist_has_no_cover_to_point_at() {
        let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
        let items = db
            .library_browse("alpha", None, BrowseKind::Artist, None, 50, 0, true)
            .unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].cover_key.is_none(), "an artist has no one album to borrow art from");
    }

    #[test]
    fn browse_search_treats_wildcards_as_literal_text() {
        let db = db_with(&[
            (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
            (track("h2", "Artist", "100% Real", 100_000), "laptop"),
        ]);
        let items = db
            .library_browse("alpha", None, BrowseKind::Track, Some("%"), 50, 0, true)
            .unwrap();
        assert_eq!(items.len(), 1, "`%` must match the track with a percent sign, not everything");
        assert_eq!(items[0].title, "100% Real");
    }

    #[test]
    fn a_track_only_one_device_has_is_missing_on_the_other() {
        let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
        db.upsert_holding("alpha", "phone", "h0", None).ok();

        let missing = db.missing_on_device("alpha", "phone", 10).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].content_hash, "h1");
    }

    #[test]
    fn a_track_both_devices_have_is_not_missing() {
        let db = db_with(&[
            (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
            (track("h1", "Nirvana", "Come As You Are", 219_000), "phone"),
        ]);
        assert!(db.missing_on_device("alpha", "phone", 10).unwrap().is_empty());
    }

    /// The point of the fuzzy layer: different bytes, same recording.
    #[test]
    fn a_different_rip_of_the_same_recording_is_not_missing() {
        let db = db_with(&[
            (track("flac", "Nirvana", "Come As You Are", 219_000), "laptop"),
            (track("mp3", "Nirvana", "Come As You Are", 220_500), "phone"),
        ]);
        assert!(
            db.missing_on_device("alpha", "phone", 10).unwrap().is_empty(),
            "a 1.5s-different encode of the same song must not be offered"
        );
    }

    /// The mistake that must never be made: a live take is not the studio cut.
    #[test]
    fn a_live_take_is_still_missing_when_you_own_the_studio_cut() {
        let db = db_with(&[
            (
                track("live", "Nirvana", "Come As You Are (Live)", 219_000),
                "laptop",
            ),
            (track("studio", "Nirvana", "Come As You Are", 219_000), "phone"),
        ]);
        let missing = db.missing_on_device("alpha", "phone", 10).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].content_hash, "live");
    }

    #[test]
    fn another_accounts_device_is_not_consulted() {
        let db = Db::new_in_memory().unwrap();
        let t = track("h1", "Nirvana", "Come As You Are", 219_000);
        db.upsert_library_track(&t).unwrap();
        db.upsert_holding("beta", "beta-laptop", "h1", None).unwrap();

        assert!(db.missing_on_device("alpha", "phone", 10).unwrap().is_empty());
    }

    #[test]
    fn forgetting_a_holding_makes_it_missing_again() {
        let db = db_with(&[
            (track("h1", "A", "B", 100_000), "laptop"),
            (track("h1", "A", "B", 100_000), "phone"),
        ]);
        assert!(db.missing_on_device("alpha", "phone", 10).unwrap().is_empty());

        db.forget_holdings("alpha", "phone", &["h1".to_string()]).unwrap();
        assert_eq!(db.missing_on_device("alpha", "phone", 10).unwrap().len(), 1);
    }

    /// `forget_holdings` filtered on the device alone, and device ids are chosen by the client —
    /// so naming someone else's device deleted their rows.
    #[test]
    fn forgetting_a_holding_cannot_reach_another_account() {
        let db = Db::new_in_memory().unwrap();
        let t = track("h1", "A", "B", 100_000);
        db.upsert_library_track(&t).unwrap();
        db.upsert_holding("alpha", "admin-desktop", "h1", None).unwrap();

        let removed = db
            .forget_holdings("guest", "admin-desktop", &["h1".to_string()])
            .unwrap();
        assert_eq!(removed, 0, "a guest deleted the admin's holding");
        assert_eq!(
            db.device_holding_hashes("alpha", "admin-desktop").unwrap().len(),
            1
        );
    }

    /// The same smuggled device id, on the read side.
    #[test]
    fn device_holdings_cannot_be_read_across_accounts() {
        let db = Db::new_in_memory().unwrap();
        let t = track("h1", "A", "B", 100_000);
        db.upsert_library_track(&t).unwrap();
        db.upsert_holding("alpha", "admin-desktop", "h1", None).unwrap();

        assert!(db
            .device_holding_hashes("guest", "admin-desktop")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn archived_path_survives_a_later_report_without_one() {
        let db = Db::new_in_memory().unwrap();
        let mut t = track("h1", "A", "B", 100_000);
        t.archived_path = Some("A/Album/01 - B.flac".to_string());
        db.upsert_library_track(&t).unwrap();

        t.archived_path = None;
        db.upsert_library_track(&t).unwrap();

        assert_eq!(
            db.library_track("h1").unwrap().unwrap().archived_path.as_deref(),
            Some("A/Album/01 - B.flac")
        );
    }

    #[test]
    fn spool_evicts_oldest_first_when_over_budget() {
        let db = Db::new_in_memory().unwrap();
        for (hash, size) in [("a", 100), ("b", 100), ("c", 100)] {
            db.spool_insert(hash, size, "laptop", "alpha", 72).unwrap();
            // created_at has second resolution in RFC3339, so order the inserts explicitly.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(db.spool_total_bytes().unwrap(), 300);

        let doomed = db.spool_evictable_for("alpha", 150).unwrap();
        assert_eq!(doomed.len(), 2, "two must go to get under 150");
        assert_eq!(doomed[0].0, "a", "oldest first");
    }

    /// One account filling the spool used to evict another's staged files: the budget was global
    /// and eviction ran oldest-first across everybody.
    #[test]
    fn spool_eviction_never_reaches_another_account() {
        let db = Db::new_in_memory().unwrap();
        db.spool_insert("admin-file", 100, "desktop", "alpha", 72).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        for (hash, size) in [("guest-1", 100), ("guest-2", 100)] {
            db.spool_insert(hash, size, "phone", "guest", 72).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let doomed = db.spool_evictable_for("guest", 100).unwrap();
        assert!(
            doomed.iter().all(|(hash, _)| hash.starts_with("guest")),
            "a guest's eviction reached the admin's spool: {doomed:?}"
        );
        assert_eq!(db.spool_bytes_for("alpha").unwrap(), 100);
        assert_eq!(db.spool_bytes_for("guest").unwrap(), 200);
    }

    #[test]
    fn spool_keeps_everything_when_under_budget() {
        let db = Db::new_in_memory().unwrap();
        db.spool_insert("a", 100, "laptop", "alpha", 72).unwrap();
        assert!(db.spool_evictable_for("alpha", 1_000).unwrap().is_empty());
    }
}
