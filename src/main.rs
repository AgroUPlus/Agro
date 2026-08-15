mod crypto;
mod auth;
mod db;
mod db_library;
mod embedded_dashboard;
mod library;
mod norm;
mod passphrase;
mod plugins;
mod schema;
mod share;
mod storage;
mod ws;

use async_graphql::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    routing::{get, post, put},
    Router,
};
use db::Db;
use schema::{AgroSchema, MutationRoot, QueryRoot};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use ws::WsHub;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub ws_hub: Arc<WsHub>,
    pub storage: storage::Storage,
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

    let state = AppState {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
        storage: store,
    };

    let schema: AgroSchema = Schema::build(QueryRoot, MutationRoot, async_graphql::EmptySubscription)
        .data(db.clone())
        .data(ws_hub.clone())
        .finish();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Everything that exposes a user's data sits behind the token check; the dashboard's own
    // static files and the capability-URL share endpoint stay public.
    let protected = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/ws/sync", get(ws::ws_handler))
        .route("/api/v1/library/upload", post(library::begin_upload))
        .route("/api/v1/library/upload/{upload_id}", put(library::put_upload))
        .route("/api/v1/library/fetch/{content_hash}", get(library::fetch))
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
        .route("/share/{token}", get(share::share_handler))
        .fallback(embedded_dashboard::static_dashboard_handler)
        .layer(cors)
        .with_state(state)
        .layer(axum::Extension(schema));

    let port = std::env::var("PORT").unwrap_or_else(|_| "8700".to_string());
    let addr = format!("0.0.0.0:{}", port);
    println!("🚀 Agro Server running at http://{}", addr);
    println!("📊 GraphQL endpoint: http://{}/graphql", addr);
    println!("🔄 WebSocket sync: ws://{}/ws/sync", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

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
