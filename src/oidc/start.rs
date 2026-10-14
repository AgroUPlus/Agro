//! Starting a sign-in or link flow, and the callback.

use super::*;
use crate::AppState;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Instant;

/// `GET /api/v1/oidc/start` — sends the browser to the provider.
#[utoipa::path(
    get,
    path = "/api/v1/oidc/start",
    tag = "oidc",
    responses(
        (status = 302, description = "Redirects to the identity provider"),
        (status = 404, description = "SSO is not configured on this server"),
    ),
)]
pub async fn start(State(state): State<AppState>) -> Response {
    begin_flow(&state, None)
}

/// `GET /api/v1/oidc/link` — the same thing, but bound to the signed-in account.
///
/// The account is taken from the token on *this* request and remembered server-side, so the
/// callback links to whoever started the flow and not to whoever the callback claims to be.
#[utoipa::path(
    get,
    path = "/api/v1/oidc/link",
    tag = "oidc",
    security(("bearer_token" = [])),
    responses(
        (status = 302, description = "Redirects to the identity provider"),
        (status = 404, description = "SSO is not configured on this server"),
    ),
)]
pub async fn start_link(
    State(state): State<AppState>,
    user: axum::Extension<crate::auth::AuthedUser>,
) -> Response {
    begin_flow(&state, Some(user.username().to_string()))
}

pub(super) fn begin_flow(state: &AppState, link_to: Option<String>) -> Response {
    let Some(config) = Config::from_env() else {
        return not_configured();
    };

    let flow_state = random_token();
    let nonce = random_token();
    let verifier = random_token();
    let challenge = base64url(Sha256::digest(verifier.as_bytes()).as_slice());

    state.oidc_flows.insert(
        flow_state.clone(),
        PendingFlow {
            started: Instant::now(),
            nonce: nonce.clone(),
            pkce_verifier: verifier,
            link_to,
        },
    );

    let url = format!(
        "{}/application/o/authorize/?response_type=code&client_id={}&redirect_uri={}\
         &scope={}&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
        config.issuer,
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&config.redirect_uri),
        urlencoding::encode(&config.scopes),
        urlencoding::encode(&flow_state),
        urlencoding::encode(&nonce),
        urlencoding::encode(&challenge),
    );
    Redirect::to(&url).into_response()
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct CallbackQuery {
    pub(super) code: Option<String>,
    pub(super) state: Option<String>,
    pub(super) error: Option<String>,
}

/// `GET /api/v1/oidc/callback` — the provider sends the browser back here.
///
/// Ends by redirecting into the dashboard with a token in the fragment. A fragment rather than a
/// query string because fragments are not sent to the server and do not land in access logs; the
/// page reads it and immediately clears it.
#[utoipa::path(
    get,
    path = "/api/v1/oidc/callback",
    tag = "oidc",
    params(CallbackQuery),
    responses(
        (status = 302, description = "Redirects into the dashboard, either with a token in the \
                                       fragment (`/#token=...`) on success or an error message \
                                       (`/#ssoError=...`) on failure"),
        (status = 404, description = "SSO is not configured on this server"),
    ),
)]
pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(config) = Config::from_env() else {
        return not_configured();
    };
    if let Some(error) = query.error {
        return fail(&format!("the identity provider refused: {error}"));
    }
    let (Some(code), Some(flow_state)) = (query.code, query.state) else {
        return fail("that sign-in did not come back with a code");
    };

    // Single-use, and unknown `state` values die here. This is the CSRF defence for the flow.
    let Some(flow) = state.oidc_flows.take(&flow_state) else {
        return fail("that sign-in link has expired — please try again");
    };

    let claims = match exchange_and_verify(&config, &code, &flow).await {
        Ok(claims) => claims,
        Err(err) => {
            tracing::warn!("an OIDC sign-in could not be verified: {err}");
            return fail("that sign-in could not be verified");
        }
    };

    match flow.link_to {
        Some(username) => finish_link(&state, &config, &username, &claims),
        None => finish_sign_in(&state, &config, &claims),
    }
}
