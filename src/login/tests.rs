use super::limiter::*;
use super::*;
use crate::db_identity::{AccountState, Role};
use crate::AppState;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    Json,
};

/// Also used by `relay`'s handler tests, so the two do not drift apart on what an `AppState`
/// is made of.
pub(crate) fn test_state(db: crate::db::Db) -> AppState {
    let ws_hub = std::sync::Arc::new(crate::ws::WsHub::new());
    AppState {
        db: db.clone(),
        ws_hub: ws_hub.clone(),
        storage: crate::storage::Storage::for_tests(),
        offers: crate::offers::OfferBatcher::spawn(db, ws_hub),
        relay_hub: crate::relay::RelayHub::new(),
        http_client: reqwest::Client::new(),
        setup_token: crate::auth::SetupToken::for_fresh_server(1),
        rate_limiter: std::sync::Arc::new(RateLimiter::new()),
        popular_rate_limiter: std::sync::Arc::new(crate::rate_limit::FixedWindow::new()),
        oidc_flows: std::sync::Arc::new(crate::oidc::FlowStore::new()),
    }
}

async fn post_signup(state: AppState, username: &str) -> StatusCode {
    signup(
        State(state),
        ConnectInfo("10.0.0.9:5000".parse().unwrap()),
        HeaderMap::new(),
        Json(SignupBody {
            username: username.to_string(),
            invite_code: None,
        }),
    )
    .await
    .status()
}

mod limiter;
mod signup;
