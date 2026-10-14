//! Handing a song to a friend.
//!
//! The smallest social act this server supports, and the first one that leaves something behind: a
//! drop outlives the moment it was sent, unlike presence, and outlives the friendship that carried
//! it, unlike listen-along.
//!
//! Authorization is friendship alone — there is no per-surface switch here, and deliberately so.
//! Every other social surface asks "may this person read something about me?", which is a question
//! about disclosure and needs consent. Sending is the other direction: nothing about the recipient
//! is revealed by giving them a song. What a drop can do instead is *pester*, so the limit that
//! matters is a rate limit rather than a visibility flag.
//!
//! Reading is self-only. An inbox is nobody else's business, including the sender's — `sentDrops`
//! shows what you sent, never whether it was read.

use crate::db::Db;
use crate::db_drops::NewDrop;
use crate::schema::{bounded, caller, forbidden, normalise_username};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct DropsQuery;

#[Object]
impl DropsQuery {
    /// What has been sent to the caller. Self-only; there is no argument for whose inbox to read.
    async fn inbox(
        &self,
        ctx: &Context<'_>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> async_graphql::Result<Vec<DropPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let (limit, offset) = page(limit, offset);
        Ok(db
            .inbox(authed.username(), limit, offset)?
            .into_iter()
            .map(to_payload)
            .collect())
    }

    /// What the caller has sent, as their own record of it.
    async fn sent_drops(
        &self,
        ctx: &Context<'_>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> async_graphql::Result<Vec<DropPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let (limit, offset) = page(limit, offset);
        Ok(db
            .sent_drops(authed.username(), limit, offset)?
            .into_iter()
            .map(to_sender_payload)
            .collect())
    }

    /// The whole exchange with one other account, oldest first, both directions in one list.
    ///
    /// This is what makes a conversation rather than a mailbox. `inbox` and `sentDrops` are two
    /// halves of the same exchange split by direction, which is the wrong seam: what someone
    /// wants to see is everything they and one friend have handed each other, in order.
    ///
    /// The sender-side blanking of `read_at` still applies, per message. A thread must not become
    /// a way around the rule that you cannot see whether your own messages have been opened.
    ///
    /// Archived drops are included: archiving clears the inbox, and a record with half of it
    /// missing would be a record that lies.
    async fn conversation(
        &self,
        ctx: &Context<'_>,
        with: String,
        limit: Option<i64>,
    ) -> async_graphql::Result<Vec<DropPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let me = authed.username();
        let (limit, _) = page(limit, None);
        Ok(db
            .conversation(me, &with, limit)?
            .into_iter()
            .map(|drop| {
                if drop.from_user.eq_ignore_ascii_case(me) {
                    to_sender_payload(drop)
                } else {
                    to_payload(drop)
                }
            })
            .collect())
    }

    /// The number a badge shows.
    async fn unread_drop_count(&self, ctx: &Context<'_>) -> async_graphql::Result<i64> {
        let authed = caller(ctx)?;
        Ok(ctx.data::<Db>()?.unread_drop_count(authed.username())?)
    }
}

#[derive(Default)]
pub struct DropsMutation;

#[Object]
impl DropsMutation {
    /// Hands a track to an accepted friend.
    ///
    /// Refuses with the same opaque error the rest of the social layer uses, for the same reason:
    /// "not your friend", "blocked you" and "no such account" must be indistinguishable, or this
    /// mutation becomes the account directory that `discoverable` exists to opt out of.
    #[allow(clippy::too_many_arguments)]
    async fn drop_track(
        &self,
        ctx: &Context<'_>,
        to: String,
        track_title: String,
        artist_name: String,
        album_name: Option<String>,
        artwork_url: Option<String>,
        content_hash: Option<String>,
        track_uri: Option<String>,
        note: Option<String>,
        note_ciphertext: Option<String>,
        note_ciphertexts: Option<Vec<DeviceCiphertextPayload>>,
        is_encrypted: Option<bool>,
    ) -> async_graphql::Result<DropPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let to = normalise_username(&to)?;

        if to.eq_ignore_ascii_case(authed.username()) {
            return Err("You cannot drop a track to yourself".into());
        }
        if !db.are_friends(authed.username(), &to)? {
            return Err(forbidden("no such account, or it is not visible to you"));
        }

        // Checked after the friendship, so the limit cannot be used to probe for accounts: a
        // stranger is refused before this ever runs.
        let window_start =
            (chrono::Utc::now() - chrono::Duration::seconds(RATE_WINDOW_SECS)).to_rfc3339();
        if db.drops_sent_since(authed.username(), &to, &window_start)? >= MAX_DROPS_PER_WINDOW {
            return Err(format!(
                "You have sent {to} rather a lot of music in the last hour. Give them a moment."
            )
            .into());
        }

        let title = bounded(&track_title, MAX_FIELD_LEN, "trackTitle")?;
        if title.is_empty() {
            return Err("A drop needs a track title".into());
        }
        let new = NewDrop {
            track_title: title,
            artist_name: bounded(&artist_name, MAX_FIELD_LEN, "artistName")?,
            album_name: optional(album_name.as_deref(), MAX_FIELD_LEN, "albumName")?,
            artwork_url: optional(artwork_url.as_deref(), MAX_URL_LEN, "artworkUrl")?,
            content_hash: optional(content_hash.as_deref(), MAX_FIELD_LEN, "contentHash")?,
            track_uri: optional(track_uri.as_deref(), MAX_FIELD_LEN, "trackUri")?,
            note: optional(note.as_deref(), MAX_NOTE_LEN, "note")?,
            note_ciphertext: optional(
                note_ciphertext.as_deref(),
                MAX_CIPHERTEXT_LEN,
                "noteCiphertext",
            )?,
            note_ciphertexts: sealed_copies(note_ciphertexts)?,
            is_encrypted: is_encrypted.unwrap_or(false),
        };

        let id = db.create_drop(authed.username(), &to, &new)?;
        let drop = db
            .drop_for(&to, &id)?
            .ok_or_else(|| async_graphql::Error::new("that drop could not be read back"))?;

        // Pushed, but never depended on. The Android client only holds a socket while it is in the
        // foreground, so the notification is an optimisation and the inbox query is the delivery.
        // A failure here must not fail a drop that is already recorded.
        if let Ok(hub) = ctx.data::<std::sync::Arc<crate::ws::WsHub>>() {
            hub.notify_user(
                &to,
                "TRACK_DROP",
                serde_json::json!({
                    "id": drop.id,
                    "from": drop.from_user,
                    "trackTitle": drop.track_title,
                    "artistName": drop.artist_name,
                    "albumName": drop.album_name,
                    "artworkUrl": drop.artwork_url,
                    "contentHash": drop.content_hash,
                    "trackUri": drop.track_uri,
                    "note": drop.note,
                    "noteCiphertext": drop.note_ciphertext,
                    "noteCiphertexts": drop
                        .note_ciphertexts
                        .iter()
                        .map(|copy| serde_json::json!({
                            "deviceId": copy.device_id,
                            "ciphertext": copy.ciphertext,
                        }))
                        .collect::<Vec<_>>(),
                    "isEncrypted": drop.is_encrypted,
                    "createdAt": drop.created_at,
                }),
            );
        }

        Ok(to_sender_payload(drop))
    }

    /// Reacts to a drop the caller received, replacing any reaction already on it.
    ///
    /// Recipient-only, enforced in the statement: reacting to your own message is not a thing, and
    /// allowing it would let a sender fabricate a response to themselves. A null or blank
    /// `emoji` clears the reaction, so tapping the same one twice undoes it.
    ///
    /// The emoji is stored as sent but capped in length. There is no allowlist: which characters
    /// count as "one emoji" is a moving target across Unicode versions and platforms, and a server
    /// that guesses would reject perfectly ordinary ones. The cap is what stops the field being
    /// used as a second, unrated message body.
    async fn react_to_drop(
        &self,
        ctx: &Context<'_>,
        id: String,
        emoji: Option<String>,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let reaction = emoji
            .map(|e| {
                e.trim()
                    .chars()
                    .take(MAX_REACTION_CHARS)
                    .collect::<String>()
            })
            .filter(|e| !e.is_empty());
        Ok(ctx
            .data::<Db>()?
            .react_to_drop(authed.username(), &id, reaction.as_deref())?)
    }

    /// Marks one of the caller's own drops read. A drop addressed to somebody else is a not-found.
    async fn mark_drop_read(&self, ctx: &Context<'_>, id: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        Ok(ctx.data::<Db>()?.mark_drop_read(authed.username(), &id)?)
    }

    /// Takes a drop out of the inbox. The row survives, so the sender's record of having sent it
    /// does too.
    async fn archive_drop(&self, ctx: &Context<'_>, id: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        Ok(ctx.data::<Db>()?.archive_drop(authed.username(), &id)?)
    }
}

mod helpers;

mod payloads;

use helpers::optional;

use helpers::page;

use helpers::sealed_copies;

use payloads::to_payload;

use payloads::to_sender_payload;

pub use payloads::DeviceCiphertextPayload;

pub use payloads::DropPayload;

mod limits;
use limits::MAX_CIPHERTEXT_LEN;
use limits::MAX_DROPS_PER_WINDOW;
use limits::MAX_FIELD_LEN;
use limits::MAX_NOTE_LEN;
use limits::MAX_PAGE;
use limits::MAX_REACTION_CHARS;
use limits::MAX_SEALED_COPIES;
use limits::MAX_URL_LEN;
use limits::RATE_WINDOW_SECS;
