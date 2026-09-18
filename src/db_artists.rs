//! Artists as rows, and who is subscribed to them.
//!
//! A recording carries its artist as free text copied off whatever tagged the file, which is a
//! name rather than an identity: "Tyler, The Creator" and "tyler the creator" are two strings and
//! one person. Nothing can be subscribed to a string like that — half the releases would arrive
//! under the other spelling and never reach the subscriber.
//!
//! So the identity is [`crate::norm::normalize_artist`] of the name, and it carries the UNIQUE.
//! That is deliberately the same normalisation the library matcher already uses to decide two tags
//! mean one artist; using a second, different rule here would mean the catalogue and the library
//! disagreed about who somebody is.
//!
//! `display_name` is only for printing. The first spelling seen wins and later ones do not
//! overwrite it — an artist whose name flickers between two taggings would otherwise rewrite every
//! subscriber's list on every publish, and neither spelling is more correct than the other.

use crate::db::Db;
use crate::norm::normalize_artist;
use rusqlite::{params, OptionalExtension, Result};

/// An artist somebody could subscribe to.
#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    pub artist_id: String,
    pub norm_name: String,
    pub display_name: String,
    /// A namespaced id at the source it came from, such as `ytm:UC…`, when one was supplied.
    pub external_id: Option<String>,
}

/// A release by a subscribed artist, as the client needs to show it.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtistRelease {
    pub recording_id: String,
    pub artist_id: String,
    pub artist: String,
    pub title: Option<String>,
    pub album: Option<String>,
    pub updated_at: i64,
}

impl Db {
    /// Finds an artist by name, or creates one.
    ///
    /// Returns the id either way, so a caller that only wants to name somebody — publishing a
    /// recording, subscribing to a search result — never has to know which happened.
    ///
    /// [`external_id`] is filled in on the first call that supplies one and never cleared by a
    /// later call that does not: the same artist arrives from the library with no id at all and
    /// from YouTube Music with a channel, and whichever lands second must not erase the other.
    pub fn upsert_artist(
        &self,
        display_name: &str,
        external_id: Option<&str>,
    ) -> Result<Option<String>> {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let norm = normalize_artist(trimmed);
        if norm.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        let existing: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT artist_id, external_id FROM artists WHERE norm_name = ?1",
                params![norm],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((id, current_external)) = existing {
            if current_external.is_none() {
                if let Some(external) = external_id.filter(|e| !e.trim().is_empty()) {
                    conn.execute(
                        "UPDATE artists SET external_id = ?1 WHERE artist_id = ?2",
                        params![external.trim(), id],
                    )?;
                }
            }
            return Ok(Some(id));
        }

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO artists (artist_id, norm_name, display_name, external_id, created_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s','now'))",
            params![
                id,
                norm,
                trimmed,
                external_id.map(str::trim).filter(|e| !e.is_empty())
            ],
        )?;
        Ok(Some(id))
    }

    /// Points a recording at an artist row, creating the row if this is the first time it is seen.
    ///
    /// Called on publish rather than on read: doing it lazily would mean a recording published
    /// while nobody was subscribed never joined an artist, and so never reached the subscriber who
    /// arrived a minute later.
    pub fn link_recording_artist(
        &self,
        recording_id: &str,
        artist: Option<&str>,
    ) -> Result<Option<String>> {
        let Some(name) = artist else { return Ok(None) };
        let Some(id) = self.upsert_artist(name, None)? else {
            return Ok(None);
        };
        self.conn.lock().unwrap().execute(
            "UPDATE catalog_recordings SET artist_id = ?1 WHERE recording_id = ?2",
            params![id, recording_id],
        )?;
        Ok(Some(id))
    }

    pub fn artist_by_id(&self, artist_id: &str) -> Result<Option<Artist>> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT artist_id, norm_name, display_name, external_id
                   FROM artists WHERE artist_id = ?1",
                params![artist_id],
                |row| {
                    Ok(Artist {
                        artist_id: row.get(0)?,
                        norm_name: row.get(1)?,
                        display_name: row.get(2)?,
                        external_id: row.get(3)?,
                    })
                },
            )
            .optional()
    }

    /// Subscribes [`user_id`] to an artist by name, creating the artist row if it is new.
    ///
    /// By name rather than by id because that is what the client has: it is looking at a track
    /// whose tag says a name, or a YouTube Music page whose channel it knows. Requiring an id
    /// first would mean a lookup that could only fail for an artist nobody has published yet —
    /// exactly the artist most worth being told about.
    pub fn subscribe_artist(
        &self,
        user_id: &str,
        display_name: &str,
        external_id: Option<&str>,
    ) -> Result<Option<Artist>> {
        let Some(id) = self.upsert_artist(display_name, external_id)? else {
            return Ok(None);
        };
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO artist_subscriptions (user_id, artist_id, since_at)
             VALUES (?1, ?2, strftime('%s','now'))",
            params![user_id, id],
        )?;
        self.artist_by_id(&id)
    }

    pub fn unsubscribe_artist(&self, user_id: &str, artist_id: &str) -> Result<bool> {
        let removed = self.conn.lock().unwrap().execute(
            "DELETE FROM artist_subscriptions WHERE user_id = ?1 AND artist_id = ?2",
            params![user_id, artist_id],
        )?;
        Ok(removed > 0)
    }

    pub fn subscribed_artists(&self, user_id: &str) -> Result<Vec<Artist>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT a.artist_id, a.norm_name, a.display_name, a.external_id
               FROM artist_subscriptions s
               JOIN artists a ON a.artist_id = s.artist_id
              WHERE s.user_id = ?1
              ORDER BY a.display_name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(Artist {
                artist_id: row.get(0)?,
                norm_name: row.get(1)?,
                display_name: row.get(2)?,
                external_id: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Whether this account is subscribed to the artist of a given name.
    ///
    /// Answering by name keeps the client from having to hold an id for a page it is only looking
    /// at, and normalising here means a track tagged one way and a page titled another agree.
    pub fn is_subscribed_to(&self, user_id: &str, display_name: &str) -> Result<bool> {
        let norm = normalize_artist(display_name.trim());
        if norm.is_empty() {
            return Ok(false);
        }
        let found: Option<i64> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM artist_subscriptions s
                   JOIN artists a ON a.artist_id = s.artist_id
                  WHERE s.user_id = ?1 AND a.norm_name = ?2",
                params![user_id, norm],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Everything published by an artist this account subscribes to, since [`since`].
    ///
    /// Bounded by [`limit`] and ordered oldest first, because the client walks forward from a
    /// watermark it stores: newest-first would mean a client that had been away for a while
    /// notifying about the most recent release and silently skipping everything before it.
    ///
    /// A recording published *before* the subscription is not new to the subscriber, so
    /// `since_at` floors each artist independently. Without it, subscribing to somebody with a
    /// deep back catalogue would announce all of it at once.
    pub fn releases_for_subscriber(
        &self,
        user_id: &str,
        since: i64,
        limit: i64,
    ) -> Result<Vec<ArtistRelease>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT r.recording_id, r.artist_id, a.display_name, r.title, r.album, r.updated_at
               FROM catalog_recordings r
               JOIN artist_subscriptions s ON s.artist_id = r.artist_id
               JOIN artists a ON a.artist_id = r.artist_id
              WHERE s.user_id = ?1
                AND r.updated_at > ?2
                AND r.updated_at >= s.since_at
              ORDER BY r.updated_at ASC
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![user_id, since, limit], |row| {
            Ok(ArtistRelease {
                recording_id: row.get(0)?,
                artist_id: row.get(1)?,
                artist: row.get(2)?,
                title: row.get(3)?,
                album: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    #[test]
    fn one_artist_however_the_name_is_spelled() {
        let db = db();
        let a = db
            .upsert_artist("Tyler, The Creator", None)
            .unwrap()
            .unwrap();
        let b = db
            .upsert_artist("tyler the creator", None)
            .unwrap()
            .unwrap();
        assert_eq!(a, b, "the same person, spelled two ways");
    }

    #[test]
    fn the_first_spelling_is_the_one_that_is_shown() {
        let db = db();
        let id = db.upsert_artist("Bjork", None).unwrap().unwrap();
        db.upsert_artist("BJORK", None).unwrap();
        assert_eq!(db.artist_by_id(&id).unwrap().unwrap().display_name, "Bjork");
    }

    #[test]
    fn an_external_id_is_filled_in_once_and_never_erased() {
        let db = db();
        let id = db.upsert_artist("Aphex Twin", None).unwrap().unwrap();
        db.upsert_artist("Aphex Twin", Some("ytm:UC123")).unwrap();
        assert_eq!(
            db.artist_by_id(&id)
                .unwrap()
                .unwrap()
                .external_id
                .as_deref(),
            Some("ytm:UC123")
        );

        // A later publish from a source that has no channel must not take it away again.
        db.upsert_artist("Aphex Twin", None).unwrap();
        assert_eq!(
            db.artist_by_id(&id)
                .unwrap()
                .unwrap()
                .external_id
                .as_deref(),
            Some("ytm:UC123")
        );
    }

    #[test]
    fn a_nameless_artist_is_not_a_row() {
        let db = db();
        assert_eq!(db.upsert_artist("   ", None).unwrap(), None);
        assert_eq!(db.upsert_artist("", None).unwrap(), None);
    }

    #[test]
    fn subscribing_twice_is_one_subscription() {
        let db = db();
        db.subscribe_artist("alpha", "Portishead", None).unwrap();
        db.subscribe_artist("alpha", "portishead", None).unwrap();
        assert_eq!(db.subscribed_artists("alpha").unwrap().len(), 1);
    }

    #[test]
    fn subscriptions_belong_to_one_account() {
        let db = db();
        db.subscribe_artist("alpha", "Boards of Canada", None)
            .unwrap();
        assert!(db.subscribed_artists("mallory").unwrap().is_empty());
        assert!(!db.is_subscribed_to("mallory", "Boards of Canada").unwrap());
        assert!(db.is_subscribed_to("alpha", "boards of canada").unwrap());
    }

    #[test]
    fn unsubscribing_reports_whether_there_was_anything_to_remove() {
        let db = db();
        let artist = db
            .subscribe_artist("alpha", "Massive Attack", None)
            .unwrap()
            .unwrap();
        assert!(db.unsubscribe_artist("alpha", &artist.artist_id).unwrap());
        assert!(!db.unsubscribe_artist("alpha", &artist.artist_id).unwrap());
        assert!(db.subscribed_artists("alpha").unwrap().is_empty());
    }
}
