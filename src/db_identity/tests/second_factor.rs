//! The second factor, end to end.
use super::*;

/// `AGRO_SECRET_KEY` is process-wide, so these share the lock `crate::totp`'s tests use.
fn enrolled_account(db: &Db) -> (String, String) {
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    let enrolment = db.begin_totp_enrolment("alpha").unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let code = current_code(&enrolment.secret_base32, now);
    let codes = db.confirm_totp_enrolment("alpha", &code, None).unwrap();
    (enrolment.secret_base32, codes[0].clone())
}

/// What the phone would be showing at `at`.
fn current_code(secret: &str, at: u64) -> String {
    crate::totp::generate_at(secret, at).expect("secret should be usable")
}

fn key() -> impl Drop {
    crate::totp::tests::with_key()
}

#[test]
fn an_account_starts_without_a_second_factor() {
    let _k = key();
    let db = db();
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    assert!(!db.totp_is_confirmed("alpha").unwrap());
    assert_eq!(
        db.verify_totp("alpha", "000000").unwrap(),
        TotpOutcome::NotEnrolled
    );
}

/// A secret that has been generated but not proved must not be enforced — otherwise a scan
/// that silently failed locks the account.
#[test]
fn a_pending_enrolment_is_not_a_second_factor() {
    let _k = key();
    let db = db();
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    db.begin_totp_enrolment("alpha").unwrap();
    assert!(!db.totp_is_confirmed("alpha").unwrap());
}

#[test]
fn confirming_with_a_real_code_enables_it_and_issues_recovery_codes() {
    let _k = key();
    let db = db();
    let (_secret, _code) = enrolled_account(&db);
    assert!(db.totp_is_confirmed("alpha").unwrap());
    assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 10);
}

#[test]
fn confirming_with_a_wrong_code_does_not_enable_it() {
    let _k = key();
    let db = db();
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    db.begin_totp_enrolment("alpha").unwrap();
    assert!(db.confirm_totp_enrolment("alpha", "000000", None).is_err());
    assert!(!db.totp_is_confirmed("alpha").unwrap());
}

/// The whole point of enrolling: tokens bought with the passphrase alone stop working.
#[test]
fn enrolling_signs_out_every_other_device() {
    let _k = key();
    let db = db();
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    let stale = db.mint_device_token("alpha", "attacker's laptop").unwrap();
    let mine = db.mint_device_token("alpha", "my phone").unwrap();

    let enrolment = db.begin_totp_enrolment("alpha").unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let code = current_code(&enrolment.secret_base32, now);
    db.confirm_totp_enrolment("alpha", &code, Some(&credentials::hash_token(&mine)))
        .unwrap();

    assert!(db.account_for_token(&stale).unwrap().is_none());
    assert!(
        db.account_for_token(&mine).unwrap().is_some(),
        "the device doing the enrolling keeps working"
    );
}

/// RFC 6238 §5.2. A code is valid for a whole step; without this, one observed over a
/// shoulder works again for the rest of that window.
#[test]
fn a_code_cannot_be_used_twice() {
    let _k = key();
    let db = db();
    let (secret, _) = enrolled_account(&db);
    // Confirmation already consumed the current step, so move to the next one.
    let later = chrono::Utc::now().timestamp() as u64 + 30;
    let code = current_code(&secret, later);

    // Verified against the wall clock, so drive it through the real entry point twice.
    let first = db.verify_totp("alpha", &code).unwrap();
    let second = db.verify_totp("alpha", &code).unwrap();
    assert!(
        first == TotpOutcome::Accepted || first == TotpOutcome::Replayed,
        "unexpected first outcome: {first:?}"
    );
    if first == TotpOutcome::Accepted {
        assert_eq!(second, TotpOutcome::Replayed, "the same code worked twice");
    }
}

#[test]
fn a_recovery_code_is_accepted_once_and_then_spent() {
    let _k = key();
    let db = db();
    let (_secret, recovery) = enrolled_account(&db);

    assert_eq!(
        db.verify_totp("alpha", &recovery).unwrap(),
        TotpOutcome::AcceptedRecoveryCode
    );
    assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 9);
    assert_eq!(
        db.verify_totp("alpha", &recovery).unwrap(),
        TotpOutcome::Rejected,
        "a spent recovery code must not work again"
    );
}

/// Written down by hand, so read back by hand.
#[test]
fn a_recovery_code_is_case_and_dash_insensitive() {
    let _k = key();
    let db = db();
    let (_secret, recovery) = enrolled_account(&db);
    let typed = recovery.to_uppercase().replace('-', "");
    assert_eq!(
        db.verify_totp("alpha", &typed).unwrap(),
        TotpOutcome::AcceptedRecoveryCode
    );
}

#[test]
fn regenerating_invalidates_the_old_codes() {
    let _k = key();
    let db = db();
    let (_secret, old) = enrolled_account(&db);
    let fresh = db.regenerate_recovery_codes("alpha").unwrap();

    assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 10);
    assert_eq!(
        db.verify_totp("alpha", &old).unwrap(),
        TotpOutcome::Rejected,
        "a code from the previous set must not still work"
    );
    assert_eq!(
        db.verify_totp("alpha", &fresh[0]).unwrap(),
        TotpOutcome::AcceptedRecoveryCode
    );
}

/// Replacing a *working* second factor without proving anything is how someone with a
/// stolen session removes it.
#[test]
fn a_confirmed_enrolment_cannot_be_silently_restarted() {
    let _k = key();
    let db = db();
    enrolled_account(&db);
    assert!(db.begin_totp_enrolment("alpha").is_err());
    assert!(db.totp_is_confirmed("alpha").unwrap());
}

#[test]
fn disabling_clears_the_secret_and_the_recovery_codes() {
    let _k = key();
    let db = db();
    let (_secret, recovery) = enrolled_account(&db);
    assert!(db.disable_totp("alpha").unwrap());

    assert!(!db.totp_is_confirmed("alpha").unwrap());
    assert_eq!(db.recovery_codes_remaining("alpha").unwrap(), 0);
    assert_eq!(
        db.verify_totp("alpha", &recovery).unwrap(),
        TotpOutcome::NotEnrolled
    );
}

/// One account's factor must not satisfy another's.
#[test]
fn a_recovery_code_only_works_for_its_own_account() {
    let _k = key();
    let db = db();
    let (_secret, recovery) = enrolled_account(&db);
    db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
        .unwrap();
    let enrolment = db.begin_totp_enrolment("mallory").unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let code = current_code(&enrolment.secret_base32, now);
    db.confirm_totp_enrolment("mallory", &code, None).unwrap();

    assert_eq!(
        db.verify_totp("mallory", &recovery).unwrap(),
        TotpOutcome::Rejected
    );
}
