//! Changing a playlist that more than one account may edit.
//!
//! Every edit names the revision it was made against, and is refused whole if the playlist has
//! moved on since: the client then fetches the new state, decides which of its changes still make
//! sense, and sends those. Nothing is merged here behind anyone's back, so two people editing at
//! once can never silently undo each other.
//!
//! Edits address items by id, never by position, so the only real conflict is one person changing
//! an item another has just removed — which the client can see and say so.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};

use crate::db::Db;
use crate::db_playlist_items::{insert_item, item_ids, renumber, NewPlaylistItem};
use crate::db_playlists::bump_revision;
use crate::playlist_access::{EditAccess, PlaylistRole};

/// The most operations one request may carry. A whole playlist's worth of adds fits; anything
/// larger is either a bug or an attempt to hold the database lock.
pub const MAX_EDITS: usize = 500;

#[derive(Clone, Debug)]
pub enum PlaylistEdit {
    /// Inserted after `after_item_id`, or at the end when that is absent.
    Add {
        track: NewPlaylistItem,
        after_item_id: Option<String>,
    },
    Remove {
        item_id: String,
    },
    /// Placed after `after_item_id`, or first when that is absent.
    Move {
        item_id: String,
        after_item_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum EditError {
    NotFound,
    /// The playlist is at `current` now, not at the revision the edit was made against.
    Stale {
        current: i64,
    },
    Forbidden(&'static str),
    Invalid(&'static str),
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for EditError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(error)
    }
}

/// Inserts `id` after `after`, or — when `after` is absent — at the end if `at_end`, else first.
fn place(
    order: &mut Vec<String>,
    id: String,
    after: Option<&str>,
    at_end: bool,
) -> Result<(), EditError> {
    let index = match after {
        Some(anchor) => {
            order
                .iter()
                .position(|it| it == anchor)
                .ok_or(EditError::Invalid("no such item to place after"))?
                + 1
        }
        None if at_end => order.len(),
        None => 0,
    };
    order.insert(index, id);
    Ok(())
}

impl Db {
    /// Applies `edits` to `playlist_id` as `actor`, all or nothing, and answers the new revision.
    pub fn apply_playlist_edits(
        &self,
        playlist_id: &str,
        actor: &str,
        base_revision: i64,
        edits: &[PlaylistEdit],
    ) -> Result<i64, EditError> {
        if edits.is_empty() {
            return Err(EditError::Invalid("no edits given"));
        }
        if edits.len() > MAX_EDITS {
            return Err(EditError::Invalid("too many edits in one request"));
        }
        let playlist = self.get_playlist(playlist_id)?.ok_or(EditError::NotFound)?;
        let role = self.playlist_role(&playlist, actor)?;
        if role == PlaylistRole::None {
            // The same answer as a playlist that does not exist, so ids cannot be probed.
            return Err(EditError::NotFound);
        }
        if !role.can_add() {
            return Err(EditError::Forbidden(
                "this playlist is not open for editing",
            ));
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let current: i64 = tx.query_row(
            "SELECT revision FROM playlists WHERE id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )?;
        if current != base_revision {
            return Err(EditError::Stale { current });
        }

        let mut order = item_ids(&tx, playlist_id)?;
        let mut added_by: HashMap<String, Option<String>> = {
            let mut stmt =
                tx.prepare("SELECT id, added_by FROM playlist_items WHERE playlist_id = ?1")?;
            let rows = stmt.query_map(params![playlist_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        for edit in edits {
            match edit {
                PlaylistEdit::Add {
                    track,
                    after_item_id,
                } => {
                    let item = insert_item(&tx, playlist_id, -1, actor, track)?;
                    added_by.insert(item.id.clone(), Some(actor.to_string()));
                    place(&mut order, item.id, after_item_id.as_deref(), true)?;
                }
                PlaylistEdit::Remove { item_id } => {
                    let owner = added_by
                        .get(item_id)
                        .ok_or(EditError::Invalid("no such item"))?;
                    if !role.can_rearrange() && owner.as_deref() != Some(actor) {
                        return Err(EditError::Forbidden("you may only remove tracks you added"));
                    }
                    tx.execute("DELETE FROM playlist_items WHERE id = ?1", params![item_id])?;
                    order.retain(|it| it != item_id);
                    added_by.remove(item_id);
                }
                PlaylistEdit::Move {
                    item_id,
                    after_item_id,
                } => {
                    if !role.can_rearrange() {
                        return Err(EditError::Forbidden("you may not reorder this playlist"));
                    }
                    if after_item_id.as_deref() == Some(item_id.as_str()) {
                        return Err(EditError::Invalid("an item cannot be placed after itself"));
                    }
                    let from = order
                        .iter()
                        .position(|it| it == item_id)
                        .ok_or(EditError::Invalid("no such item"))?;
                    let id = order.remove(from);
                    place(&mut order, id, after_item_id.as_deref(), false)?;
                }
            }
        }

        renumber(&tx, &order)?;
        let revision = bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(revision)
    }

    /// Renames a playlist or rewrites its description. Owner only, and revision-checked like any
    /// other edit.
    pub fn update_playlist_details(
        &self,
        playlist_id: &str,
        owner: &str,
        base_revision: i64,
        title: &str,
        description: Option<&str>,
    ) -> Result<i64, EditError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let found: Option<(String, i64)> = tx
            .query_row(
                "SELECT user_id, revision FROM playlists WHERE id = ?1",
                params![playlist_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (user_id, current) = found.ok_or(EditError::NotFound)?;
        if user_id != owner {
            return Err(EditError::Forbidden("only the owner may rename a playlist"));
        }
        if current != base_revision {
            return Err(EditError::Stale { current });
        }
        tx.execute(
            "UPDATE playlists SET title = ?1, description = ?2 WHERE id = ?3",
            params![title, description, playlist_id],
        )?;
        let revision = bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(revision)
    }

    /// Sets who besides the owner may edit, narrowed to what the playlist's visibility allows, and
    /// answers what was actually stored. `None` when `owner` does not own such a playlist.
    pub fn update_playlist_edit_access(
        &self,
        playlist_id: &str,
        owner: &str,
        access: EditAccess,
    ) -> rusqlite::Result<Option<EditAccess>> {
        let Some(playlist) = self.get_playlist(playlist_id)? else {
            return Ok(None);
        };
        if playlist.user_id != owner {
            return Ok(None);
        }
        let stored = access.clamped_to(playlist.visibility());
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE playlists SET edit_access = ?1 WHERE id = ?2",
            params![stored.stored(), playlist_id],
        )?;
        bump_revision(&tx, playlist_id)?;
        tx.commit()?;
        Ok(Some(stored))
    }
}
