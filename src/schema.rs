use async_graphql::{Context, Enum, InputObject, Object, Schema, SimpleObject};
use crate::auth::AuthedUser;
use crate::db::{Db, LinkKind};
use crate::db_identity::{AccountState, Role};
use crate::db_library::BrowseKind;
use crate::passphrase::generate_passphrase;
use crate::plugins::AgroPlugin;
use crate::ws::WsHub;
use std::sync::Arc;

/// The schema roots, each merged from a core half and a social half.
///
/// `schema.rs` was already 1700 lines before friends existed; `MergedObject` lets the social
/// resolvers live in their own file without becoming a separate top-level field that clients would
/// have to reach through.
#[derive(async_graphql::MergedObject, Default)]
pub struct Query(
    QueryRoot,
    crate::schema_social::SocialQuery,
    crate::schema_jam::JamQuery,
    crate::schema_feed::FeedQuery,
    crate::schema_drops::DropsQuery,
    crate::schema_playlists::PlaylistsQuery,
);

#[derive(async_graphql::MergedObject, Default)]
pub struct Mutation(
    MutationRoot,
    crate::schema_social::SocialMutation,
    crate::schema_jam::JamMutation,
    crate::schema_drops::DropsMutation,
    crate::schema_playlists::PlaylistsMutation,
);

pub type AgroSchema = Schema<Query, Mutation, async_graphql::EmptySubscription>;

/// Checks the account a caller *named* against the account its token actually proved.
///
/// Every account-scoped resolver takes a `userId` argument, and until this existed every one of
/// them simply believed it — so any valid token could read or write any other account's sessions,
/// settings, devices and library. The argument is kept (both clients send it, and it reads well in
/// the schema) but it is now checked rather than trusted.
///
/// **Fails closed.** This used to return `Ok` when there was no authenticated identity at all, to
/// leave room for a first-run window in the middleware. Both halves of that are gone: setup now
/// needs a token the operator reads from the log, and no identity means no.
fn authorize(ctx: &Context<'_>, user_id: &str) -> async_graphql::Result<()> {
    let authed = caller(ctx)?;
    if authed.username().eq_ignore_ascii_case(user_id.trim()) {
        Ok(())
    } else {
        // Deliberately does not name the account that *was* authenticated — an error message is
        // not the place to disclose it.
        Err(forbidden("that token does not belong to the requested account"))
    }
}

/// The authenticated caller, or an error. The single place an identity enters a resolver.
pub(crate) fn caller<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a AuthedUser> {
    ctx.data_opt::<AuthedUser>()
        .ok_or_else(|| forbidden("this request carries no authenticated account"))
}

/// Requires the caller to own the deployment.
///
/// Guards everything that is the server's rather than an account's: other people's accounts, the
/// plugin registry, the share-forwarding allowlist, and the library itself.
pub(crate) fn require_admin<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a AuthedUser> {
    let authed = caller(ctx)?;
    if authed.is_admin() {
        Ok(authed)
    } else {
        Err(forbidden("this account is not an administrator"))
    }
}

/// Requires that `device_id` belongs to the caller.
///
/// Several resolvers take a device id and passed it straight into SQL that filtered on the device
/// alone. Device ids are chosen by the client, so that let one account read another's holdings,
/// browse its library view, and delete its holding rows. `authorize` cannot catch this on its own:
/// the `userId` argument is the caller's own, and the *device* is the smuggled part.
fn require_own_device(ctx: &Context<'_>, device_id: &str) -> async_graphql::Result<()> {
    let authed = caller(ctx)?;
    let db = ctx.data::<Db>()?;
    let owns = db
        .device_belongs_to(authed.username(), device_id.trim())
        .unwrap_or(false);
    if owns {
        Ok(())
    } else {
        Err(forbidden("that device does not belong to this account"))
    }
}

/// One shape for every refusal, so no error message accidentally becomes an oracle.
pub(crate) fn forbidden(detail: &str) -> async_graphql::Error {
    async_graphql::Error::new(format!("Forbidden: {detail}"))
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

/// The longest a URL may be, anywhere it is accepted.
const MAX_URL_LEN: usize = 2048;

/// The longest any free-text tag may be. Every one of these is stored and rendered somewhere.
const MAX_TAG_LEN: usize = 512;

/// Rejects an over-long string rather than silently truncating it.
pub(crate) fn bounded(raw: &str, max: usize, field: &str) -> async_graphql::Result<String> {
    let clean = raw.trim();
    if clean.chars().count() > max {
        return Err(format!("{field} may be at most {max} characters").into());
    }
    Ok(clean.to_string())
}

/// The longest a username may be. Nothing here had a length limit, and every one of these strings
/// is stored, indexed, and rendered in a dashboard table.
const MAX_USERNAME_LEN: usize = 32;

/// Lower-cases and validates a username.
///
/// Restrictive on purpose: usernames are compared case-insensitively, appear in a URL as a pairing
/// parameter, and are the join key for nearly every table. Allowing whitespace or punctuation
/// invites two accounts that look identical to a human.
pub(crate) fn normalise_username(raw: &str) -> async_graphql::Result<String> {
    let clean = raw.trim().to_lowercase();
    if clean.is_empty() {
        return Err("An account needs a username".into());
    }
    if clean.chars().count() > MAX_USERNAME_LEN {
        return Err(format!("A username may be at most {MAX_USERNAME_LEN} characters").into());
    }
    if !clean.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        return Err("A username may only contain letters, digits, dot, dash and underscore".into());
    }
    Ok(clean)
}


#[derive(SimpleObject, Clone)]
/// An account as its owner sees it.
///
/// Carries **no credential**. It used to return `apiKey` and `passphrase` in cleartext, which meant
/// a revocable device token could be traded for the permanent account passphrase just by asking —
/// the escalation that made revoking a device pointless. A credential is shown once, by the
/// mutation that mints it, and never again.
pub struct AccountPayload {
    pub id: String,
    pub username: String,
    pub role: String,
    pub state: String,
    pub quota_bytes: i64,
    pub can_archive: bool,
    pub connection_url: String,
}

/// Everything a newly created account needs, shown exactly once.
/// A scannable pairing payload, shown once.
#[derive(SimpleObject, Clone)]
pub struct PairingPayload {
    pub qr_data: String,
    pub token: String,
    pub label: String,
}

#[derive(SimpleObject, Clone, serde::Serialize)]
pub struct NodePayload {
    pub device_id: String,
    pub user_id: String,
    pub petname: String,
    pub client_type: String,
    pub lan_address: Option<String>,
    pub version: Option<String>,
    pub current_track: Option<String>,
    pub last_seen_at: String,
    pub is_online: bool,
}

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

#[derive(SimpleObject, Clone)]
pub struct HandoffState {
    pub track_uri: String,
    /// How long the track is. 0 when the sender did not say, or when it is a livestream — both
    /// want a running clock rather than a progress bar that finishes at the wrong moment.
    pub duration_ms: i64,
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
pub struct SharePayload {
    pub token: String,
    pub share_url: String,
    pub expires_at: String,
    pub track_title: String,
    pub artist_name: String,
}

/// One link this account has minted, whichever of the two mechanisms produced it.
#[derive(SimpleObject, Clone)]
pub struct ShareLink {
    pub id: String,
    /// `SHORT` for `/listen?id=…`, `EPHEMERAL` for a hosted `/share/<token>` page.
    pub kind: String,
    /// Where the link goes: the forwarding target, or the hosted audio URL.
    pub target: String,
    /// The full address to hand out.
    pub url: String,
    /// What the link is of, when the row knows. Only ephemeral shares carry track metadata.
    pub label: Option<String>,
    pub created_at: Option<i64>,
    pub expires_at: Option<i64>,
    /// How many times it has been opened. An aggregate and nothing else — see migration 6.
    pub click_count: i64,
    pub last_clicked_at: Option<i64>,
    /// Which backend minted the underlying share, when known. `"navidrome"` matters at deletion.
    pub source: Option<String>,
}

/// A named total: an artist, an album, a genre, a device.
#[derive(SimpleObject, Clone)]
pub struct StatEntry {
    pub name: String,
    /// Plays for the top-N lists; seconds for the per-device breakdown.
    pub value: i64,
}

/// Listening statistics for one account, across every device that reports to it.
#[derive(SimpleObject, Clone)]
pub struct ListeningStats {
    /// The last 24 hours, not since midnight — there is no one timezone across a fleet.
    pub secs_today: i64,
    pub secs_week: i64,
    pub secs_total: i64,
    pub plays_total: i64,
    pub streak: i64,
    pub top_artists: Vec<StatEntry>,
    pub top_albums: Vec<StatEntry>,
    pub top_tracks: Vec<StatEntry>,
    pub top_genres: Vec<StatEntry>,
    /// Seconds per day for the last fourteen days, oldest first.
    pub by_day: Vec<i64>,
    /// Seconds per day for the last eight weeks, oldest first.
    pub heatmap: Vec<i64>,
    /// Seconds per hour of the day, UTC, index 0 = midnight.
    pub by_hour: Vec<i64>,
    /// Seconds per device, most-listened first.
    pub by_device: Vec<StatEntry>,
}

/// One play a client is reporting.
#[derive(InputObject)]
pub struct ScrobbleInput {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: i64,
    /// RFC3339, from the device. A phone that was offline is reporting yesterday's listening, and
    /// stamping it on arrival would pile a week of history onto one afternoon.
    ///
    /// Stored to the hour, not the second: see `Db::record_scrobbles`.
    pub played_at: String,
    /// A per-play id from the client, which is what makes ingest idempotent now that the stored
    /// timestamp is too coarse to tell two plays of one track apart. Optional, for clients that
    /// predate it.
    pub play_uid: Option<String>,
}

/// One tile in the library view.
#[derive(SimpleObject, Clone)]
pub struct LibraryItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// Fetch artwork from `/api/v1/cover/{coverKey}`. Null for artists, and for albums with none.
    pub cover_key: Option<String>,
    pub track_count: i64,
    /// False when the selected device is missing this. Null-ish only in the sense that with no
    /// device selected everything reports true — there is nothing to be missing from.
    pub present_on_device: bool,
    pub source_count: i64,
}

/// What `libraryBrowse` is listing.
#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum LibraryBrowseKind {
    Artist,
    Album,
    Track,
}

/// The outcome of deleting a link.
#[derive(SimpleObject, Clone)]
pub struct DeleteLinkPayload {
    pub deleted: bool,
    /// True when the link pointed at a Navidrome share that Agro cannot revoke on the user's
    /// behalf.
    ///
    /// Agro holds a Navidrome address and username but deliberately never the password — see the
    /// encrypted fields on `synced_settings`, and the "the password stays on each device" rule the
    /// clients are built around. Revoking a share needs that password, so the honest answer is to
    /// remove Agro's own record and say plainly that the share still exists on the music server,
    /// rather than to start storing a credential the whole design avoids.
    pub navidrome_cleanup_required: bool,
}


#[derive(SimpleObject, Clone)]
pub struct LyricsAndCoverPayload {
    pub synced_lrc: String,
    pub cover_art_url: String,
    pub is_synced: bool,
}



#[derive(InputObject)]
pub struct HandoffInput {
    pub user_id: String,
    /// Optional so an older client is still a valid sender; omitted reads as "did not say", which
    /// leaves whatever length is already stored alone.
    pub duration_ms: Option<i64>,
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
        online_wander: nodes.iter().filter(|n| is_wander(n) && online(&n.last_seen_at)).count(),
        online_wanda: nodes.iter().filter(|n| !is_wander(n) && online(&n.last_seen_at)).count(),
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


/// An app password as it is listed back. The token itself is deliberately absent: a credential is
/// shown once, when it is created, and is not recoverable afterwards.
#[derive(SimpleObject, Clone)]
pub struct AppPassword {
    /// Stable handle for this one credential. Labels repeat; this does not.
    pub id: i64,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// An identity provider account linked to an Agro account.
#[derive(SimpleObject, Clone)]
pub struct FederatedIdentity {
    pub issuer: String,
    /// The provider's stable identifier for the person. The only value treated as identity.
    pub subject: String,
    pub linked_at: String,
}

/// One entry in the security log.
#[derive(SimpleObject, Clone)]
pub struct SecurityEventPayload {
    pub id: i64,
    pub at: String,
    /// The account this concerns. Absent on a failed login for a username that does not exist.
    pub user_id: Option<String>,
    /// A stable machine-readable kind — see `audit::Event`.
    pub kind: String,
    /// The network the request came from, truncated to a /24 or /64. Never a full address.
    pub client_ip: Option<String>,
    pub device_label: Option<String>,
    pub detail: Option<String>,
}

/// The one time a token is returned. Shown once, at creation.
#[derive(SimpleObject, Clone)]
pub struct AppPasswordCreated {
    pub label: String,
    pub token: String,
}

/// The outcome of actively purging listening history.
#[derive(SimpleObject, Clone)]
pub struct PurgeScrobblesPayload {
    pub purged_count: i32,
    pub success: bool,
}

#[derive(Default)]
pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn health(&self) -> &'static str {
        "Agro Server OK"
    }

    /// Looks up the target URL for a short link UID.
    ///
    /// Authenticated: the public half of this lives at `/listen`, which is the capability URL
    /// people without an account open. This resolver is the dashboard's, and took no token, so any
    /// caller could walk other accounts' links.
    async fn resolve_short_link(&self, ctx: &Context<'_>, id: String) -> async_graphql::Result<Option<String>> {
        caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = db.get_short_link(&id)?;
        Ok(target)
    }

    /// The account's library, page by page, for looking at rather than syncing.
    ///
    /// `deviceId` picks whose shelf is being compared against: every item comes back with
    /// `presentOnDevice`, which is what lets the view grey out what that device is missing. Omit it
    /// and everything reads as present, because there is nothing to be missing from.
    async fn library_browse(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        kind: LibraryBrowseKind,
        device_id: Option<String>,
        search: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> async_graphql::Result<Vec<LibraryItem>> {
        let include_archive = authorize_library(ctx, &user_id)?;
        if let Some(device) = device_id.as_deref() {
            require_own_device(ctx, device)?;
        }
        let db = ctx.data::<Db>()?;
        let kind = match kind {
            LibraryBrowseKind::Artist => BrowseKind::Artist,
            LibraryBrowseKind::Album => BrowseKind::Album,
            LibraryBrowseKind::Track => BrowseKind::Track,
        };
        // Capped rather than trusted: a page size is a hint from a caller, and an uncapped one is
        // a request to load somebody's whole library into memory.
        let limit = limit.unwrap_or(120).clamp(1, 500);
        let offset = offset.unwrap_or(0).max(0);
        let search = search.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        Ok(db
            .library_browse(
                &user_id,
                device_id.as_deref().filter(|d| !d.is_empty()),
                kind,
                search.as_deref(),
                limit,
                offset,
                include_archive,
            )?
            .into_iter()
            .map(|item| LibraryItem {
                id: item.id,
                title: item.title,
                subtitle: item.subtitle,
                cover_key: item.cover_key,
                track_count: item.track_count,
                present_on_device: item.present_on_device,
                source_count: item.source_count,
            })
            .collect())
    }

    /// Every link this account has minted, newest first.
    ///
    /// Both mechanisms in one list: the user made "a link", and which table it landed in is an
    /// implementation detail they should not have to know to find it again.
    async fn links(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Vec<ShareLink>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let base = public_url();
        let base = base.trim_end_matches('/');
        Ok(db
            .list_links(&user_id)?
            .into_iter()
            .map(|row| ShareLink {
                url: match row.kind {
                    LinkKind::Short => format!("{base}/listen?id={}", row.id),
                    LinkKind::Ephemeral => format!("{base}/share/{}", row.id),
                },
                kind: match row.kind {
                    LinkKind::Short => "SHORT".to_string(),
                    LinkKind::Ephemeral => "EPHEMERAL".to_string(),
                },
                id: row.id,
                target: row.target,
                label: row.label,
                created_at: row.created_at,
                expires_at: row.expires_at,
                click_count: row.click_count,
                last_clicked_at: row.last_clicked_at,
                source: row.source,
            })
            .collect())
    }

    /// Every account on the server. Administrators only — this is the guest list, and a guest
    /// enumerating the other guests is the first step of anything else they might try.
    async fn users(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<AccountPayload>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db.list_accounts()?.iter().map(account_payload).collect())
    }

    /// Looks an account up. **Does not create one** — it used to, through `get_or_create_user`,
    /// which made a read-only-looking query mint accounts as a side effect: opening the dashboard
    /// recreated a deleted account, with a new passphrase, and closed the first-run setup window
    /// behind it. Accounts come from `createAccount` and nowhere else.
    /// The caller's own account. `username` is optional and defaults to whoever is asking.
    ///
    /// It has to be optional, because a client's first question is "who am I?" and it cannot name
    /// itself to ask. The dashboard used to guess — it started from a hard-coded `alpha` and asked
    /// `me(username: "alpha")` — so signing in as anyone else produced a refused query, no
    /// correction, and a page that went on displaying somebody else's name.
    async fn me(
        &self,
        ctx: &Context<'_>,
        username: Option<String>,
    ) -> async_graphql::Result<Option<AccountPayload>> {
        let caller = ctx.data::<AuthedUser>().map_err(|_| forbidden("Unauthorized"))?;
        let subject = username
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| caller.account.username.clone());
        authorize(ctx, &subject)?;
        let db = ctx.data::<Db>()?;
        Ok(db.account(subject.trim())?.as_ref().map(account_payload))
    }

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

    /// Where the account left off.
    ///
    /// `excludeDevice` asks the question a client asks about *the rest of* its fleet: give me the
    /// latest session that is not mine. Optional, so a client that only wants "where was I" —
    /// which is most of them — is unchanged.
    async fn playback_handoff(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        exclude_device: Option<String>,
    ) -> async_graphql::Result<Option<HandoffState>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let rec = match exclude_device.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(device_id) => db.get_handoff_excluding(&user_id, device_id)?,
            None => db.get_handoff(&user_id)?,
        };
        Ok(rec.map(|r| HandoffState {
            track_uri: r.track_uri,
            duration_ms: r.duration_ms,
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

    /// The account's listening, aggregated across every device that reports to it.
    ///
    /// `period` is DAY, WEEK, MONTH, YEAR or ALL. `deviceName` narrows it to one device's plays,
    /// which is how a client answers "what did *I* listen to" while still holding the fleet total.
    async fn listening_stats(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        period: Option<String>,
        device_name: Option<String>,
    ) -> async_graphql::Result<ListeningStats> {
        // Your own always; a friend's only when they have opened their statistics. This used to be
        // `authorize`, which is self-only — so `showStats` was a switch with nothing on the other
        // side of it and a friend's listening could never be read however open they set it.
        crate::schema_social::require_visible(
            ctx,
            &user_id,
            crate::schema_social::Surface::Stats,
        )?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();
        let since = crate::stats::period_start(period.as_deref().unwrap_or("ALL"), now);

        let rows = db.scrobble_rows(
            &user_id,
            device_name.as_deref().filter(|d| !d.is_empty()),
            since.as_deref(),
        )?;
        Ok(to_listening_stats(crate::stats::compute(&rows, 10, now)))
    }

    /// Computes a private Year / Month in Review recap for the account.
    async fn agro_wrapped(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        year: i32,
        month: Option<i32>,
    ) -> async_graphql::Result<AgroWrappedPayload> {
        crate::schema_social::require_visible(
            ctx,
            &user_id,
            crate::schema_social::Surface::Stats,
        )?;
        let db = ctx.data::<Db>()?;
        let rows = db.scrobble_rows(&user_id, None, None)?;
        let wrapped = crate::stats::compute_wrapped(&rows, year, month, 10);
        Ok(AgroWrappedPayload {
            year: wrapped.year,
            month: wrapped.month,
            total_minutes: wrapped.total_minutes,
            total_plays: wrapped.total_plays,
            top_artists: to_entries(wrapped.top_artists),
            top_tracks: to_entries(wrapped.top_tracks),
            top_albums: to_entries(wrapped.top_albums),
            top_genres: to_entries(wrapped.top_genres),
            top_hour_utc: wrapped.top_hour_utc,
            longest_streak_days: wrapped.longest_streak_days,
            new_artists_count: wrapped.new_artists_count,
        })
    }

    async fn active_nodes(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Vec<NodePayload>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let ws_hub = ctx.data::<Arc<WsHub>>().ok();
        let nodes = db.get_active_nodes(&user_id)?;
        let now = chrono::Utc::now();
        let payload = nodes.into_iter().map(|n| {
            let is_online = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&n.last_seen_at) {
                (now - dt.with_timezone(&chrono::Utc)).num_seconds() < NODE_ONLINE_SECONDS
            } else {
                false
            };
            let lan_address = ws_hub.as_ref().and_then(|hub| hub.get_lan_address(&user_id, &n.device_id));
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
            }
        }).collect();
        Ok(payload)
    }

    /// Everything this server holds about the caller, as a JSON string.
    ///
    /// Self-scoped like everything else — an administrator cannot use this to read someone's
    /// listening history, because `authorize` compares the caller to the named account and an admin
    /// is not exempt from it.
    async fn export_my_data(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<String> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let export = db.export_account_data(&user_id)?;
        Ok(serde_json::to_string_pretty(&export)?)
    }

    /// The SSO identities linked to an account. Self-scoped.
    async fn federated_identities(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<Vec<FederatedIdentity>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .federated_identities(&user_id)?
            .into_iter()
            .map(|(issuer, subject, linked_at)| FederatedIdentity {
                issuer,
                subject,
                linked_at,
            })
            .collect())
    }

    /// The security log for one account, or — for an administrator passing no `userId` — the whole
    /// server.
    ///
    /// Self-scoped through `authorize`, so this is not a way to read anyone else's sign-in history:
    /// naming another account is refused exactly as it is everywhere else. The server-wide view is
    /// separately gated on `require_admin`, because "no `userId`" must not read as "any user".
    async fn security_events(
        &self,
        ctx: &Context<'_>,
        user_id: Option<String>,
        limit: Option<i64>,
    ) -> async_graphql::Result<Vec<SecurityEventPayload>> {
        let db = ctx.data::<Db>()?;
        let scope = match user_id.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            Some(user) => {
                authorize(ctx, user)?;
                Some(user.to_string())
            }
            None => {
                require_admin(ctx)?;
                None
            }
        };
        Ok(db
            .security_events(scope.as_deref(), limit.unwrap_or(100))?
            .into_iter()
            .map(|e| SecurityEventPayload {
                id: e.id,
                at: e.at,
                user_id: e.user_id,
                kind: e.kind,
                client_ip: e.client_ip,
                device_label: e.device_label,
                detail: e.detail,
            })
            .collect())
    }

    async fn app_passwords(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Vec<AppPassword>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .list_app_passwords(&user_id)?
            .into_iter()
            .map(|record| AppPassword {
                id: record.id,
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
        let include_archive = authorize_library(ctx, &user_id)?;
        let stats = ctx.data::<Db>()?.library_stats(&user_id, include_archive)?;
        Ok(LibraryStatsPayload {
            track_count: stats.track_count,
            archived_count: stats.archived_count,
            total_bytes: stats.total_bytes,
            spool_bytes: stats.spool_bytes,
        })
    }

    /// What this account has used of its storage allowance.
    ///
    /// Derived from `effective_quota` and `spool_bytes_for` — the same two the upload path checks
    /// against — so the bar a client draws cannot disagree with the answer an upload gets.
    async fn storage_usage(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<StorageUsagePayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let account = db
            .account(user_id.trim())?
            .ok_or_else(|| forbidden("Unauthorized"))?;
        Ok(StorageUsagePayload {
            // Keyed by username, matching what the upload path checks against in `library.rs`.
            used_bytes: db.spool_bytes_for(&account.username)?,
            quota_bytes: account.effective_quota(),
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
        require_own_device(ctx, &device_id)?;
        Ok(ctx.data::<Db>()?.device_holding_hashes(&user_id, &device_id)?)
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
        let db = ctx.data::<Db>()?;
        let ws_hub = ctx.data::<Arc<WsHub>>().ok();
        let tracks = db.missing_on_device(&user_id, &device_id, limit)?;
        Ok(tracks
            .into_iter()
            .map(|t| to_library_payload_with_sources(db, ws_hub.as_deref().map(|a| a.as_ref()), &user_id, t))
            .collect())
    }

    /// How this account should sync — the one answer both clients branch on.
    ///
    /// Derived rather than configured: a deployment that archives and has a Navidrome address on
    /// file is a streaming setup whether or not anyone said so, and a deployment with no library
    /// root cannot be anything but index-only.
    async fn sync_mode(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<SyncMode> {
        authorize(ctx, &user_id)?;
        if !ctx.data::<crate::storage::Storage>()?.archives() {
            return Ok(SyncMode::IndexOnly);
        }
        // Presence is the whole question, and it is now the only part of the settings this server
        // can answer: the address itself is inside a blob it has no key for. The client states the
        // bit explicitly when it saves.
        let has_navidrome = ctx
            .data::<Db>()?
            .get_synced_settings(&user_id)?
            .is_some_and(|s| s.has_server_url);

        Ok(if has_navidrome {
            SyncMode::Navidrome
        } else {
            SyncMode::PeerToPeer
        })
    }

    /// Tracks this device could delete without losing them: the server holds a filed copy.
    ///
    /// Both the index *and* the disk are consulted. An `archived_path` pointing at a file that is
    /// no longer there would otherwise talk a device into deleting its only copy — the index is a
    /// record of what this server did, not proof of what is on the disk now.
    ///
    /// The size is checked rather than the hash. Re-hashing every candidate would read the whole
    /// library on every call; a size mismatch catches the truncated and half-restored cases, and
    /// the bytes were already verified against the declared hash when they were archived.
    async fn reclaimable(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<LibraryTrackPayload>> {
        authorize(ctx, &user_id)?;
        let storage = ctx.data::<crate::storage::Storage>()?;
        // Nothing is reclaimable when the server is not the durable copy.
        let Some(root) = storage.library_root.as_ref() else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(50).clamp(1, MAX_MISSING as i32) as i64;

        Ok(ctx
            .data::<Db>()?
            .reclaimable_on_device(&user_id, &device_id, limit)?
            .into_iter()
            .filter(|track| {
                let Some(relative) = track.archived_path.as_ref() else {
                    return false;
                };
                let Ok(path) = crate::storage::resolve_within(root, std::path::Path::new(relative))
                else {
                    return false;
                };
                std::fs::metadata(&path).is_ok_and(|m| m.len() == track.size_bytes as u64)
            })
            .map(to_library_payload)
            .collect())
    }

    async fn synced_settings(&self, ctx: &Context<'_>, user_id: String) -> async_graphql::Result<Option<SyncedSettingsPayload>> {
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

fn to_listening_stats(stats: crate::stats::Stats) -> ListeningStats {
    ListeningStats {
        secs_today: stats.secs_today,
        secs_week: stats.secs_week,
        secs_total: stats.secs_total,
        plays_total: stats.plays_total,
        streak: stats.streak,
        top_artists: to_entries(stats.top_artists),
        top_albums: to_entries(stats.top_albums),
        top_tracks: to_entries(stats.top_tracks),
        top_genres: to_entries(stats.top_genres),
        by_day: stats.by_day,
        heatmap: stats.heatmap,
        by_hour: stats.by_hour,
        by_device: to_entries(stats.by_device),
    }
}

fn to_entries(pairs: Vec<(String, i64)>) -> Vec<StatEntry> {
    pairs
        .into_iter()
        .map(|(name, value)| StatEntry { name, value })
        .collect()
}

#[derive(SimpleObject, Clone)]
pub struct AgroWrappedPayload {
    pub year: i32,
    pub month: Option<i32>,
    pub total_minutes: i64,
    pub total_plays: i64,
    pub top_artists: Vec<StatEntry>,
    pub top_tracks: Vec<StatEntry>,
    pub top_albums: Vec<StatEntry>,
    pub top_genres: Vec<StatEntry>,
    pub top_hour_utc: Option<i32>,
    pub longest_streak_days: i64,
    pub new_artists_count: i64,
}


/// A very stale client outbox should arrive in batches rather than in one request the server has
/// to hold in memory whole.
const MAX_SCROBBLE_BATCH: usize = 500;

/// The address clients should be told to connect to. `localhost` was hardcoded here, which made
/// the pairing QR unusable from a phone — and the QR carried no `server` parameter at all, which
/// is the one field the Android client needs to know where to connect.
fn public_url() -> String {
    std::env::var("AGRO_PUBLIC_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://agro.kolbxyz.xyz".to_string())
}

fn account_payload(account: &crate::db_identity::Account) -> AccountPayload {
    AccountPayload {
        id: account.id.clone(),
        username: account.username.clone(),
        role: account.role.as_str().to_string(),
        state: account.state.as_str().to_string(),
        quota_bytes: account.quota_bytes,
        can_archive: account.can_archive(),
        connection_url: public_url(),
    }
}

/// The pairing payload a device scans.
///
/// Carries a freshly minted **device token**, not the account passphrase. The old QR embedded the
/// passphrase, so photographing it once handed over the account permanently rather than one
/// revocable device.
/// Who may look at `subject`'s library, and whether the server's archive counts as part of it.
///
/// Three answers, in order:
///   * your own library — always, and for an administrator that includes the server archive,
///     which belongs to whoever runs the instance;
///   * an accepted friend's, but only the tracks their devices actually hold, and only when they
///     have turned `shareLibrary` on;
///   * otherwise nothing.
///
/// Being an administrator deliberately does *not* grant a view of somebody else's library. Running
/// the server is a reason to see the server's own archive, not a reason to read the collections of
/// the people using it.
fn authorize_library(ctx: &Context<'_>, subject: &str) -> async_graphql::Result<bool> {
    let caller = ctx.data::<AuthedUser>().map_err(|_| forbidden("Unauthorized"))?;
    let db = ctx.data::<Db>()?;
    let subject = subject.trim();

    if caller.account.username.eq_ignore_ascii_case(subject) {
        // The archive is the operator's own copy of the fleet's music.
        return Ok(caller.account.role == Role::Admin);
    }

    let shared = db
        .profile(subject)?
        .map(|profile| profile.share_library)
        .unwrap_or(false);
    if shared && db.are_friends(&caller.account.username, subject)? {
        return Ok(false);
    }
    Err(forbidden("that library is not shared with you"))
}

fn pairing_qr(username: &str, device_token: &str) -> String {
    let server = public_url();
    if server.is_empty() {
        format!("agro://connect?username={username}&token={device_token}")
    } else {
        format!(
            "agro://connect?username={}&token={}&server={}",
            username,
            device_token,
            urlencoding::encode(&server)
        )
    }
}

// ── Library index ───────────────────────────────────────────────────────────────────────────

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

/// How this deployment moves music between devices.
///
/// The clients used to decide this individually, from local config that knew nothing about the
/// server — which is why the same account could behave differently on the desktop and the phone.
/// The server holds every fact the decision needs, so it makes the decision and the clients
/// render it.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SyncMode {
    /// A Navidrome is configured for this account and the server archives. Devices do not need
    /// their own copies — they stream — so downloads are never offered and freeing space is
    /// always safe.
    Navidrome,
    /// The server archives, but there is no Navidrome to stream from. A device that lacks a
    /// recording is offered the file itself.
    PeerToPeer,
    /// No library root: the server keeps the index and relays through the spool, but never keeps
    /// the bytes. It cannot be the durable copy, so it never suggests deleting one.
    IndexOnly,
}

/// How much of an account's allowance is gone.
///
/// A separate field rather than two more columns on [`LibraryStatsPayload`], because it answers a
/// different question and is computed differently. `total_bytes` counts every archived track in
/// the deployment into every account's total, so it reads the same for a guest holding nothing as
/// for the admin — see `Db::spool_bytes_for`. The quota is enforced against the spool, so that is
/// what is reported here.
///
/// `quota_bytes` is null when the account is uncapped, which is not the same as a quota of zero:
/// the admin owns the disk, and a quota on them is theatre. Clients must show "no limit" for null
/// rather than a full bar.
#[derive(SimpleObject, Clone)]
pub struct StorageUsagePayload {
    pub used_bytes: i64,
    pub quota_bytes: Option<i64>,
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
        peer_sources: Vec::new(),
    }
}

fn to_library_payload_with_sources(
    db: &Db,
    ws_hub: Option<&WsHub>,
    user_id: &str,
    t: crate::db_library::LibraryTrack,
) -> LibraryTrackPayload {
    let mut peer_sources = Vec::new();
    if let Ok(sources) = db.peer_sources_for_track(user_id, &t.content_hash) {
        for s in sources {
            let is_online = chrono::DateTime::parse_from_rfc3339(&s.last_seen_at)
                .map(|seen| (chrono::Utc::now() - seen.with_timezone(&chrono::Utc)).num_seconds() < 60)
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

/// Most a diff returns in one go. The offer is a prompt, not a migration plan.
const MAX_MISSING: i64 = 200;

#[derive(Default)]
pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Removes a track from the library entirely. It will disappear from all views.
    async fn delete_library_item(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        kind: LibraryBrowseKind,
        id: String,
    ) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        let db = ctx.data::<crate::db::Db>()?;
        let db_kind = match kind {
            LibraryBrowseKind::Artist => crate::db_library::BrowseKind::Artist,
            LibraryBrowseKind::Album => crate::db_library::BrowseKind::Album,
            LibraryBrowseKind::Track => crate::db_library::BrowseKind::Track,
        };
        Ok(db.delete_library_item(db_kind, &id)?)
    }

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
        let normalized_client = if client_type.to_lowercase().contains("wanda") {
            "wanda".to_string()
        } else {
            "wander".to_string()
        };

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

    /// Registers this account's vault key, sealed by the client under the account passphrase.
    ///
    /// The server takes two opaque strings and can do nothing with either: unwrapping needs the
    /// passphrase, and it keeps only an Argon2 hash of that. What it is storing is the means for
    /// the *user* to recover their settings on a new device, not the means for this machine to read
    /// them.
    ///
    /// Enrolment is once per account. A second attempt returns `false` rather than erroring —
    /// two devices racing to set up the same account is an ordinary thing to happen, and the loser
    /// should fetch the winner's envelope and unwrap it, not treat the race as a failure.
    async fn enrol_vault_key(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        vault_salt: String,
        vault_key_wrapped: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        // Bounded so the column cannot be used as free storage. A salt and a sealed 32-byte key
        // are both far smaller than this; the limit only has to be obviously sufficient.
        for (name, value) in [("vaultSalt", &vault_salt), ("vaultKeyWrapped", &vault_key_wrapped)] {
            if value.trim().is_empty() || value.len() > 512 {
                return Err(format!("{name} is missing or too long").into());
            }
        }
        Ok(ctx
            .data::<Db>()?
            .enrol_vault_key(&user_id, vault_salt.trim(), vault_key_wrapped.trim())?)
    }

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

    /// Creates a short UID for a share URL. Returns the short link UID (e.g. "aB3x9Q").
    ///
    /// `source` records which backend minted the underlying share — `"navidrome"` when the link
    /// points at a Navidrome share, so deleting it later can also revoke it there.
    async fn create_short_link(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        target_url: String,
        source: Option<String>,
        expires_at: Option<i64>,
    ) -> async_graphql::Result<String> {
        // A link attributed to an account has to be authorised as that account. `userId` used to
        // be optional, and omitting it skipped this check entirely — minting an unowned link that
        // no account could then list or revoke.
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let target_url = target_url.trim();
        if target_url.is_empty() {
            return Err("Target URL cannot be empty".into());
        }
        if target_url.chars().count() > MAX_URL_LEN {
            return Err(format!("A URL may be at most {MAX_URL_LEN} characters").into());
        }
        // A forwarder that will point at any scheme is a phishing primitive wearing the operator's
        // domain. `/listen` checks the host against an allowlist; this checks the scheme.
        if !target_url.starts_with("https://") && !target_url.starts_with("http://") {
            return Err("A link target must be an http or https URL".into());
        }
        use rand::Rng;
        const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let mut rng = rand::thread_rng();
        let uid: String = (0..7)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect();

        db.create_short_link(&uid, target_url, Some(user_id.as_str()), source.as_deref(), expires_at)?;
        Ok(uid)
    }

    /// Ingests a batch of plays from one device.
    ///
    /// Batched because clients hold an outbox and drain it when they next have a connection — a
    /// phone that spent a day on aeroplane mode sends a day of listening in one request. Idempotent
    /// on (account, artist, title, time), so a client unsure whether its last upload landed can
    /// simply send it again. Returns how many rows were genuinely new.
    async fn record_scrobbles(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_name: String,
        client_type: Option<String>,
        entries: Vec<ScrobbleInput>,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        if entries.is_empty() {
            return Ok(0);
        }
        if entries.len() > MAX_SCROBBLE_BATCH {
            return Err(format!("at most {MAX_SCROBBLE_BATCH} plays per request").into());
        }

        let rows: Vec<crate::db::ScrobbleEntry> = entries
            .into_iter()
            .map(|entry| crate::db::ScrobbleEntry {
                track_title: entry.track_title,
                artist_name: entry.artist_name,
                album_name: entry.album_name,
                genre: entry.genre,
                duration_secs: entry.duration_secs.max(0),
                played_at: entry.played_at,
                play_uid: entry.play_uid,
            })
            .collect();

        let db = ctx.data::<Db>()?;
        let inserted = db.record_scrobbles(&user_id, &device_name, client_type.as_deref(), &rows)?;
        Ok(inserted as i32)
    }

    /// Purges listening history (scrobbles) for the account, optionally restricted by year or cutoff.
    ///
    /// Useful for wiping past listening data after viewing a Rewind or on demand.
    async fn purge_scrobbles(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        year: Option<i32>,
        before: Option<String>,
    ) -> async_graphql::Result<PurgeScrobblesPayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let count = db.purge_scrobbles(&user_id, year, before.as_deref())?;
        Ok(PurgeScrobblesPayload {
            purged_count: count as i32,
            success: true,
        })
    }

    /// Removes a link so it stops resolving.
    ///
    /// `kind` is the discriminator from `links` — `SHORT` or `EPHEMERAL`. Deleting is scoped to the
    /// owning account inside the statement itself, so a link belonging to somebody else is a
    /// not-found rather than a deletion.
    async fn delete_link(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        id: String,
        kind: String,
    ) -> async_graphql::Result<DeleteLinkPayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let kind = match kind.to_ascii_uppercase().as_str() {
            "SHORT" => LinkKind::Short,
            "EPHEMERAL" => LinkKind::Ephemeral,
            other => return Err(format!("Unknown link kind: {other}").into()),
        };

        let source = db
            .delete_link(&user_id, &id, kind)
            .map_err(|_| async_graphql::Error::new("No such link on this account"))?;

        Ok(DeleteLinkPayload {
            deleted: true,
            navidrome_cleanup_required: source.as_deref() == Some("navidrome"),
        })
    }

    // `createAccount` used to live here: an administrator could mint an account directly, with a
    // passphrase handed back in the response and the account active immediately. It is gone
    // deliberately. Accounts come from `POST /api/v1/signup` and nowhere else, so that every
    // account is subject to the same rules — the username check, the rate limiter, the approval
    // queue — rather than those rules applying to strangers and not to the people an admin adds.
    // An admin who wants to let someone in mints an invite code, which skips the queue without
    // skipping the process.

    /// Approves, suspends or restores an account.
    async fn set_account_state(
        &self,
        ctx: &Context<'_>,
        username: String,
        state: String,
    ) -> async_graphql::Result<AccountPayload> {
        let admin = require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = normalise_username(&username)?;
        let next = AccountState::parse(&state);

        // An admin who suspends themselves locks the deployment out of its own controls, and
        // nothing else can restore them.
        if target.eq_ignore_ascii_case(admin.username()) && !next.is_active() {
            return Err(forbidden("an administrator cannot deactivate their own account"));
        }
        if !db.set_account_state(&target, next)? {
            return Err("No such account".into());
        }
        db.record_event(
            crate::audit::Event::AccountStateChanged,
            crate::audit::Record::new()
                .user(&target)
                .detail(format!("set to {} by {}", next.as_str(), admin.username())),
        );
        Ok(db.account(&target)?.as_ref().map(account_payload).expect("just updated"))
    }

    /// Sets how much spool a guest may occupy, in bytes. `0` means unlimited.
    async fn set_account_quota(
        &self,
        ctx: &Context<'_>,
        username: String,
        quota_bytes: i64,
    ) -> async_graphql::Result<AccountPayload> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = normalise_username(&username)?;
        if !db.set_account_quota(&target, quota_bytes)? {
            return Err("No such account".into());
        }
        Ok(db.account(&target)?.as_ref().map(account_payload).expect("just updated"))
    }

    /// Allows or disallows a user from permanently saving music into the server's archive.
    async fn set_can_archive(
        &self,
        ctx: &Context<'_>,
        username: String,
        can_archive: bool,
    ) -> async_graphql::Result<AccountPayload> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = normalise_username(&username)?;
        if !db.set_can_archive(&target, can_archive)? {
            return Err("No such account".into());
        }
        Ok(db.account(&target)?.as_ref().map(account_payload).expect("just updated"))
    }

    /// Mints a device token and returns it as a scannable pairing payload.
    ///
    /// The pairing QR used to be built from the account passphrase, so photographing it once handed
    /// over the account permanently. Each scan now gets its own revocable credential, which is why
    /// this is a mutation: it creates something.
    async fn pair_device(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        label: Option<String>,
    ) -> async_graphql::Result<PairingPayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let label = label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .ok_or("Name the device, so its token can be told apart from the others")?
            .to_string();
        let token = db.mint_device_token(&user_id, &label)?;
        db.record_event(
            crate::audit::Event::TokenMinted,
            crate::audit::Record::new()
                .user(&user_id)
                .device(label.clone())
                .detail("paired by QR"),
        );
        Ok(PairingPayload {
            qr_data: pairing_qr(&user_id, &token),
            token,
            label,
        })
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
        // Minted, not generated from the passphrase wordlist, and stored as a hash. The previous
        // form wrote a plaintext four-word token straight into the legacy column, which the
        // hashed-token lookup cannot match — so every credential this issued was dead on arrival.
        let token = db.mint_device_token(&user_id, &label)?;
        db.record_event(
            crate::audit::Event::TokenMinted,
            crate::audit::Record::new().user(&user_id).device(label.clone()),
        );
        Ok(AppPasswordCreated { label, token })
    }

    /// Revokes one credential by the id `appPasswords` reported.
    ///
    /// Deliberately not by label. A label is a human note, chosen by the client and freely
    /// repeated — a client that re-logs in on every launch leaves a row each time, all of them
    /// named the same thing. Revoking by label signed out every one of them at once, which is
    /// precisely the opposite of the per-device revocation these credentials exist to provide.
    async fn revoke_app_password(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        id: i64,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let revoked = db.revoke_app_password(&user_id, id)?;
        if revoked {
            db.record_event(
                crate::audit::Event::TokenRevoked,
                crate::audit::Record::new().user(&user_id).detail(format!("credential {id}")),
            );
        }
        Ok(revoked)
    }

    /// Removes a linked SSO identity from the caller's account.
    ///
    /// Refuses when it would remove the last way in — an account created through SSO has a
    /// passphrase it has never been shown, so unlinking without setting one first is a lockout.
    async fn unlink_federated_identity(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        issuer: String,
        subject: String,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let removed = db
            .unlink_federated_identity(&user_id, &issuer, &subject)
            .map_err(async_graphql::Error::new)?;
        if removed {
            db.record_event(
                crate::audit::Event::IdentityUnlinked,
                crate::audit::Record::new()
                    .user(&user_id)
                    .detail(format!("{issuer} subject {subject}")),
            );
        }
        Ok(removed)
    }

    /// Changes the caller's passphrase, re-sealing the settings vault under the new one.
    ///
    /// The client does the sealing: it unwraps the vault key with the old passphrase, wraps it
    /// again with the new one, and sends both halves. The server never sees either passphrase in a
    /// form it could keep, and never sees the vault key at all — the same property the vault had
    /// before this mutation existed.
    ///
    /// **Every device is signed out, including the caller's.** A passphrase is changed because it
    /// may have leaked, and the tokens bought with it are the thing being invalidated.
    async fn change_passphrase(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        current_passphrase: String,
        new_passphrase: String,
        new_vault_salt: Option<String>,
        new_vault_key_wrapped: Option<String>,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;

        // Both halves of the envelope or neither. One without the other would write a salt that
        // does not match the wrapped key, which is a vault nothing can open.
        let vault = match (new_vault_salt.as_deref(), new_vault_key_wrapped.as_deref()) {
            (Some(salt), Some(wrapped)) => Some((salt, wrapped)),
            (None, None) => None,
            _ => {
                return Err(
                    "Send both newVaultSalt and newVaultKeyWrapped, or neither".into()
                )
            }
        };

        let changed = db
            .change_passphrase(&user_id, &current_passphrase, &new_passphrase, vault)
            .map_err(async_graphql::Error::new)?;
        if !changed {
            return Err("That passphrase was not accepted".into());
        }
        db.record_event(
            crate::audit::Event::PassphraseChanged,
            crate::audit::Record::new().user(&user_id),
        );
        Ok(true)
    }

    /// Signs out every other device on the account.
    ///
    /// The blunt instrument that per-device revocation does not cover: a passphrase that may have
    /// leaked has already been traded for tokens, and revoking them one at a time from a list means
    /// noticing every one of them. This spares only the device making the call — by token hash, not
    /// by label, for the same reason `revokeAppPassword` refuses to work by label.
    ///
    /// Returns how many were revoked.
    async fn revoke_all_devices(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<i64> {
        authorize(ctx, &user_id)?;
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        // An empty hash would spare nothing and sign the caller out too. That is the correct
        // reading of "this request did not arrive with a token" — it only happens in a test
        // harness — but it must be deliberate rather than an accident of an empty string.
        let spare = Some(authed.token_hash.as_str()).filter(|h| !h.is_empty());
        let revoked = db.revoke_all_tokens(&user_id, spare)?;
        db.record_event(
            crate::audit::Event::AllTokensRevoked,
            crate::audit::Record::new()
                .user(&user_id)
                .device(authed.device_label.clone())
                .detail(format!("{revoked} revoked")),
        );
        Ok(revoked as i64)
    }

    /// Deletes an account with its nodes, session, settings and app passwords.
    ///
    /// Irreversible, and it can lock you out: deleting the last account puts the server back into
    /// first-run, where anyone who can reach it may create the next one. The caller confirms.
    /// Removes an account and everything belonging to it.
    ///
    /// An administrator may remove anyone; anyone else may only remove themselves. Only the first
    /// half of that used to be true — the check was `authorize`, which compares the caller to the
    /// named account, so the admin could delete nobody but themselves and a guest account could
    /// never be got rid of at all.
    ///
    /// The last administrator is protected. Deleting them would leave a server whose accounts
    /// nobody can administer, and which no setup token can recover: one is only minted for a
    /// database with *no* accounts, and the guests would still be there.
    async fn delete_account(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let target = normalise_username(&username)?;
        let is_self = authed.username().eq_ignore_ascii_case(&target);

        if !is_self && !authed.is_admin() {
            return Err(forbidden("only an administrator can remove another account"));
        }

        let db = ctx.data::<Db>()?;
        let Some(account) = db.account(&target)? else {
            return Ok(false);
        };
        if account.is_admin() && db.admin_count()? <= 1 {
            return Err(forbidden("the last administrator cannot be removed"));
        }

        let removed = db.delete_user(&target)?;
        if removed {
            // Recorded against the *actor*, not the deleted account: rows keyed to a username that
            // no longer exists are exactly what a scoped audit query cannot show anyone, and an
            // account being deleted is something the admin who did it should have to answer for.
            db.record_event(
                crate::audit::Event::AccountDeleted,
                crate::audit::Record::new()
                    .user(authed.username())
                    .detail(format!("removed account {target}")),
            );
        }
        Ok(removed)
    }

    /// Enables or disables a plugin. Administrators only: `plugins_state` has no user column, so
    /// this writes server-global configuration and every account sees the result.
    async fn toggle_plugin(&self, ctx: &Context<'_>, plugin_id: String, is_enabled: bool) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
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
            input.duration_ms.unwrap_or(0).max(0),
            input.is_playing,
            &input.device_id,
            queue_json.as_deref(),
            input.queue_index.map(|i| i as i64),
        )?;

        let track_summary = format!("{} • {}", input.track_title, input.artist_name);
        // A handoff reports what is playing, not what the device is called: the name it already
        // has stands, and the invented one is only for a device seen here first.
        let petname = crate::passphrase::generate_random_petname();
        let client_type = if input.device_id.to_lowercase().contains("android") || input.device_id.to_lowercase().contains("wanda") {
            "wanda"
        } else {
            "wander"
        };
        let _ = db.upsert_node(
            &input.device_id,
            &input.user_id,
            crate::db::NodeName::KeepOr(&petname),
            client_type,
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
            crate::schema_social::fan_out_presence(db, ws_hub, &input.user_id);
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
            if let Ok(offers) = ctx.data::<crate::offers::OfferBatcher>() {
                offers.note_archived(&user_id);
            }
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
        require_own_device(ctx, &device_id)?;
        Ok(ctx.data::<Db>()?.forget_holdings(&user_id, &device_id, &hashes)? as i32)
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
        // Was hardcoded to localhost, which made every ephemeral share unopenable from any device
        // but the server itself.
        let share_url = format!("{}/share/{}", public_url().trim_end_matches('/'), token);
        let expires_at = (chrono::Utc::now() + chrono::Duration::hours(ttl)).to_rfc3339();

        Ok(SharePayload {
            token,
            share_url,
            expires_at,
            track_title,
            artist_name,
        })
    }

    /// Looks a track's lyrics up at LRCLIB.
    ///
    /// Authenticated, and the inputs are bounded. This took no token at all and made an outbound
    /// HTTP request per call with strings the caller chose — an unauthenticated amplification
    /// primitive pointed at someone else's server.
    async fn fetch_lyrics_and_cover(&self, ctx: &Context<'_>, artist: String, title: String) -> async_graphql::Result<LyricsAndCoverPayload> {
        caller(ctx)?;
        let artist = bounded(&artist, MAX_TAG_LEN, "artist")?;
        let title = bounded(&title, MAX_TAG_LEN, "title")?;
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
