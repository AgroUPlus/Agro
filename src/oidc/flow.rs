//! In-flight sign-in flows: single-use and short-lived.

use crate::lock::LockExt;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a started flow may take to come back. Long enough to type a password and a second
/// factor at the IdP, short enough that a `state` value is not usable an hour later.
pub(super) const FLOW_TTL: Duration = Duration::from_secs(600);

/// A flow in progress: what we sent, and what we expect back.
pub(super) struct PendingFlow {
    pub(super) started: Instant,
    pub(super) nonce: String,
    pub(super) pkce_verifier: String,
    /// Set when this flow was started from a signed-in session in order to *link*. The account is
    /// captured at start, so the callback cannot be pointed at a different one.
    pub(super) link_to: Option<String>,
}

/// The flows this server is waiting on.
///
/// In memory rather than in the database: a `state` is worthless after ten minutes, and a restart
/// invalidating half-finished sign-ins is the correct behaviour rather than a bug. The shape is
/// deliberately the same as `login::RateLimiter` — a mutex, a map, and an opportunistic sweep —
/// so there is one pattern for this in the codebase and not two.
#[derive(Default)]
pub struct FlowStore {
    pub(super) flows: Mutex<HashMap<String, PendingFlow>>,
}

impl FlowStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn insert(&self, state: String, flow: PendingFlow) {
        let mut flows = self.flows.lock_recover();
        if flows.len() > 1024 {
            flows.retain(|_, f| f.started.elapsed() < FLOW_TTL);
        }
        flows.insert(state, flow);
    }

    /// Takes a flow, removing it. Single-use: a `state` that has been redeemed is gone, so a
    /// callback URL cannot be replayed out of a browser history or a proxy log.
    pub(super) fn take(&self, state: &str) -> Option<PendingFlow> {
        let mut flows = self.flows.lock_recover();
        let flow = flows.remove(state)?;
        (flow.started.elapsed() < FLOW_TTL).then_some(flow)
    }
}
