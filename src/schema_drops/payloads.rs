//! The drop payload types and their builders.

use crate::db_drops::Drop;
use async_graphql::{InputObject, SimpleObject};

/// One sealed copy of a note, as the client sends and reads it.
#[derive(SimpleObject, InputObject, Clone)]
#[graphql(input_name = "DeviceCiphertextInput")]
pub struct DeviceCiphertextPayload {
    pub device_id: String,
    pub ciphertext: String,
}

#[derive(SimpleObject, Clone)]
pub struct DropPayload {
    pub id: String,
    pub from_user: String,
    pub to_user: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    /// Present when the sender's copy is in this server's index, so a client can offer to fetch the
    /// file rather than only naming it.
    pub content_hash: Option<String>,
    pub track_uri: Option<String>,
    pub note: Option<String>,
    /// The recipient's sealed copy, duplicated out of `noteCiphertexts`.
    ///
    /// Kept for clients that predate the per-device list. A client that understands
    /// `noteCiphertexts` should read that instead — this field cannot carry the sender's own copy.
    pub note_ciphertext: Option<String>,
    /// Every sealed copy of the note, by device — the recipient's devices and the sender's own.
    pub note_ciphertexts: Vec<DeviceCiphertextPayload>,
    pub is_encrypted: bool,
    pub created_at: String,
    /// Null while unread. Only ever populated on the recipient's own view; see `sentDrops`.
    pub read_at: Option<String>,
    pub archived: bool,
    /// The recipient's one-emoji reply, or null if they have not reacted.
    ///
    /// Unlike `read_at`, this *is* shown to the sender. A read receipt is something the server
    /// observed about the recipient; a reaction is something they chose to send back.
    pub reaction: Option<String>,
}

/// A drop as the *recipient* sees it — everything, including whether they have read it.
pub(super) fn to_payload(drop: Drop) -> DropPayload {
    DropPayload {
        id: drop.id,
        from_user: drop.from_user,
        to_user: drop.to_user,
        track_title: drop.track_title,
        artist_name: drop.artist_name,
        album_name: drop.album_name,
        artwork_url: drop.artwork_url,
        content_hash: drop.content_hash,
        track_uri: drop.track_uri,
        note: drop.note,
        note_ciphertext: drop.note_ciphertext,
        note_ciphertexts: drop
            .note_ciphertexts
            .into_iter()
            .map(|copy| DeviceCiphertextPayload {
                device_id: copy.device_id,
                ciphertext: copy.ciphertext,
            })
            .collect(),
        is_encrypted: drop.is_encrypted,
        created_at: drop.created_at,
        read_at: drop.read_at,
        archived: drop.archived,
        reaction: drop.reaction,
    }
}

/// A drop as the *sender* sees it.
///
/// Identical but for `read_at`, which is blanked. Whether somebody has opened what you sent them is
/// information about them, not about you — read receipts are a surveillance feature, and one that
/// nobody consented to by accepting a friend request.
pub(super) fn to_sender_payload(drop: Drop) -> DropPayload {
    DropPayload {
        read_at: None,
        ..to_payload(drop)
    }
}
