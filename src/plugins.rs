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
    /// From `synced_settings`: the Navidrome the clients agreed on, and the lyrics source.
    pub navidrome_url: Option<String>,
    pub navidrome_username: Option<String>,
    pub lrclib_url: Option<String>,
    pub lyrics_online: bool,
    /// Whether any session is currently stored for anyone.
    pub has_handoff: bool,
}

fn meta(key: &str, value: impl Into<String>) -> PluginMetaItem {
    PluginMetaItem { key: key.to_string(), value: value.into() }
}

/// Marks the features whose resolvers still answer with sample data. Saying so in the list is the
/// difference between a roadmap and a lie about what is running.
fn preview(mut plugin: AgroPlugin) -> AgroPlugin {
    plugin.is_connected = false;
    plugin.latency_ms = None;
    plugin.metadata.insert(0, meta("Status", "Preview — resolver returns sample data"));
    plugin
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
            is_connected: ctx.navidrome_url.is_some(),
            latency_ms: None,
            endpoint: ctx.navidrome_url.clone(),
            metadata: vec![
                meta("Server", ctx.navidrome_url.clone().unwrap_or_else(|| "Not set".to_string())),
                meta("Username", ctx.navidrome_username.clone().unwrap_or_else(|| "Not set".to_string())),
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
            endpoint: Some(
                ctx.lrclib_url.clone().unwrap_or_else(|| "https://lrclib.net/api".to_string()),
            ),
            metadata: vec![
                meta("Online lookup", if ctx.lyrics_online { "Enabled" } else { "Disabled" }),
                meta("Fetched by", "The client, not the server"),
            ],
        },
        preview(AgroPlugin {
            id: "acoustid-dedup".to_string(),
            name: "Duplicate detection".to_string(),
            description: "Finds the same recording held in several formats. The `duplicatesReport` query is scaffolded and answers with sample data.".to_string(),
            version: "0.0.0".to_string(),
            category: "Curation".to_string(),
            target: "Core".to_string(),
            is_enabled: false,
            is_connected: false,
            latency_ms: None,
            endpoint: None,
            metadata: vec![meta("Query", "duplicatesReport")],
        }),
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
        preview(AgroPlugin {
            id: "jam-session".to_string(),
            name: "Jam collaborative listening".to_string(),
            description: "Shared listening room with a voted queue. The `jamRoomState` query is scaffolded and answers with sample data.".to_string(),
            version: "0.0.0".to_string(),
            category: "Social".to_string(),
            target: "Core".to_string(),
            is_enabled: false,
            is_connected: false,
            latency_ms: None,
            endpoint: None,
            metadata: vec![meta("Query", "jamRoomState")],
        }),
    ]
}
