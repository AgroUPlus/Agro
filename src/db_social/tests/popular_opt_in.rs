//! Whether an account contributes to the popular charts.

use super::*;

/// Unlike every other visibility flag, this one starts open — see migration 48.
#[test]
fn defaults_on_for_a_new_account() {
    let db = Db::new_in_memory().unwrap();
    db.create_account("alpha", "alpha-pass", Role::Member, AccountState::Active)
        .unwrap();

    let profile = db.profile("alpha").unwrap().unwrap();
    assert!(profile.popular_opt_in, "Popular on Agro must default to on");
    assert!(profile.contributes_to_popular());
}

#[test]
fn opting_out_stops_contribution_without_touching_incognito() {
    let db = Db::new_in_memory().unwrap();
    db.create_account("alpha", "alpha-pass", Role::Member, AccountState::Active)
        .unwrap();

    db.set_popular_opt_in("alpha", false).unwrap();

    let profile = db.profile("alpha").unwrap().unwrap();
    assert!(!profile.popular_opt_in);
    assert!(!profile.contributes_to_popular());
    assert!(
        !profile.incognito,
        "opting out of the chart must not be incognito"
    );
}

/// Incognito is a blanket override, same as it is for now-playing and stats.
#[test]
fn incognito_suppresses_contribution_even_when_opted_in() {
    let db = Db::new_in_memory().unwrap();
    db.create_account("alpha", "alpha-pass", Role::Member, AccountState::Active)
        .unwrap();

    db.set_incognito("alpha", true).unwrap();

    let profile = db.profile("alpha").unwrap().unwrap();
    assert!(
        profile.popular_opt_in,
        "the standing consent itself must be untouched"
    );
    assert!(
        !profile.contributes_to_popular(),
        "incognito must still override it"
    );
}
