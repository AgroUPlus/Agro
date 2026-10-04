//! Browsing the library index, and the admin operations on it.

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::db_identity::Role;
use crate::db_library::BrowseKind;
use async_graphql::{Context, Enum, Object, SimpleObject};

use super::{forbidden, require_admin, require_own_device};

#[derive(SimpleObject, Clone)]
/// What one call to `reindexNormalisation` rewrote.
pub struct ReindexPayload {
    /// `library_tracks` rows brought up to date this call.
    pub tracks: i32,
    /// `playlist_items` rows brought up to date this call.
    pub playlist_items: i32,
    /// False means stale rows remain — call again with the same batch size.
    pub done: bool,
}

/// One tile in the library view.
#[derive(SimpleObject, Clone)]
pub struct LibraryItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// Fetch artwork from `/api/v1/cover/{coverKey}`. Null for artists, and for albums with none.
    pub cover_key: Option<String>,
    pub track_count: i64,
    /// False when the selected device is missing this. Null-ish only in the sense that with no
    /// device selected everything reports true — there is nothing to be missing from.
    pub present_on_device: bool,
    pub source_count: i64,
}

/// What `libraryBrowse` is listing.
#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum LibraryBrowseKind {
    Artist,
    Album,
    Track,
}

/// Who may look at `subject`'s library, and whether the server's archive counts as part of it.
///
/// Three answers, in order:
///   * your own library — always, and for an administrator that includes the server archive,
///     which belongs to whoever runs the instance;
///   * an accepted friend's, but only the tracks their devices actually hold, and only when they
///     have turned `shareLibrary` on;
///   * otherwise nothing.
///
/// Being an administrator deliberately does *not* grant a view of somebody else's library. Running
/// the server is a reason to see the server's own archive, not a reason to read the collections of
/// the people using it.
pub(super) fn authorize_library(ctx: &Context<'_>, subject: &str) -> async_graphql::Result<bool> {
    let caller = ctx
        .data::<AuthedUser>()
        .map_err(|_| forbidden("Unauthorized"))?;
    let db = ctx.data::<Db>()?;
    let subject = subject.trim();

    if caller.account.username.eq_ignore_ascii_case(subject) {
        // The archive is the operator's own copy of the fleet's music.
        return Ok(caller.account.role == Role::Admin);
    }

    let shared = db
        .profile(subject)?
        .map(|profile| profile.share_library)
        .unwrap_or(false);
    if shared && db.are_friends(&caller.account.username, subject)? {
        return Ok(false);
    }
    Err(forbidden("that library is not shared with you"))
}

#[derive(Default)]
pub struct LibraryQuery;

#[Object]
impl LibraryQuery {
    /// The account's library, page by page, for looking at rather than syncing.
    ///
    /// `deviceId` picks whose shelf is being compared against: every item comes back with
    /// `presentOnDevice`, which is what lets the view grey out what that device is missing. Omit it
    /// and everything reads as present, because there is nothing to be missing from.
    #[allow(clippy::too_many_arguments)]
    async fn library_browse(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        kind: LibraryBrowseKind,
        device_id: Option<String>,
        search: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> async_graphql::Result<Vec<LibraryItem>> {
        let include_archive = authorize_library(ctx, &user_id)?;
        if let Some(device) = device_id.as_deref() {
            require_own_device(ctx, device)?;
        }
        let db = ctx.data::<Db>()?;
        let kind = match kind {
            LibraryBrowseKind::Artist => BrowseKind::Artist,
            LibraryBrowseKind::Album => BrowseKind::Album,
            LibraryBrowseKind::Track => BrowseKind::Track,
        };
        // Capped rather than trusted: a page size is a hint from a caller, and an uncapped one is
        // a request to load somebody's whole library into memory.
        let limit = limit.unwrap_or(120).clamp(1, 500);
        let offset = offset.unwrap_or(0).max(0);
        let search = search
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(db
            .library_browse(
                &user_id,
                device_id.as_deref().filter(|d| !d.is_empty()),
                kind,
                search.as_deref(),
                limit,
                offset,
                include_archive,
            )?
            .into_iter()
            .map(|item| LibraryItem {
                id: item.id,
                title: item.title,
                subtitle: item.subtitle,
                cover_key: item.cover_key,
                track_count: item.track_count,
                present_on_device: item.present_on_device,
                source_count: item.source_count,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct LibraryMutation;

#[Object]
impl LibraryMutation {
    /// Removes a track from the library entirely. It will disappear from all views.
    async fn delete_library_item(
        &self,
        ctx: &Context<'_>,
        _user_id: String,
        kind: LibraryBrowseKind,
        id: String,
    ) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        let db = ctx.data::<crate::db::Db>()?;
        let db_kind = match kind {
            LibraryBrowseKind::Artist => crate::db_library::BrowseKind::Artist,
            LibraryBrowseKind::Album => crate::db_library::BrowseKind::Album,
            LibraryBrowseKind::Track => crate::db_library::BrowseKind::Track,
        };
        Ok(db.delete_library_item(db_kind, &id)?)
    }

    /// Recomputes the normalised matching columns from the metadata already stored.
    ///
    /// The columns are derived in Rust at insert time, so changing `norm.rs` leaves every existing
    /// row on the old convention and no amount of SQL can repair them. This is the entry point that
    /// can — run it after any deploy that touches normalisation.
    ///
    /// Admin-only, and not because the data is sensitive: it is a write across the whole index, and
    /// nobody should be able to schedule that from an ordinary account. It reads nothing about who
    /// listened to what — these columns are facts about titles.
    ///
    /// Bounded per call. Keep calling while `done` is false.
    async fn reindex_normalisation(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 500)] batch: i32,
    ) -> async_graphql::Result<ReindexPayload> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let batch = batch.clamp(1, 10_000) as usize;
        let outcome = db.reindex_normalisation(batch)?;
        Ok(ReindexPayload {
            tracks: outcome.tracks as i32,
            playlist_items: outcome.playlist_items as i32,
            done: outcome.done,
        })
    }
}
