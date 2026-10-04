//! Deleting an account: everything it owns, and every mention of it in what others own.
//!
//! Two different jobs. What belongs to the account — its history, devices, playlists, drops — is
//! deleted outright. What belongs to *someone else* but names it — a track it added to a friend's
//! playlist, a jam recap a friend kept — stays, with the name taken out. "Deleted" has to mean
//! nothing on this server can still say who the person was.
//!
//! All of it runs in one transaction: a deletion that fails halfway must not leave an account
//! half-erased, and one commit is also the cheap way to do a few dozen writes in SQLite.

use rusqlite::{params, OptionalExtension, Result, Transaction};

use super::Db;

/// Tables keyed on `users.id`.
const OWNED_BY_ID: &[&str] = &[
    "app_passwords",
    "totp_recovery_codes",
    "federated_identities",
];

/// Tables whose `user_id` column holds the username.
const OWNED_BY_USERNAME: &[&str] = &[
    "registered_nodes",
    "device_holdings",
    "handoff_state",
    "synced_settings",
    "scrobbles",
    "friend_codes",
    "ephemeral_shares",
    "short_links",
    "spool_items",
    "upload_sessions",
    "playlist_followers",
];

impl Db {
    /// Removes an account, everything that belongs to it, and its name from everything that does
    /// not. Deliberately thorough: leaving a user's nodes, session and settings behind would let a
    /// recreated account inherit them, and leaving their name behind would remember them anyway.
    pub fn delete_user(&self, username: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let user_id: Option<String> = tx
            .query_row(
                "SELECT id FROM users WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )
            .optional()?;
        let Some(user_id) = user_id else {
            return Ok(false);
        };

        erase_owned(&tx, &user_id, username)?;
        erase_social(&tx, username)?;
        erase_jams(&tx, username)?;
        scrub_mentions(&tx, username)?;

        // The audit trail is the one thing kept, and only in a form that names nobody: "an account
        // was deleted" is a fact the operator needs, and rows still carrying the username would be
        // a record of the person who asked to be forgotten.
        tx.execute(
            "UPDATE security_events SET user_id = NULL, client_ip = NULL, device_label = NULL
              WHERE user_id = ?1",
            params![username],
        )?;
        tx.execute("DELETE FROM users WHERE id = ?1", params![user_id])?;
        tx.commit()?;
        Ok(true)
    }
}

/// Every table that stores a username or a user id, or this is not a deletion.
///
/// It used to be five of them. What survived was the whole social graph — friendships in both
/// directions, drops sent and received, jam membership and votes — plus every scrobble, which is a
/// listening history, and every live share link, which kept working after the account that minted
/// it was gone. Later it was playlists, artist follows and invites, for the same reason: a table
/// added after the list was written. Foreign keys are not enforced here, so no `ON DELETE CASCADE`
/// in a schema does anything — every row has to be named.
fn erase_owned(tx: &Transaction<'_>, user_id: &str, username: &str) -> Result<()> {
    for table in OWNED_BY_ID {
        tx.execute(
            &format!("DELETE FROM {table} WHERE user_id = ?1"),
            params![user_id],
        )?;
    }
    for table in OWNED_BY_USERNAME {
        tx.execute(
            &format!("DELETE FROM {table} WHERE user_id = ?1"),
            params![username],
        )?;
    }
    // Keyed by whichever of the two the code that wrote them had to hand; both are this account.
    tx.execute(
        "DELETE FROM artist_subscriptions WHERE user_id IN (?1, ?2)",
        params![username, user_id],
    )?;
    tx.execute(
        "DELETE FROM invites WHERE created_by IN (?1, ?2)",
        params![username, user_id],
    )?;

    // Their playlists, with the tracks in them, everyone else's follows of them and, for a blend
    // they made, its recipe and members. The children go first: once the playlist row is gone
    // there is nothing left to find them by.
    let owned = "SELECT id FROM playlists WHERE user_id = ?1";
    for table in [
        "playlist_items",
        "playlist_followers",
        "blends",
        "blend_members",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE playlist_id IN ({owned})"),
            params![username],
        )?;
    }
    // Blends of other people's they were in are rewritten without them on the next read: what
    // their listening put there is theirs, even though it names nobody.
    tx.execute(
        "UPDATE blends SET refreshed_at = NULL
          WHERE playlist_id IN (SELECT playlist_id FROM blend_members WHERE username = ?1)",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM blend_members WHERE username = ?1 COLLATE NOCASE",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM playlists WHERE user_id = ?1",
        params![username],
    )?;
    Ok(())
}

/// Friendships, drops, keys, presence and listen-along: each addressed between two accounts, so
/// both directions go.
fn erase_social(tx: &Transaction<'_>, username: &str) -> Result<()> {
    // Friendship is two rows, one per direction. Removing only the row this account owns leaves
    // the other person still holding a friendship with somebody who no longer exists.
    tx.execute(
        "DELETE FROM friendships WHERE user_id = ?1 OR friend_id = ?1",
        params![username],
    )?;
    // Sealed copies are keyed by drop, not by account, so they have to go *before* the drops they
    // hang off — once the `track_drops` row is gone there is nothing left to find them by.
    tx.execute(
        "DELETE FROM drop_note_ciphertexts
          WHERE drop_id IN (SELECT id FROM track_drops WHERE from_user = ?1 OR to_user = ?1)",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM track_drops WHERE from_user = ?1 OR to_user = ?1",
        params![username],
    )?;
    // Left behind, a recreated account would inherit the public keys of the old one's devices,
    // and senders fetching the registry would seal to keys nobody holds.
    tx.execute(
        "DELETE FROM user_device_keys WHERE user_id = ?1 COLLATE NOCASE",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM handoff_presence_ciphertexts WHERE user_id = ?1 OR recipient_user_id = ?1",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM listen_along WHERE listener_id = ?1 OR host_id = ?1",
        params![username],
    )?;
    Ok(())
}

/// Their part in other people's jams, every jam they host in full, and their own recaps.
fn erase_jams(tx: &Transaction<'_>, username: &str) -> Result<()> {
    // A hosted jam goes whole. Deleting only the `jams` row, as this once did, left its members,
    // queue and votes behind with nothing to belong to.
    let hosted = "SELECT id FROM jams WHERE host = ?1";
    for table in ["jam_members", "jam_votes", "jam_skips", "jam_tracks"] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE jam_id IN ({hosted})"),
            params![username],
        )?;
    }
    tx.execute("DELETE FROM jams WHERE host = ?1", params![username])?;

    tx.execute(
        "DELETE FROM jam_members WHERE username = ?1",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM jam_votes WHERE username = ?1",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM jam_skips WHERE username = ?1",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM jam_tracks WHERE added_by = ?1",
        params![username],
    )?;
    tx.execute(
        "DELETE FROM jam_recaps WHERE username = ?1 COLLATE NOCASE",
        params![username],
    )?;
    Ok(())
}

/// The name, taken out of things that belong to other people.
fn scrub_mentions(tx: &Transaction<'_>, username: &str) -> Result<()> {
    // The track stays in the friend's playlist — it is theirs now — but not who put it there.
    tx.execute(
        "UPDATE playlist_items SET added_by = NULL WHERE added_by = ?1 COLLATE NOCASE",
        params![username],
    )?;
    crate::db_jam_recap::forget_in_jam_recaps(tx, username)
}
