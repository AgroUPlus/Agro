//! First-run creation of the administrator.

use super::*;
use crate::db_identity::{AccountState, Role};
use crate::passphrase::generate_passphrase;
use crate::AppState;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapBody {
    setup_token: String,
    username: String,
}

/// Creates the first administrator on an empty server.
///
/// Needs the one-time setup token printed to the server's log at boot. This replaces a window in
/// the middleware that let *any* unauthenticated request through while no accounts existed — a
/// race the operator had to win against whoever was scanning the port.
#[utoipa::path(
    post,
    path = "/api/v1/bootstrap",
    tag = "auth",
    request_body = BootstrapBody,
    responses(
        (status = 200, description = "First administrator created", body = crate::openapi::BootstrapResponse),
        (status = 401, description = "Invalid setup token, or the server already has an account", body = crate::openapi::ApiError),
    ),
)]
pub async fn bootstrap(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<BootstrapBody>,
) -> Response {
    if !state.rate_limiter.allow(&client_ip(addr, &headers)) {
        return too_many();
    }
    if !state.setup_token.matches(&body.setup_token) {
        return refused("That setup token is not valid");
    }
    // The token is only ever minted for an empty database, but two callers racing with it must
    // still produce one admin rather than two.
    if state.db.user_count().unwrap_or(1) > 0 {
        return refused("This server already has an account");
    }

    let Some(username) = normalise_username(&body.username) else {
        return refused("That username is not usable");
    };

    let passphrase = generate_passphrase();
    let account =
        match state
            .db
            .create_account(&username, &passphrase, Role::Admin, AccountState::Active)
        {
            Ok(account) => account,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": format!("could not create the account: {err}") })),
                )
                    .into_response()
            }
        };

    let token = state
        .db
        .mint_device_token(&account.username, "first device")
        .unwrap_or_default();
    state.setup_token.consume();

    state.db.record_event(
        crate::audit::Event::AccountCreated,
        crate::audit::Record::new()
            .user(&account.username)
            .ip(&client_ip(addr, &headers))
            .detail("first administrator, via setup token"),
    );

    Json(json!({
        "username": account.username,
        // Shown once. The server keeps an Argon2 hash and cannot show it again.
        "passphrase": passphrase,
        "token": token,
    }))
    .into_response()
}
