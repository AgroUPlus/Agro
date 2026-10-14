//! Creating, joining, leaving and configuring jams.

use super::*;
use crate::db::Db;
use crate::db_jam::{JamMode, JamVisibility};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct JamLifecycleMutation;

#[Object]
impl JamLifecycleMutation {
    /// Opens a jam with this account as its host.
    ///
    /// Leaves whatever jam you were in first: being in two at once would make "the queue" and
    /// "your vote" ambiguous, and there is no interface in which that reads well.
    async fn create_jam(
        &self,
        ctx: &Context<'_>,
        mode: Option<String>,
    ) -> async_graphql::Result<JamPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        if let Some(existing) = db.jam_for_member(authed.username())? {
            crate::schema_jam_recap::depart(
                db,
                ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
                &existing,
                authed.username(),
            )?;
        }

        let mode = mode
            .as_deref()
            .map(JamMode::parse)
            .unwrap_or(JamMode::Democracy);
        let jam = db.create_jam(authed.username(), mode)?;
        announce(ctx, db, &jam);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &jam,
            authed.username(),
        )
    }

    /// Joins a jam by its code.
    async fn join_jam(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<JamPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        // The same words for a wrong code and an ended one. A code is a credential, and telling
        // the difference apart is how you find out which codes exist.
        let jam = db
            .jam_by_code(&code)?
            .ok_or_else(|| forbidden("no jam is open with that code"))?;

        if let Some(existing) = db.jam_for_member(authed.username())? {
            if existing.id != jam.id {
                crate::schema_jam_recap::depart(
                    db,
                    ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
                    &existing,
                    authed.username(),
                )?;
            }
        }
        db.join_jam(&jam.id, authed.username())?;
        announce(ctx, db, &jam);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &jam,
            authed.username(),
        )
    }

    /// Joins a friend's jam by its id, with no code.
    ///
    /// The code is not bypassed so much as unnecessary: the jam has been opened to friends, and
    /// this checks that you are one. A jam that has *not* been opened is refused in the same words
    /// as one that does not exist, so this cannot be used to discover rooms.
    async fn join_friend_jam(
        &self,
        ctx: &Context<'_>,
        jam_id: String,
    ) -> async_graphql::Result<JamPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        let jam = db
            .friend_jams(authed.username())?
            .into_iter()
            .find(|jam| jam.id == jam_id.trim())
            .ok_or_else(|| forbidden("no jam is open to you with that id"))?;

        if let Some(existing) = db.jam_for_member(authed.username())? {
            if existing.id != jam.id {
                crate::schema_jam_recap::depart(
                    db,
                    ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
                    &existing,
                    authed.username(),
                )?;
            }
        }
        db.join_jam(&jam.id, authed.username())?;
        announce(ctx, db, &jam);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &jam,
            authed.username(),
        )
    }

    /// Opens the jam to friends, or shuts it back to code-only. The creator decides.
    async fn set_jam_visibility(
        &self,
        ctx: &Context<'_>,
        visibility: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;
        if !jam.host.eq_ignore_ascii_case(&me) {
            return Err(forbidden("only the creator can open the jam up"));
        }
        db.set_jam_visibility(&jam.id, JamVisibility::parse(&visibility))?;
        let updated = db.jam_by_id(&jam.id)?.unwrap_or(jam);
        announce(ctx, db, &updated);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &updated,
            &me,
        )
    }

    /// Leaves the jam, and deletes it once nobody is left.
    ///
    /// The creator leaving ends it for everyone — a room with no one to govern it cannot change
    /// mode or be wound up — and the rows go with it. A jam is a room, not a document: keeping a
    /// dead one means every client has to keep deciding whether it still counts.
    async fn leave_jam(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        let hub = ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?;
        let creator_left = jam.host.eq_ignore_ascii_case(&me);
        if creator_left {
            // The room ends for everyone, so everyone's recap is written now, while the creator
            // is still in the member list. Written after they had gone, the others' recaps would
            // be missing the person whose room it was.
            crate::schema_jam_recap::remember_everyone(db, hub, &jam)?;
            db.leave_jam(&jam.id, &me)?;
        } else {
            crate::schema_jam_recap::depart(db, hub, &jam, &me)?;
        }
        let remaining = db.jam_members(&jam.id)?;

        // Told before it is deleted, or there is nobody left to tell.
        announce(ctx, db, &jam);
        if creator_left || remaining.is_empty() {
            db.delete_jam(&jam.id)?;
        }
        Ok(true)
    }

    /// Switches between `open` and `democracy`. Host only — it is the rule everyone else plays by.
    async fn set_jam_mode(
        &self,
        ctx: &Context<'_>,
        mode: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;
        if !jam.host.eq_ignore_ascii_case(&me) {
            return Err(forbidden("only the host can change the mode"));
        }
        db.set_jam_mode(&jam.id, JamMode::parse(&mode))?;
        let updated = db.jam_by_id(&jam.id)?.unwrap_or(jam);
        announce(ctx, db, &updated);
        describe(
            db,
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
            &updated,
            &me,
        )
    }
}
