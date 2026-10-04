//! The one refusal every hand edit meets on a playlist Agro writes itself.

use crate::db::Db;
use crate::schema::forbidden;

/// Refuses a hand edit to a generated playlist — a Blend — before anything else is checked.
///
/// One place, called first by every write that changes a playlist's contents or settings, because
/// those writes check ownership in different ways and a blend's owner is a real account: left to
/// the owner checks, they would let its creator edit what Agro is about to overwrite.
pub(super) fn refuse_generated(db: &Db, playlist_id: &str) -> async_graphql::Result<()> {
    match db.get_playlist(playlist_id)? {
        Some(p) if p.is_generated() => Err(forbidden(
            "a Blend is written by Agro from its members' listening and cannot be edited by hand",
        )),
        _ => Ok(()),
    }
}
