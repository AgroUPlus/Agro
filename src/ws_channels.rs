//! Where published WebSocket messages go: one channel and one replay buffer per account, plus one
//! of each for the few messages addressed to everyone.
//!
//! This used to be a single channel that every socket subscribed to. Each socket then threw away
//! everything not addressed to it, so one friend's presence update woke every connected device on
//! the server, and the shared 512-message replay buffer held only a few seconds of traffic once a
//! few hundred people were online. Routing by account keeps the work proportional to the people a
//! message is actually for.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use crate::ws::WsMessage;

/// How far back a reconnecting socket can resume from.
///
/// Long enough to cover a Wi-Fi-to-cellular handover and the backoff before the client retries,
/// short enough that the buffer stays small and a client gone longer than this is told to
/// resynchronise rather than handed a stale prefix of the stream.
pub(crate) const REPLAY_TTL: Duration = Duration::from_secs(30);

/// A ceiling on the everyone-buffer regardless of age, so a burst cannot grow it without bound.
pub(crate) const REPLAY_CAPACITY: usize = 512;

/// The same ceiling for one account. Smaller, because there is one of these per account.
pub(crate) const USER_REPLAY_CAPACITY: usize = 128;

/// How far a socket may fall behind its account's channel before it is told to resynchronise.
const USER_CHANNEL_CAPACITY: usize = 64;

const GLOBAL_CHANNEL_CAPACITY: usize = 100;

/// Recently sent messages, newest last, and the newest position this buffer has had to forget.
#[derive(Default)]
struct Replay {
    messages: VecDeque<(Instant, WsMessage)>,
    /// A client positioned before this has missed something the buffer can no longer give back.
    forgotten_through: u64,
}

impl Replay {
    fn push(&mut self, now: Instant, msg: WsMessage, capacity: usize) {
        self.messages.push_back((now, msg));
        self.expire(now);
        while self.messages.len() > capacity {
            self.forget_front();
        }
    }

    fn expire(&mut self, now: Instant) {
        while self
            .messages
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > REPLAY_TTL)
        {
            self.forget_front();
        }
    }

    fn forget_front(&mut self) {
        if let Some((_, msg)) = self.messages.pop_front() {
            self.forgotten_through = self.forgotten_through.max(msg.seq.unwrap_or(0));
        }
    }

    /// What this buffer holds after `after_seq`, or `None` when it has forgotten some of it.
    fn after(
        &self,
        after_seq: u64,
        wanted: &impl Fn(&WsMessage) -> bool,
    ) -> Option<Vec<WsMessage>> {
        if self.forgotten_through > after_seq {
            return None;
        }
        Some(
            self.messages
                .iter()
                .map(|(_, m)| m)
                .filter(|m| m.seq.is_some_and(|s| s > after_seq) && wanted(m))
                .cloned()
                .collect(),
        )
    }
}

pub(crate) struct Channels {
    global: broadcast::Sender<WsMessage>,
    /// Keyed by lowercased username, because addressing is case-insensitive (see `ws::is_for`).
    /// An entry exists while at least one of the account's sockets is subscribed.
    users: RwLock<HashMap<String, broadcast::Sender<WsMessage>>>,
    global_replay: Mutex<Replay>,
    /// Created the first time an account is sent anything, and never removed: an account's entry
    /// is what remembers that it missed something while it was away. One whose messages have all
    /// expired is a few bytes, and there is at most one per account.
    user_replay: Mutex<HashMap<String, Replay>>,
    pub(crate) next_seq: AtomicU64,
    /// Every published message, for tests that watch the whole stream.
    #[cfg(test)]
    pub(crate) tap: broadcast::Sender<WsMessage>,
}

fn key(username: &str) -> String {
    username.to_ascii_lowercase()
}

impl Channels {
    pub(crate) fn new() -> Self {
        Channels {
            global: broadcast::channel(GLOBAL_CHANNEL_CAPACITY).0,
            users: RwLock::new(HashMap::new()),
            global_replay: Mutex::new(Replay::default()),
            user_replay: Mutex::new(HashMap::new()),
            next_seq: AtomicU64::new(1),
            #[cfg(test)]
            tap: broadcast::channel(1024).0,
        }
    }

    /// Stamps a message with its position, remembers it, and sends it to whoever it is for.
    pub(crate) fn publish(&self, mut msg: WsMessage) {
        msg.seq = Some(self.next_seq.fetch_add(1, Ordering::Relaxed));
        let now = Instant::now();

        #[cfg(test)]
        let _ = self.tap.send(msg.clone());

        let Some(user) = msg.user_id.as_deref().map(key) else {
            self.global_replay
                .lock()
                .unwrap()
                .push(now, msg.clone(), REPLAY_CAPACITY);
            let _ = self.global.send(msg);
            return;
        };

        self.user_replay
            .lock()
            .unwrap()
            .entry(user.clone())
            .or_default()
            .push(now, msg.clone(), USER_REPLAY_CAPACITY);
        if let Some(channel) = self.users.read().unwrap().get(&user) {
            let _ = channel.send(msg);
        }
    }

    pub(crate) fn subscribe_global(&self) -> broadcast::Receiver<WsMessage> {
        self.global.subscribe()
    }

    /// The account's own channel, created on first use.
    pub(crate) fn subscribe_user(&self, username: &str) -> broadcast::Receiver<WsMessage> {
        self.users
            .write()
            .unwrap()
            .entry(key(username))
            .or_insert_with(|| broadcast::channel(USER_CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// Messages after `after_seq` that pass `wanted`, oldest first, or `None` when the socket must
    /// resynchronise because a gap cannot be filled.
    pub(crate) fn replay_after(
        &self,
        after_seq: u64,
        username: Option<&str>,
        wanted: impl Fn(&WsMessage) -> bool,
    ) -> Option<Vec<WsMessage>> {
        // A position this boot never handed out comes from before a restart, and every message
        // since then is one the client has not seen.
        if after_seq >= self.next_seq.load(Ordering::Relaxed) {
            return None;
        }
        let now = Instant::now();
        let mut found = {
            let mut global = self.global_replay.lock().unwrap();
            global.expire(now);
            global.after(after_seq, &wanted)?
        };
        if let Some(user) = username.map(key) {
            // No entry means nothing was ever sent to this account, so there is nothing to miss.
            if let Some(replay) = self.user_replay.lock().unwrap().get_mut(&user) {
                replay.expire(now);
                found.extend(replay.after(after_seq, &wanted)?);
            }
        }
        found.sort_by_key(|m| m.seq);
        Some(found)
    }

    /// Drops expired messages and the channels no socket is listening to any more.
    pub(crate) fn sweep(&self) {
        let now = Instant::now();
        self.global_replay.lock().unwrap().expire(now);
        for replay in self.user_replay.lock().unwrap().values_mut() {
            replay.expire(now);
        }
        self.users
            .write()
            .unwrap()
            .retain(|_, channel| channel.receiver_count() > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to(user: Option<&str>) -> WsMessage {
        WsMessage {
            msg_type: "T".into(),
            payload: serde_json::json!({}),
            user_id: user.map(str::to_string),
            target_device: None,
            seq: None,
        }
    }

    /// The point of the split: one account's traffic never reaches another account's socket.
    #[test]
    fn an_account_s_channel_carries_only_its_own_messages() {
        let channels = Channels::new();
        let mut alice = channels.subscribe_user("alice");
        let mut bob = channels.subscribe_user("bob");
        channels.publish(to(Some("Alice")));

        assert_eq!(alice.try_recv().unwrap().user_id.as_deref(), Some("Alice"));
        assert!(bob.try_recv().is_err());
    }

    #[test]
    fn a_message_for_everyone_goes_to_the_global_channel() {
        let channels = Channels::new();
        let mut global = channels.subscribe_global();
        let mut alice = channels.subscribe_user("alice");
        channels.publish(to(None));

        assert!(global.try_recv().is_ok());
        assert!(alice.try_recv().is_err());
    }

    /// Falling behind surfaces as `Lagged`, which the socket turns into a resync, not a hang-up.
    #[test]
    fn a_slow_socket_is_told_it_lagged() {
        let channels = Channels::new();
        let mut alice = channels.subscribe_user("alice");
        for _ in 0..(USER_CHANNEL_CAPACITY + 1) {
            channels.publish(to(Some("alice")));
        }
        assert!(matches!(
            alice.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
    }

    /// A position from before a restart would otherwise be answered with "nothing missed".
    #[test]
    fn a_position_this_boot_never_issued_must_resync() {
        let channels = Channels::new();
        channels.publish(to(Some("alice")));
        assert!(channels
            .replay_after(500, Some("alice"), |_| true)
            .is_none());
    }

    /// Another account's burst does not push this account's messages out of replay.
    #[test]
    fn replay_is_kept_per_account() {
        let channels = Channels::new();
        channels.publish(to(Some("alice")));
        for _ in 0..(REPLAY_CAPACITY + 10) {
            channels.publish(to(Some("bob")));
        }
        let alice = channels.replay_after(0, Some("alice"), |_| true).unwrap();
        assert_eq!(alice.len(), 1);
        assert!(channels.replay_after(0, Some("bob"), |_| true).is_none());
    }

    #[test]
    fn a_sweep_drops_channels_nobody_listens_to() {
        let channels = Channels::new();
        drop(channels.subscribe_user("alice"));
        let _bob = channels.subscribe_user("bob");
        channels.sweep();
        let users = channels.users.read().unwrap();
        assert!(!users.contains_key("alice"));
        assert!(users.contains_key("bob"));
    }
}
