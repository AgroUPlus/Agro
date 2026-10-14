//! Recaps boundary tests.

use super::*;

/// A recap is an aggregate, so it is gated on the statistics switch — and honours it.
#[tokio::test]
pub(super) async fn a_recap_omits_members_who_closed_their_stats() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 5, 3600);
    h.seed_plays("beta", "Xtal", "Aphex Twin", 5, 3600);

    let closed = h.run_as(&h.alpha, r#"{ circleRecap { members } }"#).await;
    assert_allowed(&closed, "a recap with a friend who has stats closed");
    assert!(
        !closed.data.to_string().contains("beta"),
        "beta never opened their statistics: {:?}",
        closed.data
    );

    h.db.set_visibility("beta", false, true).unwrap();
    let opened = h.run_as(&h.alpha, r#"{ circleRecap { members } }"#).await;
    assert!(
        opened.data.to_string().contains("beta"),
        "beta opened their statistics and should now be in it: {:?}",
        opened.data
    );
}

/// You are always in your own recap, friends or none.
#[tokio::test]
pub(super) async fn a_recap_of_one_is_still_a_recap() {
    let h = harness();
    h.seed_plays("stranger", "Xtal", "Aphex Twin", 4, 3600);
    let response = h
        .run_as(
            &h.stranger,
            r#"{ circleRecap { members anthem { title plays } } }"#,
        )
        .await;
    assert_allowed(&response, "a recap with nobody else in it");
    assert!(response.data.to_string().contains("stranger"));
    assert!(response.data.to_string().contains("Xtal"));
}

/// The trendsetter is whoever got there first, not whoever played it most.
#[tokio::test]
pub(super) async fn the_trendsetter_is_decided_by_who_was_earliest() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("beta", false, true).unwrap();

    // Beta played it once, a week ago. Alpha played it twenty times, yesterday.
    h.seed_plays("beta", "Windowlicker", "Aphex Twin", 1, 7 * 86_400);
    h.seed_plays("alpha", "Windowlicker", "Aphex Twin", 20, 86_400);

    let response = h
        .run_as(
            &h.alpha,
            r#"{ circleRecap(period: "ALL") { trendsetter { username firsts } } }"#,
        )
        .await;
    assert_allowed(&response, "reading the trendsetter");
    assert!(
        response.data.to_string().contains("beta"),
        "beta heard it first, however much alpha has played it since: {:?}",
        response.data
    );
}

// ── Agro Replay ─────────────────────────────────────────────────────────────────────────────

/// The recap is a statistics surface, so it is gated by the statistics flag — not by friendship,
/// and not by the now-playing flag that sits next to it.
#[tokio::test]
pub(super) async fn a_wrapped_recap_is_refused_when_they_closed_their_stats() {
    let h = harness();
    h.befriend("alpha", "beta");
    // Now playing open, stats closed. One flag must not imply the other.
    h.db.set_visibility("alpha", true, false).unwrap();

    let refused = h
        .run_as(
            &h.beta,
            r#"{ agroWrapped(userId: "alpha", year: 2026) { totalPlays } }"#,
        )
        .await;
    assert_refused(&refused, "agroWrapped with show_stats off");

    h.db.set_visibility("alpha", true, true).unwrap();
    let allowed = h
        .run_as(
            &h.beta,
            r#"{ agroWrapped(userId: "alpha", year: 2026) { totalPlays } }"#,
        )
        .await;
    assert_allowed(&allowed, "agroWrapped with show_stats on");
}

/// Opening your statistics opens them to friends, not to the server.
#[tokio::test]
pub(super) async fn a_wrapped_recap_is_refused_for_a_stranger() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();

    let refused = h
        .run_as(
            &h.stranger,
            r#"{ agroWrapped(userId: "alpha", year: 2026) { totalPlays topArtists { name } } }"#,
        )
        .await;
    assert_refused(&refused, "agroWrapped read by a non-friend");
}

/// The `year` argument is a window, not a way around the visibility loop.
#[tokio::test]
pub(super) async fn a_recap_of_a_named_year_still_omits_closed_members() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("beta", true, false).unwrap();
    h.seed_plays("beta", "Windowlicker", "Aphex Twin", 3, 3600);

    let year = chrono::Utc::now().format("%Y").to_string();
    let response = h
        .run_as(
            &h.alpha,
            &format!("{{ circleRecap(year: {year}) {{ members }} }}"),
        )
        .await;
    assert_allowed(&response, "a recap of a named year");
    assert!(
        !response.data.to_string().contains("beta"),
        "beta's statistics are closed and a year window must not open them: {:?}",
        response.data
    );

    h.db.set_visibility("beta", true, true).unwrap();
    let opened = h
        .run_as(
            &h.alpha,
            &format!("{{ circleRecap(year: {year}) {{ members }} }}"),
        )
        .await;
    assert!(
        opened.data.to_string().contains("beta"),
        "and must admit them once they are open: {:?}",
        opened.data
    );
}

/// The one cross-account read in Replay. It answers a position and nothing that has a name on it.
#[tokio::test]
pub(super) async fn a_chart_standing_names_nobody() {
    let h = harness();
    h.seed_plays("alpha", "A Loud Song", "Alpha's Band", 50, 3600);
    h.seed_plays("beta", "A Quiet Song", "Beta's Band", 1, 3600);
    h.seed_plays("stranger", "Another Song", "Stranger's Band", 5, 3600);

    let year = chrono::Utc::now().format("%Y").to_string();
    let response = h
        .run_as(
            &h.stranger,
            &format!(
                "{{ replayChartStanding(year: {year}) \
                 {{ percentile cohortSize minutes suppressed }} }}"
            ),
        )
        .await;
    assert_allowed(&response, "reading your own chart standing");

    let body = response.data.to_string();
    for name in [
        "alpha",
        "beta",
        "Alpha's Band",
        "Beta's Band",
        "A Loud Song",
    ] {
        assert!(
            !body.contains(name),
            "a standing must not carry {name}: {body}"
        );
    }
}

/// Three accounts is not a crowd. Below the floor there is no ranking at all, because on a
/// household server "you are top of three" is a statement about the other two.
#[tokio::test]
pub(super) async fn a_small_server_is_told_nothing_rather_than_a_ranking() {
    let h = harness();
    h.seed_plays("alpha", "A Loud Song", "Alpha's Band", 50, 3600);
    h.seed_plays("beta", "A Quiet Song", "Beta's Band", 1, 3600);
    h.seed_plays("stranger", "Another Song", "Stranger's Band", 5, 3600);

    let year = chrono::Utc::now().format("%Y").to_string();
    let response = h
        .run_as(
            &h.stranger,
            &format!(
                "{{ replayChartStanding(year: {year}) {{ percentile cohortSize suppressed }} }}"
            ),
        )
        .await;

    let body = response.data.to_string();
    assert!(body.contains("suppressed: true"), "{body}");
    assert!(
        body.contains("cohortSize: 0"),
        "the size is not leaked either: {body}"
    );
    assert!(body.contains("percentile: 0"), "{body}");
}

// ── Song drops ──────────────────────────────────────────────────────────────────────────────
