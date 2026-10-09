//! `GET /api/v1/setup-status` — lets the dashboard tell a brand-new server from a configured one.
//!
//! Public, because the only caller is someone with no account yet. What it reveals is one bit that
//! a visitor can already see for themselves (the sign-in page versus an empty server), and it
//! hands out no way to act on it: creating the first administrator still needs the one-time setup
//! token that only the operator, reading the server's own log, can have. See [`crate::auth`].

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::net::SocketAddr;
use std::time::Duration;

use crate::AppState;

const BUDGET: usize = 30;
const WINDOW: Duration = Duration::from_secs(300);

/// Whether this server is still waiting for its first administrator.
#[utoipa::path(
    get,
    path = "/api/v1/setup-status",
    tag = "auth",
    responses(
        (status = 200, description = "`{\"needsSetup\": true}` while the server has no accounts and a setup token is live"),
        (status = 429, description = "Too many requests from this address", body = crate::openapi::ApiError),
    ),
)]
pub async fn setup_status(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let key = format!("setup|{}", crate::login::client_ip(addr, &headers));
    if !state.popular_rate_limiter.charge(&key, 1, BUDGET, WINDOW) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "too many requests"})),
        )
            .into_response();
    }
    Json(serde_json::json!({ "needsSetup": state.setup_token.is_live() })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login::tests::test_state;

    async fn needs_setup(state: AppState) -> bool {
        let response = setup_status(
            State(state),
            ConnectInfo("203.0.113.9:5000".parse().unwrap()),
            HeaderMap::new(),
        )
        .await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["needsSetup"] == true
    }

    #[tokio::test]
    async fn a_server_with_a_live_token_needs_setup_until_it_is_burned() {
        let state = AppState {
            setup_token: crate::auth::SetupToken::for_fresh_server(0),
            ..test_state(crate::db::Db::new_in_memory().unwrap())
        };
        assert!(needs_setup(state.clone()).await);
        state.setup_token.consume();
        assert!(!needs_setup(state).await);
    }
}
