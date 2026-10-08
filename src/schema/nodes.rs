//! Registered devices: registering, listing, renaming and removing them.

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::ws::WsHub;
use async_graphql::{Context, Object, SimpleObject};
use std::sync::Arc;

use super::{authorize, require_own_device};

#[derive(SimpleObject, Clone, serde::Serialize)]
pub struct NodePayload {
    pub device_id: String,
    pub user_id: String,
    pub petname: String,
    pub client_type: String,
    pub lan_address: Option<String>,
    /// The *client's* version, as the client reported it. Echoed back, never set here.
    pub version: Option<String>,
    pub current_track: Option<String>,
    pub last_seen_at: String,
    pub is_online: bool,
    /// This server's version.
    ///
    /// Distinct from [`version`], which is whatever the client said about itself. Nothing here
    /// described the server at all before, so a client had no way to find out what it was talking
    /// to except by trying something and reading the error.
    pub server_version: String,
    /// What this server can do that an older one could not.
    ///
    /// Named rather than inferred from [`server_version`]: a client that has to map version
    /// numbers to features carries a table that goes stale, and a fork or a partial deployment
    /// makes the mapping wrong anyway. A client looks for the name it needs and falls back when it
    /// is missing.
    pub capabilities: Vec<String>,
}

/// What this build of the server supports, for clients to branch on.
///
/// Additive: a name that appears here stays, because a client that learned to rely on it is still
/// out there. Removing a feature means the name goes and older clients take their fallback path,
/// which is what the fallback is for.
pub fn server_capabilities() -> Vec<String> {
    vec![
        // The catalogue accepts and returns lyrics alongside the fingerprint.
        "catalog.lyrics".to_string(),
        // ...and records what supplied them.
        "catalog.lyricsSource".to_string(),
        // `publishRecordings` takes a list, so a client need not spend one request per recording.
        "catalog.batchPublish".to_string(),
        // Blends exist: the `blend*` API, and `isBlend` on a playlist.
        "playlists.blends".to_string(),
        // The cloud vault: `vaultBackups`, and the byte routes under `/api/v1/vault/backups`.
        "vault.backups".to_string(),
        // `vaultKeyEnvelope`: a device paired without the passphrase can still unlock the vault.
        "vault.keyEnvelope".to_string(),
    ]
}

/// How long a node stays "online" after it last reported in. Clients heartbeat inside this window
/// while they are playing; anything longer and they show as away.
pub(super) const NODE_ONLINE_SECONDS: i64 = 45;

#[derive(Default)]
pub struct NodesQuery;

#[Object]
impl NodesQuery {
    async fn active_nodes(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<Vec<NodePayload>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let ws_hub = ctx.data::<Arc<WsHub>>().ok();
        let nodes = db.get_active_nodes(&user_id)?;
        let now = chrono::Utc::now();
        let payload = nodes
            .into_iter()
            .map(|n| {
                let is_online =
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&n.last_seen_at) {
                        (now - dt.with_timezone(&chrono::Utc)).num_seconds() < NODE_ONLINE_SECONDS
                    } else {
                        false
                    };
                let lan_address = ws_hub
                    .as_ref()
                    .and_then(|hub| hub.get_lan_address(&user_id, &n.device_id));
                NodePayload {
                    device_id: n.device_id,
                    user_id: n.user_id,
                    petname: n.petname,
                    client_type: n.client_type,
                    lan_address,
                    version: n.version,
                    current_track: n.current_track,
                    last_seen_at: n.last_seen_at,
                    is_online,
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                    capabilities: server_capabilities(),
                }
            })
            .collect();
        Ok(payload)
    }
}

#[derive(Default)]
pub struct NodesMutation;

#[Object]
impl NodesMutation {
    #[allow(clippy::too_many_arguments)]
    async fn register_node(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        client_type: String,
        device_name: Option<String>,
        lan_address: Option<String>,
        version: Option<String>,
        current_track: Option<String>,
    ) -> async_graphql::Result<NodePayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let normalized_client = crate::db::declared_client_type(&client_type)?;

        let existing_nodes = db.get_active_nodes(&user_id).unwrap_or_default();
        // Best name first. The invented one is the last resort, not the default: a device that was
        // named when it was paired now keeps that name here too, instead of appearing as a random
        // animal alongside a token labelled something else. One device, one name.
        let petname = if let Some(custom) = device_name.filter(|s| !s.trim().is_empty()) {
            custom
        } else if let Some(existing) = existing_nodes.iter().find(|n| n.device_id == device_id) {
            existing.petname.clone()
        } else if let Some(label) = ctx
            .data::<AuthedUser>()
            .ok()
            .map(|caller| caller.device_label.trim().to_string())
            .filter(|label| !label.is_empty())
        {
            label
        } else {
            crate::passphrase::generate_random_petname()
        };

        db.upsert_node(
            &device_id,
            &user_id,
            crate::db::NodeName::Set(&petname),
            &normalized_client,
            version.as_deref(),
            current_track.as_deref(),
        )?;

        let payload = NodePayload {
            device_id: device_id.clone(),
            user_id: user_id.clone(),
            petname: petname.clone(),
            client_type: normalized_client,
            lan_address: lan_address.clone(),
            version,
            current_track,
            last_seen_at: chrono::Utc::now().to_rfc3339(),
            is_online: true,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: server_capabilities(),
        };

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            if let Some(addr) = lan_address.as_deref() {
                ws_hub.set_lan_address(&user_id, &device_id, addr);
            }
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

    async fn unregister_node(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let removed = db.delete_node(&user_id, &device_id)?;
        if removed {
            if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
                ws_hub.notify_user(
                    &user_id,
                    "NODE_UPDATE",
                    serde_json::json!({
                        "deviceId": device_id,
                        "deleted": true
                    }),
                );
            }
        }
        Ok(removed)
    }

    /// Renames a device.
    ///
    /// Scoped to devices the caller owns. A name is what makes a device list usable, and until now
    /// the only way to change one was to make the client send a different `deviceName` — which for
    /// a name the *server* invented meant there was no way at all.
    async fn rename_node(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        petname: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        require_own_device(ctx, &device_id)?;
        let petname = petname.trim();
        if petname.is_empty() {
            return Err("A device needs a name".into());
        }
        if petname.chars().count() > 64 {
            return Err("That name is too long".into());
        }
        let db = ctx.data::<Db>()?;
        Ok(db.rename_node(&user_id, &device_id, petname)?)
    }
}
