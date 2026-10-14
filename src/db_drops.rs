//! Songs handed from one account to another.
//!
//! Its own `impl Db` block for the same reason `db_social` and `db_library` have theirs: `db.rs` is
//! already long enough without unrelated SQL in it.
//!
//! A drop is a *message that happens to be about a track*. It therefore stores the track as a
//! description rather than as a reference into the library index — the sender may not have the file
//! on this server at all, and a foreign key that only sometimes resolves would mean an inbox whose
//! rows quietly stop rendering. `content_hash` and `track_uri` ride along when the sender has them,
//! so the recipient can be offered the file rather than only the name, but neither is required.
//!
//! Nothing here is scoped by friendship. That check belongs at the API boundary, where sending
//! happens; once a drop exists it belongs to the person who received it, and withdrawing a
//! friendship does not reach back into their inbox to remove it. Every read and write below is
//! scoped to one account *inside the statement*, so a row belonging to somebody else is a
//! not-found rather than a refusal — the same shape `delete_link` uses.

use rusqlite::{params_from_iter, Result};

/// One sealed copy of a note, and the device key it was sealed to.
///
/// A note is sealed once per device in the recipient's registry *and* once to the sender's own
/// device, which is what makes a sent note readable by the person who sent it. Before this existed
/// a drop carried exactly one ciphertext, sealed to the recipient alone, and the sender's own copy
/// of their message was mathematically unopenable.
#[derive(Clone, Debug)]
pub struct DeviceCiphertext {
    pub device_id: String,
    pub ciphertext: String,
}

/// A drop as it is about to be created. The recipient is passed separately, because it is the one
/// field the caller has to have authorised.
#[derive(Clone, Debug, Default)]
pub struct NewDrop {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    /// Present when the sender's copy is in this server's index. Lets the recipient be offered the
    /// bytes rather than only the name.
    pub content_hash: Option<String>,
    /// The sender's namespaced identifier for the track, e.g. `navidrome:<id>`. Meaningful to a
    /// client that shares the same backend, and inert to one that does not.
    pub track_uri: Option<String>,
    /// What they said about it. Optional — handing someone a song without comment is a complete
    /// thought.
    pub note: Option<String>,
    /// The recipient's copy, duplicated out of [`note_ciphertexts`](Self::note_ciphertexts).
    ///
    /// Kept only for clients that predate the per-device list; nothing new should read it.
    pub note_ciphertext: Option<String>,
    /// One sealed copy per device key, including the sender's own.
    pub note_ciphertexts: Vec<DeviceCiphertext>,
    pub is_encrypted: bool,
}

#[derive(Clone, Debug)]
pub struct Drop {
    pub id: String,
    pub from_user: String,
    pub to_user: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub content_hash: Option<String>,
    pub track_uri: Option<String>,
    pub note: Option<String>,
    /// The recipient's copy. Older clients read this and nothing else.
    pub note_ciphertext: Option<String>,
    /// Every sealed copy, by device. Empty for a drop sent before the registry existed.
    pub note_ciphertexts: Vec<DeviceCiphertext>,
    pub is_encrypted: bool,
    pub created_at: String,
    /// When the recipient first read it, or `None` while it is still unread.
    pub read_at: Option<String>,
    pub archived: bool,
    /// The recipient's one-emoji reply, or `None` if they have not reacted.
    ///
    /// Unlike [`read_at`], this is shown to the sender. A read receipt is something the server
    /// observed; a reaction is something the recipient chose to send, and withholding it would
    /// make the feature pointless.
    pub reaction: Option<String>,
}

const DROP_COLUMNS: &str = "id, from_user, to_user, track_title, artist_name, album_name, \
                            artwork_url, content_hash, track_uri, note, note_ciphertext, \
                            is_encrypted, created_at, read_at, archived, reaction";

fn drop_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Drop> {
    Ok(Drop {
        id: row.get(0)?,
        from_user: row.get(1)?,
        to_user: row.get(2)?,
        track_title: row.get(3)?,
        artist_name: row.get(4)?,
        album_name: row.get(5)?,
        artwork_url: row.get(6)?,
        content_hash: row.get(7)?,
        track_uri: row.get(8)?,
        note: row.get(9)?,
        note_ciphertext: row.get(10)?,
        // Filled by `load_note_ciphertexts`; the column list above does not reach the other table.
        note_ciphertexts: Vec::new(),
        is_encrypted: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
        read_at: row.get(13)?,
        archived: row.get::<_, i64>(14)? != 0,
        reaction: row.get(15)?,
    })
}

/// Attaches each drop's per-device sealed copies.
///
/// A second statement rather than a join: joining would multiply every drop row by the number of
/// devices it was sealed to, and [`drop_from_row`] would then have to collapse what SQL had just
/// duplicated. One extra query per page is the cheaper shape.
fn load_note_ciphertexts(conn: &rusqlite::Connection, drops: &mut [Drop]) -> Result<()> {
    if drops.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; drops.len()].join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT drop_id, device_id, ciphertext FROM drop_note_ciphertexts
          WHERE drop_id IN ({placeholders})
          ORDER BY device_id ASC"
    ))?;
    let sealed = stmt
        .query_map(
            params_from_iter(drops.iter().map(|d| d.id.clone())),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    DeviceCiphertext {
                        device_id: row.get(1)?,
                        ciphertext: row.get(2)?,
                    },
                ))
            },
        )?
        .collect::<Result<Vec<_>>>()?;

    for (drop_id, copy) in sealed {
        if let Some(drop) = drops.iter_mut().find(|d| d.id == drop_id) {
            drop.note_ciphertexts.push(copy);
        }
    }
    Ok(())
}

mod read;
mod send;
