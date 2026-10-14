//! The two endpoints that can be reached without a token.
//!
//! Everything else on this server requires a bearer token, which leaves an obvious problem: there
//! has to be some way to *get* one. These are it, and they live outside `/graphql` deliberately —
//! opening the GraphQL endpoint to unauthenticated callers would expose the whole schema, its
//! parser and its executor to anyone who can reach the port, to solve a two-request problem.
//!
//! Both are rate-limited per client address. A login endpoint without one is an offline password
//! cracker with the server's own CPU doing the work.

/// Names which step of a login an answer belongs to.
///
/// A 401 from this endpoint means three different things — the credentials were wrong, a code is
/// needed, a code was wrong — and only the first is a failed authentication. They were
/// indistinguishable to anything reading responses, so a bouncer counting 401s counted the normal
/// path. The status code is unchanged, because clients match on the body and changing it would
/// break every existing install; this says what the status code cannot.
const AUTH_STAGE_HEADER: &str = "X-Agro-Auth-Stage";

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

#[cfg(test)]
mod client_ip_tests {
    use super::limiter::{FREE_FAILURES, MAX_ATTEMPTS, MAX_SECOND_FACTOR_ATTEMPTS};
    use super::*;
    use axum::http::HeaderMap;
    use std::net::SocketAddr;
    use std::time::Duration;

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
        assert!(
            !limiter.allow(&first),
            "the exhausted client is still allowed"
        );
        assert!(
            limiter.allow(&second),
            "one client's attempts locked out another"
        );
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

mod bootstrap;
mod limiter;
mod proxy;
mod sign_in;
mod signup;
pub use bootstrap::__path_bootstrap;
pub use bootstrap::bootstrap;
pub use bootstrap::BootstrapBody;
pub use limiter::RateLimiter;
pub(crate) use proxy::client_ip;
pub use sign_in::__path_login;
pub use sign_in::login;
pub use sign_in::LoginBody;
pub use signup::__path_signup;
pub use signup::normalise_username;
use signup::refused;
pub use signup::signup;
pub use signup::signups_are_closed;
use signup::too_many;
pub use signup::SignupBody;
pub(crate) use signup::SignupMode;
#[cfg(test)]
pub(crate) mod tests;
