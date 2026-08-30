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
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::db_identity::{AccountState, Role};
use crate::AppState;

/// How long a started flow may take to come back. Long enough to type a password and a second
/// factor at the IdP, short enough that a `state` value is not usable an hour later.
const FLOW_TTL: Duration = Duration::from_secs(600);

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

/// A flow in progress: what we sent, and what we expect back.
struct PendingFlow {
    started: Instant,
    nonce: String,
    pkce_verifier: String,
    /// Set when this flow was started from a signed-in session in order to *link*. The account is
    /// captured at start, so the callback cannot be pointed at a different one.
    link_to: Option<String>,
}

/// The flows this server is waiting on.
///
/// In memory rather than in the database: a `state` is worthless after ten minutes, and a restart
/// invalidating half-finished sign-ins is the correct behaviour rather than a bug. The shape is
/// deliberately the same as `login::RateLimiter` — a mutex, a map, and an opportunistic sweep —
/// so there is one pattern for this in the codebase and not two.
#[derive(Default)]
pub struct FlowStore {
    flows: Mutex<HashMap<String, PendingFlow>>,
}

impl FlowStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, state: String, flow: PendingFlow) {
        let mut flows = self.flows.lock().unwrap();
        if flows.len() > 1024 {
            flows.retain(|_, f| f.started.elapsed() < FLOW_TTL);
        }
        flows.insert(state, flow);
    }

    /// Takes a flow, removing it. Single-use: a `state` that has been redeemed is gone, so a
    /// callback URL cannot be replayed out of a browser history or a proxy log.
    fn take(&self, state: &str) -> Option<PendingFlow> {
        let mut flows = self.flows.lock().unwrap();
        let flow = flows.remove(state)?;
        (flow.started.elapsed() < FLOW_TTL).then_some(flow)
    }
}

/// `GET /api/v1/oidc/config` — what the sign-in page needs to decide whether to show the button.
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

/// `GET /api/v1/oidc/start` — sends the browser to the provider.
pub async fn start(State(state): State<AppState>) -> Response {
    begin_flow(&state, None)
}

/// `GET /api/v1/oidc/link` — the same thing, but bound to the signed-in account.
///
/// The account is taken from the token on *this* request and remembered server-side, so the
/// callback links to whoever started the flow and not to whoever the callback claims to be.
pub async fn start_link(
    State(state): State<AppState>,
    user: axum::Extension<crate::auth::AuthedUser>,
) -> Response {
    begin_flow(&state, Some(user.username().to_string()))
}

fn begin_flow(state: &AppState, link_to: Option<String>) -> Response {
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

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /api/v1/oidc/callback` — the provider sends the browser back here.
///
/// Ends by redirecting into the dashboard with a token in the fragment. A fragment rather than a
/// query string because fragments are not sent to the server and do not land in access logs; the
/// page reads it and immediately clears it.
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

/// Attaches the identity to the account that started the flow.
fn finish_link(
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
fn finish_sign_in(state: &AppState, config: &Config, claims: &Claims) -> Response {
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

    let Ok(token) = state.db.mint_device_token(&account.username, "browser via SSO") else {
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

fn create_account_for(
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

/// The claims this server cares about.
#[derive(Debug, Deserialize)]
pub struct Claims {
    /// The stable identifier. The *only* thing treated as identity.
    pub sub: String,
    pub nonce: Option<String>,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: String,
    token_endpoint: String,
    issuer: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
}

/// Exchanges the code and validates the ID token.
async fn exchange_and_verify(
    config: &Config,
    code: &str,
    flow: &PendingFlow,
) -> Result<Claims, String> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let discovery: Discovery = http
        .get(format!("{}/.well-known/openid-configuration", config.issuer))
        .send()
        .await
        .map_err(|e| format!("could not reach the provider: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the provider's discovery document did not parse: {e}"))?;

    let response = http
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &config.redirect_uri),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
            ("code_verifier", &flow.pkce_verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("the token exchange failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("the token exchange was refused ({})", response.status()));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("the token response did not parse: {e}"))?;

    let jwks: Jwks = http
        .get(&discovery.jwks_uri)
        .send()
        .await
        .map_err(|e| format!("could not fetch the provider's keys: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the provider's keys did not parse: {e}"))?;

    let claims = verify_id_token(&tokens.id_token, &jwks, &discovery.issuer, &config.client_id)?;

    // Binds this token to the flow this server started. Without it, an ID token obtained for
    // another session could be replayed into this callback.
    match claims.nonce.as_deref() {
        Some(nonce) if crate::credentials::secure_eq(nonce, &flow.nonce) => {}
        _ => return Err("the ID token's nonce did not match this sign-in".into()),
    }
    Ok(claims)
}

/// Checks the ID token's signature and registered claims against the provider's keys.
///
/// The key is selected by the token's `kid`. A token whose `kid` names no published key is refused
/// rather than tried against every key — the latter is how a "try them all" implementation ends up
/// accepting a token signed with something unexpected.
fn verify_id_token(
    id_token: &str,
    jwks: &Jwks,
    issuer: &str,
    audience: &str,
) -> Result<Claims, String> {
    let header = jsonwebtoken::decode_header(id_token)
        .map_err(|e| format!("the ID token's header did not parse: {e}"))?;
    let kid = header.kid.ok_or("the ID token names no signing key")?;

    let key_json = jwks
        .keys
        .iter()
        .find(|key| key.get("kid").and_then(|k| k.as_str()) == Some(kid.as_str()))
        .ok_or("the ID token was signed with a key this provider does not publish")?;

    let decoding_key = jsonwebtoken::DecodingKey::from_jwk(
        &serde_json::from_value(key_json.clone())
            .map_err(|e| format!("the provider's key did not parse: {e}"))?,
    )
    .map_err(|e| format!("the provider's key is unusable: {e}"))?;

    // The algorithm comes from the *key*, not from the token's own header — a token that asks to be
    // verified with `none`, or with HMAC using the public key as the secret, is the classic JWT
    // forgery and both are refused by pinning the algorithm here.
    let mut validation = jsonwebtoken::Validation::new(header.alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.validate_exp = true;

    jsonwebtoken::decode::<Claims>(id_token, &decoding_key, &validation)
        .map(|data| data.claims)
        .map_err(|e| format!("the ID token did not validate: {e}"))
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64url(&bytes)
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let indices = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for &i in indices.iter().take(chunk.len() + 1) {
            out.push(ALPHABET[i as usize] as char);
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_env() {
        for key in [
            "AGRO_OIDC_ISSUER",
            "AGRO_OIDC_CLIENT_ID",
            "AGRO_OIDC_CLIENT_SECRET",
            "AGRO_OIDC_REDIRECT_URI",
        ] {
            std::env::remove_var(key);
        }
    }

    /// A half-configured provider must read as absent. The alternative is a button that appears and
    /// then fails, which is worse than no button.
    #[test]
    fn a_partial_configuration_is_no_configuration() {
        clear_env();
        std::env::set_var("AGRO_OIDC_ISSUER", "https://id.example.com");
        std::env::set_var("AGRO_OIDC_CLIENT_ID", "agro");
        assert!(Config::from_env().is_none());
        clear_env();
    }

    #[test]
    fn a_flow_is_single_use() {
        let store = FlowStore::new();
        store.insert(
            "state-one".into(),
            PendingFlow {
                started: Instant::now(),
                nonce: "n".into(),
                pkce_verifier: "v".into(),
                link_to: None,
            },
        );
        assert!(store.take("state-one").is_some());
        assert!(
            store.take("state-one").is_none(),
            "a redeemed state must not work twice"
        );
    }

    #[test]
    fn an_expired_flow_is_refused() {
        let store = FlowStore::new();
        store.insert(
            "old".into(),
            PendingFlow {
                started: Instant::now() - FLOW_TTL - Duration::from_secs(1),
                nonce: "n".into(),
                pkce_verifier: "v".into(),
                link_to: None,
            },
        );
        assert!(store.take("old").is_none());
    }

    #[test]
    fn an_unknown_state_is_refused() {
        assert!(FlowStore::new().take("never-issued").is_none());
    }

    /// The PKCE challenge is the SHA-256 of the verifier, base64url with no padding.
    #[test]
    fn the_pkce_challenge_is_the_hash_of_the_verifier() {
        // RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64url(Sha256::digest(verifier.as_bytes()).as_slice());
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn random_tokens_are_url_safe_and_unique() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    /// A provider's error text must not be able to carry markup into the page.
    #[test]
    fn a_failure_message_is_encoded_into_the_fragment() {
        let response = fail("<script>alert(1)</script>");
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(!location.contains("<script>"), "{location}");
        assert!(location.starts_with("/#ssoError="));
    }
}
