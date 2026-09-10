//! A fixed-window counter, keyed by whatever the caller counts by.
//!
//! Pulled out of [`crate::login`], which had two copies of this loop with different budgets, once
//! the catalogue needed a third. The algorithm is the whole of it: a key gets a window, the window
//! gets a count, and the count either fits the budget or does not.
//!
//! In memory, like the login limiter it came from. A restart forgets every window, which is the
//! right trade for a server that already holds its whole database behind one mutex: persisting
//! counters would buy protection against an attacker willing to wait out a restart, and cost a
//! write on every request from everybody else.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Above this many live keys, expired ones are swept before the next insert. Bounds the map on a
/// server being scanned without paying for a sweep on every call.
const SWEEP_THRESHOLD: usize = 4096;

#[derive(Default)]
pub struct FixedWindow {
    windows: Mutex<HashMap<String, (Instant, usize)>>,
}

impl FixedWindow {
    pub fn new() -> Self {
        Self::default()
    }

    /// Charges `units` against `key` and says whether the caller may proceed.
    ///
    /// The charge lands whether or not it fits, so a caller that keeps asking keeps being refused
    /// until the window turns over — being over budget is not a reason to stop counting.
    ///
    /// `units` is what makes this usable for a batch: one call carrying twenty recordings costs
    /// twenty, so batching cannot be used to buy throughput a client could not have one request at
    /// a time.
    pub fn charge(&self, key: &str, units: usize, budget: usize, window: Duration) -> bool {
        let mut windows = self.windows.lock().unwrap();
        let now = Instant::now();

        if windows.len() > SWEEP_THRESHOLD {
            windows.retain(|_, (started, _)| now.duration_since(*started) < window);
        }

        let entry = windows.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(units);
        entry.1 <= budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(60);

    #[test]
    fn allows_up_to_the_budget_and_refuses_past_it() {
        let limiter = FixedWindow::new();
        for _ in 0..5 {
            assert!(limiter.charge("someone", 1, 5, WINDOW));
        }
        assert!(!limiter.charge("someone", 1, 5, WINDOW));
    }

    #[test]
    fn a_batch_costs_what_it_carries() {
        let limiter = FixedWindow::new();
        // Four at once fits a budget of five; the next four cannot, because the first four were
        // charged in full rather than counted as one request.
        assert!(limiter.charge("someone", 4, 5, WINDOW));
        assert!(!limiter.charge("someone", 4, 5, WINDOW));
    }

    #[test]
    fn one_key_running_out_does_not_spend_anothers_budget() {
        let limiter = FixedWindow::new();
        assert!(!limiter.charge("noisy", 9, 5, WINDOW));
        assert!(limiter.charge("quiet", 1, 5, WINDOW));
    }

    #[test]
    fn a_window_that_has_passed_starts_over() {
        let limiter = FixedWindow::new();
        let instant = Duration::from_nanos(1);
        assert!(!limiter.charge("someone", 6, 5, instant));
        // The previous window is already over by the time this asks, so the count restarts rather
        // than carrying the overage forward.
        assert!(limiter.charge("someone", 1, 5, instant));
    }
}
