//! Incognito boundary tests.

use super::*;

/// Incognito is not a sixth switch, it is an override of all of them. A test per surface,
/// because the value of putting the check in `visible_profile` is precisely that no surface can
/// opt out of it — and that is only worth anything if it is asserted surface by surface.
#[tokio::test]
pub(super) async fn incognito_closes_now_playing_even_with_the_flag_open() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let open = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert!(
        open.data.to_string().contains("A Secret Song"),
        "precondition failed: the friend should see this before incognito"
    );

    h.db.set_incognito("alpha", true).unwrap();

    let quiet = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert_allowed(&quiet, "friendsNowPlaying");
    assert_eq!(
        quiet.data.to_string(),
        r#"{friendsNowPlaying: []}"#,
        "playback leaked while the account was incognito"
    );
}

/// Turning it off must give back exactly what was there before, and nothing more.
#[tokio::test]
pub(super) async fn leaving_incognito_restores_the_switches_untouched() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();

    h.db.set_incognito("alpha", true).unwrap();
    h.db.set_incognito("alpha", false).unwrap();

    let profile = h.db.profile("alpha").unwrap().unwrap();
    assert!(profile.show_now_playing, "incognito ate show_now_playing");
    assert!(!profile.show_stats, "incognito turned show_stats on");
    assert!(!profile.incognito);
}

/// Being quiet is itself undisclosed: a friend meets the same refusal a stranger does, so they
/// cannot tell "gone incognito" from "was never allowed".
#[tokio::test]
pub(super) async fn incognito_is_indistinguishable_from_never_having_been_allowed() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.db.set_incognito("alpha", true).unwrap();

    let friend = h
        .run_as(&h.beta, r#"{ profile(username: "alpha") { username } }"#)
        .await;
    let stranger = h
        .run_as(
            &h.stranger,
            r#"{ profile(username: "alpha") { username } }"#,
        )
        .await;

    let message = |r: &async_graphql::Response| {
        r.errors
            .first()
            .map(|e| e.message.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        message(&friend),
        message(&stranger),
        "an incognito account is distinguishable from a closed one"
    );
}

/// The account's own devices must still see it, or the switch cannot be turned back off.
#[tokio::test]
pub(super) async fn you_can_still_see_your_own_profile_while_incognito() {
    let h = harness();
    h.db.set_incognito("alpha", true).unwrap();

    let own = h
        .run_as(
            &h.alpha,
            r#"{ profile(username: "alpha") { username incognito } }"#,
        )
        .await;
    assert_allowed(&own, "own profile while incognito");
    assert!(own.data.to_string().contains("incognito: true"));
}

/// Defaults matter: an upgrade must not put anybody into incognito.
#[tokio::test]
pub(super) async fn a_new_account_is_not_incognito() {
    let h = harness();
    assert!(!h.db.profile("alpha").unwrap().unwrap().incognito);
}

/// The client half needs this at launch: without a read, a device that has just started has no
/// way to learn the account is already quiet and would carry on recording.
#[tokio::test]
pub(super) async fn the_account_can_read_its_own_incognito_state() {
    let h = harness();

    let before = h.run_as(&h.alpha, "{ incognito }").await;
    assert_allowed(&before, "incognito");
    assert_eq!(before.data.to_string(), r#"{incognito: false}"#);

    h.db.set_incognito("alpha", true).unwrap();

    let after = h.run_as(&h.alpha, "{ incognito }").await;
    assert_eq!(after.data.to_string(), r#"{incognito: true}"#);
}

/// It answers for the caller and nobody else — there is no argument to point it at someone.
#[tokio::test]
pub(super) async fn one_account_cannot_read_anothers_incognito_state() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_incognito("alpha", true).unwrap();

    let beta_view = h.run_as(&h.beta, "{ incognito }").await;
    assert_eq!(
        beta_view.data.to_string(),
        r#"{incognito: false}"#,
        "the query answered about somebody other than the caller"
    );
}
