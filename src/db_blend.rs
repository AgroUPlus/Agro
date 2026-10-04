//! Blends at rest: the recipe, who is in it, and writing the playlist from their listening.
//!
//! A blend is a playlist row with `kind = 'blend'`, so it is shared, followed and polled exactly as
//! any other. What is extra lives here. Members are invited and read nothing of anyone until they
//! accept; once they have, their listening counts only while their stats are open and they are not
//! incognito — the same consent their profile already states — checked at every refresh.

use rusqlite::{params, OptionalExtension, Result};

use crate::blend_recipe::{Blend, BlendMember, BlendRefresh, BlendSettings, BlendWindow};
use crate::db::Db;
use crate::db_playlists::{bump_revision, Playlist};

impl Db {
    /// Creates a blend owned by `owner`, who is its first member, with `invited` asked to join.
    pub fn create_blend(
        &self,
        owner: &str,
        title: &str,
        invited: &[String],
        settings: BlendSettings,
    ) -> Result<Playlist> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        // Private as a playlist: who may open a blend is its joined members, which
        // `can_view_playlist` answers from `blend_members`, never the owner's friends at large.
        tx.execute(
            "INSERT INTO playlists (id, user_id, title, is_public, friends_only, created_at, updated_at, kind)
             VALUES (?1, ?2, ?3, 0, 0, ?4, ?4, 'blend')",
            params![id, owner, title, now],
        )?;
        tx.execute(
            "INSERT INTO blends (playlist_id, size, mix, time_window, refresh) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, settings.size, settings.mix, settings.window.stored(), settings.refresh.stored()],
        )?;
        tx.execute(
            "INSERT INTO blend_members (playlist_id, username, state, invited_at) VALUES (?1, ?2, 'joined', ?3)",
            params![id, owner, now],
        )?;
        for name in invited {
            tx.execute(
                "INSERT OR IGNORE INTO blend_members (playlist_id, username, state, invited_at)
                 VALUES (?1, ?2, 'invited', ?3)",
                params![id, name.trim().to_lowercase(), now],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_playlist(&id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn blend(&self, playlist_id: &str) -> Result<Option<Blend>> {
        self.read()
            .query_row(
                "SELECT playlist_id, size, mix, time_window, refresh, refreshed_at FROM blends WHERE playlist_id = ?1",
                params![playlist_id],
                |row| {
                    Ok(Blend {
                        playlist_id: row.get(0)?,
                        size: row.get(1)?,
                        mix: row.get(2)?,
                        window: BlendWindow::from_stored(&row.get::<_, String>(3)?),
                        refresh: BlendRefresh::from_stored(&row.get::<_, String>(4)?),
                        refreshed_at: row.get(5)?,
                    })
                },
            )
            .optional()
    }

    /// Everyone in a blend or asked to be, joined first, in the order they were asked.
    pub fn blend_members(&self, playlist_id: &str) -> Result<Vec<BlendMember>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT username, state = 'joined' FROM blend_members WHERE playlist_id = ?1
              ORDER BY state = 'joined' DESC, invited_at, username",
        )?;
        let rows = stmt.query_map(params![playlist_id], |row| {
            Ok(BlendMember {
                username: row.get(0)?,
                joined: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    /// Whether `username` has joined `playlist_id`. An invitation alone is not membership.
    pub fn is_blend_member(&self, playlist_id: &str, username: &str) -> Result<bool> {
        Ok(self
            .read()
            .query_row(
                "SELECT 1 FROM blend_members
                  WHERE playlist_id = ?1 AND username = ?2 COLLATE NOCASE AND state = 'joined'",
                params![playlist_id, username.trim()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The blends `username` has been asked to and not answered, newest first.
    pub fn blend_invites(&self, username: &str) -> Result<Vec<Playlist>> {
        let ids: Vec<String> = {
            let conn = self.read();
            let mut stmt = conn.prepare(
                "SELECT playlist_id FROM blend_members
                  WHERE username = ?1 COLLATE NOCASE AND state = 'invited' ORDER BY invited_at DESC",
            )?;
            let rows = stmt.query_map(params![username.trim()], |row| row.get(0))?;
            rows.collect::<Result<_>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            out.extend(self.get_playlist(&id)?);
        }
        Ok(out)
    }

    /// Accepts or declines an invitation. `false` when there was none to answer.
    ///
    /// Accepting also follows the playlist, which is what puts it in the member's library and gets
    /// them told when it changes. Declining removes the invitation outright.
    pub fn answer_blend_invite(
        &self,
        playlist_id: &str,
        username: &str,
        accept: bool,
    ) -> Result<bool> {
        let name = username.trim().to_lowercase();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let changed = if accept {
            tx.execute(
                "UPDATE blend_members SET state = 'joined'
                  WHERE playlist_id = ?1 AND username = ?2 AND state = 'invited'",
                params![playlist_id, name],
            )?
        } else {
            tx.execute(
                "DELETE FROM blend_members WHERE playlist_id = ?1 AND username = ?2 AND state = 'invited'",
                params![playlist_id, name],
            )?
        };
        if changed > 0 && accept {
            tx.execute(
                "INSERT OR IGNORE INTO playlist_followers (playlist_id, user_id, followed_at) VALUES (?1, ?2, ?3)",
                params![playlist_id, name, chrono::Utc::now().to_rfc3339()],
            )?;
            mark_due(&tx, playlist_id)?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Takes a member out, along with their follow, and has the blend written again without them.
    pub fn leave_blend(&self, playlist_id: &str, username: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM blend_members WHERE playlist_id = ?1 AND username = ?2 COLLATE NOCASE",
            params![playlist_id, username.trim()],
        )?;
        tx.execute(
            "DELETE FROM playlist_followers WHERE playlist_id = ?1 AND user_id = ?2 COLLATE NOCASE",
            params![playlist_id, username.trim()],
        )?;
        mark_due(&tx, playlist_id)?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Changes the recipe, and the title with it, and has it written again.
    pub fn update_blend(
        &self,
        playlist_id: &str,
        title: &str,
        settings: BlendSettings,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE blends SET size = ?2, mix = ?3, time_window = ?4, refresh = ?5 WHERE playlist_id = ?1",
            params![playlist_id, settings.size, settings.mix, settings.window.stored(), settings.refresh.stored()],
        )?;
        tx.execute(
            "UPDATE playlists SET title = ?2 WHERE id = ?1",
            params![playlist_id, title],
        )?;
        mark_due(&tx, playlist_id)?;
        bump_revision(&tx, playlist_id)?;
        tx.commit()
    }
}

/// Marks a blend to be written again the next time anyone reads it.
pub(crate) fn mark_due(conn: &rusqlite::Connection, playlist_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE blends SET refreshed_at = NULL WHERE playlist_id = ?1",
        params![playlist_id],
    )?;
    Ok(())
}
