use serde::{Deserialize, Serialize};

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
type P2pGrants =
    std::collections::HashMap<(String, String, String), (String, Vec<String>, std::time::Instant)>;

const P2P_GRANT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Reissue rather than hand out a token about to expire mid-transfer.
const P2P_GRANT_MIN_REMAINING: std::time::Duration = std::time::Duration::from_secs(60);

pub struct WsHub {
    /// Per-account routing and the replay buffers. See [`crate::ws_channels`].
    pub(crate) channels: crate::ws_channels::Channels,
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
}

mod handshake;
mod hub;
mod network;
mod session;
use handshake::is_plausible_host_port;
pub use handshake::ws_handler;
use handshake::MAX_LAN_ADDRESS;
use session::handle_socket;
use session::is_for;
use session::MAX_INBOUND_FRAME;
use session::SOCKET_READ_BUFFER;
#[cfg(test)]
mod tests;
