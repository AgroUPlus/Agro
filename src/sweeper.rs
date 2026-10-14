//! Background tasks: the storage sweep, the jam clock and the retention sweep.

use crate::*;

/// Sweeps expired spool entries and abandoned part files. Eviction used to happen only as a
/// side effect of a successful spool write, so on a deployment with a library root it never
/// ran at all.
pub fn spawn(state: &AppState) {
    let db = state.db.clone();
    let ws_hub = state.ws_hub.clone();
    let state = state.clone();
    // The jam clock. Its own ticker rather than a branch of the storage sweep: they run at
    // very different rates, and a jam waiting on a slow disk pass would drift the room.
    {
        let db = db.clone();
        let hub = ws_hub.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(jam_clock::TICK_SECS));
            loop {
                ticker.tick().await;
                jam_clock::tick(&db, &hub);
            }
        });
    }

    // Replay buffers expire on the scale of seconds, not of the storage sweep. Left to the
    // fifteen-minute pass, an account that stopped receiving would hold its last burst of
    // presence frames for that long.
    {
        let hub = ws_hub.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(ws_channels::REPLAY_TTL);
            loop {
                ticker.tick().await;
                hub.channels.sweep();
            }
        });
    }

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(SWEEP_SECS));
        loop {
            ticker.tick().await;
            library::sweep_storage(&state).await;
            // Rides the same ticker rather than taking its own: both are housekeeping at the
            // same rate, and a second timer would only be a second thing to reason about.
            // Before `sweep_retention`, which deletes expired links without a tombstone; this
            // one leaves the ids behind so `/listen` can say the link was deleted.
            match state.db.sweep_idle_short_links() {
                Ok(n) if n > 0 => tracing::debug!("retention sweep: {n} idle short links"),
                Ok(_) => {}
                Err(e) => tracing::warn!("retention sweep: idle short links failed: {e}"),
            }
            state.db.sweep_retention();
            // Separate call rather than a line inside `sweep_retention`, which holds the
            // connection mutex for its whole body — the mutex is not reentrant.
            match state.db.sweep_expired_tokens() {
                Ok(n) if n > 0 => tracing::debug!("retention sweep: {n} expired device tokens"),
                Ok(_) => {}
                Err(e) => tracing::warn!("retention sweep: expired tokens failed: {e}"),
            }
            match state.db.sweep_security_events() {
                Ok(n) if n > 0 => tracing::debug!("retention sweep: {n} security events"),
                Ok(_) => {}
                Err(e) => tracing::warn!("retention sweep: security events failed: {e}"),
            }
        }
    });
}
