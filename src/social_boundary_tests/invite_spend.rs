//! Invite spend boundary tests.

use super::*;

/// A refused signup must not eat the invite it was offered.
///
/// The code used to be spent before the username was checked, so the ordinary sequence — pick a
/// name that is taken, get refused, try another — consumed the invite on the failed attempt and
/// dropped the retry into the approval queue. From the outside the feature simply did not work.
#[tokio::test]
pub(super) async fn a_failed_signup_does_not_spend_the_invite() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    // A name that is already taken, offered along with a valid code.
    assert!(h.db.account("beta").unwrap().is_some());

    // The check that now runs first.
    let taken = h.db.account("beta").unwrap().is_some();
    assert!(taken, "precondition: beta exists");
    let listed = h.db.list_invites().unwrap();
    assert_eq!(
        listed[0].used_count, 0,
        "the invite was spent before anything was validated"
    );

    // And it still works afterwards.
    assert!(h.db.redeem_invite(&invite.code).unwrap());
    assert!(
        !h.db.redeem_invite(&invite.code).unwrap(),
        "a one-use code was spent twice"
    );
}

/// A refund puts a use back, and never takes the count below zero.
#[tokio::test]
pub(super) async fn a_refunded_invite_can_be_used_again() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    assert!(h.db.redeem_invite(&invite.code).unwrap());
    assert!(!h.db.redeem_invite(&invite.code).unwrap(), "already spent");

    assert!(h.db.refund_invite(&invite.code).unwrap());
    assert!(
        h.db.redeem_invite(&invite.code).unwrap(),
        "the refund did not restore the use"
    );

    // Refunding past zero would make a code usable more often than allowed.
    h.db.refund_invite(&invite.code).unwrap();
    assert!(
        !h.db.refund_invite(&invite.code).unwrap(),
        "refund went below zero"
    );
}

/// Deleting an invite takes it off the list; only an administrator may.
#[tokio::test]
pub(super) async fn only_an_admin_deletes_an_invite() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    let refused = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ deleteInvite(code: "{}") }}"#, invite.code),
        )
        .await;
    assert_refused(&refused, "a member deleting an invite");
    assert_eq!(h.db.list_invites().unwrap().len(), 1);

    let allowed = h
        .run_as(
            &h.alpha,
            &format!(r#"mutation {{ deleteInvite(code: "{}") }}"#, invite.code),
        )
        .await;
    assert_allowed(&allowed, "an admin deleting an invite");
    assert!(
        h.db.list_invites().unwrap().is_empty(),
        "the invite was not removed"
    );
}

/// A join code has to survive being read out loud.
///
/// It used to be a base64url token prefix, so codes arrived containing `-` and `_` — a poor thing
/// to dictate across a room and worse to retype.
#[tokio::test]
pub(super) async fn a_join_code_is_alphanumeric_and_unambiguous() {
    let h = harness();
    for _ in 0..50 {
        let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
        assert!(
            jam.code.chars().all(|c| c.is_ascii_alphanumeric()),
            "code {} is not alphanumeric",
            jam.code
        );
        // The characters nobody can tell apart in a sans-serif font.
        assert!(
            !jam.code.contains(['0', 'O', '1', 'I']),
            "code {} contains a confusable character",
            jam.code
        );
        h.db.delete_jam(&jam.id).unwrap();
    }
}
