//! Sign-in throttling.

use super::*;

#[test]
fn a_client_is_cut_off_after_the_limit() {
    let limiter = RateLimiter::new();
    for attempt in 1..=MAX_ATTEMPTS {
        assert!(
            limiter.allow("10.0.0.1"),
            "attempt {attempt} should be allowed"
        );
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
