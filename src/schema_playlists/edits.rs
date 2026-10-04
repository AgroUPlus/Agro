//! Revision-checked edits: what collaborators use, and what the owner uses once a playlist is
//! shared. See `db_playlist_edits` for the rules.

use async_graphql::{Context, ErrorExtensions, InputObject, Object, OneofObject};

use crate::db::Db;
use crate::db_playlist_edits::{EditError, PlaylistEdit};
use crate::playlist_access::EditAccess;
use crate::schema::{bounded, caller, forbidden};

use super::notify::announce_change;
use super::payload::{to_playlist_payload, PlaylistPayload, PlaylistTrackInput};

/// The code a client matches on to refetch and retry rather than report a failure.
pub const STALE_REVISION: &str = "STALE_REVISION";

#[derive(InputObject, Clone)]
pub struct AddEditInput {
    pub track: PlaylistTrackInput,
    /// Inserted after this item; at the end when absent.
    pub after_item_id: Option<String>,
}

#[derive(InputObject, Clone)]
pub struct MoveEditInput {
    pub item_id: String,
    /// Placed after this item; first when absent.
    pub after_item_id: Option<String>,
}

/// One change. Exactly one of the fields is given.
#[derive(OneofObject, Clone)]
pub enum PlaylistEditInput {
    Add(AddEditInput),
    Remove(String),
    Move(MoveEditInput),
}

impl PlaylistEditInput {
    fn into_edit(self) -> async_graphql::Result<PlaylistEdit> {
        Ok(match self {
            Self::Add(add) => PlaylistEdit::Add {
                track: add.track.into_item()?,
                after_item_id: add.after_item_id,
            },
            Self::Remove(item_id) => PlaylistEdit::Remove { item_id },
            Self::Move(mv) => PlaylistEdit::Move {
                item_id: mv.item_id,
                after_item_id: mv.after_item_id,
            },
        })
    }
}

fn to_graphql(error: EditError) -> async_graphql::Error {
    match error {
        EditError::NotFound => async_graphql::Error::new("playlist not found"),
        EditError::Stale { current } => {
            async_graphql::Error::new("the playlist has changed since this edit was made")
                .extend_with(|_, ext| {
                    ext.set("code", STALE_REVISION);
                    ext.set("currentRevision", current);
                })
        }
        EditError::Forbidden(detail) => forbidden(detail),
        EditError::Invalid(detail) => async_graphql::Error::new(detail),
        EditError::Db(error) => error.into(),
    }
}

/// Answers the playlist as it now stands, so the client can replace its copy in one round trip.
fn current(
    ctx: &Context<'_>,
    playlist_id: &str,
    viewer: &str,
) -> async_graphql::Result<PlaylistPayload> {
    let db = ctx.data::<Db>()?;
    let playlist = db
        .get_playlist(playlist_id)?
        .ok_or_else(|| async_graphql::Error::new("playlist not found"))?;
    to_playlist_payload(db, playlist, viewer)
}

#[derive(Default)]
pub struct PlaylistEditsMutation;

#[Object]
impl PlaylistEditsMutation {
    /// Applies `edits` in order, all or none, provided the playlist is still at `baseRevision`.
    /// Otherwise fails with `extensions.code = "STALE_REVISION"` and `currentRevision`.
    async fn apply_playlist_edits(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        base_revision: i64,
        edits: Vec<PlaylistEditInput>,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let edits = edits
            .into_iter()
            .map(PlaylistEditInput::into_edit)
            .collect::<async_graphql::Result<Vec<_>>>()?;

        db.apply_playlist_edits(&playlist_id, authed.username(), base_revision, &edits)
            .map_err(to_graphql)?;
        announce_change(ctx, &playlist_id)?;
        current(ctx, &playlist_id, authed.username())
    }

    /// Renames a playlist or rewrites its description. Owner only, revision-checked.
    async fn update_playlist_details(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        base_revision: i64,
        title: String,
        description: Option<String>,
    ) -> async_graphql::Result<PlaylistPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let title = bounded(&title, 255, "title")?;
        if title.is_empty() {
            return Err(async_graphql::Error::new("a playlist needs a title"));
        }
        let description = description
            .map(|d| bounded(&d, 1024, "description"))
            .transpose()?
            .filter(|d| !d.is_empty());

        db.update_playlist_details(
            &playlist_id,
            authed.username(),
            base_revision,
            &title,
            description.as_deref(),
        )
        .map_err(to_graphql)?;
        announce_change(ctx, &playlist_id)?;
        current(ctx, &playlist_id, authed.username())
    }

    /// Sets who besides the owner may edit. Narrowed to what the visibility allows; the answer
    /// says what was actually stored.
    async fn update_playlist_edit_access(
        &self,
        ctx: &Context<'_>,
        playlist_id: String,
        edit_access: EditAccess,
    ) -> async_graphql::Result<EditAccess> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let stored = db
            .update_playlist_edit_access(&playlist_id, authed.username(), edit_access)?
            .ok_or_else(|| forbidden("only the owner may change who can edit a playlist"))?;
        announce_change(ctx, &playlist_id)?;
        Ok(stored)
    }
}
