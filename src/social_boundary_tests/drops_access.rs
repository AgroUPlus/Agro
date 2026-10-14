//! Drops access boundary tests.

use super::*;

/// The gate on sending is friendship, and a stranger is refused the same way as everywhere else.
#[tokio::test]
pub(super) async fn a_stranger_cannot_drop_a_track_on_you() {
    let h = harness();
    let refused = h
        .run_as(
            &h.stranger,
            r#"mutation { dropTrack(to: "alpha", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    assert_refused(&refused, "a stranger dropping a track");

    h.befriend("alpha", "stranger");
    let allowed = h
        .run_as(
            &h.stranger,
            r#"mutation { dropTrack(to: "alpha", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    assert_allowed(&allowed, "a friend dropping a track");
}

/// A refusal must not distinguish "not your friend" from "no such account".
#[tokio::test]
pub(super) async fn dropping_to_a_missing_account_and_a_non_friend_answer_identically() {
    let h = harness();
    let missing = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "nobody", trackTitle: "t", artistName: "a") { id } }"#,
        )
        .await;
    let not_friend = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "stranger", trackTitle: "t", artistName: "a") { id } }"#,
        )
        .await;

    assert_refused(&missing, "dropping to an account that does not exist");
    assert_refused(&not_friend, "dropping to an account that is not a friend");
    assert_eq!(
        missing.errors[0].message, not_friend.errors[0].message,
        "the two refusals must be indistinguishable, or this becomes an account directory"
    );
}

/// An inbox is nobody else's, including the sender's.
#[tokio::test]
pub(super) async fn nobody_can_read_another_accounts_inbox() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.run_as(
        &h.alpha,
        r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
    )
    .await;

    // There is no argument for whose inbox to read, so the strongest statement available is that
    // the sender's own inbox does not contain what they sent.
    let senders_inbox = h.run_as(&h.alpha, r#"{ inbox { trackTitle } }"#).await;
    assert_allowed(&senders_inbox, "reading your own inbox");
    assert!(
        !senders_inbox.data.to_string().contains("Xtal"),
        "alpha sent this, they did not receive it: {:?}",
        senders_inbox.data
    );

    let recipients_inbox = h.run_as(&h.beta, r#"{ inbox { trackTitle } }"#).await;
    assert!(recipients_inbox.data.to_string().contains("Xtal"));
}

/// A song someone gave you is yours. Unfriending them does not reach into your inbox.
#[tokio::test]
pub(super) async fn a_drop_survives_the_friendship_that_delivered_it() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.run_as(
        &h.alpha,
        r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
    )
    .await;

    assert!(h.db.remove_friend("beta", "alpha").unwrap());

    let inbox = h
        .run_as(&h.beta, r#"{ inbox { trackTitle fromUser } }"#)
        .await;
    assert_allowed(&inbox, "reading the inbox after unfriending");
    assert!(
        inbox.data.to_string().contains("Xtal"),
        "the drop was already delivered and is not withdrawn: {:?}",
        inbox.data
    );
}

/// Marking somebody else's drop read is a not-found, not a refusal that confirms it exists.
#[tokio::test]
pub(super) async fn marking_someone_elses_drop_read_is_a_not_found() {
    let h = harness();
    h.befriend("alpha", "beta");
    let sent = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    let id = sent.data.to_string();
    let id = id
        .split('"')
        .find(|part| part.len() == 36 && part.contains('-'))
        .expect("a uuid in the response")
        .to_string();

    // The sender is not the recipient, so this is not theirs to mark either.
    let by_sender = h
        .run_as(
            &h.alpha,
            &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#),
        )
        .await;
    assert_allowed(&by_sender, "the call itself succeeds");
    assert!(
        by_sender.data.to_string().contains("false"),
        "it is not the sender's to mark: {:?}",
        by_sender.data
    );

    let by_recipient = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#),
        )
        .await;
    assert!(by_recipient.data.to_string().contains("true"));
}

/// Read receipts are not a feature. What the sender sees never says whether it was opened.
#[tokio::test]
pub(super) async fn a_sender_is_never_told_that_their_drop_was_read() {
    let h = harness();
    h.befriend("alpha", "beta");
    let sent = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    let id = sent.data.to_string();
    let id = id
        .split('"')
        .find(|part| part.len() == 36 && part.contains('-'))
        .expect("a uuid in the response")
        .to_string();

    assert!(h.db.mark_drop_read("beta", &id).unwrap());

    let sent_view = h
        .run_as(&h.alpha, r#"{ sentDrops { trackTitle readAt } }"#)
        .await;
    assert_allowed(&sent_view, "reading what you sent");
    assert!(
        sent_view.data.to_string().contains("Xtal"),
        "the sender should still see the drop: {:?}",
        sent_view.data
    );
    assert!(
        !sent_view.data.to_string().contains("readAt\":\"2"),
        "readAt must be blanked for the sender: {:?}",
        sent_view.data
    );
}

/// Archiving takes it out of the inbox without destroying the sender's record of it.
#[tokio::test]
pub(super) async fn archiving_clears_the_inbox_without_deleting_the_row() {
    let h = harness();
    h.befriend("alpha", "beta");
    let sent = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    let id = sent.data.to_string();
    let id = id
        .split('"')
        .find(|part| part.len() == 36 && part.contains('-'))
        .expect("a uuid in the response")
        .to_string();

    assert!(h.db.archive_drop("beta", &id).unwrap());

    let inbox = h
        .run_as(&h.beta, r#"{ inbox { trackTitle } unreadDropCount }"#)
        .await;
    assert!(!inbox.data.to_string().contains("Xtal"));

    let sent_view = h.run_as(&h.alpha, r#"{ sentDrops { trackTitle } }"#).await;
    assert!(
        sent_view.data.to_string().contains("Xtal"),
        "the sender's record survives the recipient tidying up: {:?}",
        sent_view.data
    );
}

/// Dropping to yourself is refused — it is a mistake, not a feature.
#[tokio::test]
pub(super) async fn you_cannot_drop_a_track_to_yourself() {
    let h = harness();
    let refused = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "alpha", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    assert_refused(&refused, "dropping to yourself");
}

/// A key list is only useful to somebody who may send you something, and answering for a stranger
/// would make it a way to learn whether an account exists. Refused with the same words `dropTrack`
/// uses, so the two cannot be told apart.
#[tokio::test]
pub(super) async fn a_stranger_cannot_read_your_device_keys() {
    let h = harness();
    h.db.register_device_key("alpha", "phone", "key-phone")
        .unwrap();

    let refused = h
        .run_as(
            &h.stranger,
            r#"{ deviceKeys(username: "alpha") { deviceId publicKey } }"#,
        )
        .await;
    assert_refused(&refused, "a stranger reading a key list");

    h.befriend("alpha", "beta");
    let allowed = h
        .run_as(
            &h.beta,
            r#"{ deviceKeys(username: "alpha") { deviceId publicKey } }"#,
        )
        .await;
    assert_allowed(&allowed, "a friend reading a key list");
}

/// A client has to be able to seal a copy to its *own* other devices, so reading your own list is
/// never gated on being your own friend.
#[tokio::test]
pub(super) async fn you_can_always_read_your_own_device_keys() {
    let h = harness();
    h.db.register_device_key("alpha", "phone", "key-phone")
        .unwrap();
    h.db.register_device_key("alpha", "laptop", "key-laptop")
        .unwrap();

    let response = h
        .run_as(
            &h.alpha,
            r#"{ deviceKeys(username: "alpha") { deviceId publicKey } }"#,
        )
        .await;
    assert_allowed(&response, "reading your own key list");
    let rendered = format!("{:?}", response.data);
    assert!(
        rendered.contains("phone"),
        "own list should carry every device: {rendered}"
    );
    assert!(
        rendered.contains("laptop"),
        "own list should carry every device: {rendered}"
    );
}
