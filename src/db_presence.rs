//! The now-playing session, sealed once per friend device.
//!
//! Its own `impl Db` block for the same reason `db_drops` and `db_social` have theirs: `db.rs` is
//! long enough without unrelated SQL in it.
//!
//! `handoff_state.encrypted_payload` seals a session under a subkey derived from the account's own
//! vault key, so only the account's other devices can open it. The plaintext columns then carry a
//! placeholder — and the social feed reads those columns, so a sealed session appeared to every
//! friend as "Private Session" no matter what `show_now_playing` said. Encrypting a session and
//! being seen by friends were mutually exclusive.
//!
//! A row here is one copy of that same metadata sealed to one friend device's public key, exactly
//! as `drop_note_ciphertexts` seals a note. The server keeps copies it cannot open and hands each
//! viewer only the one addressed to the device asking.
//!
//! Nothing here enforces friendship or visibility. Those gates live at the API boundary, where the
//! reader is known — as they do for drops. What this module does guarantee is that a copy is only
//! ever *returned* for the exact `(recipient, device)` pair that asked for it, so a caller that
//! forgets a gate leaks a ciphertext addressed to somebody else's key rather than plaintext.

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;

/// One sealed copy of a session, and the device it was sealed to.
///
/// `recipient_user_id` is carried alongside the device id because device ids are chosen by the
/// client: two accounts can pick the same one, and the pair is what identifies a key.
#[derive(Clone, Debug)]
pub struct PresenceCiphertext {
    pub recipient_user_id: String,
    pub recipient_device_id: String,
    pub ciphertext: String,
}

impl Db {
    /// Replaces the sealed copies published by one device of one account.
    ///
    /// Wholesale replacement rather than a merge: the copies describe one track, and a set left
    /// half-updated would hand some friends the previous song. Callers that mean "leave these
    /// alone" must not call this at all — see `update_handoff`, where a heartbeat passes `None`.
    pub fn replace_presence_ciphertexts(
        &self,
        user_id: &str,
        device_id: &str,
        copies: &[PresenceCiphertext],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        Self::replace_presence_ciphertexts_in(&conn, user_id, device_id, copies)
    }

    /// The same, on a connection the caller already holds.
    ///
    /// `update_handoff` writes the row and its sealed copies under one lock: a session whose copies
    /// did not land is one no friend can open, which is worse than one that was never published.
    pub(crate) fn replace_presence_ciphertexts_in(
        conn: &rusqlite::Connection,
        user_id: &str,
        device_id: &str,
        copies: &[PresenceCiphertext],
    ) -> Result<()> {
        conn.execute(
            "DELETE FROM handoff_presence_ciphertexts WHERE user_id = ?1 AND device_id = ?2",
            params![user_id, device_id],
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        for copy in copies {
            conn.execute(
                "INSERT OR REPLACE INTO handoff_presence_ciphertexts
                     (user_id, device_id, recipient_user_id, recipient_device_id, ciphertext, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    user_id,
                    device_id,
                    copy.recipient_user_id.trim(),
                    copy.recipient_device_id.trim(),
                    copy.ciphertext.trim(),
                    now,
                ],
            )?;
        }
        Ok(())
    }

    /// The copy `viewer` may open on `viewer_device`, for the session `owner` is publishing.
    ///
    /// Scoped to the asking pair inside the statement, so there is no shape of this call that
    /// returns somebody else's copy. `None` when the owner sealed nothing, or sealed nothing to
    /// this particular device — a friend who has published no key, or has just added a device that
    /// no track change has been sealed to yet.
    pub fn presence_ciphertext_for(
        &self,
        owner: &str,
        owner_device: &str,
        viewer: &str,
        viewer_device: &str,
    ) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT ciphertext
               FROM handoff_presence_ciphertexts
              WHERE user_id = ?1 AND device_id = ?2
                AND recipient_user_id = ?3 AND recipient_device_id = ?4",
            params![owner, owner_device, viewer, viewer_device],
            |row| row.get(0),
        )
        .optional()
    }

    /// Every copy an account has published to one recipient, whichever device published it.
    ///
    /// The presence fan-out sends one frame per friend, and a friend may be reachable on several
    /// devices at once; this is what lets one query answer for all of them.
    pub fn presence_ciphertexts_to(
        &self,
        owner: &str,
        recipient: &str,
    ) -> Result<Vec<PresenceCiphertext>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT recipient_user_id, recipient_device_id, ciphertext
               FROM handoff_presence_ciphertexts
              WHERE user_id = ?1 AND recipient_user_id = ?2
              ORDER BY recipient_device_id ASC",
        )?;
        let rows = stmt.query_map(params![owner, recipient], |row| {
            Ok(PresenceCiphertext {
                recipient_user_id: row.get(0)?,
                recipient_device_id: row.get(1)?,
                ciphertext: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Drops every copy either account published to the other.
    ///
    /// Called when a friendship ends. `fan_out_presence` re-checks friendship before it sends, so
    /// this is not what stops delivery — it is what stops the ciphertext being *kept* by a server
    /// that has no remaining reason to hold it.
    ///
    /// Takes the connection rather than the `Db`, because both callers are already inside the lock
    /// that ends the friendship: the copies and the edge that justified them go together or not
    /// at all.
    pub(crate) fn forget_presence_between_in(
        conn: &rusqlite::Connection,
        one: &str,
        other: &str,
    ) -> Result<usize> {
        conn.execute(
            "DELETE FROM handoff_presence_ciphertexts
              WHERE (user_id = ?1 COLLATE NOCASE AND recipient_user_id = ?2 COLLATE NOCASE)
                 OR (user_id = ?2 COLLATE NOCASE AND recipient_user_id = ?1 COLLATE NOCASE)",
            params![one.trim(), other.trim()],
        )
    }

    /// Drops every copy sealed to one device key, in either direction.
    ///
    /// Called when a device withdraws its key. A copy sealed to a key nobody holds can never be
    /// opened again, so keeping it stores an unreadable secret for no one's benefit.
    pub(crate) fn forget_presence_for_device_in(
        conn: &rusqlite::Connection,
        user_id: &str,
        device_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "DELETE FROM handoff_presence_ciphertexts
              WHERE (recipient_user_id = ?1 COLLATE NOCASE AND recipient_device_id = ?2)
                 OR (user_id = ?1 COLLATE NOCASE AND device_id = ?2)",
            params![user_id.trim(), device_id.trim()],
        )
    }

}
