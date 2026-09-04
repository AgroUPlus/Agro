use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::auth::AuthedUser;
use crate::AppState;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WsMessage {
    pub msg_type: String,
    pub payload: serde_json::Value,
    /// The account this concerns. A socket only forwards messages for its own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// One device, when the message is only that device's business — a sync offer, say. `None`
    /// means every device on the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_device: Option<String>,
    /// Position in the hub's total order, assigned on the way out.
    ///
    /// A client remembers the highest it has seen and asks to resume from it, which is the whole
    /// of how a message survives a reconnect. Absent on frames the server writes directly to one
    /// socket (`AUTH_SUCCESS`, `RESUMED`): those are not part of the ordered stream and replaying
    /// them would be meaningless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// Whether two observed public addresses put their devices behind the same edge.
///
/// IPv4 must match exactly. IPv6 matches on the `/64` prefix, which is the unit a router hands to
/// a LAN; comparing the full address there would answer "no" for every pair of devices on the same
/// network, since each picks its own host portion.
fn same_egress(a: &str, b: &str) -> bool {
    use std::net::IpAddr;
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(IpAddr::V4(a)), Ok(IpAddr::V4(b))) => a == b,
        (Ok(IpAddr::V6(a)), Ok(IpAddr::V6(b))) => a.octets()[..8] == b.octets()[..8],
        // One of the two is not an address the server could parse. Refusing is the safe answer:
        // it costs a fallback to the relay, and guessing costs a private address.
        _ => false,
    }
}

/// What is known about where a connected device sits on the network.
///
/// Both halves are volatile and both are needed together: the LAN address is where a peer would
/// connect, and the egress address is the only evidence the server has about whether connecting
/// could possibly work.
#[derive(Default, Clone)]
struct PeerNetwork {
    /// `host:port` on the device's own local network, as the device reported it.
    lan: Option<String>,
    /// The public address this device's connection arrived from, as seen by the server.
    ///
    /// Never handed to another client. It exists only to be compared with another device's, and a
    /// public address is a far more identifying thing than the RFC1918 address it gates.
    egress: Option<String>,
}

/// How long a peer-to-peer grant stays usable.
///
/// Long enough to cover a listening session's worth of track changes without reminting on every
/// frame, short enough that a token which escapes stops working the same afternoon.
/// Live P2P grants, keyed by `(host user, host device, listener user)`.
///
/// The value is the bearer token, the listener device keys it was minted against, and when it
/// expires. The key set is part of the value rather than of the key because a grant is looked up
/// by *who* is listening and only then checked against *what* they can be sealed to.
type P2pGrants = std::collections::HashMap<
    (String, String, String),
    (String, Vec<String>, std::time::Instant),
>;

const P2P_GRANT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Reissue rather than hand out a token about to expire mid-transfer.
const P2P_GRANT_MIN_REMAINING: std::time::Duration = std::time::Duration::from_secs(60);

pub struct WsHub {
    pub tx: broadcast::Sender<WsMessage>,
    /// Live network facts about actively connected devices, held strictly in memory.
    ///
    /// Keyed by `(user_id, device_id)`. Volatile on purpose: when a device disconnects or the
    /// server restarts, the network address is wiped immediately from RAM and leaves zero residue
    /// on disk.
    live_peers: std::sync::RwLock<std::collections::HashMap<(String, String), PeerNetwork>>,
    /// Outstanding peer-to-peer grants, keyed by `(host_user, host_device, listener_user)`.
    ///
    /// A grant is a bearer token the listener presents to the host's local HTTP server. It is
    /// minted here so that neither client has to trust the other, and it is what stops a shared
    /// LAN — a hotel, a campus, a coffee shop — from being a trusted one.
    ///
    /// The keys the grant was minted against are held beside it, so a listener that has since
    /// added or removed a device is issued a fresh grant rather than handed one whose bound set no
    /// longer describes them.
    p2p_grants: std::sync::RwLock<P2pGrants>,
    /// The next position in the total order.
    next_seq: std::sync::atomic::AtomicU64,
    /// Recently sent messages, newest last, for replaying to a socket that reconnects.
    ///
    /// A `broadcast` channel drops what a disconnected receiver never took, so a client that
    /// changed network mid-session came back having silently missed frames — the E2EE negotiation
    /// among them, which is why a handover could leave a session unable to decrypt. This is the
    /// short memory that makes reconnection lossless.
    ///
    /// In memory and bounded twice over, by age and by count: it holds live control traffic for
    /// long enough to reconnect, and is not a message store.
    replay: std::sync::RwLock<std::collections::VecDeque<(std::time::Instant, WsMessage)>>,
}

/// How far back a reconnecting socket can resume from.
///
/// Long enough to cover a Wi-Fi-to-cellular handover and the backoff before the client retries,
/// short enough that the buffer stays small and a client gone longer than this is told to
/// resynchronise rather than handed a stale prefix of the stream.
const REPLAY_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// A ceiling on the buffer regardless of age, so a burst cannot grow it without bound.
const REPLAY_CAPACITY: usize = 512;

impl WsHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            tx,
            live_peers: std::sync::RwLock::new(std::collections::HashMap::new()),
            p2p_grants: std::sync::RwLock::new(std::collections::HashMap::new()),
            next_seq: std::sync::atomic::AtomicU64::new(1),
            replay: std::sync::RwLock::new(std::collections::VecDeque::new()),
        }
    }

    /// Stamps a message with its position, remembers it, and sends it.
    ///
    /// Every ordered message leaves through here, so the sequence has no gaps and the buffer can
    /// never disagree with what was actually sent.
    fn publish(&self, mut msg: WsMessage) {
        let seq = self
            .next_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        msg.seq = Some(seq);

        if let Ok(mut buffer) = self.replay.write() {
            let now = std::time::Instant::now();
            buffer.push_back((now, msg.clone()));
            // Trimmed on the way in rather than on a timer: the buffer only grows when something
            // is sent, so that is the only moment it can be too large.
            while buffer
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > REPLAY_TTL)
            {
                buffer.pop_front();
            }
            while buffer.len() > REPLAY_CAPACITY {
                buffer.pop_front();
            }
        }

        let _ = self.tx.send(msg);
    }

    /// Messages after [`after_seq`] that this socket should have seen, oldest first.
    ///
    /// `None` means the buffer cannot answer — the client has been gone longer than [`REPLAY_TTL`]
    /// or a burst pushed its position out — and the caller must tell it to resynchronise rather
    /// than hand it a prefix with a hole at the front, which would be worse than admitting the gap.
    ///
    /// `pub(crate)` for the boundary suites as well as the resume path: what a socket is *sent* is
    /// half of what one account can learn about another, and a frame carrying somebody else's
    /// sealed copy would not be visible in any query-level assertion.
    pub(crate) fn replay_after(
        &self,
        after_seq: u64,
        username: Option<&str>,
        device: Option<&str>,
    ) -> Option<Vec<WsMessage>> {
        let buffer = self.replay.read().ok()?;
        let oldest = buffer.front().map(|(_, m)| m.seq.unwrap_or(0))?;
        // The client's next expected message must still be in the buffer. Equality is fine: it
        // means nothing has been dropped since it left.
        if oldest > after_seq + 1 {
            return None;
        }
        Some(
            buffer
                .iter()
                .filter(|(_, m)| m.seq.is_some_and(|s| s > after_seq))
                .filter(|(_, m)| is_for(m, username, device))
                .map(|(_, m)| m.clone())
                .collect(),
        )
    }

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
            .and_then(|map| map.get(&(user_id.to_string(), device_id.to_string())).cloned())
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
    pub fn same_network(
        &self,
        a_user: &str,
        a_device: &str,
        b_user: &str,
        b_device: &str,
    ) -> bool {
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
    fn announce_p2p_grant(
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

    /// Sends to every device on every account. Used for things that are not account-specific.
    pub fn broadcast(&self, msg_type: &str, payload: serde_json::Value) {
        self.publish(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: None,
            target_device: None,
            seq: None,
        });
    }

    /// Sends to one account's devices.
    pub fn notify_user(&self, user_id: &str, msg_type: &str, payload: serde_json::Value) {
        self.publish(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: Some(user_id.to_string()),
            target_device: None,
            seq: None,
        });
    }

    /// Sends the same message to several accounts.
    ///
    /// One send per recipient rather than a broadcast with a recipient list, because the socket
    /// side filters on `user_id` alone — a list in the payload would arrive at everyone and rely on
    /// each client to ignore what is not theirs, which is not a boundary.
    pub fn notify_users(&self, user_ids: &[String], msg_type: &str, payload: serde_json::Value) {
        for user_id in user_ids {
            self.notify_user(user_id, msg_type, payload.clone());
        }
    }

    /// Sends to exactly one device.
    pub fn notify_device(
        &self,
        user_id: &str,
        device_id: &str,
        msg_type: &str,
        payload: serde_json::Value,
    ) {
        self.publish(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: Some(user_id.to_string()),
            target_device: Some(device_id.to_string()),
            seq: None,
        });
    }
}

/// Identifies the socket, so messages can be addressed rather than shouted.
///
/// The device id is a query parameter because a WebSocket handshake from a browser cannot carry
/// custom headers — the same reason the token is accepted that way.
#[derive(Deserialize)]
pub struct SocketQuery {
    device: Option<String>,
    /// Where this device can be reached on the local network, `host:port`.
    ///
    /// Carried on the handshake rather than only by `registerNode`, and that is the whole point.
    /// The address lives in memory and is dropped when the socket closes, so something has to put
    /// it back afterwards. `registerNode` cannot: both clients call it exactly once at startup, so
    /// an address cleared by a redeploy or a dropped socket never returned and peer-to-peer
    /// transfers silently fell back to the relay until the app was restarted by hand.
    ///
    /// Arriving with the connection makes the lifetime coherent — the address exists for exactly
    /// as long as the socket does — and self-healing, because both clients already reconnect with
    /// backoff.
    lan: Option<String>,
}

/// The longest `host:port` worth accepting. Enough for an IPv6 literal with a port, and short
/// enough that the field cannot be used as storage.
const MAX_LAN_ADDRESS: usize = 64;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: Option<axum::Extension<AuthedUser>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(query): Query<SocketQuery>,
) -> Response {
    let username = user.map(|u| u.username().to_string());
    let device = query.device;

    // Where this connection actually reached the server from. Recorded for every authenticated
    // socket, not only ones that claim a LAN address, because the comparison needs both ends and
    // only one of them is the device asking. `client_ip` is the same reading the rate limiter
    // trusts, so a reverse proxy does not collapse every device onto one address.
    if let (Some(u), Some(d)) = (&username, &device) {
        state
            .ws_hub
            .set_egress_address(u, d, &crate::login::client_ip(peer, &headers));
    }

    // Before the node upsert, so a device is reachable as soon as its socket is up.
    if let (Some(u), Some(d), Some(lan)) = (&username, &device, query.lan.as_deref()) {
        let lan = lan.trim();
        // Only ever this account's own device, and only a plausible address: this is attacker
        // input, and it is handed to other devices as somewhere to connect to.
        if !lan.is_empty() && lan.len() <= MAX_LAN_ADDRESS && is_plausible_host_port(lan) {
            state.ws_hub.set_lan_address(u, d, lan);
        }
    }

    if let (Some(u), Some(d)) = (&username, &device) {
        // A connect is not a naming event. The invented name is only the fallback for a device
        // that has never been seen, or the socket would rename it on every reconnect — which is
        // every time the server is redeployed.
        let petname = crate::passphrase::generate_random_petname();
        let client_type = if d.to_lowercase().contains("android") || d.to_lowercase().contains("wanda") {
            "wanda"
        } else {
            "wander"
        };
        let _ = state.db.upsert_node(
            d,
            u,
            crate::db::NodeName::KeepOr(&petname),
            client_type,
            None,
            None,
        );
        state.offers.note_archived(u);
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state, username, device))
}

/// Forwards hub messages this socket is entitled to see.
///
/// Supports both pre-authenticated connections (via HTTP Authorization header or query parameter)
/// and post-handshake in-band authentication (via an `AUTH` message frame within 5 seconds),
/// preventing bearer tokens from being logged in reverse proxy access logs.
async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    mut username: Option<String>,
    mut device: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.ws_hub.tx.subscribe();

    // If unauthenticated, wait for an in-band AUTH frame within 5 seconds
    if username.is_none() {
        let auth_future = async {
            while let Some(Ok(msg)) = receiver.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        if ws_msg.msg_type.eq_ignore_ascii_case("AUTH") {
                            if let Some(token) = ws_msg.payload.get("token").and_then(|t| t.as_str()) {
                                if let Ok(Some((account, _))) = state.db.account_for_token(token) {
                                    if account.state.is_active() {
                                        let u = account.username.to_string();
                                        let d = ws_msg
                                            .payload
                                            .get("device")
                                            .and_then(|dev| dev.as_str())
                                            .map(str::to_string);
                                        let lan = ws_msg
                                            .payload
                                            .get("lan")
                                            .and_then(|l| l.as_str());

                                        if let (Some(dev_id), Some(lan_addr)) = (&d, lan) {
                                            let lan_addr = lan_addr.trim();
                                            if !lan_addr.is_empty()
                                                && lan_addr.len() <= MAX_LAN_ADDRESS
                                                && is_plausible_host_port(lan_addr)
                                            {
                                                state.ws_hub.set_lan_address(&u, dev_id, lan_addr);
                                            }
                                        }

                                        if let Some(dev_id) = &d {
                                            let petname = crate::passphrase::generate_random_petname();
                                            let client_type = if dev_id.to_lowercase().contains("android")
                                                || dev_id.to_lowercase().contains("wanda")
                                            {
                                                "wanda"
                                            } else {
                                                "wander"
                                            };
                                            let _ = state.db.upsert_node(
                                                dev_id,
                                                &u,
                                                crate::db::NodeName::KeepOr(&petname),
                                                client_type,
                                                None,
                                                None,
                                            );
                                            state.offers.note_archived(&u);
                                        }
                                        return Some((u, d));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None
        };

        match tokio::time::timeout(std::time::Duration::from_secs(5), auth_future).await {
            Ok(Some((u, d))) => {
                username = Some(u);
                device = d;
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({
                            "msg_type": "AUTH_SUCCESS",
                            "payload": { "status": "authenticated" }
                        })
                        .to_string()
                        .into(),
                    ))
                    .await;
            }
            _ => {
                return; // Failed to authenticate in-band within timeout
            }
        }
    }

    // Both loops below need to write to the socket — the live stream, and a RESUME being answered
    // — so the sink is owned by one task and fed through a channel rather than shared behind a
    // lock. A bounded channel also means a socket that stops reading applies backpressure here
    // instead of growing a queue.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let hub = state.ws_hub.clone();
    let (user_for_recv, device_for_recv) = (username.clone(), device.clone());
    let resume_tx = out_tx.clone();

    tokio::select! {
        _ = async {
            while let Ok(msg) = rx.recv().await {
                if !is_for(&msg, username.as_deref(), device.as_deref()) {
                    continue;
                }
                if let Ok(text) = serde_json::to_string(&msg) {
                    if out_tx.send(text).await.is_err() {
                        break;
                    }
                }
            }
        } => {},
        _ = async {
            while let Some(Ok(msg)) = receiver.next().await {
                let Message::Text(text) = msg else { continue };
                let Ok(frame) = serde_json::from_str::<WsMessage>(&text) else { continue };
                if !frame.msg_type.eq_ignore_ascii_case("RESUME") {
                    // Every other inbound frame is still read and dropped. See above.
                    continue;
                }

                // A client that has never seen a message sends 0 and is caught up by definition.
                let after = frame.payload.get("last_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                let missed = hub.replay_after(
                    after,
                    user_for_recv.as_deref(),
                    device_for_recv.as_deref(),
                );

                // The answer goes first, so the client knows whether what follows is the rest of
                // its stream or the start of a new one before it reads any of it.
                let answer = serde_json::json!({
                    "msg_type": "RESUMED",
                    "payload": {
                        "from": after,
                        "replayed": missed.as_ref().map(Vec::len).unwrap_or(0),
                        // The client must refetch state rather than trust its own: the gap is
                        // longer than the server can account for.
                        "resync_required": missed.is_none(),
                    }
                });
                if resume_tx.send(answer.to_string()).await.is_err() {
                    break;
                }

                for msg in missed.unwrap_or_default() {
                    let Ok(text) = serde_json::to_string(&msg) else { continue };
                    if resume_tx.send(text).await.is_err() {
                        return;
                    }
                }
            }
        } => {}
    }

    writer.abort();

    if let (Some(u), Some(d)) = (username.as_deref(), device.as_deref()) {
        state.ws_hub.clear_lan_address(u, d);
    }
}

/// A cheap sanity check on a `host:port` a client says it can be reached at.
///
/// Not a validation of reachability — that is the peer's job, and `PeerReachability` on the client
/// already probes before trusting one. This only rejects the obviously malformed, so that a value
/// handed to another device as a connection target cannot carry a scheme, a path, or a credential.
fn is_plausible_host_port(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return false;
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if port.parse::<u16>().map(|p| p == 0).unwrap_or(true) {
        return false;
    }
    // An IPv6 literal is bracketed; anything else is a bare host or IPv4.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    !host.is_empty()
        && host.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':' || b == b'_'
        })
}

/// Whether a socket for [`username`]/[`device`] should see [`msg`].
fn is_for(msg: &WsMessage, username: Option<&str>, device: Option<&str>) -> bool {
    if let Some(target_user) = &msg.user_id {
        // An unauthenticated socket only exists during the first-run window, before any account
        // has data worth addressing.
        match username {
            Some(name) if name.eq_ignore_ascii_case(target_user) => {}
            _ => return false,
        }
    }
    match (&msg.target_device, device) {
        (None, _) => true,
        (Some(target), Some(own)) => target == own,
        // Addressed to a specific device by a socket that never said which one it is. Dropping it
        // is the safe answer: a client that wants targeted messages identifies itself.
        (Some(_), None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(user: Option<&str>, device: Option<&str>) -> WsMessage {
        WsMessage {
            msg_type: "SYNC_OFFER".to_string(),
            payload: json!({}),
            user_id: user.map(str::to_string),
            target_device: device.map(str::to_string),
            seq: None,
        }
    }

    #[test]
    fn a_broadcast_reaches_everyone() {
        assert!(is_for(&msg(None, None), Some("alpha"), Some("phone")));
        assert!(is_for(&msg(None, None), None, None));
    }

    #[test]
    fn another_account_never_sees_it() {
        assert!(!is_for(&msg(Some("alpha"), None), Some("beta"), Some("phone")));
        assert!(is_for(&msg(Some("alpha"), None), Some("alpha"), Some("phone")));
    }

    #[test]
    fn a_targeted_message_reaches_only_that_device() {
        let m = msg(Some("alpha"), Some("phone"));
        assert!(is_for(&m, Some("alpha"), Some("phone")));
        assert!(!is_for(&m, Some("alpha"), Some("laptop")));
        assert!(!is_for(&m, Some("alpha"), None));
    }

    /// The same-network test is what decides whether a private address is ever handed to another
    /// account, so both directions of it are pinned here.
    mod same_network {
        use super::*;

        fn hub_with(peers: &[(&str, &str, &str)]) -> WsHub {
            let hub = WsHub::new();
            for (user, device, egress) in peers {
                hub.set_egress_address(user, device, egress);
                hub.set_lan_address(user, device, "192.168.1.50:8702");
            }
            hub
        }

        #[test]
        fn one_public_address_means_one_network() {
            let hub = hub_with(&[
                ("alpha", "phone", "203.0.113.7"),
                ("beta", "laptop", "203.0.113.7"),
            ]);
            assert!(hub.same_network("alpha", "phone", "beta", "laptop"));
            assert!(hub.shares_network_with_user("alpha", "phone", "beta"));
        }

        #[test]
        fn different_public_addresses_mean_different_networks() {
            let hub = hub_with(&[
                ("alpha", "phone", "203.0.113.7"),
                ("beta", "laptop", "198.51.100.4"),
            ]);
            assert!(!hub.same_network("alpha", "phone", "beta", "laptop"));
            assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
        }

        /// A device nobody has heard from has no egress address, and an unknown answer is not a
        /// yes: it falls through to the relay rather than disclosing an address.
        #[test]
        fn an_unseen_device_is_never_on_your_network() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            assert!(!hub.same_network("alpha", "phone", "beta", "laptop"));
            assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
        }

        /// IPv6 has no NAT, so two devices on one LAN never share a full address — they share the
        /// prefix their router advertises. Comparing the whole address would answer "no" for every
        /// real pair.
        #[test]
        fn ipv6_is_compared_on_the_routed_prefix() {
            assert!(same_egress(
                "2001:db8:1:2::1000",
                "2001:db8:1:2::abcd"
            ));
            assert!(!same_egress("2001:db8:1:2::1", "2001:db8:9:9::1"));
        }

        /// Mixing families, or anything unparseable, is refused rather than guessed at.
        #[test]
        fn an_unparseable_address_is_refused() {
            assert!(!same_egress("203.0.113.7", "::ffff:203.0.113.7"));
            assert!(!same_egress("not-an-address", "not-an-address"));
            assert!(!same_egress("", ""));
        }

        /// The token is the gate, so it has to be stable enough to use across a track change and
        /// scoped to the one listener it was minted for.
        #[test]
        fn a_grant_is_reused_for_the_same_pair_and_distinct_per_listener() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            let first = hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap();
            let again = hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap();
            let other = hub.grant_p2p_token("alpha", "phone", "gamma", &[]).unwrap();
            assert_eq!(first, again);
            assert_ne!(first, other);
        }

        /// The bound key set is part of what a grant *is*, so it cannot be reused across a
        /// change to it. A listener that has just added a phone would otherwise be handed a grant
        /// that cannot seal to it, and one that has just signed a phone out would be handed a
        /// grant that still can.
        #[test]
        fn a_grant_is_reminted_when_the_listeners_keys_change() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            let one = vec!["key-one".to_string()];
            let two = vec!["key-one".to_string(), "key-two".to_string()];

            let first = hub.grant_p2p_token("alpha", "phone", "beta", &one).unwrap();
            let same = hub.grant_p2p_token("alpha", "phone", "beta", &one).unwrap();
            assert_eq!(first, same, "an unchanged key set reuses the grant");

            let after = hub.grant_p2p_token("alpha", "phone", "beta", &two).unwrap();
            assert_ne!(first, after, "an added device must force a fresh grant");

            let back = hub.grant_p2p_token("alpha", "phone", "beta", &one).unwrap();
            assert_ne!(after, back, "a removed device must force a fresh grant too");
        }

        /// A device sharing its own account's network is still a match — the same-account case is
        /// the original point of the LAN transfer — but callers that mean "somebody else" filter
        /// the holder out themselves.
        #[test]
        fn your_own_second_device_is_on_your_network() {
            let hub = hub_with(&[
                ("alpha", "phone", "203.0.113.7"),
                ("alpha", "laptop", "203.0.113.7"),
            ]);
            assert!(hub.shares_network_with_user("alpha", "phone", "alpha"));
        }

        /// The device being asked about is never its own peer, or every host would look reachable
        /// from itself.
        #[test]
        fn a_device_is_not_its_own_peer() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            assert!(!hub.shares_network_with_user("alpha", "phone", "alpha"));
        }

        /// A disconnect takes the address and every grant issued for that device with it.
        #[test]
        fn disconnecting_clears_the_address_and_the_grants() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            let token = hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap();
            hub.clear_lan_address("alpha", "phone");
            assert!(hub.get_lan_address("alpha", "phone").is_none());
            assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
            assert_ne!(
                token,
                hub.grant_p2p_token("alpha", "phone", "beta", &[]).unwrap(),
                "a grant must not survive the socket it was minted for"
            );
        }
    }

    /// The regression that made the volatile design worth fixing rather than reverting: an
    /// address set once at startup and cleared on a dropped socket never came back, because
    /// neither client re-registers. The handshake carrying it is what closes that loop, so these
    /// pin the shape of what the handshake is allowed to hand over.
    #[test]
    fn a_plausible_host_port_is_accepted() {
        for good in [
            "192.168.1.50:8702",
            "10.0.0.5:8702",
            "phone.local:8702",
            "[fe80::1]:8702",
        ] {
            assert!(is_plausible_host_port(good), "should accept {good}");
        }
    }

    /// The value is handed to another device as somewhere to connect to, so anything carrying a
    /// scheme, a path or a credential has to be refused rather than forwarded.
    #[test]
    fn a_malformed_or_dangerous_address_is_refused() {
        for bad in [
            "",
            "192.168.1.50",              // no port
            "192.168.1.50:",             // empty port
            "192.168.1.50:0",            // port zero
            "192.168.1.50:notaport",
            "192.168.1.50:99999",        // will not fit a u16
            ":8702",                     // no host
            "http://192.168.1.50:8702",  // a scheme
            "192.168.1.50:8702/steal",   // a path
            "user:pass@10.0.0.1:8702",   // a credential
            // A zone id names an interface on the *sender's* machine, so it means nothing to the
            // peer being told to connect there.
            "[fe80::1%eth0]:8702",
        ] {
            assert!(!is_plausible_host_port(bad), "should refuse {bad:?}");
        }
    }

    #[test]
    fn lan_addresses_are_stored_and_retrieved_in_memory() {
        let hub = WsHub::new();
        assert_eq!(hub.get_lan_address("alpha", "phone"), None);

        hub.set_lan_address("alpha", "phone", "192.168.1.50:8702");
        assert_eq!(
            hub.get_lan_address("alpha", "phone"),
            Some("192.168.1.50:8702".to_string())
        );

        hub.clear_lan_address("alpha", "phone");
        assert_eq!(hub.get_lan_address("alpha", "phone"), None);
    }

    #[test]
    fn lan_addresses_are_scoped_by_user_and_device() {
        let hub = WsHub::new();
        hub.set_lan_address("alpha", "phone", "192.168.1.50:8702");
        hub.set_lan_address("beta", "phone", "10.0.0.5:8702");

        assert_eq!(
            hub.get_lan_address("alpha", "phone"),
            Some("192.168.1.50:8702".to_string())
        );
        assert_eq!(
            hub.get_lan_address("beta", "phone"),
            Some("10.0.0.5:8702".to_string())
        );
        assert_eq!(hub.get_lan_address("alpha", "laptop"), None);
    }

    /// A reconnecting socket is handed exactly what it missed, in order.
    #[test]
    fn replay_returns_messages_after_the_client_s_position() {
        let hub = WsHub::new();
        hub.notify_user("alice", "JAM_UPDATED", serde_json::json!({ "n": 1 }));
        hub.notify_user("alice", "JAM_UPDATED", serde_json::json!({ "n": 2 }));
        hub.notify_user("alice", "JAM_UPDATED", serde_json::json!({ "n": 3 }));

        let all = hub.replay_after(0, Some("alice"), None).expect("in buffer");
        assert_eq!(all.len(), 3);
        assert!(all.windows(2).all(|w| w[0].seq < w[1].seq), "ordered");

        let tail = hub.replay_after(2, Some("alice"), None).expect("in buffer");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].payload["n"], 3);
    }

    /// Replay is not a way around addressing: it answers with the socket's own messages only.
    #[test]
    fn replay_does_not_cross_accounts_or_devices() {
        let hub = WsHub::new();
        hub.notify_user("alice", "FRIEND_PRESENCE", serde_json::json!({}));
        hub.notify_device("alice", "phone", "SYNC_OFFER", serde_json::json!({}));

        assert_eq!(hub.replay_after(0, Some("bob"), None).unwrap().len(), 0);
        // Alice's laptop sees the account-wide message but not the one addressed to her phone.
        let laptop = hub.replay_after(0, Some("alice"), Some("laptop")).unwrap();
        assert_eq!(laptop.len(), 1);
        assert_eq!(laptop[0].msg_type, "FRIEND_PRESENCE");
        assert_eq!(hub.replay_after(0, Some("alice"), Some("phone")).unwrap().len(), 2);
    }

    /// Gone too long is answered with "resynchronise", never with a prefix missing its front.
    #[test]
    fn replay_refuses_when_the_client_s_position_has_been_dropped() {
        let hub = WsHub::new();
        for n in 0..(REPLAY_CAPACITY + 50) {
            hub.notify_user("alice", "JAM_UPDATED", serde_json::json!({ "n": n }));
        }
        // Position 0 fell out of the buffer when it was trimmed, so the gap cannot be filled.
        assert!(hub.replay_after(0, Some("alice"), None).is_none());
        // A position still inside the buffer is answered normally.
        let recent = hub.next_seq.load(std::sync::atomic::Ordering::Relaxed) - 2;
        assert!(hub.replay_after(recent, Some("alice"), None).is_some());
    }

    /// The sequence is the buffer's index into the stream; a hole in it would misplace a replay.
    #[test]
    fn every_published_message_is_numbered_without_gaps() {
        let hub = WsHub::new();
        hub.broadcast("RELEASE", serde_json::json!({}));
        hub.notify_user("alice", "JAM_UPDATED", serde_json::json!({}));
        hub.notify_device("alice", "phone", "SYNC_OFFER", serde_json::json!({}));

        let seqs: Vec<u64> = hub
            .replay
            .read()
            .unwrap()
            .iter()
            .map(|(_, m)| m.seq.expect("published messages are numbered"))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }
}
