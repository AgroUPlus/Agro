//! Recording plays, and purging them.

use crate::db::Db;
use async_graphql::{Context, InputObject, Object, SimpleObject};

use super::authorize;

/// One play a client is reporting.
#[derive(InputObject)]
pub struct ScrobbleInput {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: i64,
    /// RFC3339, from the device. A phone that was offline is reporting yesterday's listening, and
    /// stamping it on arrival would pile a week of history onto one afternoon.
    ///
    /// Stored to the hour, not the second: see `Db::record_scrobbles`.
    pub played_at: String,
    /// A per-play id from the client, which is what makes ingest idempotent now that the stored
    /// timestamp is too coarse to tell two plays of one track apart. Optional, for clients that
    /// predate it.
    pub play_uid: Option<String>,
}

/// The outcome of actively purging listening history.
#[derive(SimpleObject, Clone)]
pub struct PurgeScrobblesPayload {
    pub purged_count: i32,
    pub success: bool,
}

/// A very stale client outbox should arrive in batches rather than in one request the server has
/// to hold in memory whole.
const MAX_SCROBBLE_BATCH: usize = 500;

#[derive(Default)]
pub struct ScrobblesMutation;

#[Object]
impl ScrobblesMutation {
    /// Ingests a batch of plays from one device.
    ///
    /// Batched because clients hold an outbox and drain it when they next have a connection — a
    /// phone that spent a day on aeroplane mode sends a day of listening in one request. Idempotent
    /// on (account, artist, title, time), so a client unsure whether its last upload landed can
    /// simply send it again. Returns how many rows were genuinely new.
    async fn record_scrobbles(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_name: String,
        client_type: Option<String>,
        entries: Vec<ScrobbleInput>,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        if entries.is_empty() {
            return Ok(0);
        }
        if entries.len() > MAX_SCROBBLE_BATCH {
            return Err(format!("at most {MAX_SCROBBLE_BATCH} plays per request").into());
        }

        let rows: Vec<crate::db::ScrobbleEntry> = entries
            .into_iter()
            .map(|entry| crate::db::ScrobbleEntry {
                track_title: entry.track_title,
                artist_name: entry.artist_name,
                album_name: entry.album_name,
                genre: entry.genre,
                duration_secs: entry.duration_secs.max(0),
                played_at: entry.played_at,
                play_uid: entry.play_uid,
            })
            .collect();

        let db = ctx.data::<Db>()?;
        let inserted =
            db.record_scrobbles(&user_id, &device_name, client_type.as_deref(), &rows)?;
        Ok(inserted as i32)
    }

    /// Purges listening history (scrobbles) for the account, optionally restricted by year or cutoff.
    ///
    /// Useful for wiping past listening data after viewing a Rewind or on demand.
    async fn purge_scrobbles(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        year: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<PurgeScrobblesPayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let count = db.purge_scrobbles(&user_id, year, before.as_deref())?;
        Ok(PurgeScrobblesPayload {
            purged_count: count as i32,
            success: true,
        })
    }
}
