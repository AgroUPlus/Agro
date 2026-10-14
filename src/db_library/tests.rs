use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use crate::norm::recording_key;

pub(super) fn track(hash: &str, artist: &str, title: &str, duration_ms: i64) -> LibraryTrack {
    LibraryTrack {
        content_hash: hash.to_string(),
        title: title.to_string(),
        artist: artist.to_string(),
        album: Some("Album".to_string()),
        album_artist: None,
        track_no: Some(1),
        disc_no: None,
        year: None,
        genre: None,
        duration_ms,
        size_bytes: 1_000,
        format: Some("flac".to_string()),
        bitrate_kbps: None,
        archived_path: None,
    }
}

pub(super) fn db_with(tracks: &[(LibraryTrack, &str)]) -> Db {
    let db = Db::new_in_memory().unwrap();
    for (t, device) in tracks {
        db.upsert_library_track(t).unwrap();
        // Registered as well as holding. A device only reports holdings after it has
        // registered, and since holdings from a device that no longer exists are no longer a
        // source, a fixture that skips this is testing a state that cannot occur.
        db.upsert_node(
            device,
            "alpha",
            crate::db::NodeName::KeepOr(device),
            "wander",
            None,
            None,
        )
        .unwrap();
        db.upsert_holding("alpha", device, &t.content_hash, None)
            .unwrap();
    }
    db
}

mod browse;
mod holdings;
mod reindex;
mod spool;
mod tracks;
