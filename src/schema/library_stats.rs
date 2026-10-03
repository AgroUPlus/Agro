//! How big the library is, and how much of the server's disk it takes.

use crate::db::Db;
use async_graphql::{Context, Object, SimpleObject};

use super::library::authorize_library;
use super::{authorize, forbidden};

/// How much of an account's allowance is gone.
///
/// A separate field rather than two more columns on [`LibraryStatsPayload`], because it answers a
/// different question and is computed differently. `total_bytes` counts every archived track in
/// the deployment into every account's total, so it reads the same for a guest holding nothing as
/// for the admin — see `Db::spool_bytes_for`. The quota is enforced against the spool, so that is
/// what is reported here.
///
/// `quota_bytes` is null when the account is uncapped, which is not the same as a quota of zero:
/// the admin owns the disk, and a quota on them is theatre. Clients must show "no limit" for null
/// rather than a full bar.
#[derive(SimpleObject, Clone)]
pub struct StorageUsagePayload {
    pub used_bytes: i64,
    pub quota_bytes: Option<i64>,
}

#[derive(SimpleObject, Clone)]
pub struct LibraryStatsPayload {
    pub track_count: i64,
    pub archived_count: i64,
    pub total_bytes: i64,
    pub spool_bytes: i64,
}

#[derive(Default)]
pub struct LibraryStatsQuery;

#[Object]
impl LibraryStatsQuery {
    /// How much this account's library holds, and how much of it the server has the bytes for.
    async fn library_stats(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<LibraryStatsPayload> {
        let include_archive = authorize_library(ctx, &user_id)?;
        let stats = ctx.data::<Db>()?.library_stats(&user_id, include_archive)?;
        Ok(LibraryStatsPayload {
            track_count: stats.track_count,
            archived_count: stats.archived_count,
            total_bytes: stats.total_bytes,
            spool_bytes: stats.spool_bytes,
        })
    }

    /// What this account has used of its storage allowance.
    ///
    /// Derived from `effective_quota` and `spool_bytes_for` — the same two the upload path checks
    /// against — so the bar a client draws cannot disagree with the answer an upload gets.
    async fn storage_usage(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<StorageUsagePayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let account = db
            .account(user_id.trim())?
            .ok_or_else(|| forbidden("Unauthorized"))?;
        Ok(StorageUsagePayload {
            // Keyed by username, matching what the upload path checks against in `library.rs`.
            used_bytes: db.spool_bytes_for(&account.username)?,
            quota_bytes: account.effective_quota(),
        })
    }
}
