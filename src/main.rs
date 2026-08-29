mod crypto;
mod credentials;
mod auth;
mod db;
mod db_identity;
mod db_library;
mod db_drops;
mod db_feed;
mod db_jam;
mod db_social;
mod guest_boundary_tests;
mod embedded_dashboard;
mod jam_clock;
mod library;
mod listen;
mod login;
mod norm;
mod offers;
mod passphrase;
mod plugins;
mod schema;
mod schema_drops;
mod schema_feed;
mod schema_jam;
mod schema_social;
mod social_boundary_tests;
mod share;
mod stats;
mod storage;
mod relay;
mod ws;

use async_graphql::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    extract::DefaultBodyLimit,
    http::HeaderValue,
    routing::{get, post, put},
    Router,
};
use db::Db;
use schema::{AgroSchema, Mutation, Query};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
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
    /// Live only while the server has no accounts at all. See [`auth::SetupToken`].
    pub setup_token: Arc<auth::SetupToken>,
    /// Throttles the two endpoints that can be reached without a token.
    pub rate_limiter: Arc<login::RateLimiter>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

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

    let state = AppState {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
        storage: store,
        offers: offers::OfferBatcher::spawn(db.clone(), ws_hub.clone()),
        relay_hub: relay::RelayHub::new(),
        setup_token: setup_token.clone(),
        rate_limiter: Arc::new(login::RateLimiter::new()),
    };

    // A one-shot pass rather than something that runs at every boot: cover extraction only happens
    // as files are archived, so a library that predates the feature has none. Run once after
    // upgrading and the dashboard has artwork.
    if std::env::args().any(|arg| arg == "reindex-covers") {
        let found = library::reindex_covers(&state).await;
        println!("🖼  Extracted {found} album covers");
        return Ok(());
    }

    let schema: AgroSchema = Schema::build(Query::default(), Mutation::default(), async_graphql::EmptySubscription)
        .data(db.clone())
        .data(ws_hub.clone())
        .data(state.offers.clone())
        // Resolvers need to know whether this deployment archives at all, and where — that is what
        // decides which sync mode the clients are told to run in.
        .data(state.storage.clone())
        .data(setup_token.clone())
        // A public endpoint with no depth or complexity limit is a denial-of-service primitive:
        // GraphQL lets one request ask for a deeply nested or heavily aliased tree, and the cost
        // is paid by the server before any resolver decides the caller was not allowed to ask.
        .limit_depth(12)
        .limit_complexity(500)
        .finish();

    // The dashboard is served from this same origin, so a wildcard buys nothing and lets any page
    // on the internet make authenticated-looking requests from a visitor's browser.
    let cors = match std::env::var("AGRO_ALLOWED_ORIGIN") {
        Ok(origin) if !origin.trim().is_empty() => match origin.trim().parse::<HeaderValue>() {
            Ok(value) => CorsLayer::new()
                .allow_origin(value)
                .allow_methods(Any)
                .allow_headers(Any),
            Err(_) => {
                tracing::error!("AGRO_ALLOWED_ORIGIN is not a valid origin; refusing all CORS");
                CorsLayer::new()
            }
        },
        _ => CorsLayer::new(),
    };

    // Sweeps expired spool entries and abandoned part files. Eviction used to happen only as a
    // side effect of a successful spool write, so on a deployment with a library root it never
    // ran at all.
    {
        let state = state.clone();
        // The jam clock. Its own ticker rather than a branch of the storage sweep: they run at
        // very different rates, and a jam waiting on a slow disk pass would drift the room.
        {
            let db = db.clone();
            let hub = ws_hub.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                    jam_clock::TICK_SECS,
                ));
                loop {
                    ticker.tick().await;
                    jam_clock::tick(&db, &hub);
                }
            });
        }

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(SWEEP_SECS));
            loop {
                ticker.tick().await;
                library::sweep_storage(&state).await;
            }
        });
    }

    // Everything that exposes a user's data sits behind the token check; the dashboard's own
    // static files and the capability-URL share endpoint stay public.
    let protected = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/ws/sync", get(ws::ws_handler))
        .route("/api/v1/library/upload", post(library::begin_upload))
        .route(
            "/api/v1/library/upload/{upload_id}",
            put(library::put_upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/v1/library/fetch/{content_hash}", get(library::fetch))
        .route("/api/v1/cover/{album_key}", get(library::cover))
        .route("/api/v1/relay/open", post(relay::open_relay))
        .route(
            "/api/v1/relay/{session_id}/send",
            post(relay::send_relay).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/v1/relay/{session_id}/receive", get(relay::receive_relay))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ));

    // `/api/v1/dropbox/upload` used to be routed here, *outside* `protected` — an unauthenticated
    // endpoint that joined a caller-supplied filename onto a path (so `../..` escaped the upload
    // directory), buffered whole files in RAM on a 512 MB host, and wrote nothing to the database.
    // It is replaced by the authenticated streaming routes in `library`, which live inside
    // `protected` above.
    let app = Router::new()
        .merge(protected)
        // The only two routes that can be reached without a token: there has to be some way to
        // get one. Both are rate-limited; see `login`.
        .route("/api/v1/login", post(login::login))
        .route("/api/v1/bootstrap", post(login::bootstrap))
        .route("/api/v1/signup", post(login::signup))
        .route("/share/{token}", get(share::share_handler))
        // Public by design: a shared link is opened by someone with no account here.
        .route("/listen", get(listen::listen_handler))
        .fallback(embedded_dashboard::static_dashboard_handler)
        // Without this a single request can stream unbounded bytes into any JSON handler. The
        // upload routes opt back out, because that is exactly what they are for.
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        .layer(cors)
        .with_state(state)
        .layer(axum::Extension(schema));

    let port = std::env::var("PORT").unwrap_or_else(|_| "8700".to_string());
    let addr = format!("0.0.0.0:{}", port);
    println!("🚀 Agro Server running at http://{}", addr);
    println!("📊 GraphQL endpoint: http://{}/graphql", addr);
    println!("🔄 WebSocket sync: ws://{}/ws/sync", addr);

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

/// Moves the authenticated identity from the request into the GraphQL context.
///
/// `Option`, because `require_token` deliberately lets requests through unauthenticated while the
/// server has no accounts — the window in which the first account is created. Resolvers treat a
/// missing identity as that first-run window; see `schema::authorize`.
async fn graphql_handler(
    schema: axum::Extension<AgroSchema>,
    user: Option<axum::Extension<auth::AuthedUser>>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let mut request = req.into_inner();
    if let Some(axum::Extension(user)) = user {
        request = request.data(user);
    }
    schema.execute(request).await.into()
}
