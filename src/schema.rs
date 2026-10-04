//! The GraphQL schema: its two roots, and the checks every resolver shares.
//!
//! The resolvers themselves live under `schema/`, one file per subject, and in the `schema_*` files
//! beside this one. Each defines its own `Query` and `Mutation` objects, merged into the roots here.

use crate::auth::AuthedUser;
use crate::db::Db;
use async_graphql::{Context, Object, Schema};

mod accounts;
mod app_passwords;
mod handoff;
mod handoff_input;
mod holdings;
mod library;
mod library_album;
mod library_payload;
mod library_stats;
mod links;
mod lyrics;
mod nodes;
mod pairing;
mod plugins;
mod scrobbles;
mod security;
mod settings;
mod stats;
mod sync_offers;

pub use stats::StatEntry;

/// The schema roots, merged from one object per subject.
///
/// `MergedObject` lets each subject's resolvers live in their own file — under `schema/`, or in a
/// `schema_*` file beside this one — without becoming a separate top-level field that clients would
/// have to reach through.
#[derive(async_graphql::MergedObject, Default)]
pub struct Query(
    HealthQuery,
    accounts::AccountsQuery,
    app_passwords::AppPasswordsQuery,
    handoff::HandoffQuery,
    holdings::HoldingsQuery,
    library::LibraryQuery,
    library_album::LibraryAlbumQuery,
    library_stats::LibraryStatsQuery,
    links::LinksQuery,
    nodes::NodesQuery,
    plugins::PluginsQuery,
    security::SecurityQuery,
    settings::SettingsQuery,
    stats::StatsQuery,
    sync_offers::SyncOffersQuery,
    crate::schema_social::SocialQuery,
    crate::schema_jam::JamQuery,
    crate::schema_jam_recap::JamRecapQuery,
    crate::schema_feed::FeedQuery,
    crate::schema_drops::DropsQuery,
    crate::schema_playlists::PlaylistsQuery,
    crate::schema_catalog::CatalogQuery,
    crate::schema_artists::ArtistQuery,
    crate::schema_popularity::PopularityQuery,
    crate::schema_acoustic::AcousticQuery,
    crate::schema_replay::ReplayQuery,
);

#[derive(async_graphql::MergedObject, Default)]
pub struct Mutation(
    accounts::AccountsMutation,
    app_passwords::AppPasswordsMutation,
    handoff::HandoffMutation,
    holdings::HoldingsMutation,
    library::LibraryMutation,
    links::LinksMutation,
    lyrics::LyricsMutation,
    nodes::NodesMutation,
    pairing::PairingMutation,
    plugins::PluginsMutation,
    scrobbles::ScrobblesMutation,
    security::SecurityMutation,
    settings::SettingsMutation,
    sync_offers::SyncOffersMutation,
    crate::schema_social::SocialMutation,
    crate::schema_jam::JamMutation,
    crate::schema_jam_recap::JamRecapMutation,
    crate::schema_drops::DropsMutation,
    crate::schema_playlists::PlaylistsMutation,
    crate::schema_catalog::CatalogMutation,
    crate::schema_artists::ArtistMutation,
    crate::schema_popularity::PopularityMutation,
    crate::schema_acoustic::AcousticMutation,
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
pub(crate) fn authorize(ctx: &Context<'_>, user_id: &str) -> async_graphql::Result<()> {
    let authed = caller(ctx)?;
    if authed.username().eq_ignore_ascii_case(user_id.trim()) {
        Ok(())
    } else {
        // Deliberately does not name the account that *was* authenticated — an error message is
        // not the place to disclose it.
        Err(forbidden(
            "that token does not belong to the requested account",
        ))
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
    if !clean
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("A username may only contain letters, digits, dot, dash and underscore".into());
    }
    Ok(clean)
}

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

#[derive(Default)]
pub struct HealthQuery;

#[Object]
impl HealthQuery {
    async fn health(&self) -> &'static str {
        "Agro Server OK"
    }
}
