//! The two endpoints that can be reached without a token.
//!
//! Everything else on this server requires a bearer token, which leaves an obvious problem: there
//! has to be some way to *get* one. These are it, and they live outside `/graphql` deliberately —
//! opening the GraphQL endpoint to unauthenticated callers would expose the whole schema, its
//! parser and its executor to anyone who can reach the port, to solve a two-request problem.
//!
//! Both are rate-limited per client address. A login endpoint without one is an offline password
//! cracker with the server's own CPU doing the work.

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::db_identity::{AccountState, Role};
use crate::passphrase::generate_passphrase;
use crate::AppState;

/// How many attempts one address gets per window.
const MAX_ATTEMPTS: usize = 10;
const WINDOW: Duration = Duration::from_secs(300);

/// A fixed-window counter per client address.
///
/// Deliberately simple: a token bucket per account would be more precise but is also a way for an
/// attacker to lock a known account out by exhausting *its* bucket. Counting by source address
/// costs the attacker something and costs the account nothing.
#[derive(Default)]
pub struct RateLimiter {
    hits: Mutex<HashMap<String, (Instant, usize)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an attempt and reports whether it is allowed.
    fn allow(&self, key: &str) -> bool {
        let mut hits = self.hits.lock().unwrap();
        let now = Instant::now();

        // Opportunistic sweep, so the map cannot grow without bound on a server being scanned.
        if hits.len() > 4096 {
            hits.retain(|_, (started, _)| now.duration_since(*started) < WINDOW);
        }

        let entry = hits.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= MAX_ATTEMPTS
    }
}

/// Whether a peer is allowed to speak for someone else by setting `X-Forwarded-For`.
///
/// Only a reverse proxy may, and the header is worthless from anyone else — it is chosen by the
/// client, so honouring it unconditionally would turn the rate limiter off for anybody who can
/// type a header. Loopback and the private ranges are trusted by default because a request that
/// genuinely came from the internet cannot arrive with a private source address, and `AGRO_TRUSTED_PROXY`
/// overrides that for a proxy on a public address.
fn is_trusted_proxy(peer: IpAddr) -> bool {
    if let Ok(configured) = std::env::var("AGRO_TRUSTED_PROXY") {
        return configured
            .split(',')
            .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
            .any(|allowed| allowed == peer);
    }
    match peer {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// The address the request really came from, for rate-limiting purposes.
///
/// Behind a reverse proxy every request shares one peer address — the proxy's — so counting by
/// peer put the entire internet into a single bucket of [`MAX_ATTEMPTS`]. One person mistyping a
/// passphrase locked out everybody, which is a denial of service against your own users and was
/// exactly what a rate limiter is supposed to prevent.
///
/// The *rightmost* untrusted entry is taken, not the leftmost. `X-Forwarded-For` is appended to as
/// it passes through each hop, so the left end is whatever the original client chose to send and
/// only the right end was written by infrastructure that can be trusted.
fn client_ip(peer: SocketAddr, headers: &HeaderMap) -> String {
    if is_trusted_proxy(peer.ip()) {
        if let Some(forwarded) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            let candidate = forwarded
                .split(',')
                .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
                .rev()
                .find(|address| !is_trusted_proxy(*address));
            if let Some(address) = candidate {
                return address.to_string();
            }
        }
    }
    peer.ip().to_string()
}

#[derive(Deserialize)]
pub struct LoginBody {
    username: String,
    passphrase: String,
    /// What to call this device in the app-password list. Defaults to something honest.
    label: Option<String>,
}

/// Exchanges a passphrase for a device token.
///
/// The passphrase itself is never a bearer token — that equivalence is what let a revocable device
/// credential be traded for the permanent account one. What comes back is scoped to this device
/// and can be revoked on its own.
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    if !state.rate_limiter.allow(&client_ip(addr, &headers)) {
        return too_many();
    }

    let username = body.username.trim().to_lowercase();
    let Ok(Some(account)) = state.db.verify_login(&username, &body.passphrase) else {
        // One answer for "no such account" and "wrong passphrase" alike. Telling them apart turns
        // this into a way to find out who has an account here.
        return refused("Those credentials were not accepted");
    };

    if !account.state.is_active() {
        return refused("This account is not active yet");
    }

    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("device");

    match state.db.mint_device_token(&account.username, label) {
        Ok(token) => Json(json!({
            "username": account.username,
            "role": account.role.as_str(),
            "token": token,
        }))
        .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("could not issue a token: {err}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct BootstrapBody {
    setup_token: String,
    username: String,
}

/// Creates the first administrator on an empty server.
///
/// Needs the one-time setup token printed to the server's log at boot. This replaces a window in
/// the middleware that let *any* unauthenticated request through while no accounts existed — a
/// race the operator had to win against whoever was scanning the port.
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
    let account = match state
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

    Json(json!({
        "username": account.username,
        // Shown once. The server keeps an Argon2 hash and cannot show it again.
        "passphrase": passphrase,
        "token": token,
    }))
    .into_response()
}

/// How an instance treats strangers.
///
/// Read from `AGRO_SIGNUP` once per request rather than cached, so an operator can close signups on
/// a server that is being abused without restarting it and dropping every live WebSocket.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SignupMode {
    /// Anyone may register; the account waits for an admin to let it in.
    Approval,
    /// A valid invite code is required, and spending one lets the account straight in.
    Invite,
    Closed,
}

impl SignupMode {
    fn from_env() -> Self {
        match std::env::var("AGRO_SIGNUP")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "invite" => SignupMode::Invite,
            "closed" => SignupMode::Closed,
            // Anything unrecognised — including unset — is the documented default. A typo in the
            // environment must not silently throw the doors open.
            _ => SignupMode::Approval,
        }
    }
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

#[derive(Deserialize)]
pub struct SignupBody {
    username: String,
    invite_code: Option<String>,
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

    Json(json!({
        "username": account.username,
        "state": account.state.as_str(),
        // Shown once. The server keeps an Argon2 hash and cannot show it again.
        "passphrase": passphrase,
    }))
    .into_response()
}

fn refused(message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
}

fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "Too many attempts. Wait a few minutes and try again." })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_is_cut_off_after_the_limit() {
        let limiter = RateLimiter::new();
        for attempt in 1..=MAX_ATTEMPTS {
            assert!(limiter.allow("10.0.0.1"), "attempt {attempt} should be allowed");
        }
        assert!(!limiter.allow("10.0.0.1"), "the limit was not enforced");
    }

    /// One client being throttled must not throttle everyone else.
    #[test]
    fn the_limit_is_per_client() {
        let limiter = RateLimiter::new();
        for _ in 0..=MAX_ATTEMPTS {
            limiter.allow("10.0.0.1");
        }
        assert!(!limiter.allow("10.0.0.1"));
        assert!(limiter.allow("10.0.0.2"));
    }

    fn test_state(db: crate::db::Db) -> AppState {
        let ws_hub = std::sync::Arc::new(crate::ws::WsHub::new());
        AppState {
            db: db.clone(),
            ws_hub: ws_hub.clone(),
            storage: crate::storage::Storage::for_tests(),
            offers: crate::offers::OfferBatcher::spawn(db, ws_hub),
            relay_hub: crate::relay::RelayHub::new(),
            setup_token: crate::auth::SetupToken::for_fresh_server(1),
            rate_limiter: std::sync::Arc::new(RateLimiter::new()),
        }
    }

    async fn post_signup(state: AppState, username: &str) -> StatusCode {
        signup(
            State(state),
            ConnectInfo("10.0.0.9:5000".parse().unwrap()),
            HeaderMap::new(),
            Json(SignupBody {
                username: username.to_string(),
                invite_code: None,
            }),
        )
        .await
        .status()
    }

    /// Bootstrap has to come first, or the instance has no administrator and cannot get one.
    ///
    /// A stranger signing up on an empty database creates account #1, and account #1 existing is
    /// exactly what makes `/api/v1/bootstrap` refuse from then on. The result is a server with a
    /// pending account and nobody holding the authority to approve it — unrecoverable without
    /// touching the database by hand.
    #[tokio::test]
    async fn nobody_can_sign_up_before_the_server_has_an_administrator() {
        let db = crate::db::Db::new_in_memory().unwrap();
        let state = test_state(db.clone());

        assert_eq!(
            post_signup(state.clone(), "squatter").await,
            StatusCode::UNAUTHORIZED,
            "signup was allowed to claim the empty database"
        );
        assert_eq!(db.user_count().unwrap(), 0, "a refused signup still created an account");

        // With an administrator in place it behaves normally again.
        db.create_account("alpha", "alpha-pass", Role::Admin, AccountState::Active)
            .unwrap();
        assert_eq!(
            post_signup(state, "beta").await,
            StatusCode::OK,
            "signup stayed shut after the server had an administrator"
        );
    }
}

#[cfg(test)]
mod client_ip_tests {
    use super::*;

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    /// The whole point: two people behind one proxy must not share a bucket.
    ///
    /// Counting by peer address put every public request into a single counter, because behind a
    /// reverse proxy every request has the same peer. Ten attempts later nobody could log in.
    #[test]
    fn two_clients_behind_one_proxy_are_counted_separately() {
        let proxy: SocketAddr = "192.168.1.99:44100".parse().unwrap();
        let first = client_ip(proxy, &forwarded("203.0.113.7"));
        let second = client_ip(proxy, &forwarded("198.51.100.4"));
        assert_ne!(first, second, "both clients landed in the same bucket");
        assert_eq!(first, "203.0.113.7");

        let limiter = RateLimiter::new();
        for _ in 0..MAX_ATTEMPTS {
            limiter.allow(&first);
        }
        assert!(!limiter.allow(&first), "the exhausted client is still allowed");
        assert!(limiter.allow(&second), "one client's attempts locked out another");
    }

    /// A header from someone who is not a proxy is a claim, not a fact.
    #[test]
    fn a_direct_caller_cannot_choose_its_own_identity() {
        let direct: SocketAddr = "203.0.113.7:44100".parse().unwrap();
        assert_eq!(
            client_ip(direct, &forwarded("198.51.100.4")),
            "203.0.113.7",
            "an untrusted caller was allowed to spoof its address and dodge the limiter"
        );
    }

    /// The header is appended to at each hop, so only the right-hand end is trustworthy.
    #[test]
    fn a_spoofed_prefix_is_ignored() {
        let proxy: SocketAddr = "10.0.0.5:44100".parse().unwrap();
        assert_eq!(
            client_ip(proxy, &forwarded("1.2.3.4, 203.0.113.7")),
            "203.0.113.7",
            "the client's own invented entry was believed over the proxy's"
        );
    }

    /// Nothing to go on falls back to the peer rather than to a shared key.
    #[test]
    fn a_missing_or_unusable_header_falls_back_to_the_peer() {
        let proxy: SocketAddr = "10.0.0.5:44100".parse().unwrap();
        assert_eq!(client_ip(proxy, &HeaderMap::new()), "10.0.0.5");
        assert_eq!(client_ip(proxy, &forwarded("not-an-address")), "10.0.0.5");
    }
}
