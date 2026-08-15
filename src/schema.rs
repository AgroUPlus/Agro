use async_graphql::{Context, InputObject, Object, Schema, SimpleObject};
use crate::auth::AuthedUser;
use crate::db::Db;
use crate::passphrase::generate_passphrase;
use crate::plugins::AgroPlugin;
use crate::ws::WsHub;
use std::sync::Arc;

pub type AgroSchema = Schema<QueryRoot, MutationRoot, async_graphql::EmptySubscription>;

/// Checks the account a caller *named* against the account its token actually proved.
///
/// Every account-scoped resolver takes a `userId` argument, and until this existed every one of
/// them simply believed it — so any valid token could read or write any other account's sessions,
/// settings, devices and library. The argument is kept (both clients send it, and it reads well in
/// the schema) but it is now checked rather than trusted.
///
/// Returns `Ok` when there is no authenticated identity at all: that is the first-run window
/// `require_token` deliberately leaves open while the database has no accounts, and there is
/// nothing to protect yet. It closes the moment the first account exists.
fn authorize(ctx: &Context<'_>, user_id: &str) -> async_graphql::Result<()> {
    let Some(authed) = ctx.data_opt::<AuthedUser>() else {
        return Ok(());
    };
    if authed.username.eq_ignore_ascii_case(user_id.trim()) {
        Ok(())
    } else {
        // Deliberately does not name the account that *was* authenticated — an error message is
        // not the place to disclose it.
        Err(async_graphql::Error::new(
            "Forbidden: that token does not belong to the requested account",
        ))
    }
}

#[derive(SimpleObject, Clone)]
pub struct AuthPayload {
    pub success: bool,
    pub username: String,
    pub token: String,
    pub message: String,
}

#[derive(SimpleObject, Clone)]
pub struct AccountPayload {
    pub id: String,
    pub username: String,
    pub api_key: String,
    pub passphrase: String,
    pub connection_url: String,
    pub qr_data: String,
}

#[derive(SimpleObject, Clone, serde::Serialize)]
pub struct NodePayload {
    pub device_id: String,
    pub user_id: String,
    pub petname: String,
    pub client_type: String,
    pub ip_address: Option<String>,
    pub version: Option<String>,
    pub current_track: Option<String>,
    pub last_seen_at: String,
    pub is_online: bool,
}

#[derive(SimpleObject, Clone)]
pub struct SyncedSettingsPayload {
    pub user_id: String,
    pub server_url: Option<String>,
    pub server_username: Option<String>,
    pub lrclib_url: Option<String>,
    pub lyrics_fetch_online: bool,
    pub stream_format: String,
    pub updated_at: String,
}

#[derive(InputObject)]
pub struct SyncedSettingsInput {
    pub user_id: String,
    pub server_url: Option<String>,
    pub server_username: Option<String>,
    pub lrclib_url: Option<String>,
    pub lyrics_fetch_online: Option<bool>,
    pub stream_format: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct HandoffState {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    pub updated_at: String,
    /// The rest of the session: every track in the queue, so picking it up on another device
    /// continues the listening rather than playing one song and stopping.
    pub queue: Vec<HandoffTrack>,
    /// Where `queue` was playing. -1 when the sender reported no queue at all.
    pub queue_index: i32,
}

/// One entry of a handed-over queue. `track_uri` is the sending client's own id for it — a
/// receiving client resolves it against its own backends, falling back to title and artist when
/// the two devices do not share that source.
#[derive(SimpleObject, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandoffTrack {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(InputObject, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandoffTrackInput {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct TrackItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: i64,
    pub stream_url: String,
    pub artwork_url: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct TopStatItem {
    pub name: String,
    pub count: i64,
    pub percentage: f64,
}

#[derive(SimpleObject, Clone)]
pub struct RewindReport {
    pub period: String,
    pub total_listen_time_minutes: i64,
    pub total_tracks_played: i64,
    pub top_artists: Vec<TopStatItem>,
    pub top_genres: Vec<TopStatItem>,
    pub peak_hour: i32,
}

#[derive(SimpleObject, Clone)]
pub struct SharePayload {
    pub token: String,
    pub share_url: String,
    pub expires_at: String,
    pub track_title: String,
    pub artist_name: String,
}

#[derive(SimpleObject, Clone)]
pub struct DuplicateCluster {
    pub group_id: String,
    pub reason: String,
    pub tracks: Vec<TrackItem>,
}

#[derive(SimpleObject, Clone)]
pub struct LyricsAndCoverPayload {
    pub synced_lrc: String,
    pub cover_art_url: String,
    pub is_synced: bool,
}

#[derive(SimpleObject, Clone)]
pub struct JamTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub submitted_by: String,
    pub votes: i32,
}

#[derive(SimpleObject, Clone)]
pub struct JamRoomState {
    pub room_id: String,
    pub currently_playing: Option<JamTrack>,
    pub queue: Vec<JamTrack>,
}

#[derive(InputObject)]
pub struct HandoffInput {
    pub user_id: String,
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    /// Optional so a heartbeat can refresh position without re-sending the whole queue; when it is
    /// omitted the stored queue is kept as-is.
    pub queue: Option<Vec<HandoffTrackInput>>,
    pub queue_index: Option<i32>,
}

/// See `update_handoff`.
const MAX_QUEUE_TRACKS: usize = 100;

/// How long a node stays "online" after it last reported in. Clients heartbeat inside this window
/// while they are playing; anything longer and they show as away.
const NODE_ONLINE_SECONDS: i64 = 45;

/// Gathers what the server actually knows, so the plugin list describes this deployment
/// rather than a fixed example of one.
fn plugin_context(db: &Db) -> crate::plugins::PluginContext {
    let nodes = db.get_all_nodes().unwrap_or_default();
    let now = chrono::Utc::now();
    let online = |last_seen: &str| {
        chrono::DateTime::parse_from_rfc3339(last_seen)
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_seconds() < NODE_ONLINE_SECONDS)
            .unwrap_or(false)
    };
    let is_wander = |n: &crate::db::NodeRecord| n.client_type == "wander";

    let settings = nodes
        .first()
        .and_then(|n| db.get_synced_settings(&n.user_id).ok().flatten());

    crate::plugins::PluginContext {
        online_wander: nodes.iter().filter(|n| is_wander(n) && online(&n.last_seen_at)).count(),
        online_wanda: nodes.iter().filter(|n| !is_wander(n) && online(&n.last_seen_at)).count(),
        known_wander: nodes.iter().filter(|n| is_wander(n)).count(),
        known_wanda: nodes.iter().filter(|n| !is_wander(n)).count(),
        navidrome_url: settings.as_ref().and_then(|s| s.server_url.clone()),
        navidrome_username: settings.as_ref().and_then(|s| s.server_username.clone()),
        lrclib_url: settings.as_ref().and_then(|s| s.lrclib_url.clone()),
        lyrics_online: settings
            .as_ref()
            .and_then(|s| s.lyrics_fetch_online)
            .unwrap_or(true),
        has_handoff: nodes
            .first()
            .map(|n| db.get_handoff(&n.user_id).ok().flatten().is_some())
            .unwrap_or(false),
    }
}


/// An app password as it is listed back. The token itself is deliberately absent: a credential is
/// shown once, when it is created, and is not recoverable afterwards.
#[derive(SimpleObject, Clone)]
pub struct AppPassword {
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// The one time a token is returned. Shown once, at creation.
#[derive(SimpleObject, Clone)]
pub struct AppPasswordCreated {
    pub label: String,
    pub token: String,
}

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn health(&self) -> &'static str {
        "Agro Server OK"
    }

    async fn users(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<String>> {
        let db = ctx.data::<Db>()?;
        Ok(db.list_users()?)
    }

    async fn authenticate(&self, ctx: &Context<'_>, username: String, passphrase: String) -> async_graphql::Result<AuthPayload> {
        let db = ctx.data::<Db>()?;
        let clean_user = username.trim().to_lowercase();
        let clean_pass = passphrase.trim();
        if clean_user.is_empty() || clean_pass.is_empty() {
            return Ok(AuthPayload {
                success: false,
                username: clean_user,
                token: String::new(),
                message: "Username and passphrase cannot be empty".to_string(),
            });
        }
        let valid = db.authenticate_user(&clean_user, clean_pass)?;
        if valid {
            Ok(AuthPayload {
                success: true,
                username: clean_user,
                token: clean_pass.to_string(),
                message: "Authenticated successfully".to_string(),
            })
        } else {
            Ok(AuthPayload {
                success: false,
                username: clean_user,
                token: String::new(),
                message: "Invalid passphrase for this user".to_string(),
            })
        }
    }

    /// Looks an account up. **Does not create one** — it used to, through `get_or_create_user`,
    /// which made a read-only-looking query mint accounts as a side effect: opening the dashboard
    /// recreated a deleted account, with a new passphrase, and closed the first-run setup window
    /// behind it. Accounts come from `createAccount` and nowhere else.
    async fn me(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<Option<AccountPayload>> {
        authorize(ctx, &username)?;
        let db = ctx.data::<Db>()?;
        let clean_user = username.trim().to_lowercase();
        let Some((id, _, key)) = db.get_user_by_username(&clean_user)? else {
            return Ok(None);
        };
        Ok(Some(account_payload(id, clean_user, key)))
    }

    async fn plugins(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<AgroPlugin>> {
        let db = ctx.data::<Db>()?;
        let saved_states = db.get_plugin_states().unwrap_or_default();
        let mut plugins = crate::plugins::get_plugins(&plugin_context(db));
        for p in &mut plugins {
            if let Some(&enabled) = saved_states.get(&p.id) {
                p.is_enabled = enabled;
            }
        }
        Ok(plugins)
    }

    async fn playback_handoff(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Option<HandoffState>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let rec = db.get_handoff(&user_id)?;
        Ok(rec.map(|r| HandoffState {
            track_uri: r.track_uri,
            track_title: r.track_title,
            artist_name: r.artist_name,
            album_name: r.album_name,
            artwork_url: r.artwork_url,
            position_ms: r.position_ms,
            is_playing: r.is_playing,
            device_id: r.device_id,
            updated_at: r.updated_at,
            // Stored opaquely as JSON; a value written by an older client that predates the queue
            // simply reads back as an empty one rather than failing the whole query.
            queue: r
                .queue_json
                .and_then(|json| serde_json::from_str::<Vec<HandoffTrack>>(&json).ok())
                .unwrap_or_default(),
            queue_index: r.queue_index.unwrap_or(-1) as i32,
        }))
    }

    async fn smart_cache_tracks(&self, _ctx: &Context<'_>, limit: Option<i32>) -> async_graphql::Result<Vec<TrackItem>> {
        let count = limit.unwrap_or(15);
        let sample_tracks = vec![
            TrackItem {
                id: "trk-1".to_string(),
                title: "Midnight City".to_string(),
                artist: "M83".to_string(),
                album: Some("Hurry Up, We're Dreaming".to_string()),
                duration_secs: 243,
                stream_url: "/api/v1/stream/trk-1".to_string(),
                artwork_url: Some("https://images.unsplash.com/photo-1514525253161-7a46d19cd819?w=300".to_string()),
            },
            TrackItem {
                id: "trk-2".to_string(),
                title: "Get Lucky".to_string(),
                artist: "Daft Punk".to_string(),
                album: Some("Random Access Memories".to_string()),
                duration_secs: 248,
                stream_url: "/api/v1/stream/trk-2".to_string(),
                artwork_url: Some("https://images.unsplash.com/photo-1470225620780-dba8ba36b745?w=300".to_string()),
            },
            TrackItem {
                id: "trk-3".to_string(),
                title: "Resonance".to_string(),
                artist: "HOME".to_string(),
                album: Some("Odyssey".to_string()),
                duration_secs: 212,
                stream_url: "/api/v1/stream/trk-3".to_string(),
                artwork_url: Some("https://images.unsplash.com/photo-1511671782779-c97d3d27a1d4?w=300".to_string()),
            },
        ];
        let mut res = Vec::new();
        for i in 0..count {
            let base = &sample_tracks[i as usize % sample_tracks.len()];
            res.push(TrackItem {
                id: format!("trk-{}", i + 1),
                title: base.title.clone(),
                artist: base.artist.clone(),
                album: base.album.clone(),
                duration_secs: base.duration_secs,
                stream_url: base.stream_url.clone(),
                artwork_url: base.artwork_url.clone(),
            });
        }
        Ok(res)
    }

    async fn agro_rewind(&self, _ctx: &Context<'_>, period: String) -> async_graphql::Result<RewindReport> {
        Ok(RewindReport {
            period,
            total_listen_time_minutes: 4280,
            total_tracks_played: 1142,
            top_artists: vec![
                TopStatItem { name: "Daft Punk".to_string(), count: 320, percentage: 28.0 },
                TopStatItem { name: "M83".to_string(), count: 210, percentage: 18.4 },
                TopStatItem { name: "HOME".to_string(), count: 185, percentage: 16.2 },
                TopStatItem { name: "Gorillaz".to_string(), count: 140, percentage: 12.3 },
            ],
            top_genres: vec![
                TopStatItem { name: "Synthwave".to_string(), count: 450, percentage: 39.4 },
                TopStatItem { name: "French House".to_string(), count: 380, percentage: 33.2 },
                TopStatItem { name: "Indie Electronic".to_string(), count: 210, percentage: 18.4 },
            ],
            peak_hour: 22,
        })
    }

    async fn duplicates_report(&self, _ctx: &Context<'_>) -> async_graphql::Result<Vec<DuplicateCluster>> {
        Ok(vec![
            DuplicateCluster {
                group_id: "dup-1".to_string(),
                reason: "Exact AcoustID Chromaprint Match (99.8%)".to_string(),
                tracks: vec![
                    TrackItem {
                        id: "dup-1a".to_string(),
                        title: "Get Lucky (FLAC 24bit)".to_string(),
                        artist: "Daft Punk".to_string(),
                        album: Some("Random Access Memories".to_string()),
                        duration_secs: 248,
                        stream_url: "/music/Daft_Punk/Get_Lucky.flac".to_string(),
                        artwork_url: None,
                    },
                    TrackItem {
                        id: "dup-1b".to_string(),
                        title: "Get Lucky (MP3 320k)".to_string(),
                        artist: "Daft Punk feat Pharrell".to_string(),
                        album: Some("Random Access Memories".to_string()),
                        duration_secs: 248,
                        stream_url: "/music/Downloads/Get_Lucky.mp3".to_string(),
                        artwork_url: None,
                    },
                ],
            }
        ])
    }

    async fn jam_room_state(&self, _ctx: &Context<'_>, room_id: String) -> async_graphql::Result<JamRoomState> {
        Ok(JamRoomState {
            room_id,
            currently_playing: Some(JamTrack {
                id: 1,
                title: "Starboy".to_string(),
                artist: "The Weeknd".to_string(),
                submitted_by: "Alice".to_string(),
                votes: 5,
            }),
            queue: vec![
                JamTrack {
                    id: 2,
                    title: "One More Time".to_string(),
                    artist: "Daft Punk".to_string(),
                    submitted_by: "Bob".to_string(),
                    votes: 8,
                },
                JamTrack {
                    id: 3,
                    title: "Blinding Lights".to_string(),
                    artist: "The Weeknd".to_string(),
                    submitted_by: "Charlie".to_string(),
                    votes: 3,
                },
            ],
        })
    }

    async fn active_nodes(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Vec<NodePayload>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let nodes = db.get_active_nodes(&user_id)?;
        let now = chrono::Utc::now();
        let payload = nodes.into_iter().map(|n| {
            let is_online = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&n.last_seen_at) {
                (now - dt.with_timezone(&chrono::Utc)).num_seconds() < NODE_ONLINE_SECONDS
            } else {
                false
            };
            NodePayload {
                device_id: n.device_id,
                user_id: n.user_id,
                petname: n.petname,
                client_type: n.client_type,
                ip_address: n.ip_address,
                version: n.version,
                current_track: n.current_track,
                last_seen_at: n.last_seen_at,
                is_online,
            }
        }).collect();
        Ok(payload)
    }

    async fn app_passwords(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Vec<AppPassword>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .list_app_passwords(&user_id)?
            .into_iter()
            .map(|record| AppPassword {
                label: record.label,
                created_at: record.created_at,
                last_used_at: record.last_used_at,
            })
            .collect())
    }


    // ── Library ─────────────────────────────────────────────────────────────────────────────

    /// How much this account's library holds, and how much of it the server has the bytes for.
    async fn library_stats(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<LibraryStatsPayload> {
        authorize(ctx, &user_id)?;
        let stats = ctx.data::<Db>()?.library_stats(&user_id)?;
        Ok(LibraryStatsPayload {
            track_count: stats.track_count,
            archived_count: stats.archived_count,
            total_bytes: stats.total_bytes,
            spool_bytes: stats.spool_bytes,
        })
    }

    /// Every content hash a device has reported, for reconciling against what it actually holds.
    async fn device_holdings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
    ) -> async_graphql::Result<Vec<String>> {
        authorize(ctx, &user_id)?;
        Ok(ctx.data::<Db>()?.device_holding_hashes(&device_id)?)
    }

    /// Tracks another of this account's devices holds that this one does not.
    ///
    /// Matched on the recording rather than the bytes, so owning a different rip of the same song
    /// counts as having it — see `Db::missing_on_device`.
    async fn missing_on_device(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<LibraryTrackPayload>> {
        authorize(ctx, &user_id)?;
        let limit = limit.unwrap_or(50).clamp(1, MAX_MISSING as i32) as i64;
        Ok(ctx
            .data::<Db>()?
            .missing_on_device(&user_id, &device_id, limit)?
            .into_iter()
            .map(to_library_payload)
            .collect())
    }

    async fn synced_settings(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Option<SyncedSettingsPayload>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let settings = db.get_synced_settings(&user_id)?;
        let passphrase = db.get_user_by_username(&user_id)?
            .map(|(_, _, p)| p)
            .unwrap_or_default();

        Ok(settings.map(|s| {
            let server_url = s.server_url.and_then(|u| crate::crypto::decrypt_field(&u, &passphrase).ok());
            let server_username = s.server_username.and_then(|u| crate::crypto::decrypt_field(&u, &passphrase).ok());
            let lrclib_url = s.lrclib_url.and_then(|u| crate::crypto::decrypt_field(&u, &passphrase).ok());

            SyncedSettingsPayload {
                user_id,
                server_url,
                server_username,
                lrclib_url,
                lyrics_fetch_online: s.lyrics_fetch_online.unwrap_or(true),
                stream_format: s.stream_format.unwrap_or_else(|| "FLAC".to_string()),
                updated_at: s.updated_at,
            }
        }))
    }
}

/// The address clients should be told to connect to. `localhost` was hardcoded here, which made
/// the pairing QR unusable from a phone — and the QR carried no `server` parameter at all, which
/// is the one field the Android client needs to know where to connect.
fn public_url() -> String {
    std::env::var("AGRO_PUBLIC_URL").unwrap_or_default()
}

fn account_payload(id: String, username: String, key: String) -> AccountPayload {
    let server = public_url();
    let qr_data = if server.is_empty() {
        format!("agro://connect?username={}&passphrase={}", username, key)
    } else {
        format!(
            "agro://connect?username={}&passphrase={}&server={}",
            username,
            key,
            urlencoding::encode(&server)
        )
    };
    AccountPayload {
        id,
        username,
        api_key: key.clone(),
        passphrase: key,
        connection_url: server,
        qr_data,
    }
}

// ── Library index ───────────────────────────────────────────────────────────────────────────

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
}

#[derive(SimpleObject, Clone)]
pub struct LibraryStatsPayload {
    pub track_count: i64,
    pub archived_count: i64,
    pub total_bytes: i64,
    pub spool_bytes: i64,
}

/// What a device reports it holds. Metadata travels with it so the server can index a file it has
/// never been sent — an index-only library still answers "who has what".
#[derive(InputObject)]
pub struct HoldingInput {
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
    /// The device's own handle for the file. Stored opaquely and never interpreted.
    pub local_ref: Option<String>,
}

fn to_library_payload(t: crate::db_library::LibraryTrack) -> LibraryTrackPayload {
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
    }
}

/// Most a diff returns in one go. The offer is a prompt, not a migration plan.
const MAX_MISSING: i64 = 200;

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn register_node(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        client_type: String,
        device_name: Option<String>,
        ip_address: Option<String>,
        version: Option<String>,
        current_track: Option<String>,
    ) -> async_graphql::Result<NodePayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let normalized_client = if client_type.to_lowercase().contains("wanda") {
            "wanda".to_string()
        } else {
            "wander".to_string()
        };

        let existing_nodes = db.get_active_nodes(&user_id).unwrap_or_default();
        let petname = if let Some(custom) = device_name.filter(|s| !s.trim().is_empty()) {
            custom
        } else if let Some(existing) = existing_nodes.iter().find(|n| n.device_id == device_id) {
            existing.petname.clone()
        } else {
            crate::passphrase::generate_random_petname()
        };

        db.upsert_node(
            &device_id,
            &user_id,
            &petname,
            &normalized_client,
            ip_address.as_deref(),
            version.as_deref(),
            current_track.as_deref(),
        )?;

        let payload = NodePayload {
            device_id: device_id.clone(),
            user_id: user_id.clone(),
            petname: petname.clone(),
            client_type: normalized_client,
            ip_address,
            version,
            current_track,
            last_seen_at: chrono::Utc::now().to_rfc3339(),
            is_online: true,
        };

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            // Scoped to the account. These used to go to every connected socket regardless of
            // whose device they described.
            ws_hub.notify_user(
                &user_id,
                "NODE_UPDATE",
                serde_json::to_value(&payload).unwrap_or_default(),
            );
        }

        Ok(payload)
    }

    async fn update_synced_settings(
        &self,
        ctx: &Context<'_>,
        input: SyncedSettingsInput,
    ) -> async_graphql::Result<SyncedSettingsPayload> {
        authorize(ctx, &input.user_id)?;
        let db = ctx.data::<Db>()?;
        let passphrase = db.get_user_by_username(&input.user_id)?
            .map(|(_, _, p)| p)
            .unwrap_or_else(|| "default".to_string());

        let enc_server_url = input.server_url.as_deref().and_then(|u| crate::crypto::encrypt_field(u, &passphrase).ok());
        let enc_server_username = input.server_username.as_deref().and_then(|u| crate::crypto::encrypt_field(u, &passphrase).ok());
        let enc_lrclib_url = input.lrclib_url.as_deref().and_then(|u| crate::crypto::encrypt_field(u, &passphrase).ok());

        db.upsert_synced_settings(
            &input.user_id,
            enc_server_url.as_deref(),
            enc_server_username.as_deref(),
            enc_lrclib_url.as_deref(),
            input.lyrics_fetch_online,
            input.stream_format.as_deref(),
        )?;

        let settings = db.get_synced_settings(&input.user_id)?.unwrap();
        let payload = SyncedSettingsPayload {
            user_id: input.user_id.clone(),
            server_url: input.server_url.clone().or(settings.server_url.and_then(|u| crate::crypto::decrypt_field(&u, &passphrase).ok())),
            server_username: input.server_username.clone().or(settings.server_username.and_then(|u| crate::crypto::decrypt_field(&u, &passphrase).ok())),
            lrclib_url: input.lrclib_url.clone().or(settings.lrclib_url.and_then(|u| crate::crypto::decrypt_field(&u, &passphrase).ok())),
            lyrics_fetch_online: settings.lyrics_fetch_online.unwrap_or(true),
            stream_format: settings.stream_format.unwrap_or_else(|| "FLAC".to_string()),
            updated_at: settings.updated_at,
        };

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            ws_hub.notify_user(
                &input.user_id,
                "SETTINGS_SYNC",
                serde_json::json!({
                    "userId": input.user_id,
                    "updatedAt": payload.updated_at
                }),
            );
        }

        Ok(payload)
    }
    async fn create_account(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<AccountPayload> {
        let db = ctx.data::<Db>()?;
        let username = username.trim().to_lowercase();
        if username.is_empty() {
            return Err("An account needs a username".into());
        }
        if db.get_user_by_username(&username)?.is_some() {
            return Err("That account already exists".into());
        }
        let passphrase = generate_passphrase();
        let id = db.create_user(&username, &passphrase)?;
        Ok(account_payload(id, username, passphrase))
    }

    /// Issues a credential for one client, so that client can be revoked on its own rather than
    /// by rotating the account passphrase every other device is using.
    async fn create_app_password(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        label: String,
    ) -> async_graphql::Result<AppPasswordCreated> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let label = label.trim().to_string();
        if label.is_empty() {
            return Err("An app password needs a label, so you can tell which device it is".into());
        }
        let token = generate_passphrase();
        db.create_app_password(&user_id, &label, &token)?;
        Ok(AppPasswordCreated { label, token })
    }

    async fn revoke_app_password(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        label: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db.revoke_app_password(&user_id, &label)?)
    }

    /// Deletes an account with its nodes, session, settings and app passwords.
    ///
    /// Irreversible, and it can lock you out: deleting the last account puts the server back into
    /// first-run, where anyone who can reach it may create the next one. The caller confirms.
    async fn delete_account(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<bool> {
        authorize(ctx, &username)?;
        let db = ctx.data::<Db>()?;
        Ok(db.delete_user(&username.trim().to_lowercase())?)
    }

    async fn toggle_plugin(&self, ctx: &Context<'_>, plugin_id: String, is_enabled: bool) -> async_graphql::Result<bool> {
        let db = ctx.data::<Db>()?;
        db.set_plugin_enabled(&plugin_id, is_enabled)?;
        Ok(true)
    }

    async fn update_handoff(&self, ctx: &Context<'_>, input: HandoffInput) -> async_graphql::Result<bool> {
        authorize(ctx, &input.user_id)?;
        let db = ctx.data::<Db>()?;
        // A queue is capped rather than rejected: an endless-radio client can hold hundreds of
        // entries, and the first hundred is far more session than anyone resumes through.
        let queue_json = input.queue.as_ref().map(|tracks| {
            let capped: Vec<&HandoffTrackInput> = tracks.iter().take(MAX_QUEUE_TRACKS).collect();
            serde_json::to_string(&capped).unwrap_or_else(|_| "[]".to_string())
        });

        db.update_handoff(
            &input.user_id,
            &input.track_uri,
            &input.track_title,
            &input.artist_name,
            input.album_name.as_deref(),
            input.artwork_url.as_deref(),
            input.position_ms,
            input.is_playing,
            &input.device_id,
            queue_json.as_deref(),
            input.queue_index.map(|i| i as i64),
        )?;

        let track_summary = format!("{} • {}", input.track_title, input.artist_name);
        let existing_nodes = db.get_active_nodes(&input.user_id).unwrap_or_default();
        let petname = if let Some(existing) = existing_nodes.iter().find(|n| n.device_id == input.device_id) {
            existing.petname.clone()
        } else {
            crate::passphrase::generate_random_petname()
        };
        let client_type = if input.device_id.to_lowercase().contains("android") || input.device_id.to_lowercase().contains("wanda") {
            "wanda"
        } else {
            "wander"
        };
        let _ = db.upsert_node(
            &input.device_id,
            &input.user_id,
            &petname,
            client_type,
            None,
            None,
            Some(&track_summary),
        );

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            ws_hub.notify_user(
                &input.user_id,
                "HANDOFF",
                serde_json::json!({
                    "trackTitle": input.track_title,
                    "artistName": input.artist_name,
                    "albumName": input.album_name,
                    "positionMs": input.position_ms,
                    "isPlaying": input.is_playing,
                    "deviceId": input.device_id,
                    "petname": petname,
                }),
            );
        }

        Ok(true)
    }


    // ── Library ─────────────────────────────────────────────────────────────────────────────

    /// Records what a device holds.
    ///
    /// Batched and idempotent, so a client sends its whole library once and only deltas after —
    /// re-sending everything is wasteful but never wrong.
    ///
    /// Each entry also indexes the track, so the server knows about files it has never been sent.
    /// That is what makes index-only mode work: the diff needs metadata, not bytes.
    async fn report_holdings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        tracks: Vec<HoldingInput>,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;

        let mut accepted = 0;
        for input in tracks {
            // A malformed hash would create an index entry nothing can ever match or fetch.
            if input.content_hash.len() != 64
                || !input.content_hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            let track = crate::db_library::LibraryTrack {
                content_hash: input.content_hash.clone(),
                title: input.title,
                artist: input.artist,
                album: input.album,
                album_artist: input.album_artist,
                track_no: input.track_no.map(i64::from),
                disc_no: input.disc_no.map(i64::from),
                year: input.year.map(i64::from),
                genre: input.genre,
                duration_ms: input.duration_ms,
                size_bytes: input.size_bytes,
                format: input.format,
                bitrate_kbps: input.bitrate_kbps.map(i64::from),
                // Never cleared by a report: only the server decides where it filed something.
                archived_path: None,
            };
            db.upsert_library_track(&track)?;
            db.upsert_holding(
                &user_id,
                &device_id,
                &input.content_hash,
                input.local_ref.as_deref(),
            )?;
            accepted += 1;
        }

        if accepted > 0 {
            if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
                ws_hub.notify_user(
                    &user_id,
                    "LIBRARY_UPDATED",
                    serde_json::json!({ "deviceId": device_id, "count": accepted }),
                );
            }
        }
        Ok(accepted)
    }

    /// Forgets holdings a device no longer has — deleted locally, or moved to the server and
    /// removed. The index entry survives: another device may still hold it.
    async fn forget_holdings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        hashes: Vec<String>,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        Ok(ctx.data::<Db>()?.forget_holdings(&device_id, &hashes)? as i32)
    }

    /// Nudges one device to look at what it is missing.
    ///
    /// Addressed to that device alone rather than broadcast, so the other devices on the account
    /// are not prompted about a library that is not theirs.
    async fn offer_sync(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        let missing = ctx
            .data::<Db>()?
            .missing_on_device(&user_id, &device_id, MAX_MISSING)?;
        if missing.is_empty() {
            return Ok(0);
        }
        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            ws_hub.notify_device(
                &user_id,
                &device_id,
                "SYNC_OFFER",
                serde_json::json!({
                    "count": missing.len(),
                    "sample": missing.iter().take(3)
                        .map(|t| format!("{} — {}", t.artist, t.title))
                        .collect::<Vec<_>>(),
                }),
            );
        }
        Ok(missing.len() as i32)
    }

    async fn create_ephemeral_share(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        track_title: String,
        artist_name: String,
        album_name: Option<String>,
        audio_url: String,
        ttl_hours: Option<i64>,
    ) -> async_graphql::Result<SharePayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let ttl = ttl_hours.unwrap_or(24);
        let token = db.create_ephemeral_share(&user_id, &track_title, &artist_name, album_name.as_deref(), &audio_url, ttl)?;
        let share_url = format!("http://localhost:8700/share/{}", token);
        let expires_at = (chrono::Utc::now() + chrono::Duration::hours(ttl)).to_rfc3339();

        Ok(SharePayload {
            token,
            share_url,
            expires_at,
            track_title,
            artist_name,
        })
    }

    async fn fetch_lyrics_and_cover(&self, _ctx: &Context<'_>, artist: String, title: String) -> async_graphql::Result<LyricsAndCoverPayload> {
        // Query LRCLIB API dynamically
        let client = reqwest::Client::new();
        let url = format!("https://lrclib.net/api/get?artist_name={}&track_name={}", urlencoding::encode(&artist), urlencoding::encode(&title));
        
        let synced_lrc = if let Ok(resp) = client.get(&url).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json["syncedLyrics"].as_str().unwrap_or("[00:00.00] Synchronized lyrics not found").to_string()
            } else {
                "[00:00.00] Synchronized lyrics unavailable".to_string()
            }
        } else {
            "[00:00.00] LRCLIB service unreachable".to_string()
        };

        Ok(LyricsAndCoverPayload {
            synced_lrc,
            cover_art_url: "https://images.unsplash.com/photo-1514525253161-7a46d19cd819?w=500".to_string(),
            is_synced: true,
        })
    }
}
