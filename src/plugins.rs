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

/// Live facts the plugin list is built from, so what the dashboard shows is what the server
/// actually knows rather than a fixed description of an ideal deployment.
pub struct PluginContext {
    /// Nodes seen within the online window, by client type ("wander" / "wanda").
    pub online_wander: usize,
    pub online_wanda: usize,
    pub known_wander: usize,
    pub known_wanda: usize,
    /// Whether the account has a Navidrome address on file. Only whether, not what: since
    /// migration 27 the address lives inside a blob the server has no key for, so "Not set" and a
    /// full URL are the only two things it can still tell apart.
    pub navidrome_configured: bool,
    pub lyrics_online: bool,
    /// Whether any session is currently stored for anyone.
    pub has_handoff: bool,
}

fn meta(key: &str, value: impl Into<String>) -> PluginMetaItem {
    PluginMetaItem { key: key.to_string(), value: value.into() }
}

pub fn get_plugins(ctx: &PluginContext) -> Vec<AgroPlugin> {
    vec![
        AgroPlugin {
            id: "wander-tui".to_string(),
            name: "Wander TUI Connector".to_string(),
            description: "Playback handoff for the Wander Rust desktop client: registers as a node, publishes the playing track, position and queue.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Client".to_string(),
            target: "Wander (TUI)".to_string(),
            is_enabled: true,
            is_connected: ctx.online_wander > 0,
            // Nothing here measures round-trip time, so reporting a number would be inventing one.
            latency_ms: None,
            endpoint: Some("/ws/sync".to_string()),
            metadata: vec![
                meta("Listening now", ctx.online_wander.to_string()),
                meta("Registered devices", ctx.known_wander.to_string()),
                meta("Transport", "GraphQL over HTTP, WebSocket for push"),
            ],
        },
        AgroPlugin {
            id: "wanda-android".to_string(),
            name: "Wanda Android Bridge".to_string(),
            description: "Playback handoff for the Wanda Android client: Media3 playback coordination, QR or manual pairing, resume with the full queue.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Client".to_string(),
            target: "Wanda (Android)".to_string(),
            is_enabled: true,
            is_connected: ctx.online_wanda > 0,
            latency_ms: None,
            endpoint: Some("/graphql".to_string()),
            metadata: vec![
                meta("Listening now", ctx.online_wanda.to_string()),
                meta("Registered devices", ctx.known_wanda.to_string()),
                meta("Session stored", if ctx.has_handoff { "Yes" } else { "No" }),
            ],
        },
        AgroPlugin {
            id: "subsonic-navidrome".to_string(),
            name: "Navidrome address sync".to_string(),
            description: "Carries the Navidrome server address and username between clients so a new device knows where to sign in. Credentials are never stored or forwarded.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Backend".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: ctx.navidrome_configured,
            latency_ms: None,
            // The server cannot name the endpoint it is syncing. It holds the address sealed and
            // hands it to the clients unopened, so there is nothing to display here but whether
            // one is set — which is a better description of what this plugin does than the URL
            // was.
            endpoint: None,
            metadata: vec![
                meta(
                    "Server",
                    if ctx.navidrome_configured { "Set — readable only on your devices" } else { "Not set" },
                ),
                meta("Username", "Stored encrypted, alongside the address"),
                meta("Password", "Never synced — entered on each device"),
            ],
        },
        AgroPlugin {
            id: "lrclib-lyrics".to_string(),
            name: "LRCLIB lyrics source".to_string(),
            description: "The synced-lyrics endpoint the clients are told to use. Wander and Wanda fetch lyrics themselves; this is the address they agree on.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Enrichment".to_string(),
            target: "Core".to_string(),
            is_enabled: ctx.lyrics_online,
            is_connected: ctx.lyrics_online,
            latency_ms: None,
            // The default, not the account's configured value: that one is inside the sealed blob
            // now. This is the address the clients fall back to, which is what a deployment
            // overview is actually asking about.
            endpoint: Some("https://lrclib.net/api".to_string()),
            metadata: vec![
                meta("Online lookup", if ctx.lyrics_online { "Enabled" } else { "Disabled" }),
                meta("Fetched by", "The client, not the server"),
            ],
        },
        AgroPlugin {
            id: "ephemeral-share".to_string(),
            name: "Ephemeral share links".to_string(),
            description: "Self-expiring share URLs served at /share/{token}.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Sharing".to_string(),
            target: "Cloud".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: None,
            endpoint: Some("/share/{token}".to_string()),
            metadata: vec![
                meta("Created by", "createEphemeralShare"),
                meta("Token", "UUIDv4"),
            ],
        },
        AgroPlugin {
            id: "listen-along".to_string(),
            name: "Listen along".to_string(),
            description: "Follow a friend's playback in real time. Supersedes the jam-session \
                placeholder, which advertised a query that no longer exists."
                .to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Social".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: None,
            endpoint: Some("/ws/sync".to_string()),
            metadata: vec![
                meta("Started by", "startListenAlong"),
                meta("Pushed as", "LISTEN_ALONG"),
            ],
        },
        AgroPlugin {
            id: "privacy-relay".to_string(),
            name: "Privacy Relay".to_string(),
            description: "Proxies metadata and lyric requests (Internet Archive, LRCLIB, Nyaa) through this server to hide client IP addresses. Caches responses to reduce API calls.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Privacy".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            is_connected: true,
            latency_ms: None,
            endpoint: Some("/api/v1/proxy".to_string()),
            metadata: vec![
                meta("Caching", "Enabled (24 hours)"),
                meta("Target", "Metadata APIs only (no media streams)"),
            ],
        },
    ]
}
