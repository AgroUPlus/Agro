//! Bearer-token authentication for the API.
//!
//! A presented token resolves to an account, and that account — its identity, its role and its
//! state — is what every resolver authorizes against. Nothing else is trusted: a caller-supplied
//! `userId` is only ever *compared* to the authenticated one, never believed.
//!
//! Three things this deliberately no longer does:
//!
//! - **The account passphrase is not a bearer token.** It buys a device token through `login` and
//!   is never itself presented. Previously the two were the same string, so a revocable device
//!   credential could be traded for the permanent account one.
//! - **There is no open first-run window.** The API used to skip authentication entirely while the
//!   database held no accounts, which on a public bind is a race against whoever scans the port
//!   first. Setup now needs a token printed to the server's own log — see [`SetupToken`].
//! - **A missing identity is not "no identity yet".** It is a 401. The old behaviour paired with a
//!   resolver guard that returned `Ok` when nobody was authenticated, so the two fail-open paths
//!   met in the middle.
//!
//! One exemption remains, and it is a capability URL rather than a hole: `/share/{token}` is opened
//! by people who have no account here, and the token in the path *is* the credential.

use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::credentials;
use crate::db_identity::Account;
use crate::AppState;

/// The one-shot credential that lets an unconfigured server be set up.
///
/// Generated in memory at boot when the database has no accounts, printed once to the log, and
/// burned by the first successful `bootstrapAdmin` call. It is never persisted — a restart mints a
/// new one, and a server that already has an account never mints one at all.
///
/// This replaces a window that was open to the network. The operator can read their own logs; a
/// scanner cannot.
pub struct SetupToken {
    secret: std::sync::Mutex<Option<String>>,
}

impl SetupToken {
    /// Mints a setup token, or holds nothing when the server is already configured.
    pub fn for_fresh_server(user_count: i64) -> Arc<Self> {
        let secret = if user_count == 0 {
            let minted = credentials::mint_token().secret;
            tracing::warn!(
                "No accounts exist yet. Create the admin with this one-time setup token \
                 (it is not stored, and a restart replaces it):\n\n    {minted}\n"
            );
            Some(minted)
        } else {
            None
        };
        Arc::new(Self {
            secret: std::sync::Mutex::new(secret),
        })
    }

    /// True when `presented` is the live setup token. Does not burn it.
    pub fn matches(&self, presented: &str) -> bool {
        let guard = self.secret.lock().unwrap();
        guard
            .as_deref()
            .is_some_and(|s| credentials::secure_eq(s, presented.trim()))
    }

    /// Burns the token so it cannot be replayed.
    pub fn consume(&self) {
        *self.secret.lock().unwrap() = None;
    }
}

/// Who the presented token belongs to, and what they are allowed to be.
///
/// Carries the whole [`Account`] rather than just a username, because authorization needs the role
/// and the state on every request and re-reading them per resolver would put a query in each one.
///
/// The username, not the `users.id` UUID, is what the data is keyed by: `registered_nodes`,
/// `handoff_state`, `synced_settings` and the library tables all store a username in their
/// `user_id` column. Only `app_passwords` uses the UUID.
#[derive(Clone, Debug)]
pub struct AuthedUser {
    pub account: Account,
    /// The name given to this device when its token was issued. Empty for anything not
    /// token-authenticated, such as a test harness.
    pub device_label: String,
}

impl AuthedUser {
    pub fn username(&self) -> &str {
        &self.account.username
    }

    pub fn is_admin(&self) -> bool {
        self.account.is_admin()
    }
}

/// Browsers cannot set headers on a WebSocket handshake, so `/ws/sync` also accepts the token as a
/// query parameter. Clients that can send a header should.
#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

pub async fn require_token(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .map(str::to_string)
        .or(query.token)
        .unwrap_or_default();

    if presented.trim().is_empty() {
        return unauthorized();
    }

    // The setup token authenticates *nobody* — it carries no account, so no `AuthedUser` is
    // inserted. It is let through only so the single bootstrap mutation can check it for itself;
    // every resolver that needs an identity still refuses, because there is none.
    if state.setup_token.matches(&presented) {
        return next.run(request).await;
    }

    match state.db.account_for_token(&presented) {
        Ok(Some((account, device_label))) => {
            // A suspended or not-yet-approved account holds a technically valid token. Refusing it
            // here rather than in each resolver means the approval gate cannot be forgotten in one
            // code path and left open.
            if !account.state.is_active() {
                return not_active();
            }
            request.extensions_mut().insert(AuthedUser { account, device_label });
            next.run(request).await
        }
        _ => unauthorized(),
    }
}

fn unauthorized() -> Response {
    // A GraphQL-shaped error, because every caller of this endpoint parses GraphQL.
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "errors": [{
                "message": "Unauthorized: send Authorization: Bearer <device token>"
            }]
        })),
    )
        .into_response()
}

fn not_active() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "errors": [{
                "message": "This account is not active. An administrator must approve or restore it."
            }]
        })),
    )
        .into_response()
}
