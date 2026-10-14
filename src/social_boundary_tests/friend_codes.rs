//! Friend codes boundary tests.

use super::*;

/// The whole point of a five-minute code is that it cannot be used twice.
#[tokio::test]
pub(super) async fn a_friend_code_is_single_use() {
    let h = harness();
    let code = h.mint_friend_code(&h.alpha).await;

    let first = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#),
        )
        .await;
    assert!(
        first.data.to_string().contains("alpha"),
        "the first redemption failed: {:?}",
        first.data
    );
    assert!(h.db.are_friends("alpha", "beta").unwrap());

    let second = h
        .run_as(
            &h.stranger,
            &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#),
        )
        .await;
    assert!(
        second.data.to_string().contains("null"),
        "a spent code was redeemed again: {:?}",
        second.data
    );
    assert!(!h.db.are_friends("alpha", "stranger").unwrap());
}

/// Minting a new code has to kill the old one, or every code ever shown stays live.
#[tokio::test]
pub(super) async fn minting_a_friend_code_invalidates_the_previous_one() {
    let h = harness();
    let old = h.mint_friend_code(&h.alpha).await;
    let new = h.mint_friend_code(&h.alpha).await;
    assert_ne!(old, new, "re-minting returned the same code");

    let stale = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ redeemFriendCode(code: "{old}") }}"#),
        )
        .await;
    assert!(
        stale.data.to_string().contains("null"),
        "a replaced code still worked: {:?}",
        stale.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

/// Revoking is what a closed QR panel does, and it has to actually take effect.
#[tokio::test]
pub(super) async fn a_revoked_friend_code_stops_working() {
    let h = harness();
    let code = h.mint_friend_code(&h.alpha).await;
    h.run_as(&h.alpha, r#"mutation { revokeFriendCode }"#).await;

    let after = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#),
        )
        .await;
    assert!(
        after.data.to_string().contains("null"),
        "a revoked code still worked: {:?}",
        after.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

/// A code is a stand-in for the username search, not for consent. A block still wins.
#[tokio::test]
pub(super) async fn a_friend_code_cannot_be_used_to_get_around_a_block() {
    let h = harness();
    h.db.block_user("alpha", "beta").unwrap();
    let code = h.mint_friend_code(&h.alpha).await;

    let blocked = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#),
        )
        .await;
    assert!(
        blocked.data.to_string().contains("null"),
        "a block was talked past with a code: {:?}",
        blocked.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

/// An expired code is dead, which is the only reason the short lifetime is worth anything.
#[tokio::test]
pub(super) async fn an_expired_friend_code_is_refused() {
    let h = harness();
    let code = h.mint_friend_code(&h.alpha).await;
    // Reaching past the API deliberately: there is no way to wait five minutes in a test, and the
    // expiry check is the thing being tested rather than the clock.
    h.expire_friend_codes();

    let after = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#),
        )
        .await;
    assert!(
        after.data.to_string().contains("null"),
        "an expired code still worked: {:?}",
        after.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}
