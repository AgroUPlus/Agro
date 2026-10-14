//! Where each device can be reached: LAN addresses, egress and P2P grants.

use super::*;

impl WsHub {
    /// Stores a node's local LAN address in memory for peer-to-peer transfers while online.
    pub fn set_lan_address(&self, user_id: &str, device_id: &str, lan_address: &str) {
        if let Ok(mut map) = self.live_peers.write() {
            map.entry((user_id.to_string(), device_id.to_string()))
                .or_default()
                .lan = Some(lan_address.to_string());
        }
    }

    /// Records where a device's connection reached the server from.
    ///
    /// Separate from the LAN address because the server learns them at different moments and from
    /// different sources: the LAN address is claimed by the client, this is observed.
    pub fn set_egress_address(&self, user_id: &str, device_id: &str, egress: &str) {
        if let Ok(mut map) = self.live_peers.write() {
            map.entry((user_id.to_string(), device_id.to_string()))
                .or_default()
                .egress = Some(egress.to_string());
        }
    }

    /// Looks up a node's active local LAN address from memory.
    pub fn get_lan_address(&self, user_id: &str, device_id: &str) -> Option<String> {
        self.live_peers
            .read()
            .ok()
            .and_then(|map| {
                map.get(&(user_id.to_string(), device_id.to_string()))
                    .cloned()
            })
            .and_then(|peer| peer.lan)
    }

    /// Whether two connected devices look like they are on the same local network.
    ///
    /// The test is that both connections reached the server from the same public address, which is
    /// what being behind one NAT looks like from here. It is a heuristic in one direction only:
    ///
    /// - **False positives exist.** Carrier-grade NAT puts thousands of unrelated mobile
    ///   subscribers behind one IPv4 address, as do large campus and office networks. This is why
    ///   the LAN address it unlocks is never the *authorisation* for anything — the grant token is
    ///   — and why the worst case is a connection that is refused rather than a disclosure.
    /// - **False negatives are harmless.** Two devices that really are on one LAN but egress
    ///   differently simply fall through to the relay, which is the tier below.
    ///
    /// IPv6 is compared on the `/64` prefix rather than the whole address. There is no NAT to hide
    /// behind, so every device on a LAN has a distinct global address but shares the prefix its
    /// router advertises — which makes it a stronger signal here than the IPv4 case, not a weaker
    /// one.
    pub fn same_network(&self, a_user: &str, a_device: &str, b_user: &str, b_device: &str) -> bool {
        let Ok(map) = self.live_peers.read() else {
            return false;
        };
        let egress_of = |user: &str, device: &str| -> Option<String> {
            map.get(&(user.to_string(), device.to_string()))
                .and_then(|peer| peer.egress.clone())
        };
        let (Some(a), Some(b)) = (egress_of(a_user, a_device), egress_of(b_user, b_device)) else {
            return false;
        };
        same_egress(&a, &b)
    }

    /// Whether *any* device belonging to `viewer_user` shares a network with `host_device`.
    ///
    /// The pairwise question the callers actually have. A listener does not tell the server which
    /// of their devices is asking — and should not have to, since the answer is the same either
    /// way: if any of their connected devices could reach the host directly, the direct tier is
    /// worth offering.
    pub fn shares_network_with_user(
        &self,
        host_user: &str,
        host_device: &str,
        viewer_user: &str,
    ) -> bool {
        let Ok(map) = self.live_peers.read() else {
            return false;
        };
        let Some(host_egress) = map
            .get(&(host_user.to_string(), host_device.to_string()))
            .and_then(|peer| peer.egress.clone())
        else {
            return false;
        };
        map.iter().any(|((user, device), peer)| {
            user == viewer_user
                && !(user == host_user && device == host_device)
                && peer
                    .egress
                    .as_deref()
                    .is_some_and(|egress| same_egress(&host_egress, egress))
        })
    }

    /// A bearer token letting `listener_user` fetch audio from `host_device`'s local server.
    ///
    /// Reused while it has real life left in it, so a listening session does not mint a token per
    /// track change, and pushed to the host the moment it is created — the host cannot accept a
    /// token it has never been told about.
    ///
    /// ## What `listener_keys` is for
    ///
    /// The host seals the audio's room key to an X25519 public key so that a shared network cannot
    /// read the stream. Until this argument existed, that key arrived in a request *header*, and
    /// nothing tied it to the grant: an attacker able to rewrite the request in flight could
    /// substitute their own key and be sealed to. The bearer token proved the requester was
    /// authorised; it did not prove they were the party whose key was in the header.
    ///
    /// So the grant carries the listener's published device keys, and the host seals only to a key
    /// it finds in that set. A substituted key is not in it and is refused.
    ///
    /// It is the whole *set* rather than one key because a listener may have several devices and
    /// the grant is minted for the account, not for whichever device happens to answer. Passing an
    /// empty set is meaningful and permitted: it says this account has published no keys, and the
    /// host falls back to the header exactly as before rather than refusing to serve at all.
    pub fn grant_p2p_token(
        &self,
        host_user: &str,
        host_device: &str,
        listener_user: &str,
        listener_keys: &[String],
    ) -> Option<String> {
        let key = (
            host_user.to_string(),
            host_device.to_string(),
            listener_user.to_string(),
        );
        let now = std::time::Instant::now();

        let reusable = self.p2p_grants.read().ok().and_then(|grants| {
            grants.get(&key).and_then(|(token, bound, expires)| {
                // A grant whose bound set no longer matches the listener's devices is not reusable:
                // reusing it would leave a device they have just added unable to be sealed to, and
                // one they have just removed still able to be.
                let live = expires.saturating_duration_since(now) > P2P_GRANT_MIN_REMAINING;
                (live && bound.as_slice() == listener_keys).then(|| token.clone())
            })
        });

        // A reused grant is pushed again rather than returned quietly, and that is not redundant.
        // The host keeps its grants in memory only, so a restart — which for a backgrounded
        // Android process is ordinary, not exceptional — loses every one of them. This side would
        // then go on handing the listener the same cached token for the rest of its ten minutes
        // while the host refused it, and replaying the track could not help: nothing on this path
        // mints a new one. The host treats a repeat as an upsert, so re-announcing costs a frame
        // and makes the pair self-healing.
        if let Some(token) = reusable {
            self.announce_p2p_grant(host_user, host_device, listener_user, &token, listener_keys);
            return Some(token);
        }

        // 256 bits from the OS CSPRNG, minted by the same helper as a device token.
        let token = crate::credentials::mint_token().secret;
        {
            let mut grants = self.p2p_grants.write().ok()?;
            grants.retain(|_, (_, _, expires)| *expires > now);
            grants.insert(
                key,
                (token.clone(), listener_keys.to_vec(), now + P2P_GRANT_TTL),
            );
        }

        // The host is told before the listener is, because a grant the host has not seen is one it
        // is obliged to refuse.
        self.announce_p2p_grant(host_user, host_device, listener_user, &token, listener_keys);
        Some(token)
    }

    /// Tells the host about a grant it is expected to honour.
    ///
    /// Sent on every hand-out, not only on the first, so that a host which has lost its in-memory
    /// grants is told again rather than left refusing a token this side still considers live.
    pub(super) fn announce_p2p_grant(
        &self,
        host_user: &str,
        host_device: &str,
        listener_user: &str,
        token: &str,
        listener_keys: &[String],
    ) {
        self.notify_device(
            host_user,
            host_device,
            "P2P_GRANT",
            serde_json::json!({
                "token": token,
                "forUser": listener_user,
                "forKeys": listener_keys,
                "ttlSeconds": P2P_GRANT_TTL.as_secs(),
            }),
        );
    }

    /// Purges a node's local network facts and its grants when it disconnects.
    pub fn clear_lan_address(&self, user_id: &str, device_id: &str) {
        if let Ok(mut map) = self.live_peers.write() {
            map.remove(&(user_id.to_string(), device_id.to_string()));
        }
        if let Ok(mut grants) = self.p2p_grants.write() {
            grants.retain(|(host_user, host_device, _), _| {
                !(host_user == user_id && host_device == device_id)
            });
        }
    }
}
