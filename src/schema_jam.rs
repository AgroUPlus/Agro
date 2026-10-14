//! The jam session API.
//!
//! Authorization here is membership, not friendship: a jam is joined with a code, and the code is
//! the whole credential. That is deliberate — a jam is a room you invite people into, including
//! people you have not added as friends — but it means the code is the only thing standing between
//! a stranger and the queue, so it is minted rather than chosen.
//!
//! One asymmetry: the creator is the host. They set the mode, may drop a track anyone added, and
//! may end the session. Every other action is equal between members, which is the point of the
//! thing — `democracy` mode exists precisely so the queue is not one person's.

use crate::auth::AuthedUser;
use crate::db::Db;
use async_graphql::{Context, Object};

fn caller<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a AuthedUser> {
    ctx.data::<AuthedUser>()
        .map_err(|_| async_graphql::Error::new("Unauthorized"))
}

fn forbidden(message: &str) -> async_graphql::Error {
    async_graphql::Error::new(format!("Forbidden: {message}"))
}

#[derive(Default)]
pub struct JamQuery;

#[Object]
impl JamQuery {
    /// Jams your friends have opened up, so one can be joined without being handed a code.
    ///
    /// Friendship and the switch, both — the same rule everything else social here follows.
    async fn friend_jams(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<FriendJamPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;

        let mut open = Vec::new();
        for jam in db.friend_jams(authed.username())? {
            let now_playing_title = db.jam_now_playing(&jam)?.map(|now| now.title);
            open.push(FriendJamPayload {
                id: jam.id.clone(),
                host: jam.host.clone(),
                mode: jam.mode.as_str().to_string(),
                members: db.jam_members(&jam.id)?,
                now_playing_title,
            });
        }
        Ok(open)
    }

    /// The jam this account is in, or null. There is at most one.
    async fn jam(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<JamPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::Jams.require(db)?;
        match db.jam_for_member(authed.username())? {
            Some(jam) => Ok(Some(describe(
                db,
                ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?,
                &jam,
                authed.username(),
            )?)),
            None => Ok(None),
        }
    }
}

mod jam_lifecycle;
mod jam_queue;
mod payloads;
pub use jam_lifecycle::JamLifecycleMutation;
pub use jam_queue::JamQueueMutation;
use payloads::announce;
use payloads::current_jam;
use payloads::describe;
pub use payloads::FriendJamPayload;
pub use payloads::JamPayload;
