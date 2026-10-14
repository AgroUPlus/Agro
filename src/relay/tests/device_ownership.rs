//! Relay sessions are bound to the devices that opened them.

use super::*;

use crate::db::{Db, NodeName};
use crate::db_identity::{AccountState, Role};

/// Two accounts, each with one registered device.
fn two_accounts() -> (AppState, AuthedUser) {
    let db = Db::new_in_memory().unwrap();
    for (user, device) in [
        ("alpha", "alpha-pc"),
        ("mallory", "mallory-pc"),
        ("friend", "friend-phone"),
    ] {
        db.create_account(user, "passphrase", Role::Member, AccountState::Active)
            .unwrap();
        db.upsert_node(device, user, NodeName::Set(device), "wander", None, None)
            .unwrap();
    }
    let account = db.account("alpha").unwrap().unwrap();
    let state = crate::login::tests::test_state(db);
    (
        state,
        AuthedUser {
            account,
            device_label: "alpha-pc".to_string(),
            token_hash: String::new(),
        },
    )
}

async fn open(state: AppState, user: AuthedUser, from: &str, to: &str) -> StatusCode {
    open_relay(
        State(state),
        axum::Extension(user),
        Json(OpenRelayRequest {
            jam_id: None,
            listener_public_key: None,
            content_hash: "hash123".to_string(),
            from_device: from.to_string(),
            to_device: to.to_string(),
        }),
    )
    .await
    .status()
}

#[tokio::test]
async fn a_relay_between_two_of_my_own_devices_is_allowed() {
    let (state, alpha) = two_accounts();
    state
        .db
        .upsert_node(
            "alpha-phone",
            "alpha",
            NodeName::Set("phone"),
            "wanda",
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        open(state, alpha, "alpha-pc", "alpha-phone").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_relay_from_a_friend_is_allowed() {
    let (state, alpha) = two_accounts();
    // Accept friend relation
    state.db.send_friend_request("alpha", "friend").unwrap();
    state.db.accept_friend_request("friend", "alpha").unwrap();
    assert_eq!(
        open(state, alpha, "friend-phone", "alpha-pc").await,
        StatusCode::OK
    );
}

/// A request that has been *sent* and not answered is not a friendship. Treating any
/// `friend_state` row as permission let a stranger who merely asked make this account's
/// device upload a hash of their choosing.
#[tokio::test]
async fn a_relay_from_a_pending_request_is_refused() {
    let (state, alpha) = two_accounts();
    state.db.send_friend_request("friend", "alpha").unwrap();
    assert_eq!(
        open(state, alpha, "friend-phone", "alpha-pc").await,
        StatusCode::FORBIDDEN
    );
}

/// And a block is the strongest answer there is, including over a shared jam.
#[tokio::test]
async fn a_relay_from_a_blocked_account_is_refused() {
    let (state, alpha) = two_accounts();
    state.db.send_friend_request("alpha", "friend").unwrap();
    state.db.accept_friend_request("friend", "alpha").unwrap();
    state.db.block_user("alpha", "friend").unwrap();
    assert_eq!(
        open(state, alpha, "friend-phone", "alpha-pc").await,
        StatusCode::FORBIDDEN
    );
}

/// Naming an unrelated stranger's device as the sender is forbidden.
#[tokio::test]
async fn i_cannot_make_a_strangers_device_the_sender() {
    let (state, alpha) = two_accounts();
    assert_eq!(
        open(state, alpha, "mallory-pc", "alpha-pc").await,
        StatusCode::FORBIDDEN
    );
}

/// And naming someone else's device as the *receiver* is always forbidden.
#[tokio::test]
async fn i_cannot_aim_a_relay_at_another_accounts_device() {
    let (state, alpha) = two_accounts();
    assert_eq!(
        open(state, alpha, "alpha-pc", "mallory-pc").await,
        StatusCode::FORBIDDEN
    );
}

/// A device id nobody has registered is refused.
#[tokio::test]
async fn an_unknown_device_is_refused_like_someone_elses() {
    let (state, alpha) = two_accounts();
    assert_eq!(
        open(state, alpha, "alpha-pc", "no-such-device").await,
        StatusCode::FORBIDDEN
    );
}
