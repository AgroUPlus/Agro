//! Settings an account syncs across its devices.

use crate::db::Db;
use crate::ws::WsHub;
use async_graphql::{Context, InputObject, Object, SimpleObject};
use std::sync::Arc;

use super::{authorize, require_admin, MAX_URL_LEN};

#[derive(SimpleObject, Clone)]
pub struct SyncedSettingsPayload {
    pub user_id: String,
    /// The account's upstream settings, as the client sealed them. This server cannot read it and
    /// has no key to try: it is handed back exactly as it arrived.
    pub settings_blob: Option<String>,
    /// Whether the blob contains a server address, which is all `syncMode` ever needed to know.
    pub has_server_url: bool,
    pub lyrics_fetch_online: bool,
    pub stream_format: String,
    /// The domain the players rewrite share links onto, e.g. `frwd.top`. Empty means they each
    /// share their backend's own link, which is also what happens with no Agro at all.
    pub share_domain: Option<String>,
    /// Comma-separated hosts `/listen` will forward to. The allowlist, in other words: without
    /// one, the route would be an open redirect wearing the user's domain.
    pub share_hosts: Option<String>,
    pub share_enabled: bool,
    pub updated_at: String,
}

#[derive(InputObject)]
pub struct SyncedSettingsInput {
    pub user_id: String,
    /// Sealed by the client before it is sent. The server stores it without looking.
    pub settings_blob: Option<String>,
    pub has_server_url: Option<bool>,
    pub lyrics_fetch_online: Option<bool>,
    pub stream_format: Option<String>,
    pub share_domain: Option<String>,
    pub share_hosts: Option<String>,
    pub share_enabled: Option<bool>,
}

/// A comma-separated allowlist, checked host by host.
fn validate_share_hosts(raw: &str) -> async_graphql::Result<()> {
    if raw.chars().count() > MAX_URL_LEN {
        return Err("That host list is too long".into());
    }
    for host in raw.split(',').map(str::trim).filter(|h| !h.is_empty()) {
        validate_host(host)?;
    }
    Ok(())
}

/// A bare hostname — no scheme, no path, no credentials, no wildcard.
///
/// Anything looser than this stops being a hostname and starts being a URL the forwarder would
/// happily paste into a `Location` header.
fn validate_host(raw: &str) -> async_graphql::Result<()> {
    let host = raw.trim();
    if host.is_empty() || host.len() > 253 {
        return Err(format!("`{host}` is not a valid hostname").into());
    }
    let shaped = host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host.contains('.');
    if !shaped {
        return Err(format!("`{host}` is not a valid hostname").into());
    }
    Ok(())
}

#[derive(Default)]
pub struct SettingsQuery;

#[Object]
impl SettingsQuery {
    async fn synced_settings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<Option<SyncedSettingsPayload>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let settings = db.get_synced_settings(&user_id)?;

        Ok(settings.map(|s| {
            SyncedSettingsPayload {
                user_id,
                // Returned exactly as stored. There is no decryption step here any more, and no
                // key on this machine that could perform one — which is the entire point of
                // migration 27.
                settings_blob: s.settings_blob,
                has_server_url: s.has_server_url,
                lyrics_fetch_online: s.lyrics_fetch_online.unwrap_or(true),
                stream_format: s.stream_format.unwrap_or_else(|| "FLAC".to_string()),
                // Plaintext, unlike the blob: these are the operator's forwarding policy, enforced
                // by this server on a public route, so it has to be able to read them. See
                // migration 4 in `db`.
                share_domain: s.share_domain,
                share_hosts: s.share_hosts,
                share_enabled: s.share_enabled.unwrap_or(false),
                updated_at: s.updated_at,
            }
        }))
    }
}

#[derive(Default)]
pub struct SettingsMutation;

#[Object]
impl SettingsMutation {
    async fn update_synced_settings(
        &self,
        ctx: &Context<'_>,
        input: SyncedSettingsInput,
    ) -> async_graphql::Result<SyncedSettingsPayload> {
        authorize(ctx, &input.user_id)?;

        // The share fields are not ordinary preferences: `/listen` reads them to decide where it
        // will forward a visitor, and it is served from the operator's own domain. A guest able to
        // widen that allowlist has an open redirect wearing someone else's reputation.
        let touches_sharing = input.share_domain.is_some()
            || input.share_hosts.is_some()
            || input.share_enabled.is_some();
        if touches_sharing {
            require_admin(ctx)?;
            if let Some(hosts) = input.share_hosts.as_deref() {
                validate_share_hosts(hosts)?;
            }
            if let Some(domain) = input.share_domain.as_deref() {
                validate_host(domain)?;
            }
        }

        let db = ctx.data::<Db>()?;

        // Stored as received. The client sealed the blob before sending it, and this server has
        // neither the key nor a reason to want one.
        db.upsert_synced_settings(
            &input.user_id,
            input.settings_blob.as_deref(),
            input.has_server_url,
            input.lyrics_fetch_online,
            input.stream_format.as_deref(),
            crate::db::ShareSettingsInput {
                domain: input.share_domain.as_deref(),
                hosts: input.share_hosts.as_deref(),
                enabled: input.share_enabled,
            },
        )?;

        let settings = db.get_synced_settings(&input.user_id)?.unwrap();
        let payload = SyncedSettingsPayload {
            user_id: input.user_id.clone(),
            settings_blob: settings.settings_blob,
            has_server_url: settings.has_server_url,
            lyrics_fetch_online: settings.lyrics_fetch_online.unwrap_or(true),
            stream_format: settings.stream_format.unwrap_or_else(|| "FLAC".to_string()),
            share_domain: settings.share_domain,
            share_hosts: settings.share_hosts,
            share_enabled: settings.share_enabled.unwrap_or(false),
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
}
