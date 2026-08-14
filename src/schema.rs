use async_graphql::{Context, InputObject, Object, Schema, SimpleObject};
use crate::db::Db;
use crate::passphrase::generate_passphrase;
use crate::plugins::{get_default_plugins, AgroPlugin};
use crate::ws::{WsHub, WsMessage};
use std::sync::Arc;

pub type AgroSchema = Schema<QueryRoot, MutationRoot, async_graphql::EmptySubscription>;

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
}

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn health(&self) -> &'static str {
        "Agro Server OK"
    }

    async fn me(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<AccountPayload> {
        let db = ctx.data::<Db>()?;
        if let Some((id, uname, passphrase_or_key)) = db.get_user_by_username(&username)? {
            let qr_data = format!("agro://connect?username={}&passphrase={}", uname, passphrase_or_key);
            let connection_url = format!("http://localhost:8700/connect?passphrase={}", passphrase_or_key);
            Ok(AccountPayload {
                id,
                username: uname,
                api_key: passphrase_or_key.clone(),
                passphrase: passphrase_or_key,
                connection_url,
                qr_data,
            })
        } else {
            // Auto initialize default user if requested
            let passphrase = generate_passphrase();
            let id = db.create_user(&username, &passphrase)?;
            let qr_data = format!("agro://connect?username={}&passphrase={}", username, passphrase);
            let connection_url = format!("http://localhost:8700/connect?passphrase={}", passphrase);
            Ok(AccountPayload {
                id,
                username,
                api_key: passphrase.clone(),
                passphrase,
                connection_url,
                qr_data,
            })
        }
    }

    async fn plugins(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<AgroPlugin>> {
        let db = ctx.data::<Db>()?;
        let saved_states = db.get_plugin_states().unwrap_or_default();
        let mut plugins = get_default_plugins();
        for p in &mut plugins {
            if let Some(&enabled) = saved_states.get(&p.id) {
                p.is_enabled = enabled;
            }
        }
        Ok(plugins)
    }

    async fn playback_handoff(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Option<HandoffState>> {
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
        let db = ctx.data::<Db>()?;
        let nodes = db.get_active_nodes(&user_id)?;
        let now = chrono::Utc::now();
        let payload = nodes.into_iter().map(|n| {
            let is_online = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&n.last_seen_at) {
                (now - dt.with_timezone(&chrono::Utc)).num_seconds() < 45
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

    async fn synced_settings(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Option<SyncedSettingsPayload>> {
        let db = ctx.data::<Db>()?;
        let settings = db.get_synced_settings(&user_id)?;
        Ok(settings.map(|s| SyncedSettingsPayload {
            user_id,
            server_url: s.server_url,
            server_username: s.server_username,
            lrclib_url: s.lrclib_url,
            lyrics_fetch_online: s.lyrics_fetch_online.unwrap_or(true),
            stream_format: s.stream_format.unwrap_or_else(|| "FLAC".to_string()),
            updated_at: s.updated_at,
        }))
    }
}

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
            user_id,
            petname: petname.clone(),
            client_type: normalized_client,
            ip_address,
            version,
            current_track,
            last_seen_at: chrono::Utc::now().to_rfc3339(),
            is_online: true,
        };

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            let _ = ws_hub.tx.send(WsMessage {
                msg_type: "NODE_UPDATE".to_string(),
                payload: serde_json::to_value(&payload).unwrap_or_default(),
            });
        }

        Ok(payload)
    }

    async fn update_synced_settings(
        &self,
        ctx: &Context<'_>,
        input: SyncedSettingsInput,
    ) -> async_graphql::Result<SyncedSettingsPayload> {
        let db = ctx.data::<Db>()?;
        db.upsert_synced_settings(
            &input.user_id,
            input.server_url.as_deref(),
            input.server_username.as_deref(),
            input.lrclib_url.as_deref(),
            input.lyrics_fetch_online,
            input.stream_format.as_deref(),
        )?;

        let settings = db.get_synced_settings(&input.user_id)?.unwrap();
        let payload = SyncedSettingsPayload {
            user_id: input.user_id.clone(),
            server_url: settings.server_url,
            server_username: settings.server_username,
            lrclib_url: settings.lrclib_url,
            lyrics_fetch_online: settings.lyrics_fetch_online.unwrap_or(true),
            stream_format: settings.stream_format.unwrap_or_else(|| "FLAC".to_string()),
            updated_at: settings.updated_at,
        };

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            let _ = ws_hub.tx.send(WsMessage {
                msg_type: "SETTINGS_SYNC".to_string(),
                payload: serde_json::json!({
                    "userId": input.user_id,
                    "updatedAt": payload.updated_at
                }),
            });
        }

        Ok(payload)
    }
    async fn create_account(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<AccountPayload> {
        let db = ctx.data::<Db>()?;
        let passphrase = generate_passphrase();
        
        let id = db.create_user(&username, &passphrase)?;
        let qr_data = format!("agro://connect?username={}&passphrase={}", username, passphrase);
        let connection_url = format!("http://localhost:8700/connect?passphrase={}", passphrase);

        Ok(AccountPayload {
            id,
            username,
            api_key: passphrase.clone(),
            passphrase,
            connection_url,
            qr_data,
        })
    }

    async fn toggle_plugin(&self, ctx: &Context<'_>, plugin_id: String, is_enabled: bool) -> async_graphql::Result<bool> {
        let db = ctx.data::<Db>()?;
        db.set_plugin_enabled(&plugin_id, is_enabled)?;
        Ok(true)
    }

    async fn update_handoff(&self, ctx: &Context<'_>, input: HandoffInput) -> async_graphql::Result<bool> {
        let db = ctx.data::<Db>()?;
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
            let _ = ws_hub.tx.send(WsMessage {
                msg_type: "HANDOFF".to_string(),
                payload: serde_json::json!({
                    "trackTitle": input.track_title,
                    "artistName": input.artist_name,
                    "albumName": input.album_name,
                    "positionMs": input.position_ms,
                    "isPlaying": input.is_playing,
                    "deviceId": input.device_id,
                    "petname": petname,
                }),
            });
        }

        Ok(true)
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
