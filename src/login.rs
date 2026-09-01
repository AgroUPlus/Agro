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

/// Names which step of a login an answer belongs to.
///
/// A 401 from this endpoint means three different things — the credentials were wrong, a code is
/// needed, a code was wrong — and only the first is a failed authentication. They were
/// indistinguishable to anything reading responses, so a bouncer counting 401s counted the normal
/// path. The status code is unchanged, because clients match on the body and changing it would
/// break every existing install; this says what the status code cannot.
const AUTH_STAGE_HEADER: &str = "X-Agro-Auth-Stage";

/// How many attempts one address gets per window.
const MAX_ATTEMPTS: usize = 10;
const WINDOW: Duration = Duration::from_secs(300);

/// How many second-factor attempts one address gets per window, once a passphrase has been proved.
///
/// Deliberately far more generous than [`MAX_ATTEMPTS`], because it is counting something else. A
/// TOTP code lives for thirty seconds: a person who opens their authenticator, reads six digits and
/// mistypes one has already spent two attempts, and the code they carefully re-read may expire
/// between the reading and the tapping. Under the anonymous bucket that was ten tries for the whole
/// login — and the *first* request of every login, the one that asks whether a code is needed at
/// all, spent one of them before the user had done anything.
///
/// This is not a hole. Reaching this bucket at all requires a correct passphrase, so an attacker
/// here is one who already has the password and is guessing a six-digit code that changes every
/// thirty seconds; 30 tries per five minutes against a million possibilities is not a threat the
/// smaller number was protecting anyone from.
const MAX_SECOND_FACTOR_ATTEMPTS: usize = 30;

/// How many consecutive failures an account tolerates before answers start being slowed.
const FREE_FAILURES: u32 = 5;

/// The longest an account's failed answer is delayed. Enough to make a distributed guessing run
/// impractical, short enough that a person who mistyped their passphrase does not think the server
/// has hung.
const MAX_BACKOFF: Duration = Duration::from_secs(4);

/// How long a run of failures is remembered. A person who gets it wrong twice at breakfast and
/// once at lunch is not in the middle of an attack.
const FAILURE_MEMORY: Duration = Duration::from_secs(900);

/// A fixed-window counter per client address, plus a per-account slowdown.
///
/// **By address**, this is a hard limit: an attacker guessing from one place is cut off.
///
/// **By account**, it deliberately is *not*. A hard per-account limit is a way to lock a known
/// account out by exhausting its bucket on purpose — the original note here said so, and it was
/// right. What it did not cover is the case that motivates counting by account at all: a botnet
/// spreading guesses across thousands of addresses never fills any one address bucket, so the
/// address limit alone never fires.
///
/// The answer is to make failures *slow* rather than *refused*. Consecutive failures against one
/// account add a delay that doubles, capped at [`MAX_BACKOFF`], and reset the moment someone signs
/// in successfully. An attacker cannot lock anyone out — the legitimate user's correct passphrase
/// still works on the first try — but a distributed run pays the delay on every guess.
///
/// The delay applies to usernames that do not exist too. Otherwise "this one answered instantly"
/// becomes the enumeration oracle that [`login`] refuses to be in every other respect.
#[derive(Default)]
pub struct RateLimiter {
    hits: Mutex<HashMap<String, (Instant, usize)>>,
    /// Second-factor attempts, kept apart from [`hits`]. See [`MAX_SECOND_FACTOR_ATTEMPTS`].
    second_factor_hits: Mutex<HashMap<String, (Instant, usize)>>,
    failures: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long to wait before answering a failed attempt for `account`.
    pub fn backoff_for(&self, account: &str) -> Duration {
        let failures = self.failures.lock().unwrap();
        let Some((seen, count)) = failures.get(&account.to_ascii_lowercase()) else {
            return Duration::ZERO;
        };
        if seen.elapsed() >= FAILURE_MEMORY || *count <= FREE_FAILURES {
            return Duration::ZERO;
        }
        // Doubling from a quarter second: 0.25, 0.5, 1, 2, then the cap.
        let steps = (*count - FREE_FAILURES).min(8);
        Duration::from_millis(250u64.saturating_mul(1 << (steps - 1))).min(MAX_BACKOFF)
    }

    /// Records a failed attempt against an account.
    pub fn note_failure(&self, account: &str) {
        let mut failures = self.failures.lock().unwrap();
        let now = Instant::now();
        if failures.len() > 4096 {
            failures.retain(|_, (seen, _)| now.duration_since(*seen) < FAILURE_MEMORY);
        }
        let entry = failures
            .entry(account.to_ascii_lowercase())
            .or_insert((now, 0));
        if now.duration_since(entry.0) >= FAILURE_MEMORY {
            *entry = (now, 0);
        }
        entry.0 = now;
        entry.1 = entry.1.saturating_add(1);
    }

    /// Clears the run of failures after a successful sign-in.
    pub fn note_success(&self, account: &str) {
        self.failures
            .lock()
            .unwrap()
            .remove(&account.to_ascii_lowercase());
    }

    /// Records a second-factor attempt and reports whether it is allowed.
    ///
    /// Keyed on address *and* account together. Per-address alone would let one person's fumbling
    /// with an authenticator lock out everybody else behind the same NAT — a household, an office,
    /// a campus — which is the failure this whole change exists to stop, reintroduced one level
    /// down. Per-account alone would let anyone who knows a username exhaust that account's bucket
    /// from anywhere, which is the lockout weapon `backoff_for` is written to avoid.
    fn allow_second_factor(&self, client_ip: &str, account: &str) -> bool {
        let key = format!("{client_ip}|{}", account.to_ascii_lowercase());
        let mut hits = self.second_factor_hits.lock().unwrap();
        let now = Instant::now();
        if hits.len() > 4096 {
            hits.retain(|_, (started, _)| now.duration_since(*started) < WINDOW);
        }
        let entry = hits.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= MAX_SECOND_FACTOR_ATTEMPTS
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
pub(crate) fn client_ip(peer: SocketAddr, headers: &HeaderMap) -> String {
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
    let needs_totp = state.db.totp_is_confirmed(&account.username).unwrap_or(false);

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
        if !state.rate_limiter.allow_second_factor(&client_ip, &account.username) {
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
                        crate::audit::Record::new().user(&account.username).ip(&client_ip),
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
        && !state.db.totp_is_confirmed(&account.username).unwrap_or(false);

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
fn refused(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(AUTH_STAGE_HEADER, "credentials-rejected")],
        Json(json!({ "error": message })),
    )
        .into_response()
}

fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "Too many attempts. Wait a few minutes and try again." })),
    )
        .into_response()
}

#[cfg(test)]
pub(crate) mod tests {
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

    #[test]
    fn an_account_is_slowed_only_after_a_run_of_failures() {
        let limiter = RateLimiter::new();
        for _ in 0..FREE_FAILURES {
            limiter.note_failure("alpha");
        }
        assert!(
            limiter.backoff_for("alpha").is_zero(),
            "a few mistyped passphrases must not be punished"
        );
        limiter.note_failure("alpha");
        assert!(!limiter.backoff_for("alpha").is_zero());
    }

    /// A hard per-account limit would let anyone lock a known account out. The delay grows but is
    /// capped, and the correct passphrase still works on the first try.
    #[test]
    fn the_slowdown_is_capped_and_never_becomes_a_refusal() {
        let limiter = RateLimiter::new();
        for _ in 0..200 {
            limiter.note_failure("alpha");
        }
        let backoff = limiter.backoff_for("alpha");
        assert!(backoff <= MAX_BACKOFF, "backoff ran away: {backoff:?}");
        assert!(!backoff.is_zero());
    }

    #[test]
    fn signing_in_clears_the_run() {
        let limiter = RateLimiter::new();
        for _ in 0..20 {
            limiter.note_failure("alpha");
        }
        limiter.note_success("alpha");
        assert!(limiter.backoff_for("alpha").is_zero());
    }

    /// Otherwise "this one answered instantly" tells an anonymous caller which usernames exist.
    #[test]
    fn an_unknown_username_is_slowed_the_same_way() {
        let limiter = RateLimiter::new();
        for _ in 0..20 {
            limiter.note_failure("nobody-by-that-name");
            limiter.note_failure("alpha");
        }
        assert_eq!(
            limiter.backoff_for("nobody-by-that-name"),
            limiter.backoff_for("alpha")
        );
    }

    #[test]
    fn the_account_slowdown_is_case_insensitive() {
        let limiter = RateLimiter::new();
        for _ in 0..20 {
            limiter.note_failure("Alpha");
        }
        assert!(!limiter.backoff_for("alpha").is_zero());
    }

    /// One account's failures must not slow another's sign-in.
    #[test]
    fn the_slowdown_is_per_account() {
        let limiter = RateLimiter::new();
        for _ in 0..20 {
            limiter.note_failure("alpha");
        }
        assert!(limiter.backoff_for("mallory").is_zero());
    }

    /// Also used by `relay`'s handler tests, so the two do not drift apart on what an `AppState`
    /// is made of.
    pub(crate) fn test_state(db: crate::db::Db) -> AppState {
        let ws_hub = std::sync::Arc::new(crate::ws::WsHub::new());
        AppState {
            db: db.clone(),
            ws_hub: ws_hub.clone(),
            storage: crate::storage::Storage::for_tests(),
            offers: crate::offers::OfferBatcher::spawn(db, ws_hub),
            relay_hub: crate::relay::RelayHub::new(),
            http_client: reqwest::Client::new(),
            setup_token: crate::auth::SetupToken::for_fresh_server(1),
            rate_limiter: std::sync::Arc::new(RateLimiter::new()),
            oidc_flows: std::sync::Arc::new(crate::oidc::FlowStore::new()),
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

    /// The bug: fumbling an authenticator used to spend the anonymous login bucket.
    #[test]
    fn second_factor_attempts_do_not_exhaust_the_login_bucket() {
        let limiter = RateLimiter::new();
        for _ in 0..MAX_SECOND_FACTOR_ATTEMPTS {
            assert!(limiter.allow_second_factor("203.0.113.7", "ada"));
        }
        assert!(
            limiter.allow("203.0.113.7"),
            "second-factor attempts consumed the address's login attempts"
        );
    }

    /// Bounded, though — a correct passphrase does not buy unlimited guesses at the code.
    #[test]
    fn second_factor_attempts_are_still_capped() {
        let limiter = RateLimiter::new();
        for _ in 0..MAX_SECOND_FACTOR_ATTEMPTS {
            limiter.allow_second_factor("203.0.113.7", "ada");
        }
        assert!(!limiter.allow_second_factor("203.0.113.7", "ada"));
    }

    /// One person's authenticator trouble must not lock out the household behind the same NAT.
    #[test]
    fn one_account_exhausting_its_codes_does_not_block_another_behind_one_address() {
        let limiter = RateLimiter::new();
        for _ in 0..(MAX_SECOND_FACTOR_ATTEMPTS + 5) {
            limiter.allow_second_factor("203.0.113.7", "ada");
        }
        assert!(
            limiter.allow_second_factor("203.0.113.7", "grace"),
            "a housemate was locked out by someone else's authenticator"
        );
    }

    /// And nobody can lock an account out from elsewhere by burning its bucket.
    #[test]
    fn an_account_cannot_be_locked_out_from_another_address() {
        let limiter = RateLimiter::new();
        for _ in 0..(MAX_SECOND_FACTOR_ATTEMPTS + 5) {
            limiter.allow_second_factor("198.51.100.4", "ada");
        }
        assert!(
            limiter.allow_second_factor("203.0.113.7", "ada"),
            "an attacker elsewhere locked this account out of its own second factor"
        );
    }

    /// A wrong code must not slow the account's real logins: that counter is for passphrases.
    #[test]
    fn a_wrong_code_does_not_add_passphrase_backoff() {
        let limiter = RateLimiter::new();
        for _ in 0..(FREE_FAILURES + 4) {
            limiter.allow_second_factor("203.0.113.7", "ada");
        }
        assert_eq!(
            limiter.backoff_for("ada"),
            Duration::ZERO,
            "second-factor attempts leaked into the passphrase backoff"
        );
    }
}
