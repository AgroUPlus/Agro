//! Sealed drops and conversations keep every copy and never the plaintext.

use crate::db::*;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

/// #49: a note sealed only to the recipient is one the sender cannot read. Every copy is
/// stored, and the sender's own is among them.
#[test]
fn a_drop_keeps_every_sealed_copy_including_the_senders() {
    let db = db();
    let new_drop = crate::db_drops::NewDrop {
        track_title: "Secret Track".to_string(),
        artist_name: "Secret Artist".to_string(),
        note_ciphertext: Some("sealed-to-recipient".to_string()),
        note_ciphertexts: vec![
            crate::db_drops::DeviceCiphertext {
                device_id: "beta-phone".to_string(),
                ciphertext: "sealed-to-recipient".to_string(),
            },
            crate::db_drops::DeviceCiphertext {
                device_id: "alpha-phone".to_string(),
                ciphertext: "sealed-to-sender".to_string(),
            },
        ],
        is_encrypted: true,
        ..Default::default()
    };
    let drop_id = db.create_drop("alpha", "beta", &new_drop).unwrap();

    // The recipient's inbox carries both copies...
    let inbox = db.inbox("beta", 10, 0).unwrap();
    let received = inbox.iter().find(|d| d.id == drop_id).unwrap();
    assert_eq!(received.note_ciphertexts.len(), 2);

    // ...and so does the sender's own record of it, which is the bug.
    let sent = db.sent_drops("alpha", 10, 0).unwrap();
    let mine = sent.iter().find(|d| d.id == drop_id).unwrap();
    assert_eq!(
        mine.note_ciphertexts
            .iter()
            .find(|c| c.device_id == "alpha-phone")
            .map(|c| c.ciphertext.as_str()),
        Some("sealed-to-sender"),
        "a sender must be able to read the note they sent"
    );

    // The single-copy column still holds the recipient's, for clients that read only that.
    assert_eq!(mine.note_ciphertext.as_deref(), Some("sealed-to-recipient"));
}

/// A conversation is the surface where both halves are read at once, so both must carry their
/// copies — this is the view that rendered `[Encrypted Note]` for everything you had sent.
#[test]
fn a_conversation_carries_sealed_copies_in_both_directions() {
    let db = db();
    let sealed = |device: &str, text: &str| crate::db_drops::DeviceCiphertext {
        device_id: device.to_string(),
        ciphertext: text.to_string(),
    };
    let outgoing = crate::db_drops::NewDrop {
        track_title: "Mine".to_string(),
        artist_name: "A".to_string(),
        note_ciphertexts: vec![
            sealed("alpha-phone", "mine"),
            sealed("beta-phone", "theirs"),
        ],
        is_encrypted: true,
        ..Default::default()
    };
    let incoming = crate::db_drops::NewDrop {
        track_title: "Theirs".to_string(),
        artist_name: "B".to_string(),
        note_ciphertexts: vec![
            sealed("beta-phone", "theirs2"),
            sealed("alpha-phone", "mine2"),
        ],
        is_encrypted: true,
        ..Default::default()
    };
    db.create_drop("alpha", "beta", &outgoing).unwrap();
    db.create_drop("beta", "alpha", &incoming).unwrap();

    let thread = db.conversation("alpha", "beta", 50).unwrap();
    assert_eq!(thread.len(), 2);
    for message in &thread {
        assert!(
            message
                .note_ciphertexts
                .iter()
                .any(|c| c.device_id == "alpha-phone"),
            "alpha must hold a copy of every message in their own thread"
        );
    }
}

#[test]
fn e2ee_encrypted_drop_stores_and_reads_ciphertext() {
    let db = db();
    let new_drop = crate::db_drops::NewDrop {
        track_title: "Secret Track".to_string(),
        artist_name: "Secret Artist".to_string(),
        note: None,
        note_ciphertext: Some("sealed-ciphertext-payload-base64".to_string()),
        is_encrypted: true,
        ..Default::default()
    };
    let drop_id = db.create_drop("alpha", "beta", &new_drop).unwrap();
    let inbox = db.inbox("beta", 10, 0).unwrap();
    let found = inbox.iter().find(|d| d.id == drop_id).unwrap();
    assert_eq!(
        found.note_ciphertext.as_deref(),
        Some("sealed-ciphertext-payload-base64")
    );
    assert!(found.is_encrypted);
    assert_eq!(found.note, None);
}
