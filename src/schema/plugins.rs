//! Which plugins are installed, and switching them on and off.

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::plugins::AgroPlugin;
use async_graphql::{Context, Object};

use super::nodes::NODE_ONLINE_SECONDS;
use super::require_admin;

/// Gathers what the server actually knows, so the plugin list describes this deployment
/// rather than a fixed example of one.
/// `caller` is the account whose settings the overview describes. It used to be whichever account
/// happened to own the first registered node, which meant an admin looking at the plugin list was
/// shown someone else's deployment — and, before `plugins` became admin-only, a guest was shown the
/// admin's. The caller is the only defensible answer to "whose settings are these".
fn plugin_context(db: &Db, caller: &str) -> crate::plugins::PluginContext {
    let nodes = db.get_all_nodes().unwrap_or_default();
    let now = chrono::Utc::now();
    let online = |last_seen: &str| {
        chrono::DateTime::parse_from_rfc3339(last_seen)
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_seconds() < NODE_ONLINE_SECONDS)
            .unwrap_or(false)
    };
    let is_wander = |n: &crate::db::NodeRecord| n.client_type == "wander";

    let settings = db.get_synced_settings(caller).ok().flatten();

    crate::plugins::PluginContext {
        online_wander: nodes
            .iter()
            .filter(|n| is_wander(n) && online(&n.last_seen_at))
            .count(),
        online_wanda: nodes
            .iter()
            .filter(|n| !is_wander(n) && online(&n.last_seen_at))
            .count(),
        known_wander: nodes.iter().filter(|n| is_wander(n)).count(),
        known_wanda: nodes.iter().filter(|n| !is_wander(n)).count(),
        navidrome_configured: settings.as_ref().is_some_and(|s| s.has_server_url),
        lyrics_online: settings
            .as_ref()
            .and_then(|s| s.lyrics_fetch_online)
            .unwrap_or(true),
        // The caller's own session, for the same reason as the settings above.
        has_handoff: db.get_handoff(caller).ok().flatten().is_some(),
    }
}

#[derive(Default)]
pub struct PluginsQuery;

#[Object]
impl PluginsQuery {
    /// The plugin registry. Administrators only, and described from the caller's own account:
    /// `plugin_context` used to read whichever account owned the first registered node, so this
    /// answered with a stranger's settings.
    async fn plugins(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<AgroPlugin>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let caller = ctx.data::<AuthedUser>()?.username().to_string();
        let saved_states = db.get_plugin_states().unwrap_or_default();
        let mut plugins = crate::plugins::get_plugins(&plugin_context(db, &caller));
        for p in &mut plugins {
            if let Some(&enabled) = saved_states.get(&p.id) {
                p.is_enabled = enabled;
            }
        }
        Ok(plugins)
    }
}

#[derive(Default)]
pub struct PluginsMutation;

#[Object]
impl PluginsMutation {
    /// Enables or disables a plugin. Administrators only: `plugins_state` has no user column, so
    /// this writes server-global configuration and every account sees the result.
    async fn toggle_plugin(
        &self,
        ctx: &Context<'_>,
        plugin_id: String,
        is_enabled: bool,
    ) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        db.set_plugin_enabled(&plugin_id, is_enabled)?;
        Ok(true)
    }
}
