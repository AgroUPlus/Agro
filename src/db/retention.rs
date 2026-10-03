//! The retention sweep, and how long each kind of row is allowed to live.

use rusqlite::{params, Connection};

use super::Db;

/// How long a play keeps its exact timestamp. Past this, no outbox is still holding it, so
/// deduplication no longer needs the seconds and they are rounded away.
pub(super) const SCROBBLE_EXACT_TIME_DAYS: i64 = 14;

/// How long a device may be quiet before the queue it was playing is scrubbed from its handoff
/// row. Tightened to 2 days to minimize metadata footprint at rest.
pub(super) const HANDOFF_QUEUE_TTL_DAYS: i64 = 2;

/// How long before the handoff row itself goes. A device silent this long has been replaced or
/// wiped, and its row is a record of what someone was listening to and nothing else.
pub(super) const HANDOFF_ROW_TTL_DAYS: i64 = 30;

/// How long a sealed presence copy is kept.
///
/// Deliberately an hour rather than a day: the only reader is the now-playing feed, which already
/// refuses a session older than `NOW_PLAYING_STALE_AFTER_SECS` (five minutes). The margin over that
/// is for clock skew between the server writing `created_at` and the sweep reading it, not for any
/// reader — nothing can open one of these after five minutes, so the rest of the hour is only how
/// long the unreadable bytes are tolerated before being removed.
pub(super) const PRESENCE_CIPHERTEXT_TTL_SECS: i64 = 60 * 60;

/// How long before an inactive registered node is purged from the database.
pub(super) const INACTIVE_NODE_TTL_DAYS: i64 = 90;

impl Db {
    /// Deletes what has expired, and strips the residue from what has gone quiet.
    ///
    /// Nothing in this database was ever swept. `ephemeral_shares`, `friend_codes` and
    /// `short_links` all carry an expiry that was only ever consulted in a `WHERE` clause at read
    /// time, so an expired row stopped being *usable* but never stopped being *readable* — it sat
    /// in the file, and in every backup of it, indefinitely. `friend_codes` even has an index on
    /// `expires_at` that until now nothing used.
    ///
    /// `handoff_state` is the different case. It is keyed `(user_id, device_id)` and overwritten
    /// constantly, so it does not grow; what accumulates is the `queue_json` blob on the row of a
    /// device someone stopped using, which is a snapshot of what they were listening to, kept
    /// forever. Scrubbing the queue is the part that matters, and it is done separately from
    /// deleting the row: an active device's row would only be recreated by its next `updateHandoff`
    /// anyway, so there is no point racing it.
    ///
    /// Returns nothing and takes no arguments because there is nothing a caller could usefully do
    /// with the result. Failures are logged, not propagated: a sweep that cannot run is not a
    /// reason to take the ticker down with it.
    pub fn sweep_retention(&self) {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now();
        let now_rfc = now.to_rfc3339();
        let now_unix = now.timestamp();

        let run =
            |what: &str, sql: &str, args: &[&dyn rusqlite::ToSql]| match conn.execute(sql, args) {
                Ok(n) if n > 0 => tracing::debug!("retention sweep: {what} removed {n} rows"),
                Ok(_) => {}
                Err(e) => tracing::warn!("retention sweep: {what} failed: {e}"),
            };

        run(
            "ephemeral_shares",
            "DELETE FROM ephemeral_shares WHERE expires_at < ?1",
            params![now_rfc],
        );
        run(
            "friend_codes",
            "DELETE FROM friend_codes WHERE expires_at < ?1",
            params![now_rfc],
        );
        // A null `expires_at` is a link that was minted without one, which means it does not
        // expire. Only rows that named a deadline and are past it go.
        run(
            "short_links",
            "DELETE FROM short_links WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![now_unix],
        );

        let stale = (now - chrono::Duration::days(HANDOFF_QUEUE_TTL_DAYS)).to_rfc3339();
        run(
            "handoff queues",
            "UPDATE handoff_state
                SET queue_json = NULL, queue_index = NULL, position_ms = 0
              WHERE updated_at < ?1 AND queue_json IS NOT NULL",
            params![stale],
        );

        let dead = (now - chrono::Duration::days(HANDOFF_ROW_TTL_DAYS)).to_rfc3339();
        run(
            "handoff rows",
            "DELETE FROM handoff_state WHERE updated_at < ?1",
            params![dead],
        );

        // Sealed presence has a far shorter life than the row that carries it. A handoff row is
        // durable on purpose — it is what lets a session be resumed on another device hours later
        // — but a *copy sealed to a friend* is only ever read by the now-playing feed, and that
        // feed refuses anything older than `NOW_PLAYING_STALE_AFTER_SECS` regardless. Past that
        // point the ciphertext cannot be delivered to anyone and is only being stored.
        //
        // The freshness check in `live_now_playing` is what actually withholds a stale session, not
        // this sweep. This runs every fifteen minutes and is retention, not access control: no
        // window it leaves open is a window anything can be read through.
        let cold = (now - chrono::Duration::seconds(PRESENCE_CIPHERTEXT_TTL_SECS)).to_rfc3339();
        run(
            "presence ciphertexts",
            "DELETE FROM handoff_presence_ciphertexts WHERE created_at < ?1",
            params![cold],
        );
        // Copies whose session has been swept out from under them, whatever their own age.
        run(
            "orphaned presence ciphertexts",
            "DELETE FROM handoff_presence_ciphertexts
              WHERE NOT EXISTS (
                    SELECT 1 FROM handoff_state h
                     WHERE h.user_id = handoff_presence_ciphertexts.user_id
                       AND h.device_id = handoff_presence_ciphertexts.device_id)",
            params![],
        );

        // Opt-in, and off by default. A listening history is the point of the product for some
        // people and a liability for others, so the operator chooses — but an upgrade must never
        // silently delete years of it, which is what a default would do.
        if let Some(days) = std::env::var("AGRO_SCROBBLE_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|d| *d > 0)
        {
            let cutoff = (now - chrono::Duration::days(days)).to_rfc3339();
            run(
                "scrobbles",
                "DELETE FROM scrobbles WHERE played_at < ?1",
                params![cutoff],
            );
        }

        let stale_nodes = (now - chrono::Duration::days(INACTIVE_NODE_TTL_DAYS)).to_rfc3339();
        run(
            "inactive nodes",
            "DELETE FROM registered_nodes WHERE last_seen_at < ?1",
            params![stale_nodes],
        );

        self.coarsen_settled_scrobbles(&conn, now);
    }

    /// Rounds down the play times of history old enough that nothing will be re-sent for it.
    ///
    /// Ingest blurs a timestamp only when the client named the play (see `record_scrobbles`), which
    /// leaves two kinds of exact row behind: everything recorded before any of this existed, and
    /// anything still arriving from a client that has not been updated. Both stop being at risk of
    /// a retry once they are old enough — an outbox does not hold a fortnight — so past that point
    /// the seconds can go the same way.
    ///
    /// Done in SQL rather than by reading rows into Rust because the timestamps are RFC3339 in a
    /// fixed-width UTC form and truncation is a string operation on them. Rows whose format does
    /// not match are left alone, exactly as `to_hour` leaves an unparseable value alone.
    fn coarsen_settled_scrobbles(&self, conn: &Connection, now: chrono::DateTime<chrono::Utc>) {
        let cutoff = (now - chrono::Duration::days(SCROBBLE_EXACT_TIME_DAYS)).to_rfc3339();
        // `OR IGNORE` so one collision does not abort the batch: two exact plays of a track in the
        // same hour cannot both round to it under the legacy partial index, and the right outcome
        // is to leave that pair alone and coarsen everything else, not to give up on all of it.
        let sql = "UPDATE OR IGNORE scrobbles
                      SET played_at = substr(played_at, 1, 13) || ':00:00+00:00'
                    WHERE played_at < ?1
                      AND substr(played_at, 14) != ':00:00+00:00'
                      AND length(played_at) >= 19";
        match conn.execute(sql, params![cutoff]) {
            Ok(n) if n > 0 => tracing::debug!("retention sweep: coarsened {n} play times"),
            Ok(_) => {}
            Err(e) => tracing::warn!("retention sweep: coarsening play times failed: {e}"),
        }
    }
}
