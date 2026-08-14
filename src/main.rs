mod db;
mod embedded_dashboard;
mod ingest;
mod passphrase;
mod plugins;
mod schema;
mod share;
mod ws;

use async_graphql::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    routing::{get, post},
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let db = Db::new("agro_data.db")?;
    let ws_hub = Arc::new(WsHub::new());
    let state = AppState {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
    };

    let schema: AgroSchema = Schema::build(QueryRoot, MutationRoot, async_graphql::EmptySubscription)
        .data(db.clone())
        .data(ws_hub.clone())
        .finish();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/ws/sync", get(ws::ws_handler))
        .route("/api/v1/dropbox/upload", post(ingest::dropbox_upload_handler))
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

async fn graphql_handler(
    schema: axum::Extension<AgroSchema>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}
