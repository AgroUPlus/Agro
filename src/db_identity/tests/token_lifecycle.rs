//! A token that never stops working makes every other credential control decorative, so these
//! pin both halves: that expiry is enforced, and that it does not fire on anyone it shouldn't.
use super::*;

fn account_with_token(db: &Db) -> String {
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    db.mint_device_token("alpha", "laptop").unwrap()
}

/// Backdates a token's `last_used_at`, standing in for a device that stopped checking in.
fn last_used_days_ago(db: &Db, days: i64) {
    let when = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    db.conn
        .lock()
        .unwrap()
        .execute("UPDATE app_passwords SET last_used_at = ?1", params![when])
        .unwrap();
}

#[test]
fn a_freshly_minted_token_works() {
    let db = db();
    let token = account_with_token(&db);
    assert!(db.account_for_token(&token).unwrap().is_some());
}

#[test]
fn a_token_past_its_expires_at_is_refused() {
    let db = db();
    let token = account_with_token(&db);
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    db.conn
        .lock()
        .unwrap()
        .execute("UPDATE app_passwords SET expires_at = ?1", params![past])
        .unwrap();
    assert!(db.account_for_token(&token).unwrap().is_none());
}

#[test]
fn a_token_with_a_future_expires_at_still_works() {
    let db = db();
    let token = account_with_token(&db);
    let future = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    db.conn
        .lock()
        .unwrap()
        .execute("UPDATE app_passwords SET expires_at = ?1", params![future])
        .unwrap();
    assert!(db.account_for_token(&token).unwrap().is_some());
}

/// The regression that matters most: every token in the wild today has a NULL `expires_at`,
/// and must keep working exactly as it did before migration 29.
#[test]
fn a_token_with_no_expires_at_is_not_treated_as_expired() {
    let db = db();
    let token = account_with_token(&db);
    let stored: Option<String> = db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT expires_at FROM app_passwords", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, None, "minting must not set an expiry by default");
    assert!(db.account_for_token(&token).unwrap().is_some());
}

#[test]
fn a_token_idle_past_the_window_is_refused() {
    let db = db();
    let token = account_with_token(&db);
    last_used_days_ago(&db, DEFAULT_TOKEN_IDLE_DAYS + 1);
    assert!(db.account_for_token(&token).unwrap().is_none());
}

#[test]
fn a_token_used_within_the_window_still_works() {
    let db = db();
    let token = account_with_token(&db);
    last_used_days_ago(&db, DEFAULT_TOKEN_IDLE_DAYS - 1);
    assert!(db.account_for_token(&token).unwrap().is_some());
}

/// The sweep and the accept-check must agree. A token the sweep would delete must not
/// still authenticate in the window before the sweep next runs, and vice versa.
#[test]
fn the_sweep_removes_exactly_what_the_check_refuses() {
    let db = db();
    let token = account_with_token(&db);
    last_used_days_ago(&db, DEFAULT_TOKEN_IDLE_DAYS + 1);

    assert!(db.account_for_token(&token).unwrap().is_none());
    assert_eq!(db.sweep_expired_tokens().unwrap(), 1);
    assert_eq!(db.sweep_expired_tokens().unwrap(), 0, "sweep is idempotent");
}

#[test]
fn the_sweep_spares_a_live_token() {
    let db = db();
    let token = account_with_token(&db);
    assert_eq!(db.sweep_expired_tokens().unwrap(), 0);
    assert!(db.account_for_token(&token).unwrap().is_some());
}

/// Without this, enrolling a second factor changes nothing for whoever already traded the
/// passphrase for a token.
#[test]
fn revoking_everything_leaves_no_token_working() {
    let db = db();
    let first = account_with_token(&db);
    let second = db.mint_device_token("alpha", "phone").unwrap();

    assert_eq!(db.revoke_all_tokens("alpha", None).unwrap(), 2);
    assert!(db.account_for_token(&first).unwrap().is_none());
    assert!(db.account_for_token(&second).unwrap().is_none());
}

/// "Sign out my other devices" must not sign out the device asking.
#[test]
fn revoking_can_spare_the_caller() {
    let db = db();
    let mine = account_with_token(&db);
    let other = db.mint_device_token("alpha", "phone").unwrap();

    let spare = credentials::hash_token(&mine);
    assert_eq!(db.revoke_all_tokens("alpha", Some(&spare)).unwrap(), 1);
    assert!(db.account_for_token(&mine).unwrap().is_some());
    assert!(db.account_for_token(&other).unwrap().is_none());
}

/// Revocation is scoped to one account, or it is a denial of service against the server.
#[test]
fn revoking_one_account_leaves_another_alone() {
    let db = db();
    let alpha = account_with_token(&db);
    db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
        .unwrap();
    let mallory = db.mint_device_token("mallory", "laptop").unwrap();

    db.revoke_all_tokens("mallory", None).unwrap();
    assert!(db.account_for_token(&alpha).unwrap().is_some());
    assert!(db.account_for_token(&mallory).unwrap().is_none());
}
