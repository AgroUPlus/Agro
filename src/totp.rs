//! The second factor: RFC 6238 time-based one-time passwords.
//!
//! A passphrase is one secret, and one secret is one theft away from an account. This adds a second
//! that lives on a device rather than in a database, so a leaked passphrase — from a backup, a
//! reused password, a shoulder — is no longer enough on its own.
//!
//! **Why the secret is encrypted and the vault key is not.** The settings vault
//! ([`crate::db_identity::Db::vault_envelope`]) is sealed by the *client*, and the server genuinely
//! cannot open it. A TOTP secret cannot work that way: verifying a code means computing it, which
//! means holding the secret in the clear at verification time. The best available property is
//! therefore weaker but still worth having — the secret is encrypted at rest under a key from the
//! environment ([`seal`]), so a stolen database file alone does not hand over anybody's second
//! factor. Someone who has both the database *and* the server's environment has everything either
//! way, and no arrangement of this module changes that.
//!
//! **Why enrolment is two steps.** [`crate::db_identity::Db::begin_totp_enrolment`] writes a secret;
//! nothing enforces it until [`crate::db_identity::Db::confirm_totp_enrolment`] has seen a code
//! computed from it. A one-step version locks out anyone whose authenticator failed to scan the QR
//! — the failure mode of a second factor is always lockout, and the design should make the
//! recoverable ordering the only one available.
//!
//! **Why there is a replay guard.** A code is valid for a whole time step, plus the neighbouring
//! ones for clock skew. Without recording which step was accepted, a code observed once can be
//! replayed inside that window. [`crate::db_identity::Db::verify_totp`] refuses a step it has
//! already accepted.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use totp_rs::{Algorithm, Builder, Secret, Totp};

/// Digits in a code. Six, because that is what every authenticator app shows.
const DIGITS: u8 = 6;

/// Seconds per step.
const STEP: u64 = 30;

/// How many steps either side of now are accepted, for clock drift between the phone and the
/// server. One step is ±30 seconds, which covers ordinary drift without widening the window a
/// guess has to hit.
const SKEW: u16 = 1;

/// How many recovery codes are issued at enrolment.
const RECOVERY_CODE_COUNT: usize = 10;

/// A freshly generated secret, ready to be shown once and then confirmed.
pub struct Enrolment {
    /// Base32, for typing into an authenticator by hand.
    pub secret_base32: String,
    /// The `otpauth://` URI, for the QR code.
    pub otpauth_uri: String,
    /// The sealed form, for the database.
    pub sealed: String,
}

/// Builds a `Totp` from a base32 secret, or `None` if the secret is unusable.
fn totp_for(secret_base32: &str) -> Option<Totp> {
    let secret = Secret::try_from_base32(secret_base32).ok()?;
    Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(DIGITS)
        .with_skew(SKEW)
        .with_step_duration(STEP)
        .with_secret(secret)
        .build()
        .ok()
}

/// Generates a new secret for `username`.
///
/// The issuer is what the authenticator app shows above the code, so it names the server rather
/// than the software: a user with three self-hosted things called "Agro" cannot tell them apart.
pub fn begin(username: &str) -> Result<Enrolment, String> {
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    let secret = Secret::new(bytes.to_vec().into_boxed_slice());
    let secret_base32 = secret.to_base32();

    let issuer = std::env::var("AGRO_TOTP_ISSUER")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Agro".to_string());

    let uri = format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        urlencoding::encode(&issuer),
        urlencoding::encode(username),
        secret_base32,
        urlencoding::encode(&issuer),
        DIGITS,
        STEP,
    );

    Ok(Enrolment {
        sealed: seal(&secret_base32)?,
        secret_base32,
        otpauth_uri: uri,
    })
}

/// Checks `code` against `secret_base32` at `now`, returning the time step it matched.
///
/// The step is returned rather than a bare bool so the caller can refuse a step it has already
/// accepted — see the replay note in the module docs. `None` means the code is wrong.
pub fn verify(secret_base32: &str, code: &str, now: u64) -> Option<u64> {
    let code = code.trim();
    // Length and shape are checked first so a malformed input cannot reach the comparison at all.
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // `check` returns the step it matched rather than a bool, precisely so a caller can refuse a
    // step it has already accepted. RFC 6238 §5.2 requires that, and the library documents that it
    // does not do it for you — `Db::verify_totp` is where it is done.
    totp_for(secret_base32)?.check(code, now)
}

/// Computes the code for a given secret and instant — the authenticator's side of the exchange.
///
/// Exists so tests can act as the phone. Production code never needs this: the server's job is to
/// *check* a code, and generating one is only useful to whoever is supposed to be holding the
/// secret.
#[cfg(test)]
pub fn generate_at(secret_base32: &str, at: u64) -> Option<String> {
    Some(totp_for(secret_base32)?.generate(at).to_string())
}

/// Encrypts a secret for storage, as `base64url(nonce || ciphertext)`.
///
/// Fails loudly when no key is configured. A silent fallback to storing the secret in the clear is
/// precisely the kind of quiet downgrade that makes a security feature worse than not having one:
/// the operator would believe the secrets were protected.
pub fn seal(secret_base32: &str) -> Result<String, String> {
    let key = secret_key()?;
    let cipher = Aes256Gcm::new(&key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, secret_base32.as_bytes())
        .map_err(|_| "could not seal the TOTP secret".to_string())?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(base64_encode(&blob))
}

/// Reverses [`seal`].
pub fn unseal(sealed: &str) -> Result<String, String> {
    let key = secret_key()?;
    let blob = base64_decode(sealed).ok_or("stored TOTP secret is not valid base64")?;
    if blob.len() < 12 {
        return Err("stored TOTP secret is too short to contain a nonce".into());
    }
    let (nonce, ciphertext) = blob.split_at(12);
    let plaintext = Aes256Gcm::new(&key)
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| {
            // Almost always a changed `AGRO_SECRET_KEY` rather than tampering, and the operator
            // needs to be told which of the two to go looking for.
            "could not open the TOTP secret — has AGRO_SECRET_KEY changed?".to_string()
        })?;
    String::from_utf8(plaintext).map_err(|_| "TOTP secret is not valid UTF-8".into())
}

/// True when a key is configured, so callers can refuse to *offer* enrolment on a server that
/// cannot store the result.
pub fn is_configured() -> bool {
    secret_key().is_ok()
}

/// The at-rest key, derived from `AGRO_SECRET_KEY`.
///
/// Hashed rather than used raw so any passphrase an operator sets becomes 32 bytes. This is not a
/// password-hashing context — the input is expected to be high-entropy and generated, and the
/// derivation runs on every verification — so a plain digest is the right primitive here, for the
/// same reason `credentials::hash_token` uses one.
fn secret_key() -> Result<Key<Aes256Gcm>, String> {
    let raw = std::env::var("AGRO_SECRET_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or(
            "AGRO_SECRET_KEY is not set, so two-factor secrets cannot be stored. Generate one with \
             `openssl rand -base64 32` and set it in the server's environment.",
        )?;
    if raw.len() < 16 {
        return Err("AGRO_SECRET_KEY is too short; use at least 16 characters".into());
    }
    Ok(*Key::<Aes256Gcm>::from_slice(&Sha256::digest(raw.as_bytes())))
}

/// Mints recovery codes: the way back in when the authenticator is gone.
///
/// Returned in the clear exactly once. The database keeps SHA-256 digests — these are generated and
/// high-entropy, so there is no dictionary to run against them and Argon2 would only make the
/// verification path expensive, which is the same argument `credentials` makes about tokens.
pub fn mint_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODE_COUNT)
        .map(|_| {
            let mut bytes = [0u8; 8];
            rand::thread_rng().fill_bytes(&mut bytes);
            // Grouped with a dash because these get written down by hand.
            let hex = hex::encode(bytes);
            format!("{}-{}", &hex[..8], &hex[8..])
        })
        .collect()
}

/// The stored form of a recovery code. Case- and dash-insensitive, because these are typed by a
/// person reading their own handwriting.
pub fn hash_recovery_code(code: &str) -> String {
    let normalised: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    hex::encode(Sha256::digest(normalised.as_bytes()))
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let indices = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for &i in indices.iter().take(chunk.len() + 1) {
            out.push(ALPHABET[i as usize] as char);
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    let chars: Vec<u8> = text.trim().bytes().collect();
    let mut out = Vec::with_capacity(chars.len() / 4 * 3);
    for chunk in chars.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push(((n >> (16 - 8 * i)) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
pub mod tests {
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
    fn matches_the_rfc_6238_test_vectors() {
        // The RFC's seed is the ASCII "12345678901234567890".
        let secret = Secret::new(b"12345678901234567890".to_vec().into_boxed_slice())
            .to_base32();
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
    fn a_correct_code_verifies_and_a_wrong_one_does_not() {
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
    fn a_code_from_the_neighbouring_step_is_accepted() {
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
    fn a_code_from_outside_the_window_is_refused() {
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
    fn verification_reports_the_step_that_matched() {
        let _env = with_key();
        let e = begin("alpha").unwrap();
        let now = 1_700_000_000u64;
        let totp = totp_for(&e.secret_base32).unwrap();
        let previous = now / STEP - 1;

        assert_eq!(
            verify(&e.secret_base32, &totp.generate(previous * STEP).to_string(), now),
            Some(previous)
        );
    }

    #[test]
    fn malformed_codes_are_refused_without_panicking() {
        let _env = with_key();
        let e = begin("alpha").unwrap();
        for bad in ["", "12345", "1234567", "abcdef", "12 34 56", "٣٤٥٦٧٨"] {
            assert_eq!(verify(&e.secret_base32, bad, 1_700_000_000), None, "{bad:?}");
        }
    }

    #[test]
    fn a_sealed_secret_round_trips() {
        let _env = with_key();
        let e = begin("alpha").unwrap();
        assert_eq!(unseal(&e.sealed).unwrap(), e.secret_base32);
    }

    /// The point of sealing: the stored blob must not be the secret.
    #[test]
    fn the_sealed_form_does_not_contain_the_secret() {
        let _env = with_key();
        let e = begin("alpha").unwrap();
        assert!(!e.sealed.contains(&e.secret_base32));
    }

    /// Two enrolments of the same secret must not produce identical ciphertext, or the nonce is
    /// not doing its job.
    #[test]
    fn sealing_is_randomised() {
        let _env = with_key();
        assert_ne!(seal("JBSWY3DPEHPK3PXP").unwrap(), seal("JBSWY3DPEHPK3PXP").unwrap());
    }

    #[test]
    fn a_secret_sealed_under_a_different_key_does_not_open() {
        let _env = with_key();
        let sealed = seal("JBSWY3DPEHPK3PXP").unwrap();
        std::env::set_var("AGRO_SECRET_KEY", "a-completely-different-key-value");
        assert!(unseal(&sealed).is_err());
        // The key is restored by `EnvGuard::drop`.
    }

    /// A garbled column must be an error, never a panic and never an empty secret that verifies
    /// against nothing in a way the caller might read as success.
    #[test]
    fn a_corrupt_sealed_secret_is_an_error() {
        let _env = with_key();
        assert!(unseal("not-valid-base64!!!").is_err());
        assert!(unseal("AAAA").is_err());
        assert!(unseal("").is_err());
    }

    #[test]
    fn recovery_codes_are_unique_and_normalise_when_hashed() {
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
    fn the_otpauth_uri_names_the_account_and_carries_the_secret() {
        let _env = with_key();
        let e = begin("alpha").unwrap();
        assert!(e.otpauth_uri.starts_with("otpauth://totp/"));
        assert!(e.otpauth_uri.contains("alpha"));
        assert!(e.otpauth_uri.contains(&e.secret_base32));
    }

    /// A server with no key must refuse to start an enrolment rather than store the secret in the
    /// clear — the quiet downgrade is the failure worth guarding against.
    #[test]
    fn enrolment_refuses_without_a_key() {
        // Takes the lock first, then clears the key inside it, so no other test can observe the
        // gap. `EnvGuard::drop` puts the key back.
        let _env = with_key();
        std::env::remove_var("AGRO_SECRET_KEY");
        assert!(!is_configured());
        assert!(begin("alpha").is_err());
    }
}
