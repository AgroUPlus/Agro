//! How a library track is shown to a client, with the devices that can serve it.

use crate::db::Db;
use crate::ws::WsHub;
use async_graphql::SimpleObject;

#[derive(SimpleObject, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerSourcePayload {
    pub device_id: String,
    pub petname: String,
    pub lan_address: Option<String>,
    pub is_online: bool,
    pub is_server_archive: bool,
}

/// One file in the shared library index, as the clients see it.
#[derive(SimpleObject, Clone)]
pub struct LibraryTrackPayload {
    /// SHA-256 of the file's bytes — the identity everything here keys on.
    pub content_hash: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<i32>,
    pub disc_no: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub format: Option<String>,
    pub bitrate_kbps: Option<i32>,
    /// Where the server filed it, relative to the library root. Null when the server holds only
    /// the index entry — which is the whole of index-only mode, and of a track that lives on a
    /// peer.
    pub archived_path: Option<String>,
    /// Peer devices that currently hold this track.
    pub peer_sources: Vec<PeerSourcePayload>,
}

pub(super) fn to_library_payload(t: crate::db_library::LibraryTrack) -> LibraryTrackPayload {
    LibraryTrackPayload {
        content_hash: t.content_hash,
        title: t.title,
        artist: t.artist,
        album: t.album,
        album_artist: t.album_artist,
        track_no: t.track_no.map(|v| v as i32),
        disc_no: t.disc_no.map(|v| v as i32),
        year: t.year.map(|v| v as i32),
        genre: t.genre,
        duration_ms: t.duration_ms,
        size_bytes: t.size_bytes,
        format: t.format,
        bitrate_kbps: t.bitrate_kbps.map(|v| v as i32),
        archived_path: t.archived_path,
        peer_sources: Vec::new(),
    }
}

pub(super) fn to_library_payload_with_sources(
    db: &Db,
    ws_hub: Option<&WsHub>,
    user_id: &str,
    t: crate::db_library::LibraryTrack,
) -> LibraryTrackPayload {
    let mut peer_sources = Vec::new();
    if let Ok(sources) = db.peer_sources_for_track(user_id, &t.content_hash) {
        for s in sources {
            let is_online = chrono::DateTime::parse_from_rfc3339(&s.last_seen_at)
                .map(|seen| {
                    (chrono::Utc::now() - seen.with_timezone(&chrono::Utc)).num_seconds() < 60
                })
                .unwrap_or(false);
            let lan_address = ws_hub.and_then(|hub| hub.get_lan_address(user_id, &s.device_id));
            peer_sources.push(PeerSourcePayload {
                device_id: s.device_id,
                petname: s.petname,
                lan_address,
                is_online,
                is_server_archive: false,
            });
        }
    }
    if t.archived_path.is_some() {
        peer_sources.push(PeerSourcePayload {
            device_id: "server".to_string(),
            petname: "Server Archive".to_string(),
            lan_address: None,
            is_online: true,
            is_server_archive: true,
        });
    }
    LibraryTrackPayload {
        content_hash: t.content_hash,
        title: t.title,
        artist: t.artist,
        album: t.album,
        album_artist: t.album_artist,
        track_no: t.track_no.map(|v| v as i32),
        disc_no: t.disc_no.map(|v| v as i32),
        year: t.year.map(|v| v as i32),
        genre: t.genre,
        duration_ms: t.duration_ms,
        size_bytes: t.size_bytes,
        format: t.format,
        bitrate_kbps: t.bitrate_kbps.map(|v| v as i32),
        archived_path: t.archived_path,
        peer_sources,
    }
}
