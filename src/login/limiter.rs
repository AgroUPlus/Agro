//! Per-client and per-account throttling for sign-in attempts.

use crate::lock::LockExt;
use crate::rate_limit::FixedWindow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many attempts one address gets per window.
pub(super) const MAX_ATTEMPTS: usize = 10;

pub(super) const WINDOW: Duration = Duration::from_secs(300);

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
pub(super) const MAX_SECOND_FACTOR_ATTEMPTS: usize = 30;

/// How many consecutive failures an account tolerates before answers start being slowed.
pub(super) const FREE_FAILURES: u32 = 5;

/// The longest an account's failed answer is delayed. Enough to make a distributed guessing run
/// impractical, short enough that a person who mistyped their passphrase does not think the server
/// has hung.
pub(super) const MAX_BACKOFF: Duration = Duration::from_secs(4);

/// How long a run of failures is remembered. A person who gets it wrong twice at breakfast and
/// once at lunch is not in the middle of an attack.
pub(super) const FAILURE_MEMORY: Duration = Duration::from_secs(900);

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
    hits: FixedWindow,
    /// Second-factor attempts, kept apart from [`hits`]. See [`MAX_SECOND_FACTOR_ATTEMPTS`].
    second_factor_hits: FixedWindow,
    failures: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long to wait before answering a failed attempt for `account`.
    pub fn backoff_for(&self, account: &str) -> Duration {
        let failures = self.failures.lock_recover();
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
        let mut failures = self.failures.lock_recover();
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
            .lock_recover()
            .remove(&account.to_ascii_lowercase());
    }

    /// Records a second-factor attempt and reports whether it is allowed.
    ///
    /// Keyed on address *and* account together. Per-address alone would let one person's fumbling
    /// with an authenticator lock out everybody else behind the same NAT — a household, an office,
    /// a campus — which is the failure this whole change exists to stop, reintroduced one level
    /// down. Per-account alone would let anyone who knows a username exhaust that account's bucket
    /// from anywhere, which is the lockout weapon `backoff_for` is written to avoid.
    pub(super) fn allow_second_factor(&self, client_ip: &str, account: &str) -> bool {
        let key = format!("{client_ip}|{}", account.to_ascii_lowercase());
        self.second_factor_hits
            .charge(&key, 1, MAX_SECOND_FACTOR_ATTEMPTS, WINDOW)
    }

    /// Records an attempt and reports whether it is allowed.
    pub(super) fn allow(&self, key: &str) -> bool {
        self.hits.charge(key, 1, MAX_ATTEMPTS, WINDOW)
    }
}
