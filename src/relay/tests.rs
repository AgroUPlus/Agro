use super::*;
use crate::auth::AuthedUser;
use crate::lock::LockExt;
use crate::AppState;
use axum::body::Bytes;
use axum::{extract::State, http::StatusCode, Json};

#[test]
pub(super) fn creates_and_retrieves_relay_session() {
    let hub = RelayHub::new();
    let (id, session) = hub.create_session("alpha", "phone", "alpha", "pc", "hash123");
    assert_eq!(session.receiver_user, "alpha");
    assert_eq!(session.sender_user, "alpha");
    assert_eq!(session.sender_device, "pc");
    assert_eq!(session.receiver_device, "phone");
    assert_eq!(session.content_hash, "hash123");

    let retrieved = hub.get_session(&id);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().session_id, id);

    hub.remove_session(&id);
    assert!(hub.get_session(&id).is_none());
}

/// One upload, every listener. This is the whole point of the fanout.
#[tokio::test]
async fn a_jam_upload_reaches_every_attached_listener() {
    let hub = RelayHub::new();
    let (_, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");

    let RelayPipe::Fanout { tx, .. } = &session.pipe else {
        panic!("create_jam_session makes a fanout pipe");
    };
    let mut first = tx.subscribe();
    let mut second = tx.subscribe();

    tx.send(Bytes::from_static(b"chunk"))
        .expect("listeners attached");

    assert_eq!(first.recv().await.unwrap(), Bytes::from_static(b"chunk"));
    assert_eq!(second.recv().await.unwrap(), Bytes::from_static(b"chunk"));
}

/// A listener attaching after the upload started joins in progress; a jam plays in lockstep,
/// so there is nothing sensible to replay to it.
#[tokio::test]
async fn a_late_listener_does_not_receive_earlier_chunks() {
    let hub = RelayHub::new();
    let (_, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");
    let RelayPipe::Fanout { tx, .. } = &session.pipe else {
        panic!("fanout");
    };

    // Nobody attached yet: the send fails and the chunk is gone, which is intended.
    let _ = tx.send(Bytes::from_static(b"early"));
    let mut late = tx.subscribe();
    tx.send(Bytes::from_static(b"later"))
        .expect("listener attached");

    assert_eq!(late.recv().await.unwrap(), Bytes::from_static(b"later"));
}

/// The host uploads once. A second upload to the same session is refused rather than
/// interleaved into the listeners' streams.
#[test]
pub(super) fn a_fanout_accepts_only_one_sender() {
    use std::sync::atomic::Ordering;
    let hub = RelayHub::new();
    let (_, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");
    let RelayPipe::Fanout {
        sender_attached, ..
    } = &session.pipe
    else {
        panic!("fanout");
    };

    assert!(
        !sender_attached.swap(true, Ordering::SeqCst),
        "first upload attaches"
    );
    assert!(
        sender_attached.swap(true, Ordering::SeqCst),
        "second is refused"
    );
}

/// A fanout carries the jam it belongs to, because that is what every attach is checked
/// against — not a receiver fixed when the session was opened.
#[test]
pub(super) fn a_fanout_is_addressed_to_a_jam_not_a_device() {
    let hub = RelayHub::new();
    let (id, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");

    assert_eq!(session.jam_id.as_deref(), Some("jam-1"));
    assert!(session.receiver_device.is_empty());
    assert!(hub.get_session(&id).is_some());
}

#[tokio::test]
async fn pipes_bytes_between_sender_and_receiver() {
    let hub = RelayHub::new();
    let (id, session) = hub.create_session("alpha", "phone", "alpha", "pc", "hash123");
    let RelayPipe::Direct { tx, rx } = &session.pipe else {
        panic!("create_session makes a direct pipe");
    };
    let tx = tx.lock_recover().take().unwrap();
    let mut rx = rx.lock_recover().take().unwrap();

    tokio::spawn(async move {
        tx.send(Ok(Bytes::from_static(b"audio stream chunk 1")))
            .await
            .unwrap();
        tx.send(Ok(Bytes::from_static(b"audio stream chunk 2")))
            .await
            .unwrap();
    });

    let chunk1 = rx.recv().await.unwrap().unwrap();
    let chunk2 = rx.recv().await.unwrap().unwrap();
    assert_eq!(&chunk1[..], b"audio stream chunk 1");
    assert_eq!(&chunk2[..], b"audio stream chunk 2");
    hub.remove_session(&id);
}

mod device_ownership;
