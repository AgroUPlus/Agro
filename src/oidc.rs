//! Signing in with an external identity provider (Authentik, Keycloak, anything OIDC).
//!
//! Authorization Code flow with PKCE. The server is a confidential client: it holds a client secret
//! and does the code exchange itself, so a token never passes through the browser.
//!
//! # The rule that shapes everything here
//!
//! > **An unauthenticated callback can sign into an already-linked account, or create a brand-new
//! > one. It can never attach itself to an existing account.**
//!
//! Linking an identity to an account that already exists happens from inside a signed-in session,
//! through [`start_link`], and nowhere else. This makes the dangerous case *unreachable* rather
//! than defended against: there is no claim-matching heuristic in the callback to get wrong, no
//! "same email, probably the same person" inference, and no administrator being asked to approve a
//! link they have no way to verify.
//!
//! The join key is `(issuer, subject)`. An IdP's `email` and `preferred_username` are display
//! hints — an IdP administrator can edit them — so neither is ever identity.
//!
//! # What is validated, and why it is not hand-rolled
//!
//! The ID token's signature is checked against the provider's JWKS, along with `iss`, `aud`, `exp`
//! and the `nonce` this server generated. A mistake anywhere in that list is a full authentication
//! bypass rather than a bug — a forged token with an unverified signature *is* a valid login — so
//! the verification is `jsonwebtoken`'s rather than ours.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};

/// What an operator has to set for any of this to be offered.
pub struct Config {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// Where the IdP sends the browser back. Must match what is registered at the provider.
    pub redirect_uri: String,
    /// What the button says.
    pub display_name: String,
    pub scopes: String,
}

impl Config {
    /// Reads the configuration, or `None` when this server does not offer OIDC.
    ///
    /// All four required values or nothing: a half-configured provider that silently does not work
    /// is worse than one that is plainly absent, because the button appears and then fails.
    pub fn from_env() -> Option<Self> {
        let get = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Some(Config {
            issuer: get("AGRO_OIDC_ISSUER")?.trim_end_matches('/').to_string(),
            client_id: get("AGRO_OIDC_CLIENT_ID")?,
            client_secret: get("AGRO_OIDC_CLIENT_SECRET")?,
            redirect_uri: get("AGRO_OIDC_REDIRECT_URI")?,
            display_name: get("AGRO_OIDC_DISPLAY_NAME").unwrap_or_else(|| "SSO".to_string()),
            scopes: get("AGRO_OIDC_SCOPES").unwrap_or_else(|| "openid profile email".to_string()),
        })
    }

    /// Whether new accounts created through OIDC skip the approval queue.
    fn auto_approve() -> bool {
        matches!(
            std::env::var("AGRO_OIDC_AUTO_APPROVE")
                .unwrap_or_default()
                .trim(),
            "1" | "true" | "yes" | "on"
        )
    }
}

/// `GET /api/v1/oidc/config` — what the sign-in page needs to decide whether to show the button.
#[utoipa::path(
    get,
    path = "/api/v1/oidc/config",
    tag = "oidc",
    responses((status = 200, description = "`{ enabled: false }`, or `{ enabled: true, displayName }`")),
)]
pub async fn config() -> Response {
    match Config::from_env() {
        Some(config) => axum::Json(serde_json::json!({
            "enabled": true,
            "displayName": config.display_name,
        }))
        .into_response(),
        None => axum::Json(serde_json::json!({ "enabled": false })).into_response(),
    }
}

fn not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({ "error": "this server does not offer SSO" })),
    )
        .into_response()
}

/// Sends the browser back to the dashboard with a message, rather than rendering an error page.
///
/// The message is URL-encoded into the fragment and rendered by the dashboard as text, so nothing
/// from a provider's error response is ever interpreted as markup.
fn fail(message: &str) -> Response {
    Redirect::to(&format!("/#ssoError={}", urlencoding::encode(message))).into_response()
}

mod finish;
mod flow;
mod start;
mod token;
use finish::finish_link;
use finish::finish_sign_in;
pub use flow::FlowStore;
use flow::PendingFlow;
pub use start::__path_callback;
pub use start::__path_start;
pub use start::__path_start_link;
pub use start::callback;
pub use start::start;
pub use start::start_link;
use token::base64url;
use token::exchange_and_verify;
use token::random_token;
pub use token::Claims;
#[cfg(test)]
mod tests;
