//! Drops receipts boundary tests.

use super::*;

/// A friend may hand you a song. A friend may not fill your inbox.
#[tokio::test]
pub(super) async fn a_sender_is_rate_limited_per_recipient() {
    let h = harness();
    h.befriend("alpha", "beta");

    for i in 0..20 {
        let response = h
            .run_as(
                &h.alpha,
                &format!(
                    r#"mutation {{ dropTrack(to: "beta", trackTitle: "t{i}", artistName: "a") {{ id }} }}"#
                ),
            )
            .await;
        assert_allowed(&response, "a drop inside the limit");
    }

    let refused = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "one too many", artistName: "a") { id } }"#,
        )
        .await;
    assert_refused(&refused, "the twenty-first drop in an hour");
}

/// The unread count is what a badge shows, and it counts only what is still waiting.
#[tokio::test]
pub(super) async fn the_unread_count_ignores_read_and_archived_drops() {
    let h = harness();
    h.befriend("alpha", "beta");
    for i in 0..3 {
        h.run_as(
            &h.alpha,
            &format!(r#"mutation {{ dropTrack(to: "beta", trackTitle: "t{i}", artistName: "a") {{ id }} }}"#),
        )
        .await;
    }

    let count = h.run_as(&h.beta, r#"{ unreadDropCount }"#).await;
    assert!(count.data.to_string().contains('3'), "{:?}", count.data);

    let ids: Vec<String> =
        h.db.inbox("beta", 10, 0)
            .unwrap()
            .into_iter()
            .map(|drop| drop.id)
            .collect();
    h.db.mark_drop_read("beta", &ids[0]).unwrap();
    h.db.archive_drop("beta", &ids[1]).unwrap();

    let after = h.run_as(&h.beta, r#"{ unreadDropCount }"#).await;
    assert!(after.data.to_string().contains('1'), "{:?}", after.data);
}

// ── Reactions and conversations ─────────────────────────────────────────────────────────────

/// Reacting is the recipient's reply. A sender must not be able to answer their own message.
#[tokio::test]
pub(super) async fn only_the_recipient_can_react_to_a_drop() {
    let h = harness();
    h.befriend("alpha", "beta");
    let id = h.drop_a_track("alpha", "beta", "Xtal").await;

    let by_sender = h
        .run_as(
            &h.alpha,
            &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#),
        )
        .await;
    assert_allowed(&by_sender, "the call itself succeeds");
    assert!(
        by_sender.data.to_string().contains("false"),
        "a sender reacted to their own drop: {:?}",
        by_sender.data
    );

    let by_stranger = h
        .run_as(
            &h.stranger,
            &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#),
        )
        .await;
    assert!(
        by_stranger.data.to_string().contains("false"),
        "a stranger reacted to somebody else's drop: {:?}",
        by_stranger.data
    );

    let by_recipient = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#),
        )
        .await;
    assert!(by_recipient.data.to_string().contains("true"));
}

/// Unlike a read receipt, a reaction is something the recipient chose to send — so it goes back.
#[tokio::test]
pub(super) async fn a_reaction_reaches_the_sender_but_a_read_receipt_still_does_not() {
    let h = harness();
    h.befriend("alpha", "beta");
    let id = h.drop_a_track("alpha", "beta", "Xtal").await;

    h.run_as(
        &h.beta,
        &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#),
    )
    .await;
    h.run_as(
        &h.beta,
        &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#),
    )
    .await;

    let seen = h
        .run_as(&h.alpha, r#"{ sentDrops { reaction readAt } }"#)
        .await;
    let body = seen.data.to_string();
    assert!(
        body.contains('🔥'),
        "the sender cannot see the reaction: {body}"
    );
    assert!(
        body.contains("readAt\":null") || body.contains("readAt: null"),
        "a read receipt leaked to the sender: {body}"
    );
}

/// A thread is not a way around the read-receipt rule.
#[tokio::test]
pub(super) async fn a_conversation_still_hides_read_receipts_on_your_own_messages() {
    let h = harness();
    h.befriend("alpha", "beta");
    let id = h.drop_a_track("alpha", "beta", "Xtal").await;
    h.run_as(
        &h.beta,
        &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#),
    )
    .await;

    let thread = h
        .run_as(
            &h.alpha,
            r#"{ conversation(with: "beta") { fromUser readAt } }"#,
        )
        .await;
    assert_allowed(&thread, "reading your own thread");
    let body = thread.data.to_string();
    assert!(
        !body.contains("readAt\":\"2") && !body.contains("readAt\":\"1"),
        "the thread told the sender their drop was read: {body}"
    );
}

/// A conversation is between two people. A third cannot read it by naming them.
#[tokio::test]
pub(super) async fn a_conversation_only_ever_contains_the_callers_own_messages() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.drop_a_track("alpha", "beta", "Xtal").await;

    let peeked = h
        .run_as(
            &h.stranger,
            r#"{ conversation(with: "beta") { trackTitle } }"#,
        )
        .await;
    assert_allowed(&peeked, "the query itself is not an error");
    assert!(
        !peeked.data.to_string().contains("Xtal"),
        "a stranger read somebody else's conversation: {:?}",
        peeked.data
    );
}

// ── Friend codes ────────────────────────────────────────────────────────────────────────────
