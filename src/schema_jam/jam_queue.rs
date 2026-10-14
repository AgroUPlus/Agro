//! The jam queue: adding, approving, removing and skipping.

use super::*;
use crate::db::Db;
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct JamQueueMutation;

#[Object]
impl JamQueueMutation {
    /// Votes to skip whatever is playing.
    ///
    /// One vote per person per track, and the votes die with the track — a skip is about *this*
    /// song, not a standing objection. Once more than half the room has asked, the track is retired
    /// immediately rather than at the end of its duration, and the clock takes the next one.
    async fn vote_skip_jam_track(&self, ctx: &Context<'_>) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        let Some(track_id) = jam.now_playing_id.clone() else {
            return Err("Nothing is playing to skip".into());
        };

        if db.vote_skip(&jam.id, &track_id, &me)? {
            // The room has had enough. Retired here rather than left to the clock, which would
            // keep playing it until its duration was up — a skip that takes effect in two minutes
            // is not a skip.
            db.mark_jam_track_played(&jam.id, &track_id)?;
            db.clear_jam_now_playing(&jam.id)?;
        }
        let updated = db.jam_by_id(&jam.id)?.unwrap_or(jam);
        announce(ctx, db, &updated);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &updated,
            &me,
        )
    }

    /// Suggests a track.
    ///
    /// In `open` mode it joins the queue immediately. In `democracy` it waits as a proposal until a
    /// majority of the *other* members accept it — which is what voting is for here. It sorts
    /// nothing: a track that has been accepted takes its place by when it was added, like any
    /// other.
    ///
    /// `durationMs` matters more than it looks: the server advances the room on that number, so a
    /// track without one would be skipped past the moment it started.
    #[allow(clippy::too_many_arguments)]
    async fn add_jam_track(
        &self,
        ctx: &Context<'_>,
        track_uri: String,
        title: String,
        artist: String,
        artwork_url: Option<String>,
        duration_ms: Option<i64>,
        // `is_live` marks a stream with no end: it holds the room until someone skips it, rather
        // than being retired by a clock that has no way to know a broadcast is over.
        is_live: Option<bool>,
        // `device_id` and `content_hash` are only meaningful together: they are what lets another
        // member play the queueing member's own copy instead of hunting for the track by name.
        // Omitted for anything queued from a streaming source, which is most of a queue.
        device_id: Option<String>,
        content_hash: Option<String>,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        let title = title.trim();
        if title.is_empty() {
            return Err("A track needs a title".into());
        }
        db.add_jam_track(
            &jam.id,
            &me,
            track_uri.trim(),
            title,
            artist.trim(),
            artwork_url.as_deref(),
            duration_ms.unwrap_or(0),
            is_live.unwrap_or(false),
            jam.mode,
            device_id.as_deref(),
            content_hash.as_deref(),
        )?;
        announce(ctx, db, &jam);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &jam,
            &me,
        )
    }

    /// Accepts somebody's suggestion.
    ///
    /// One-way, unlike the toggle this replaces. Taking an approval back could drop a track out of
    /// the queue after the room had already accepted it — possibly while it was playing — so
    /// somebody who changes their mind removes the track rather than un-approving it.
    ///
    /// Approving your own suggestion is accepted and recorded but never counts. The rule lives here
    /// rather than in each client's decision about whether to show a button.
    async fn approve_jam_track(
        &self,
        ctx: &Context<'_>,
        track_id: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        if db.jam_track_owner(&jam.id, &track_id)?.is_none() {
            return Err(forbidden("that track is not in this jam"));
        }
        db.approve_jam_track(&jam.id, &track_id, &me)?;
        announce(ctx, db, &jam);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &jam,
            &me,
        )
    }

    /// Removes a track. Your own, or anyone's if you are the host.
    async fn remove_jam_track(
        &self,
        ctx: &Context<'_>,
        track_id: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        let owner = db
            .jam_track_owner(&jam.id, &track_id)?
            .ok_or_else(|| forbidden("that track is not in this jam"))?;
        let is_host = jam.host.eq_ignore_ascii_case(&me);
        if !is_host && !owner.eq_ignore_ascii_case(&me) {
            return Err(forbidden("only the host can remove someone else's track"));
        }
        db.remove_jam_track(&jam.id, &track_id)?;
        announce(ctx, db, &jam);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &jam,
            &me,
        )
    }

    // `advanceJam` used to live here. It is gone: the server advances the room on its own clock
    // (see `jam_clock.rs`), which is the only way every device can be hearing the same thing at the
    // same moment. Letting clients advance made whoever finished first decide for everybody, and a
    // client comparing the wrong two ids once drained an entire queue in a single pass.
}
