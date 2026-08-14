use async_graphql::SimpleObject;
use serde::{Deserialize, Serialize};

#[derive(SimpleObject, Clone, Serialize, Deserialize, Debug)]
pub struct AgroPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub category: String,
    pub target: String, // "Wander (TUI)", "Wanda (Android)", "Core", "Cloud"
    pub is_enabled: bool,
    pub is_connected: bool,
    pub latency_ms: Option<i32>,
    pub endpoint: Option<String>,
    pub metadata: Vec<PluginMetaItem>,
}

#[derive(SimpleObject, Clone, Serialize, Deserialize, Debug)]
pub struct PluginMetaItem {
    pub key: String,
    pub value: String,
}

pub fn get_default_plugins() -> Vec<AgroPlugin> {
    vec![
        AgroPlugin {
            id: "wander-tui".to_string(),
            name: "Wander TUI Connector".to_string(),
            description: "Native link for Wander Rust Desktop client: zero-latency playback handoff, MPRIS v2 shell bridge, and Discord Rich Presence.".to_string(),
            version: "1.4.0".to_string(),
            category: "Client".to_string(),
            target: "Wander (TUI)".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: Some(2),
            endpoint: Some("ws://127.0.0.1:8700/ws/sync".to_string()),
            metadata: vec![
                PluginMetaItem { key: "Protocol".to_string(), value: "Async Tokio WS".to_string() },
                PluginMetaItem { key: "Active Sessions".to_string(), value: "1 (Desktop TUI)".to_string() },
                PluginMetaItem { key: "Hand-off Buffer".to_string(), value: "Lossless Ring".to_string() },
            ],
        },
        AgroPlugin {
            id: "wanda-android".to_string(),
            name: "Wanda Android Bridge".to_string(),
            description: "Modular connection for Wanda Android client: Media3 playback coordination, Wi-Fi smart background cache, and QR passkey handshake.".to_string(),
            version: "1.2.0".to_string(),
            category: "Client".to_string(),
            target: "Wanda (Android)".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: Some(8),
            endpoint: Some("http://192.168.1.100:8700/graphql".to_string()),
            metadata: vec![
                PluginMetaItem { key: "Transport".to_string(), value: "Ktor GraphQL".to_string() },
                PluginMetaItem { key: "Security".to_string(), value: "Passphrase Auth".to_string() },
                PluginMetaItem { key: "Offline Cache".to_string(), value: "15 Tracks Preloaded".to_string() },
            ],
        },
        AgroPlugin {
            id: "subsonic-navidrome".to_string(),
            name: "Subsonic & Navidrome Gateway".to_string(),
            description: "Upstream synchronization with self-hosted Navidrome instances via Subsonic 1.16 salted md5/sha256 authentication.".to_string(),
            version: "2.1.0".to_string(),
            category: "Backend".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: Some(14),
            endpoint: Some("https://music.home.internal".to_string()),
            metadata: vec![
                PluginMetaItem { key: "API Version".to_string(), value: "Subsonic 1.16.1".to_string() },
                PluginMetaItem { key: "Salted Token".to_string(), value: "Active".to_string() },
                PluginMetaItem { key: "Transcoding".to_string(), value: "Direct Stream (Raw)".to_string() },
            ],
        },
        AgroPlugin {
            id: "lrclib-lyrics".to_string(),
            name: "LRCLIB Synced Lyrics Engine".to_string(),
            description: "Automated real-time synchronized lyrics resolver powered by LRCLIB database, serving both Wander and Wanda clients.".to_string(),
            version: "1.0.8".to_string(),
            category: "Enrichment".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: Some(45),
            endpoint: Some("https://lrclib.net/api".to_string()),
            metadata: vec![
                PluginMetaItem { key: "Format".to_string(), value: "Enhanced LRC [mm:ss.xx]".to_string() },
                PluginMetaItem { key: "Cache TTL".to_string(), value: "30 Days (SQLite)".to_string() },
            ],
        },
        AgroPlugin {
            id: "acoustid-dedup".to_string(),
            name: "AcoustID Chromaprint Matcher".to_string(),
            description: "Acoustic fingerprinting engine that detects audio duplicates across formats (FLAC vs MP3) and consolidates library metadata.".to_string(),
            version: "1.1.2".to_string(),
            category: "Curation".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: None,
            endpoint: None,
            metadata: vec![
                PluginMetaItem { key: "Engine".to_string(), value: "Lofty / Chromaprint".to_string() },
                PluginMetaItem { key: "Match Threshold".to_string(), value: "98.5% Confidence".to_string() },
            ],
        },
        AgroPlugin {
            id: "ephemeral-share".to_string(),
            name: "Ephemeral 24h Web Player".to_string(),
            description: "Generates self-expiring, zero-login web streaming URLs for sharing favorite tracks with external friends safely.".to_string(),
            version: "1.0.0".to_string(),
            category: "Sharing".to_string(),
            target: "Cloud".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: Some(1),
            endpoint: Some("/share/{token}".to_string()),
            metadata: vec![
                PluginMetaItem { key: "Default Expiry".to_string(), value: "24 Hours".to_string() },
                PluginMetaItem { key: "Security".to_string(), value: "UUIDv4 Nonce".to_string() },
            ],
        },
        AgroPlugin {
            id: "jam-session".to_string(),
            name: "Jam Collaborative Listening".to_string(),
            description: "Multi-user synchronized listening room with dynamic democratic queue voting across mobile and desktop listeners.".to_string(),
            version: "1.3.1".to_string(),
            category: "Social".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: Some(5),
            endpoint: Some("ws://127.0.0.1:8700/ws/sync".to_string()),
            metadata: vec![
                PluginMetaItem { key: "Room Mode".to_string(), value: "Democratic Vote Order".to_string() },
                PluginMetaItem { key: "Clock Drift".to_string(), value: "< 15ms NTP Synchronized".to_string() },
            ],
        },
    ]
}
