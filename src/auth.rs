//! Bearer-token authentication for the API.
//!
//! Until this existed, anyone who could reach `/graphql` could read and write every user's
//! sessions, settings and passphrase — the `Authorization` header clients already sent was never
//! looked at. It is now required, and accepts either the account passphrase or one of the account's
//! app passwords, so a device can be revoked without rotating the credential every other device
//! uses.
//!
//! Two deliberate exemptions:
//!
//! - **First run.** With no accounts in the database there is no credential to present, so the API
//!   is open exactly long enough to create the first one. It closes the moment that account exists.
//! - **Share links.** `/share/{token}` is a capability URL handed to people who have no account;
//!   the token in the path is the credential.

use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

/// Browsers cannot set headers on a WebSocket handshake, so `/ws/sync` also accepts the token as a
/// query parameter. Clients that can send a header should.
#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

pub async fn require_token(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
    request: Request,
    next: Next,
) -> Response {
    // An unconfigured server has nobody to authenticate as. This is the setup window, and it is
    // closed by the existence of the first account rather than by a timer.
    if state.db.user_count().unwrap_or(0) == 0 {
        return next.run(request).await;
    }

    let header_token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .map(str::to_string);

    let token = header_token.or(query.token).unwrap_or_default();

    match state.db.user_for_token(&token) {
        Ok(Some(_)) => next.run(request).await,
        _ => unauthorized(),
    }
}

fn unauthorized() -> Response {
    // A GraphQL-shaped error, because every caller of this endpoint parses GraphQL.
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "errors": [{
                "message": "Unauthorized: send Authorization: Bearer <passphrase or app password>"
            }]
        })),
    )
        .into_response()
}
