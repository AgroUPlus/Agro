//! The resume window.

use super::*;

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
    assert_eq!(
        hub.replay_after(0, Some("alice"), Some("phone"))
            .unwrap()
            .len(),
        2
    );
}

/// Gone too long is answered with "resynchronise", never with a prefix missing its front.
#[test]
fn replay_refuses_when_the_client_s_position_has_been_dropped() {
    let hub = WsHub::new();
    for n in 0..(crate::ws_channels::USER_REPLAY_CAPACITY + 50) {
        hub.notify_user("alice", "JAM_UPDATED", serde_json::json!({ "n": n }));
    }
    // Position 0 fell out of the buffer when it was trimmed, so the gap cannot be filled.
    assert!(hub.replay_after(0, Some("alice"), None).is_none());
    // A position still inside the buffer is answered normally.
    let recent = hub
        .channels
        .next_seq
        .load(std::sync::atomic::Ordering::Relaxed)
        - 2;
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
        .replay_after(0, Some("alice"), Some("phone"))
        .expect("in buffer")
        .iter()
        .map(|m| m.seq.expect("published messages are numbered"))
        .collect();
    assert_eq!(seqs, vec![1, 2, 3]);
}
