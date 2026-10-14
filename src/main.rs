// `MergedObject` nests one future per merged object when it resolves a field, and the schema roots
// merge over twenty each. The default limit of 128 is not deep enough to lay those futures out.
#![recursion_limit = "256"]

mod app_schema;
mod audit;
mod auth;
mod blend;
mod blend_boundary_tests;
mod blend_contract_tests;
mod blend_recipe;
mod blend_tests;
mod catalog_boundary_tests;
mod cover_lookup;
mod credentials;
mod db;
mod db_acoustic;
mod db_artists;
mod db_blend;
mod db_blend_refresh;
mod db_catalog;
mod db_drops;
mod db_feed;
mod db_identity;
mod db_jam;
mod db_jam_recap;
mod db_library;
mod db_library_album;
mod db_playlist_edits;
mod db_playlist_followers;
mod db_playlist_items;
mod db_playlists;
mod db_popularity;
mod db_popularity_movement;
mod db_presence;
mod db_replay;
mod db_short_links;
mod db_social;
mod db_vault;
mod embedded_dashboard;
mod feature_toggle_tests;
mod features;
mod graphql_http;
mod guest_boundary_tests;
mod importer;
mod jam_clock;
mod jam_recap;
mod jam_recap_forget;
mod library;
mod listen;
mod listen_playlist;
mod listen_retired;
mod lock;
mod login;
mod norm;
mod offers;
mod oidc;
mod openapi;
mod passphrase;
mod playlist_access;
mod playlist_boundary_tests;
mod playlist_edit_tests;
mod playlist_visibility;
mod plugins;
mod popular;
#[cfg(test)]
mod property_tests;
mod proxy;
mod rate_limit;
mod relay;
mod routes;
mod schema;
mod schema_acoustic;
mod schema_artists;
mod schema_blend;
mod schema_catalog;
mod schema_drops;
mod schema_feed;
mod schema_jam;
mod schema_jam_recap;
mod schema_playlists;
mod schema_popularity;
mod schema_replay;
#[cfg(test)]
mod schema_snapshot_tests;
mod schema_social;
mod schema_vault;
mod secret_key_file;
mod security_headers;
mod setup_status;
mod share;
mod social_boundary_tests;
mod stats;
mod stats_wrapped;
mod storage;
mod sweeper;
mod totp;
mod vault_contract_tests;
mod vault_http;
mod ws;
mod ws_channels;

use async_graphql::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::HeaderValue,
    routing::{get, post, put},
    Router,
};
use db::Db;
use features::Feature;
use schema::{AgroSchema, Mutation, Query};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi as _;
use utoipa_swagger_ui::SwaggerUi;
use ws::WsHub;

/// How often the storage sweeper runs.
const SWEEP_SECS: u64 = 15 * 60;

/// The largest body any non-upload route will accept. Uploads stream and opt out.
const MAX_JSON_BODY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub ws_hub: Arc<WsHub>,
    pub storage: storage::Storage,
    pub offers: offers::OfferBatcher,
    pub relay_hub: relay::RelayHub,
    /// Shared HTTP client with connection pool for outbound proxy requests.
    pub http_client: reqwest::Client,
    /// Live only while the server has no accounts at all. See [`auth::SetupToken`].
    pub setup_token: Arc<auth::SetupToken>,
    /// Throttles the two endpoints that can be reached without a token.
    pub rate_limiter: Arc<login::RateLimiter>,
    /// Throttles the public, unauthenticated popularity chart. Deliberately separate from
    /// `rate_limiter`: that one is budgeted for login brute-force guesses (ten per five minutes)
    /// and keyed only by IP, so sharing it would let a handful of chart page-views lock a visitor
    /// out of signing in from the same address.
    pub popular_rate_limiter: Arc<rate_limit::FixedWindow>,
    /// SSO sign-ins waiting for the browser to come back. See [`oidc::FlowStore`].
    pub oidc_flows: Arc<oidc::FlowStore>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Prints the OpenAPI document and exits, touching no state. This is what the GitHub Pages
    // workflow calls to publish an up-to-date spec — the document is built entirely from
    // `#[utoipa::path]` annotations, so nothing here needs a database or a bound port.
    if std::env::args().any(|arg| arg == "dump-openapi") {
        println!("{}", openapi::ApiDoc::openapi().to_pretty_json()?);
        return Ok(());
    }

    // Two-factor secrets need a key. An operator-supplied AGRO_SECRET_KEY always wins; otherwise one
    // is generated beside the database the first time. A failure is reported, never papered over:
    // two-factor then stays unavailable and the dashboard says why.
    if std::env::var("AGRO_SECRET_KEY").map_or(true, |key| key.trim().is_empty()) {
        match secret_key_file::provide(std::path::Path::new(secret_key_file::FILE_NAME)) {
            Ok(true) => tracing::info!(
                "Generated a two-factor key in {}. Keep it with your database backups.",
                secret_key_file::FILE_NAME
            ),
            Ok(false) => {}
            Err(err) => tracing::warn!("Two-factor is unavailable: {err}"),
        }
    }

    let db = Db::new("agro_data.db")?;
    let ws_hub = Arc::new(WsHub::new());
    let store = storage::Storage::from_env();
    tokio::fs::create_dir_all(&store.spool_root).await?;
    match &store.library_root {
        Some(root) => println!("📁 Library root: {}", root.display()),
        // Worth saying out loud: this is the difference between "uploads are archived" and
        // "uploads only ever go to a peer", and it is decided by an environment variable that is
        // easy to forget on a new host.
        None => println!("📁 No AGRO_LIBRARY_ROOT — index-only mode, uploads spool for peers"),
    }
    if store.archive_hook.is_some() {
        println!("🪝 Archive hook set — runs after each file is filed");
    }

    // Hashing cannot happen in SQL, so migration 9 leaves the credential columns empty and this
    // fills them. Runs on every boot and does nothing once there is nothing left to convert.
    match db.migrate_credentials() {
        Ok(0) => {}
        Ok(n) => tracing::info!("Hashed {n} legacy plaintext passphrase(s)"),
        Err(e) => tracing::error!("Could not migrate credentials: {e}"),
    }

    let setup_token = auth::SetupToken::for_fresh_server(db.user_count().unwrap_or(0));
    let http_client = reqwest::Client::builder().build()?;

    let state = AppState {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
        storage: store,
        offers: offers::OfferBatcher::spawn(db.clone(), ws_hub.clone()),
        relay_hub: relay::RelayHub::new(),
        http_client,
        setup_token: setup_token.clone(),
        rate_limiter: Arc::new(login::RateLimiter::new()),
        popular_rate_limiter: Arc::new(rate_limit::FixedWindow::new()),
        oidc_flows: Arc::new(oidc::FlowStore::new()),
    };

    // A one-shot pass rather than something that runs at every boot: cover extraction only happens
    // as files are archived, so a library that predates the feature has none. Run once after
    // upgrading and the dashboard has artwork.
    if std::env::args().any(|arg| arg == "reindex-covers") {
        let found = library::reindex_covers(&state).await;
        println!("🖼  Extracted {found} album covers");
        return Ok(());
    }

    let schema = app_schema::build(&state, setup_token);

    sweeper::spawn(&state);

    let app = routes::build(state, schema);

    let port = std::env::var("PORT").unwrap_or_else(|_| "8700".to_string());
    let addr = format!("0.0.0.0:{}", port);
    println!("🚀 Agro Server running at http://{}", addr);
    println!("📊 GraphQL endpoint: http://{}/graphql", addr);
    println!("🔄 WebSocket sync: ws://{}/ws/sync", addr);
    println!("📖 REST API docs: http://{}/api/docs", addr);
    println!("🧭 GraphQL playground: http://{}/graphql/playground", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // `into_make_service_with_connect_info` is what makes the peer address available to the
    // rate limiter on the unauthenticated routes.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}
