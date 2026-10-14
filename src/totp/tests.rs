use super::*;

/// `AGRO_SECRET_KEY` is process-wide state, and `cargo test` runs these in parallel. One test
/// asserts on the key being *absent*, so without serialising them it removes the key out from
/// under whichever test is sealing a secret at that moment — which failed four tests at random.
///
/// Every test that touches the variable takes this lock for its whole body and restores the key
/// on the way out.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds the lock and guarantees the key is put back, even if the test panics while it is
/// unset — otherwise one failure cascades into every test that runs after it.
pub struct EnvGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::set_var("AGRO_SECRET_KEY", TEST_KEY);
    }
}

const TEST_KEY: &str = "test-key-that-is-long-enough";
use totp_rs::Secret;

pub fn with_key() -> EnvGuard {
    // A poisoned lock means an earlier test panicked; the state it guards is restored by the
    // guard's `Drop` regardless, so recovering is correct rather than merely convenient.
    let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("AGRO_SECRET_KEY", TEST_KEY);
    EnvGuard(guard)
}

/// RFC 6238 Appendix B, the SHA-1 rows. The whole reason to have a shared implementation is
/// that it agrees with every authenticator app in the world, and this is what checks that.
#[test]
pub(super) fn matches_the_rfc_6238_test_vectors() {
    // The RFC's seed is the ASCII "12345678901234567890".
    let secret = Secret::new(b"12345678901234567890".to_vec().into_boxed_slice()).to_base32();
    let totp = totp_for(&secret).unwrap();
    for (time, expected) in [
        (59u64, "287082"),
        (1111111109, "081804"),
        (1111111111, "050471"),
        (1234567890, "005924"),
        (2000000000, "279037"),
    ] {
        assert_eq!(totp.generate(time).to_string(), expected, "at t={time}");
    }
}

#[test]
pub(super) fn a_correct_code_verifies_and_a_wrong_one_does_not() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    let now = 1_700_000_000u64;
    let totp = totp_for(&e.secret_base32).unwrap();
    let code = totp.generate(now).to_string();

    assert_eq!(verify(&e.secret_base32, &code, now), Some(now / STEP));
    assert_eq!(verify(&e.secret_base32, "000000", now), None);
}

/// A phone whose clock is a few seconds out must still work.
#[test]
pub(super) fn a_code_from_the_neighbouring_step_is_accepted() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    let now = 1_700_000_000u64;
    let totp = totp_for(&e.secret_base32).unwrap();

    for offset in [-(STEP as i64), 0, STEP as i64] {
        let code = totp.generate((now as i64 + offset) as u64).to_string();
        assert!(
            verify(&e.secret_base32, &code, now).is_some(),
            "offset {offset}s should be inside the window"
        );
    }
}

/// But the window has to end somewhere, or it stops being time-based.
#[test]
pub(super) fn a_code_from_outside_the_window_is_refused() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    let now = 1_700_000_000u64;
    let totp = totp_for(&e.secret_base32).unwrap();

    for offset in [-(STEP as i64) * 3, STEP as i64 * 3] {
        let code = totp.generate((now as i64 + offset) as u64).to_string();
        assert_eq!(
            verify(&e.secret_base32, &code, now),
            None,
            "offset {offset}s should be outside the window"
        );
    }
}

/// The step is what the caller stores to refuse a replay, so it has to be the step that
/// actually matched rather than whatever step is current.
#[test]
pub(super) fn verification_reports_the_step_that_matched() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    let now = 1_700_000_000u64;
    let totp = totp_for(&e.secret_base32).unwrap();
    let previous = now / STEP - 1;

    assert_eq!(
        verify(
            &e.secret_base32,
            &totp.generate(previous * STEP).to_string(),
            now
        ),
        Some(previous)
    );
}

#[test]
pub(super) fn malformed_codes_are_refused_without_panicking() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    for bad in ["", "12345", "1234567", "abcdef", "12 34 56", "٣٤٥٦٧٨"] {
        assert_eq!(
            verify(&e.secret_base32, bad, 1_700_000_000),
            None,
            "{bad:?}"
        );
    }
}

#[test]
pub(super) fn a_sealed_secret_round_trips() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    assert_eq!(unseal(&e.sealed).unwrap(), e.secret_base32);
}

/// The point of sealing: the stored blob must not be the secret.
#[test]
pub(super) fn the_sealed_form_does_not_contain_the_secret() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    assert!(!e.sealed.contains(&e.secret_base32));
}

/// Two enrolments of the same secret must not produce identical ciphertext, or the nonce is
/// not doing its job.
#[test]
pub(super) fn sealing_is_randomised() {
    let _env = with_key();
    assert_ne!(
        seal("JBSWY3DPEHPK3PXP").unwrap(),
        seal("JBSWY3DPEHPK3PXP").unwrap()
    );
}

#[test]
pub(super) fn a_secret_sealed_under_a_different_key_does_not_open() {
    let _env = with_key();
    let sealed = seal("JBSWY3DPEHPK3PXP").unwrap();
    std::env::set_var("AGRO_SECRET_KEY", "a-completely-different-key-value");
    assert!(unseal(&sealed).is_err());
    // The key is restored by `EnvGuard::drop`.
}

/// A garbled column must be an error, never a panic and never an empty secret that verifies
/// against nothing in a way the caller might read as success.
#[test]
pub(super) fn a_corrupt_sealed_secret_is_an_error() {
    let _env = with_key();
    assert!(unseal("not-valid-base64!!!").is_err());
    assert!(unseal("AAAA").is_err());
    assert!(unseal("").is_err());
}

#[test]
pub(super) fn recovery_codes_are_unique_and_normalise_when_hashed() {
    let codes = mint_recovery_codes();
    assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
    let mut seen = codes.clone();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), codes.len(), "recovery codes must not repeat");

    // Typed back by a person: case and dashes must not matter.
    let code = &codes[0];
    assert_eq!(
        hash_recovery_code(code),
        hash_recovery_code(&code.to_uppercase().replace('-', " "))
    );
    assert_ne!(hash_recovery_code(code), hash_recovery_code(&codes[1]));
}

#[test]
pub(super) fn the_otpauth_uri_names_the_account_and_carries_the_secret() {
    let _env = with_key();
    let e = begin("alpha").unwrap();
    assert!(e.otpauth_uri.starts_with("otpauth://totp/"));
    assert!(e.otpauth_uri.contains("alpha"));
    assert!(e.otpauth_uri.contains(&e.secret_base32));
}

/// A server with no key must refuse to start an enrolment rather than store the secret in the
/// clear — the quiet downgrade is the failure worth guarding against.
#[test]
pub(super) fn enrolment_refuses_without_a_key() {
    // Takes the lock first, then clears the key inside it, so no other test can observe the
    // gap. `EnvGuard::drop` puts the key back.
    let _env = with_key();
    std::env::remove_var("AGRO_SECRET_KEY");
    assert!(!is_configured());
    assert!(begin("alpha").is_err());
}
