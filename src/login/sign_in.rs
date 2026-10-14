//! Passphrase sign-in.

use super::*;
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
pub struct LoginBody {
    username: String,
    passphrase: String,
    /// What to call this device in the app-password list. Defaults to something honest.
    label: Option<String>,
    /// The second factor, when the account has one. A recovery code is also accepted here.
    ///
    /// Optional because the client does not know whether it is needed until it has asked: the
    /// first attempt comes without it and is answered with `totpRequired`, and the client asks the
    /// user and sends the whole thing again.
    #[serde(default)]
    totp_code: Option<String>,
}

/// Exchanges a passphrase for a device token.
///
/// The passphrase itself is never a bearer token — that equivalence is what let a revocable device
/// credential be traded for the permanent account one. What comes back is scoped to this device
/// and can be revoked on its own.
#[utoipa::path(
    post,
    path = "/api/v1/login",
    tag = "auth",
    request_body = LoginBody,
    responses(
        (status = 200, description = "Signed in", body = crate::openapi::LoginResponse),
        (status = 401, description = "Credentials rejected, or a second factor is required or was \
                                       rejected — disambiguated by the X-Agro-Auth-Stage response \
                                       header and the totpRequired body field, since the status \
                                       code is the same for all three by design (kept for client \
                                       compatibility).", body = crate::openapi::ApiError),
        (status = 429, description = "Too many attempts", body = crate::openapi::ApiError),
    ),
)]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let client_ip = client_ip(addr, &headers);
    if !state.rate_limiter.allow(&client_ip) {
        return too_many();
    }

    let username = body.username.trim().to_lowercase();

    // Paid before the answer, not after, and *before* the account is looked up — so an account that
    // exists and one that does not are slowed identically.
    let backoff = state.rate_limiter.backoff_for(&username);
    if !backoff.is_zero() {
        tokio::time::sleep(backoff).await;
    }

    let Ok(Some(account)) = state.db.verify_login(&username, &body.passphrase) else {
        state.rate_limiter.note_failure(&username);
        // One answer for "no such account" and "wrong passphrase" alike. Telling them apart turns
        // this into a way to find out who has an account here.
        //
        // The *log* may distinguish them, and does — it is read by the operator, not the caller,
        // and "someone is guessing usernames that do not exist" is a different situation from
        // "someone is guessing one real account's passphrase". The attempted username is recorded
        // in `detail` because a failed login has no account to attach to; the passphrase never is.
        state.db.record_event(
            crate::audit::Event::LoginFailed,
            crate::audit::Record::new()
                .ip(&client_ip)
                .detail(format!("username={username}")),
        );
        return refused("Those credentials were not accepted");
    };

    if !account.state.is_active() {
        state.rate_limiter.note_failure(&username);
        state.db.record_event(
            crate::audit::Event::LoginFailed,
            crate::audit::Record::new()
                .user(&account.username)
                .ip(&client_ip)
                .detail(format!("account state is {}", account.state.as_str())),
        );
        return refused("This account is not active yet");
    }

    // The second factor, checked *after* the passphrase and never before it. Asking for a code
    // first would tell an anonymous caller which usernames exist and which have 2FA enabled.
    //
    // Kept as one request/response rather than a challenge the server has to remember: a two-step
    // flow needs pending-login state, and — more importantly — the vault envelope below can only be
    // handed over in a response the client receives while it still holds the passphrase. Re-sending
    // the passphrase with the code costs one extra Argon2 verification and keeps that property.
    // Only a *confirmed* enrolment is demanded here. An admin under enforcement who has not
    // enrolled yet cannot be asked for a code they do not have — they are let in, and the GraphQL
    // gate confines them to the enrolment mutations until they finish.
    let needs_totp = state
        .db
        .totp_is_confirmed(&account.username)
        .unwrap_or(false);

    if needs_totp {
        let presented = body.totp_code.as_deref().unwrap_or_default();
        if presented.trim().is_empty() {
            // Not a failure, and it must not be logged or counted as one.
            //
            // This is the *ordinary first step* of every 2FA login: the client cannot know a code
            // is wanted until it has asked, so it sends the passphrase alone and is told. Counting
            // it meant every single login began by spending an attempt from the anonymous bucket,
            // and any log parser watching for repeated 401s on this endpoint saw one per login —
            // which is exactly how a person doing nothing wrong ends up banned.
            tracing::info!(
                target: "agro::auth",
                stage = "second-factor-required",
                "a second factor was requested"
            );
            return (
                StatusCode::UNAUTHORIZED,
                [(AUTH_STAGE_HEADER, "second-factor-required")],
                Json(json!({
                    "error": "This account needs a code from its authenticator",
                    "totpRequired": true,
                })),
            )
                .into_response();
        }

        // From here the passphrase is already proved, so these attempts belong in their own bucket
        // rather than the anonymous one they used to share.
        if !state
            .rate_limiter
            .allow_second_factor(&client_ip, &account.username)
        {
            tracing::warn!(
                target: "agro::auth",
                stage = "second-factor-throttled",
                "too many second-factor attempts"
            );
            return too_many();
        }
        match state.db.verify_totp(&account.username, presented) {
            Ok(outcome) if outcome.is_satisfied() => {
                if outcome == crate::db_identity::TotpOutcome::AcceptedRecoveryCode {
                    state.db.record_event(
                        crate::audit::Event::RecoveryCodeUsed,
                        crate::audit::Record::new()
                            .user(&account.username)
                            .ip(&client_ip),
                    );
                }
            }
            Ok(outcome) => {
                // Deliberately *not* `note_failure`. That counter drives the per-account backoff
                // against passphrase guessing, and a wrong code from someone who already typed the
                // right passphrase is a different event: usually the owner, whose code expired
                // while they were reading it. Feeding it into the same counter meant a fumbled
                // authenticator slowed the account's real logins for the next fifteen minutes.
                //
                // It is still bounded — by `allow_second_factor` above, which cuts this off at 30
                // tries per five minutes from one address for one account.
                tracing::warn!(
                    target: "agro::auth",
                    stage = "second-factor-rejected",
                    "a second factor was rejected"
                );
                state.db.record_event(
                    crate::audit::Event::TotpFailed,
                    crate::audit::Record::new()
                        .user(&account.username)
                        .ip(&client_ip)
                        // "replayed" is worth telling apart from "wrong": it means a code that was
                        // already spent is being presented again, which is what a captured code
                        // looks like.
                        .detail(format!("{outcome:?}")),
                );
                return (
                    StatusCode::UNAUTHORIZED,
                    [(AUTH_STAGE_HEADER, "second-factor-rejected")],
                    Json(json!({
                        "error": "That code was not accepted",
                        "totpRequired": true,
                    })),
                )
                    .into_response();
            }
            Err(err) => {
                tracing::error!("could not check a second factor: {err}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "could not check the second factor" })),
                )
                    .into_response();
            }
        }
    }

    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("device");

    // The sealed vault key rides along with the token. The client has the passphrase in hand at
    // exactly this moment and at no other — it is discarded immediately afterwards, and a device
    // paired by QR never sees it at all — so this is the one point where it can unwrap the key that
    // reads its settings. Both fields are null on an account that has not enrolled one yet, which
    // is the client's cue to generate a key and enrol it.
    let (vault_salt, vault_key_wrapped) = state
        .db
        .vault_envelope(&account.username)
        .unwrap_or((None, None));

    let enrolment_owed = account.is_admin()
        && crate::auth::admin_totp_required()
        && !state
            .db
            .totp_is_confirmed(&account.username)
            .unwrap_or(false);

    match state.db.mint_device_token(&account.username, label) {
        Ok(token) => {
            state.rate_limiter.note_success(&username);
            state.db.record_event(
                crate::audit::Event::LoginSucceeded,
                crate::audit::Record::new()
                    .user(&account.username)
                    .ip(&client_ip)
                    .device(label),
            );
            Json(json!({
                "username": account.username,
                "role": account.role.as_str(),
                "token": token,
                "vaultSalt": vault_salt,
                "vaultKeyWrapped": vault_key_wrapped,
                // True when this account may do nothing but enrol a second factor. The client uses
                // it to go straight to the enrolment screen instead of showing a dashboard whose
                // every query is about to be refused.
                "totpEnrolmentRequired": enrolment_owed,
            }))
            .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("could not issue a token: {err}") })),
        )
            .into_response(),
    }
}
