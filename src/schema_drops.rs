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

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;
use crate::db_drops::{Drop, NewDrop};
use crate::schema::{bounded, caller, forbidden, normalise_username};

/// The longest note that may ride along with a drop.
///
/// A comment, not an essay. Long enough for the reason you sent it, short enough that an inbox row
/// stays a row.
const MAX_NOTE_LEN: usize = 500;

/// The longest any piece of track description may be. Titles are attacker-controlled here in a way
/// they are not elsewhere: this is text one account writes into another account's inbox.
const MAX_FIELD_LEN: usize = 512;
const MAX_URL_LEN: usize = 2048;

/// Most drops one account may send another inside [`RATE_WINDOW_SECS`].
///
/// Generous for anybody using the feature as intended — handing over an album a track at a time is
/// still comfortably inside it — and low enough that an inbox cannot be buried. The limit is per
/// *pair*, not per sender, so one tiresome friend cannot exhaust the quota you have for everyone
/// else, and blocking remains the answer to somebody who keeps hitting it.
const MAX_DROPS_PER_WINDOW: i64 = 20;
const RATE_WINDOW_SECS: i64 = 3600;

/// How many drops may be asked for at once.
const MAX_PAGE: i64 = 100;

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
    pub created_at: String,
    /// Null while unread. Only ever populated on the recipient's own view; see `sentDrops`.
    pub read_at: Option<String>,
    pub archived: bool,
}

/// A drop as the *recipient* sees it — everything, including whether they have read it.
fn to_payload(drop: Drop) -> DropPayload {
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
        created_at: drop.created_at,
        read_at: drop.read_at,
        archived: drop.archived,
    }
}

/// A drop as the *sender* sees it.
///
/// Identical but for `read_at`, which is blanked. Whether somebody has opened what you sent them is
/// information about them, not about you — read receipts are a surveillance feature, and one that
/// nobody consented to by accepting a friend request.
fn to_sender_payload(drop: Drop) -> DropPayload {
    DropPayload {
        read_at: None,
        ..to_payload(drop)
    }
}

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
        let window_start = (chrono::Utc::now() - chrono::Duration::seconds(RATE_WINDOW_SECS))
            .to_rfc3339();
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
                    "createdAt": drop.created_at,
                }),
            );
        }

        Ok(to_sender_payload(drop))
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

/// Clamps a page request. A page size is a hint from a caller, not an instruction.
fn page(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    (
        limit.unwrap_or(50).clamp(1, MAX_PAGE),
        offset.unwrap_or(0).max(0),
    )
}

/// [`bounded`], but for a field that may legitimately be absent. An empty string is treated as
/// absent rather than stored, so a client that sends `""` for "no album" does not produce a row
/// that renders as a blank album name.
fn optional(raw: Option<&str>, max: usize, field: &str) -> async_graphql::Result<Option<String>> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let clean = bounded(value, max, field)?;
            Ok(if clean.is_empty() { None } else { Some(clean) })
        }
    }
}
