//! Short links, and the one listing that shows them beside ephemeral shares.

use rusqlite::{params, OptionalExtension, Result};

use super::{rfc3339_to_unix, unix_now, Db};

impl Db {
    /// Every host any account on this server has allowed, for the public `/listen` route.
    ///
    /// That route has no user in context — a shared link is opened by a stranger, with no token —
    /// so the allowlist it enforces is the union of what the accounts here have set. Only rows
    /// with forwarding actually switched on contribute to it.
    pub fn allowed_share_hosts(&self) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT share_hosts FROM synced_settings
             WHERE share_enabled = 1 AND share_hosts IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        let mut hosts: Vec<String> = Vec::new();
        while let Some(row) = rows.next()? {
            let raw: String = row.get(0)?;
            hosts.extend(
                raw.split(',')
                    .map(|host| host.trim().to_lowercase())
                    .filter(|host| !host.is_empty()),
            );
        }
        hosts.sort();
        hosts.dedup();
        Ok(hosts)
    }

    /// Stores a short link UID mapping to a target URL.
    ///
    /// `source` names where the link came from — `"navidrome"` for a share the music server itself
    /// minted, anything else (or nothing) for a link that exists only here. Deletion reads it to
    /// decide whether removing the row is the whole job.
    pub fn create_short_link(
        &self,
        id: &str,
        target_url: &str,
        user_id: Option<&str>,
        source: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO short_links (id, target_url, user_id, created_at, source, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, target_url, user_id, unix_now(), source, expires_at],
        )?;
        Ok(())
    }

    /// Retrieves the target URL for a short link UID.
    ///
    /// Expiry is enforced here. It was not, so a link given a deliberate lifetime kept forwarding
    /// for ever — the column was written and then never read.
    pub fn get_short_link(&self, id: &str) -> Result<Option<String>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT target_url FROM short_links
             WHERE id = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
        )?;
        let mut rows = stmt.query(params![id, unix_now()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Bumps a short link's hit counter. Aggregate only — see migration 6.
    pub fn record_short_link_click(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        // A counter is never worth failing a redirect over: the visitor gets their page either way.
        let _ = conn.execute(
            "UPDATE short_links SET click_count = click_count + 1, last_clicked_at = ?2
             WHERE id = ?1",
            params![id, unix_now()],
        );
    }

    /// Every link this account has minted, newest first, across both link mechanisms.
    ///
    /// The two tables have almost nothing in common — one holds a hosted audio URL with an RFC3339
    /// expiry, the other a forwarding target with a Unix one — so they are normalised here rather
    /// than in the resolver, which should not have to know that "a link" is two different things.
    pub fn list_links(&self, user_id: &str) -> Result<Vec<LinkRow>> {
        let conn = self.conn.lock().unwrap();
        let mut links = Vec::new();

        let mut stmt = conn.prepare(
            "SELECT id, target_url, created_at, expires_at, click_count, last_clicked_at, source
             FROM short_links WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let mut rows = stmt.query(params![user_id])?;
        while let Some(row) = rows.next()? {
            links.push(LinkRow {
                id: row.get(0)?,
                kind: LinkKind::Short,
                target: row.get(1)?,
                label: None,
                created_at: row.get(2)?,
                expires_at: row.get(3)?,
                click_count: row.get(4)?,
                last_clicked_at: row.get(5)?,
                source: row.get(6)?,
            });
        }

        let mut stmt = conn.prepare(
            "SELECT token, audio_url, track_title, artist_name, expires_at, click_count,
                    last_clicked_at
             FROM ephemeral_shares WHERE user_id = ?1",
        )?;
        let mut rows = stmt.query(params![user_id])?;
        while let Some(row) = rows.next()? {
            let title: String = row.get(2)?;
            let artist: String = row.get(3)?;
            let expires: Option<String> = row.get(4)?;
            links.push(LinkRow {
                id: row.get(0)?,
                kind: LinkKind::Ephemeral,
                target: row.get(1)?,
                label: Some(format!("{artist} — {title}")),
                // Ephemeral shares carry no creation timestamp, only an expiry. Deriving the one
                // from the other would be a guess at the TTL, so the field stays honest and empty.
                created_at: None,
                expires_at: expires.as_deref().and_then(rfc3339_to_unix),
                click_count: row.get(5)?,
                last_clicked_at: row.get(6)?,
                source: None,
            });
        }

        links.sort_by_key(|l| std::cmp::Reverse(l.created_at));
        Ok(links)
    }

    /// Deletes a link. Returns the row's `source` so the caller can decide what else has to happen.
    ///
    /// Scoped by `user_id` in the statement itself rather than checked first: a link belonging to
    /// somebody else must not be deletable, and a check-then-delete leaves a window where it is.
    pub fn delete_link(&self, user_id: &str, id: &str, kind: LinkKind) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        match kind {
            LinkKind::Short => {
                let source: Option<String> = conn
                    .query_row(
                        "SELECT source FROM short_links WHERE id = ?1 AND user_id = ?2",
                        params![id, user_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                let removed = conn.execute(
                    "DELETE FROM short_links WHERE id = ?1 AND user_id = ?2",
                    params![id, user_id],
                )?;
                if removed == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(source)
            }
            LinkKind::Ephemeral => {
                let removed = conn.execute(
                    "DELETE FROM ephemeral_shares WHERE token = ?1 AND user_id = ?2",
                    params![id, user_id],
                )?;
                if removed == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(None)
            }
        }
    }
}

/// Which of the two link mechanisms a row belongs to. See [`Db::list_links`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkKind {
    /// `/listen?id=…` — forwards to a target URL on an allowed host.
    Short,
    /// `/share/<token>` — a hosted audio page with a hard expiry.
    Ephemeral,
}

/// One link, normalised across both tables.
pub struct LinkRow {
    pub id: String,
    pub kind: LinkKind,
    pub target: String,
    /// What the link is *of*, when the row knows. Only ephemeral shares carry track metadata.
    pub label: Option<String>,
    pub created_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub click_count: i64,
    pub last_clicked_at: Option<i64>,
    pub source: Option<String>,
}
