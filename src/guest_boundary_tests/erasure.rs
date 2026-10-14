//! Erasure boundary tests.

use super::*;

/// Every table that stores a username, checked by writing to all of them and counting after.
#[tokio::test]
pub(super) async fn deleting_an_account_leaves_nothing_of_it_behind() {
    let h = harness();
    let username = "mallory";

    // A row in every table the account can reach.
    h.db.upsert_node(
        "m-phone",
        username,
        crate::db::NodeName::Set("Phone"),
        "wanda",
        None,
        None,
    )
    .unwrap();
    h.db.mint_device_token(username, "laptop").unwrap();
    h.db.link_federated_identity(username, "https://id.example.com", "sub-1", None)
        .unwrap();
    {
        let conn = h.db.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        for (sql, args) in [
            ("INSERT INTO scrobbles (user_id, track_title, artist_name, duration_secs, device_name, played_at) VALUES (?1, 't', 'a', 180, 'phone', ?2)", vec![username, now.as_str()]),
            ("INSERT INTO friendships (user_id, friend_id, state, created_at) VALUES (?1, 'alpha', 'accepted', ?2)", vec![username, now.as_str()]),
            ("INSERT INTO friendships (user_id, friend_id, state, created_at) VALUES ('alpha', ?1, 'accepted', ?2)", vec![username, now.as_str()]),
            ("INSERT INTO listen_along (listener_id, host_id, started_at) VALUES (?1, 'alpha', ?2)", vec![username, now.as_str()]),
        ] {
            conn.execute(sql, rusqlite::params_from_iter(args)).unwrap();
        }
    }
    h.db.record_event(
        crate::audit::Event::LoginSucceeded,
        crate::audit::Record::new().user(username).ip("203.0.113.9"),
    );

    assert!(h.db.delete_user(username).unwrap());

    let conn = h.db.conn.lock().unwrap();
    for (table, column) in [
        ("registered_nodes", "user_id"),
        ("scrobbles", "user_id"),
        ("federated_identities", "user_id"),
    ] {
        let remaining: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} WHERE {column} IN \
                     (?1, (SELECT id FROM users WHERE username = ?1))"
                ),
                rusqlite::params![username],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining, 0,
            "{table} still holds rows for the deleted account"
        );
    }

    // Friendship is two rows. Removing only one leaves alpha friends with a ghost.
    let friendships: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM friendships WHERE user_id = ?1 OR friend_id = ?1",
            rusqlite::params![username],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        friendships, 0,
        "a friendship survived in the other direction"
    );

    let listening: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM listen_along WHERE listener_id = ?1 OR host_id = ?1",
            rusqlite::params![username],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(listening, 0);
}

/// The audit trail survives, but stripped: "an account was deleted" is a fact the operator needs;
/// rows still naming the person are a record of someone who asked to be forgotten.
#[tokio::test]
pub(super) async fn deletion_keeps_the_audit_trail_but_not_the_identity_in_it() {
    let h = harness();
    h.db.record_event(
        crate::audit::Event::LoginSucceeded,
        crate::audit::Record::new()
            .user("mallory")
            .ip("203.0.113.9"),
    );
    h.db.delete_user("mallory").unwrap();

    let events = h.db.security_events(None, 100).unwrap();
    assert!(!events.is_empty(), "the trail itself should remain");
    assert!(
        events
            .iter()
            .all(|e| e.user_id.is_none() && e.client_ip.is_none()),
        "a deleted account must not still be named in the log"
    );
}

/// An export is the counterpart to erasure, and carries the same boundary: your data, not anyone
/// else's — and not a way for an administrator to read a member's listening history either.
#[tokio::test]
pub(super) async fn an_export_is_self_scoped_even_for_an_admin() {
    let h = harness();
    assert_forbidden(
        &h.run_as(&h.guest, r#"{ exportMyData(userId: "alpha") }"#)
            .await,
        "mallory exporting alpha's data",
    );
    assert_forbidden(
        &h.run_as(&h.admin, r#"{ exportMyData(userId: "mallory") }"#)
            .await,
        "an admin exporting a member's data",
    );
    assert_allowed(
        &h.run_as(&h.guest, r#"{ exportMyData(userId: "mallory") }"#)
            .await,
        "mallory exporting their own data",
    );
}

/// An export must not hand back the credentials themselves — a token hash is no use to its owner,
/// and a passphrase hash was never theirs to have.
#[tokio::test]
pub(super) async fn an_export_carries_data_but_no_credentials() {
    let h = harness();
    h.db.mint_device_token("mallory", "laptop").unwrap();
    let response = h
        .run_as(&h.guest, r#"{ exportMyData(userId: "mallory") }"#)
        .await;
    assert_allowed(&response, "exporting own data");

    let rendered = response.data.to_string();
    for forbidden in [
        "passphrase_hash",
        "token_hash",
        "totp_secret",
        "vault_key_wrapped",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "an export must not include {forbidden}"
        );
    }
    assert!(
        rendered.contains("listening_history"),
        "but it must include the data"
    );
}
