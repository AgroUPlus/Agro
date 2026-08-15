//! Telling the other devices that new music arrived.
//!
//! An archive is a per-file event, but "you have new music" is not a per-file message. Uploading a
//! 40-track album fires forty archives in a row, and forty notifications is not a feature — so
//! archives are announced through here, which collects them for a moment and then sends one offer
//! per device however many files landed.
//!
//! The offer itself is recomputed at flush time from [`Db::missing_on_device`] rather than
//! assembled from the batch. That is deliberate: the diff matches on the recording, not the bytes,
//! so a device that already owns a different rip of what just arrived is correctly told nothing.
//! It also means a flush is always a truthful snapshot, even if it fires late.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::time::timeout_at;

use crate::db::Db;
use crate::ws::WsHub;

/// How long an arriving file waits for its neighbours before anyone is told.
///
/// Long enough to swallow an album at upload speed, short enough that a single track still feels
/// immediate.
const DEBOUNCE: Duration = Duration::from_secs(5);

/// Most tracks counted into one offer. The same ceiling the `offerSync` mutation uses — an offer
/// is a prompt, not a manifest.
const MAX_MISSING: i64 = 200;

/// How many album names ride along in the payload, so a client can say what arrived rather than
/// just how much.
const ALBUM_SAMPLE: usize = 5;

#[derive(Clone)]
pub struct OfferBatcher {
    tx: UnboundedSender<String>,
}

impl OfferBatcher {
    /// Starts the collector. One task for the whole process.
    pub fn spawn(db: Db, hub: Arc<WsHub>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(async move {
            // Wait indefinitely for something to happen, then take everything that happens in the
            // next DEBOUNCE before acting on any of it.
            while let Some(first) = rx.recv().await {
                let mut users = HashSet::from([first]);
                let deadline = tokio::time::Instant::from_std(Instant::now() + DEBOUNCE);

                loop {
                    match timeout_at(deadline, rx.recv()).await {
                        Ok(Some(user)) => {
                            users.insert(user);
                        }
                        // Sender gone: flush what is in hand rather than dropping it.
                        Ok(None) => break,
                        // The quiet period elapsed, which is the signal to send.
                        Err(_) => break,
                    }
                }

                for user in users {
                    announce(&db, &hub, &user);
                }
            }
        });

        OfferBatcher { tx }
    }

    /// Records that a file was filed for this account. Never blocks and never fails the caller:
    /// a missed notification must not turn a successful upload into an error.
    pub fn note_archived(&self, user_id: &str) {
        let _ = self.tx.send(user_id.to_string());
    }
}

/// Offers each of the account's devices whatever it is currently missing.
fn announce(db: &Db, hub: &Arc<WsHub>, user_id: &str) {
    let Ok(nodes) = db.get_active_nodes(user_id) else {
        return;
    };

    for node in nodes {
        let Ok(missing) = db.missing_on_device(user_id, &node.device_id, MAX_MISSING) else {
            continue;
        };
        if missing.is_empty() {
            continue;
        }

        // Distinct albums, in the order they appear, so the client can name what arrived.
        let mut albums: Vec<String> = Vec::new();
        for track in &missing {
            if let Some(album) = track.album.as_ref().filter(|a| !a.trim().is_empty()) {
                if !albums.iter().any(|seen| seen == album) {
                    albums.push(album.clone());
                }
            }
        }
        let album_total = albums.len();
        albums.truncate(ALBUM_SAMPLE);

        hub.notify_device(
            user_id,
            &node.device_id,
            "SYNC_OFFER",
            serde_json::json!({
                "count": missing.len(),
                "albums": albums,
                "albumCount": album_total,
                "sample": missing.iter().take(3)
                    .map(|t| format!("{} — {}", t.artist, t.title))
                    .collect::<Vec<_>>(),
            }),
        );
    }
}
