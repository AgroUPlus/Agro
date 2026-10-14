//! Finishing a flow: linking an identity or signing in.

use super::*;
use crate::db_identity::{AccountState, Role};
use crate::AppState;
use axum::response::{IntoResponse, Redirect, Response};

/// Attaches the identity to the account that started the flow.
pub(super) fn finish_link(
    state: &AppState,
    config: &Config,
    username: &str,
    claims: &Claims,
) -> Response {
    match state.db.link_federated_identity(
        username,
        &config.issuer,
        &claims.sub,
        claims.preferred_username.as_deref(),
    ) {
        Ok(()) => {
            state.db.record_event(
                crate::audit::Event::IdentityLinked,
                crate::audit::Record::new()
                    .user(username)
                    .detail(format!("{} subject {}", config.issuer, claims.sub)),
            );
            Redirect::to("/#linked").into_response()
        }
        Err(err) => fail(&err),
    }
}

/// Signs in an already-linked identity, or creates a new account for an unknown one.
pub(super) fn finish_sign_in(state: &AppState, config: &Config, claims: &Claims) -> Response {
    let existing = state
        .db
        .account_for_federated_identity(&config.issuer, &claims.sub)
        .unwrap_or(None);

    let account = match existing {
        Some(account) => account,
        None => {
            // An unknown subject is always a *new* account. It is never matched against an existing
            // one by email or username — see the module docs.
            match create_account_for(state, config, claims) {
                Ok(account) => account,
                Err(err) => return fail(&err),
            }
        }
    };

    if !account.state.is_active() {
        state.db.record_event(
            crate::audit::Event::LoginFailed,
            crate::audit::Record::new()
                .user(&account.username)
                .detail("OIDC sign-in for an account awaiting approval"),
        );
        return fail("this account is waiting for an administrator to approve it");
    }

    let Ok(token) = state
        .db
        .mint_device_token(&account.username, "browser via SSO")
    else {
        return fail("could not issue a token");
    };
    state.db.record_event(
        crate::audit::Event::LoginSucceeded,
        crate::audit::Record::new()
            .user(&account.username)
            .detail("via SSO"),
    );

    // The vault envelope rides along exactly as it does for a passphrase login. It is null for an
    // account that has not enrolled one, which is the client's cue to ask for a vault PIN and enrol
    // — the server still never learns the PIN, so an SSO account keeps the same zero-knowledge
    // property a passphrase account has.
    let (vault_salt, vault_key_wrapped) = state
        .db
        .vault_envelope(&account.username)
        .unwrap_or((None, None));

    Redirect::to(&format!(
        "/#token={}&username={}&vaultSalt={}&vaultKeyWrapped={}",
        urlencoding::encode(&token),
        urlencoding::encode(&account.username),
        urlencoding::encode(vault_salt.as_deref().unwrap_or("")),
        urlencoding::encode(vault_key_wrapped.as_deref().unwrap_or("")),
    ))
    .into_response()
}

pub(super) fn create_account_for(
    state: &AppState,
    config: &Config,
    claims: &Claims,
) -> Result<crate::db_identity::Account, String> {
    // Signups being closed closes this door too. Identities that are *already* linked keep working
    // — only the creation of new accounts stops.
    if crate::login::signups_are_closed() {
        return Err("this server is not accepting new accounts".into());
    }

    let preferred = claims
        .preferred_username
        .as_deref()
        .or(claims.email.as_deref().and_then(|e| e.split('@').next()))
        .unwrap_or("user");
    let username = state
        .db
        .available_username_like(preferred)
        .ok_or("could not derive a usable username from that identity")?;

    let account_state = if Config::auto_approve() {
        AccountState::Active
    } else {
        AccountState::Pending
    };

    // The passphrase exists so the column is not empty; nobody is ever shown it. `has_usable_
    // passphrase` is what stops the account being unlinked into a state with no way in.
    let passphrase = crate::passphrase::generate_passphrase();
    let account = state
        .db
        .create_account(&username, &passphrase, Role::Member, account_state)
        .map_err(|e| format!("could not create an account: {e}"))?;
    state.db.mark_passphrase_unusable(&username).ok();

    state
        .db
        .link_federated_identity(
            &username,
            &config.issuer,
            &claims.sub,
            claims.preferred_username.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    state.db.record_event(
        crate::audit::Event::AccountCreated,
        crate::audit::Record::new()
            .user(&username)
            .detail(format!("via SSO, state {}", account.state.as_str())),
    );
    Ok(account)
}
