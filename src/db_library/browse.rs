//! Browsing the library by track, album or artist.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::Result;

impl Db {
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
    #[allow(clippy::too_many_arguments)]
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
        let conn = self.conn.lock_recover();

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
                term.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
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
}
