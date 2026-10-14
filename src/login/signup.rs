//! Invite-based signup.

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

/// How an instance treats strangers.
///
/// Read from `AGRO_SIGNUP` once per request rather than cached, so an operator can close signups on
/// a server that is being abused without restarting it and dropping every live WebSocket.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignupMode {
    /// Anyone may register; the account waits for an admin to let it in.
    Approval,
    /// A valid invite code is required, and spending one lets the account straight in.
    Invite,
    Closed,
}

/// Whether this server refuses new accounts outright.
///
/// Read by `oidc`, so an operator who closed signups does not find that SSO quietly reopened them.
/// Already-linked identities keep working; only the creation of new accounts stops.
pub fn signups_are_closed() -> bool {
    SignupMode::from_env() == SignupMode::Closed
}

/// The username rule, in one place.
///
/// Restrictive on purpose: usernames are compared case-insensitively, travel in a pairing URL, and
/// are the join key for nearly every table. Anything looser invites two accounts a human cannot
/// tell apart. `schema.rs` enforces the identical rule for admin-created accounts; both must agree,
/// which is why neither owns its own copy of the character set.
pub fn normalise_username(raw: &str) -> Option<String> {
    let clean = raw.trim().to_lowercase();
    let usable = !clean.is_empty()
        && clean.chars().count() <= 32
        && clean
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    usable.then_some(clean)
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignupBody {
    pub(super) username: String,
    pub(super) invite_code: Option<String>,
}

/// Registers a stranger.
///
/// The account is created `Pending` and **no device token is minted for it**: a pending account
/// cannot authenticate, so a token would be a credential that does nothing but confuse whoever
/// holds it. What comes back is the passphrase, once — it is Argon2-hashed before this returns and
/// the server genuinely cannot produce it again.
///
/// A taken username is refused with the same words as an unusable one. That makes this endpoint a
/// poor way to test whether an account exists — the public directory only lists people who asked to
/// be listed, and this must not quietly undo that choice.
#[utoipa::path(
    post,
    path = "/api/v1/signup",
    tag = "auth",
    request_body = SignupBody,
    responses(
        (status = 200, description = "Account created (Pending approval, or Active if an invite was spent)", body = crate::openapi::SignupResponse),
        (status = 401, description = "Signups closed, username unavailable, or invite invalid", body = crate::openapi::ApiError),
    ),
)]
pub async fn signup(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<SignupBody>,
) -> Response {
    if !state.rate_limiter.allow(&client_ip(addr, &headers)) {
        return too_many();
    }

    let mode = SignupMode::from_env();
    if mode == SignupMode::Closed {
        return refused("This server is not accepting new accounts");
    }

    // Bootstrap has to come first. On an empty database this endpoint would otherwise let the first
    // stranger through to create account #1 — which is enough to make `/api/v1/bootstrap` refuse
    // ever after, leaving an instance with no administrator and a pending account that nobody has
    // the authority to approve. `user_count` failing counts as "not empty" for the same reason it
    // does in bootstrap: the cautious answer is the one that refuses.
    if state.db.user_count().unwrap_or(1) == 0 {
        return refused("This server is not accepting new accounts");
    }

    let Some(username) = normalise_username(&body.username) else {
        return refused("That username is not available");
    };

    // Everything that can refuse this signup is checked *before* the invite is spent.
    //
    // The order used to be the other way round, and the consequence was the whole feature failing
    // in the most ordinary way possible: pick a username that is already taken, get refused, try
    // again — and the code had already been consumed by the attempt that failed, so the retry
    // landed in the approval queue as though no invite had been offered.
    if state.db.account(&username).unwrap_or(None).is_some() {
        return refused("That username is not available");
    }

    let offered = body
        .invite_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty());

    let (account_state, spent_code) = match mode {
        SignupMode::Invite => {
            let Some(code) = offered else {
                return refused("That invite code is not valid");
            };
            if !state.db.redeem_invite(code).unwrap_or(false) {
                return refused("That invite code is not valid");
            }
            (AccountState::Active, Some(code))
        }
        SignupMode::Approval => match offered {
            // An invite is still honoured when one is offered: it is what skips the queue.
            Some(code) if state.db.redeem_invite(code).unwrap_or(false) => {
                (AccountState::Active, Some(code))
            }
            _ => (AccountState::Pending, None),
        },
        SignupMode::Closed => unreachable!("handled above"),
    };

    let passphrase = generate_passphrase();
    let account = match state
        .db
        .create_account(&username, &passphrase, Role::Member, account_state)
    {
        Ok(account) => account,
        // A unique-constraint violation lands here when two signups race for one name — the one
        // case the check above cannot catch. The loser's invite is handed back rather than burnt
        // on an account that does not exist.
        Err(_) => {
            if let Some(code) = spent_code {
                let _ = state.db.refund_invite(code);
            }
            return refused("That username is not available");
        }
    };

    state.db.record_event(
        crate::audit::Event::AccountCreated,
        crate::audit::Record::new()
            .user(&account.username)
            .ip(&client_ip(addr, &headers))
            .detail(format!("self-signup, state {}", account.state.as_str())),
    );

    Json(json!({
        "username": account.username,
        "state": account.state.as_str(),
        // Shown once. The server keeps an Argon2 hash and cannot show it again.
        "passphrase": passphrase,
    }))
    .into_response()
}

/// A genuine authentication failure — the one a bouncer *should* count.
///
/// Tagged as such so that it can be told apart from the two other things this endpoint answers 401
/// to, both of which are ordinary steps of a login that is going fine.
pub(super) fn refused(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(AUTH_STAGE_HEADER, "credentials-rejected")],
        Json(json!({ "error": message })),
    )
        .into_response()
}

pub(super) fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "Too many attempts. Wait a few minutes and try again." })),
    )
        .into_response()
}
