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
    /// Whether an admin can switch it. The client connectors describe what is connected; there is
    /// nothing on the server to turn off, so offering a switch would be offering a fake one.
    pub toggleable: bool,
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
    /// Nodes seen within the online window, by client type ("wander" / "wanda"); any other client is not counted.
    pub online_wander: usize,
    pub online_wanda: usize,
    pub known_wander: usize,
    pub known_wanda: usize,
    /// Whether the account has a Subsonic-compatible server address on file. Only whether, not what: since
    /// migration 27 the address lives inside a blob the server has no key for, so "Not set" and a
    /// full URL are the only two things it can still tell apart.
    pub subsonic_configured: bool,
    pub lyrics_online: bool,
    /// Whether any session is currently stored for anyone.
    pub has_handoff: bool,
}

fn meta(key: &str, value: impl Into<String>) -> PluginMetaItem {
    PluginMetaItem {
        key: key.to_string(),
        value: value.into(),
    }
}

pub fn get_plugins(ctx: &PluginContext, db: &crate::db::Db) -> Vec<AgroPlugin> {
    let mut plugins = vec![
        AgroPlugin {
            id: "wander-tui".to_string(),
            name: "Wander TUI Connector".to_string(),
            description: "Playback handoff for the Wander Rust desktop client: registers as a node, publishes the playing track, position and queue.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Client".to_string(),
            target: "Wander (TUI)".to_string(),
            is_enabled: true,
            toggleable: false,
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
            toggleable: false,
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
            name: "Subsonic server address sync".to_string(),
            description: "Carries the Subsonic-compatible server address (Navidrome, Gonic, Airsonic and the like) and username between clients so a new device knows where to sign in. Credentials are never stored or forwarded.".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: "Backend".to_string(),
            target: "Core".to_string(),
            is_enabled: true,
            toggleable: false,
            is_connected: ctx.subsonic_configured,
            latency_ms: None,
            // The server cannot name the endpoint it is syncing. It holds the address sealed and
            // hands it to the clients unopened, so there is nothing to display here but whether
            // one is set — which is a better description of what this plugin does than the URL
            // was.
            endpoint: None,
            metadata: vec![
                meta(
                    "Server",
                    if ctx.subsonic_configured { "Set — readable only on your devices" } else { "Not set" },
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
            toggleable: false,
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
    ];
    // The switchable server features, described by `features` so the switch and the refusal it
    // controls cannot drift apart.
    plugins.extend(
        crate::features::Feature::ALL
            .into_iter()
            .map(|feature| feature.plugin(db)),
    );
    plugins
}
