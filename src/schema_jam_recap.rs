//! Jam recaps over the API, and the one way out of a jam that leaves one behind.
//!
//! A recap is readable only by the member it was written for. There is no id lookup across
//! accounts and no listing for anyone else: it is a memory of a room, not a shared document.

use async_graphql::{Context, Object, SimpleObject};

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::db_jam::Jam;
use crate::jam_recap::JamRecap;
use crate::ws::WsHub;

fn caller<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a AuthedUser> {
    ctx.data::<AuthedUser>()
        .map_err(|_| async_graphql::Error::new("Unauthorized"))
}

#[derive(SimpleObject, Clone)]
pub struct JamRecapPayload {
    pub id: String,
    pub created_at: String,
    pub recap: JamRecap,
}

/// Writes `member`'s recap of `jam`, tells their devices, then takes them out of it.
///
/// Every path out of a jam goes through here — leaving, switching to another jam, and being in the
/// room when its host ends it — because the jam's rows can be gone a moment later and the recap is
/// read from them. A recap that cannot be written is logged and the member still leaves: being
/// stuck in a room is worse than missing its summary, and the leave is what they asked for.
pub(crate) fn depart(db: &Db, hub: &WsHub, jam: &Jam, member: &str) -> rusqlite::Result<()> {
    remember(db, hub, jam, member);
    db.leave_jam(&jam.id, member)?;
    Ok(())
}

/// Writes recaps for everyone still in `jam`, for when it is about to be deleted around them.
pub(crate) fn remember_everyone(db: &Db, hub: &WsHub, jam: &Jam) -> rusqlite::Result<()> {
    for member in db.jam_members(&jam.id)? {
        remember(db, hub, jam, &member);
    }
    Ok(())
}

fn remember(db: &Db, hub: &WsHub, jam: &Jam, member: &str) {
    match db.record_jam_recap(jam, member) {
        Ok(Some(id)) => hub.notify_user(member, "JAM_RECAP", serde_json::json!({ "recapId": id })),
        Ok(None) => {}
        Err(err) => tracing::warn!("jam {}: recap for a member was not written: {err}", jam.id),
    }
}

#[derive(Default)]
pub struct JamRecapQuery;

#[Object]
impl JamRecapQuery {
    /// Your recaps of jams you have left, newest first.
    async fn jam_recaps(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<JamRecapPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;
        Ok(db
            .jam_recaps(authed.username())?
            .into_iter()
            .map(|stored| JamRecapPayload {
                id: stored.id,
                created_at: stored.created_at,
                recap: stored.recap,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct JamRecapMutation;

#[Object]
impl JamRecapMutation {
    /// Forgets one of your recaps. `false` when there was no such recap of yours.
    ///
    /// Not behind the jams switch: an operator turning jams off must not stop anyone deleting
    /// what is already kept about them.
    async fn dismiss_jam_recap(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db.dismiss_jam_recap(authed.username(), &id)?)
    }
}
